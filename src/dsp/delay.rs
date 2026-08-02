//! The delay core: one line behind all nine of scsynth's delay names.
//!
//! `DelayN/L/C`, `CombN/L/C` and `AllpassN/L/C` are the same circular buffer
//! with two independent parameters — how a fractional tap is interpolated
//! (`N` none, `L` linear, `C` cubic) and what, if anything, is fed back
//! (nothing, a comb, an allpass). One implementation, nine rows, no algebra
//! written three times.
//!
//! **The line is synth-private memory, not a pool buffer.** A buffer in the
//! pool is immutable once built — the invariant that already put the spectral
//! frame in private scratch — and a delay line is per-instance mutable memory
//! written every sample. So it is allocated in `build`, on the network thread,
//! sized from the static `max_delay` and the [`BuildCtx`](super::registry::BuildCtx)
//! sample rate. That is the whole reason `build` receives a sample rate at all.
//! It also means there is no `BufDelay*` family here: a delay over a pool buffer
//! would have to mutate one.
//!
//! **`max_delay` is static configuration, not an input.** scsynth passes
//! `maxdelaytime` as an initial-rate *input* because its `ir` inputs double as
//! build-time constants; here the field that sizes an allocation lives where
//! `fft_size` and `partitions` already live, and the signal inputs are only the
//! things that vary.
//!
//! **These do not report [`UGen::latency`].** Their delay is the point of the
//! UGen, not an artifact to be compensated — that hook exists for a UGen whose
//! processing happens to lag (the partitioned convolver) and feeds a future
//! plugin-delay compensation. Compensating a musical delay would silently undo
//! what the user asked for.

use crate::dsp::{ProcessCtx, UGen, at};

/// How a fractional delay is read between two stored samples.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Interp {
    /// Round to the nearest stored sample (`*N`). Cheapest, and the only one
    /// that is exact — at the price of quantizing the delay to whole samples,
    /// which zipper-modulates when the delay time moves.
    None,
    /// Linear between the two neighbours (`*L`).
    Linear,
    /// Four-point Catmull-Rom (`*C`), the same interpolation the buffer readers
    /// use.
    Cubic,
}

/// What the line feeds back into itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Feedback {
    /// A pure delay: what goes in comes out later, once.
    None,
    /// Feedback comb — `y[n] = x[n-D] + g*y[n-D]`, a resonator with a harmonic
    /// series of peaks.
    Comb,
    /// Schroeder allpass — the same pole/zero pair arranged so the magnitude
    /// response is exactly flat and only the phase is shaped. The building
    /// block of a reverb's diffusion stage.
    Allpass,
}

/// Four-point Catmull-Rom, evaluated in `f64`.
///
/// `y1` and `y2` bracket the point; `x` is the fraction between them.
#[inline]
fn cubic(x: f64, y0: f64, y1: f64, y2: f64, y3: f64) -> f64 {
    let c0 = y1;
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * x + c2) * x + c1) * x + c0
}

/// The feedback gain that decays by 60 dB... by 1/1000, rather: scsynth's
/// convention is that `decaytime` is the time to decay to **-60 dB**, which is
/// a factor of 1000, and the gain per round trip follows from the ratio of the
/// delay to it.
///
/// A negative decay time gives a negated gain — scsynth allows it, and it
/// inverts the comb's peaks into troughs. A zero delay or a zero decay silences
/// the feedback path rather than dividing by zero.
#[inline]
fn feedback_gain(delay_secs: f64, decay_secs: f64) -> f64 {
    const LOG001: f64 = -6.907_755_278_982_137; // ln(0.001)
    if delay_secs == 0.0 || decay_secs == 0.0 {
        return 0.0;
    }
    if decay_secs > 0.0 {
        (LOG001 * delay_secs / decay_secs).exp()
    } else {
        -(LOG001 * delay_secs / -decay_secs).exp()
    }
}

/// One delay line. Inputs: 0 the signal, 1 the delay time in seconds, and — for
/// the comb and allpass forms — 2 the decay time in seconds.
pub struct Delay {
    /// The line itself. `f32`: this is signal, not recursive filter state, so
    /// the extra precision would buy nothing and cost twice the cache.
    line: Vec<f32>,
    /// Next write position.
    write: usize,
    interp: Interp,
    feedback: Feedback,
    /// Longest delay this instance can read, in frames — always at least three
    /// short of the line so a cubic tap's neighbours stay inside it.
    max_frames: f64,
}

impl Delay {
    /// Allocates the line. Runs on the network thread; `max_delay` is in
    /// seconds and `sample_rate` is the engine's.
    pub fn new(interp: Interp, feedback: Feedback, max_delay: f32, sample_rate: f32) -> Self {
        // Four frames of headroom: a cubic tap reads one sample newer and two
        // older than the bracketing pair, and the delay is clamped inside that.
        let frames = ((max_delay.max(0.0) as f64 * sample_rate as f64).ceil() as usize + 4).max(8);
        Self {
            line: vec![0.0; frames],
            write: 0,
            interp,
            feedback,
            max_frames: (frames - 4) as f64,
        }
    }

    /// The sample `back` frames before the write head. `back == 0` is the most
    /// recently written one.
    #[inline]
    fn tap(&self, back: usize) -> f64 {
        let len = self.line.len();
        self.line[(self.write + len - back % len) % len] as f64
    }

    /// Reads the line at a fractional distance behind the write head, with this
    /// instance's interpolation.
    #[inline]
    fn read(&self, back: f64) -> f64 {
        match self.interp {
            Interp::None => self.tap(back.round() as usize),
            Interp::Linear => {
                let i = back.floor();
                let frac = back - i;
                let i = i as usize;
                let (a, b) = (self.tap(i), self.tap(i + 1));
                a + frac * (b - a)
            }
            Interp::Cubic => {
                let i = back.floor();
                let frac = back - i;
                let i = i as usize;
                // `i` is clamped to at least 1 by the caller, so `i - 1` is a
                // real sample rather than a wrap into the far end of the line.
                cubic(
                    frac,
                    self.tap(i - 1),
                    self.tap(i),
                    self.tap(i + 1),
                    self.tap(i + 2),
                )
            }
        }
    }

    /// Clamps a delay in frames into what this line can actually serve.
    ///
    /// The lower bound is the interpolation's, not a safety margin: cubic needs
    /// a sample on each side, and any feedback form needs at least one frame or
    /// the loop has no delay in it at all and is not computable.
    #[inline]
    fn clamp_frames(&self, frames: f64) -> f64 {
        let lo = if self.interp == Interp::Cubic || self.feedback != Feedback::None {
            1.0
        } else {
            0.0
        };
        frames.clamp(lo, self.max_frames)
    }
}

impl UGen for Delay {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let (sig, dtime) = (inputs[0], inputs[1]);
        let decay = inputs.get(2).copied();
        let len = self.line.len();

        // A scalar delay time means one clamp and one floor for the whole
        // block instead of one per sample; the same block-level fast path the
        // rest of the track uses.
        let const_frames = (dtime.len() == 1).then(|| self.clamp_frames(dtime[0] as f64 * sr));

        for (i, s) in output.iter_mut().enumerate() {
            let frames =
                const_frames.unwrap_or_else(|| self.clamp_frames(at(dtime, i) as f64 * sr));
            let x = at(sig, i) as f64;
            let y = match self.feedback {
                Feedback::None => {
                    // Store first, so a delay of zero frames reads back the
                    // sample just written rather than the far end of the line.
                    self.line[self.write] = x as f32;
                    self.read(frames)
                }
                fb => {
                    let g =
                        feedback_gain(frames / sr, decay.map(|d| at(d, i) as f64).unwrap_or(0.0));
                    let delayed = self.read(frames);
                    let (stored, out) = match fb {
                        Feedback::Comb => (x + g * delayed, delayed),
                        // Schroeder allpass: the stored value carries the
                        // feedback, and the output mixes it against the tap so
                        // the numerator and denominator are reciprocal — which
                        // is what makes the magnitude exactly flat.
                        _ => {
                            let v = x + g * delayed;
                            (v, delayed - g * v)
                        }
                    };
                    self.line[self.write] = stored as f32;
                    out
                }
            };
            self.write = (self.write + 1) % len;
            *s = y as f32;
        }
    }
}
