//! The delay core: one line behind all nine of scsynth's delay names.
//!
//! `DelayN/L/C`, `CombN/L/C` and `AllpassN/L/C` are the same circular buffer
//! with two independent parameters — how a fractional tap is interpolated
//! (`N` none, `L` linear, `C` cubic) and what, if anything, is fed back
//! (nothing, a comb, an allpass). One implementation, nine rows, no algebra
//! written three times.
//!
//! **The line is either synth-private memory or a pool buffer**, and the same
//! nine algorithms serve both — which is why there are eighteen names and one
//! implementation. A private line is allocated in `build`, on the network
//! thread, sized from the static `max_delay` and the
//! [`BuildCtx`](super::registry::BuildCtx) sample rate (that is the whole reason
//! `build` receives a sample rate at all), and it is nobody else's. A
//! `BufDelay*`/`BufComb*`/`BufAllpass*` reads and writes a **channel of a pool
//! buffer** instead, resolved per block from its `bufnum` input: the delay's
//! contents are then addressable, so they can be inspected, resampled, saved or
//! played by another node — the *shared* case, which is the whole difference.
//! What the two share is the circular line's arithmetic, held here once
//! ([`Storage`] says where the samples are and nothing else does).
//!
//! **A pool line is not zeroed by the UGen.** Its contents are whatever the
//! buffer holds, which is scsynth's behaviour too: allocating it and clearing
//! it are the client's, and a delay that silently zeroed a buffer somebody else
//! was using would be worse than one that plays what is there.
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

use crate::dsp::buffer::Buffer;
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

/// Where a delay line's samples live.
///
/// The one thing that differs between the private family and the `Buf*` one.
/// Everything else — the interpolation, the feedback, the wrap — is the same
/// arithmetic over whichever of these is underneath.
pub enum Storage {
    /// Synth-private memory, allocated at build and read by nobody else.
    /// `f32`: this is signal, not recursive filter state, so the extra
    /// precision would change nothing and cost twice the cache.
    Private(Vec<f32>),
    /// One channel of a pool buffer, both named by **inputs** and resolved per
    /// block — `bufnum` and `chan`, exactly as every other buffer UGen names
    /// them, so a line can be moved between buffers and channels by a
    /// `/node_set` like anything else.
    Pool,
}

/// A line as one block sees it: the samples, wherever they are.
///
/// Borrowed for the length of `process` and no longer, because a pool line is
/// resolved out of the buffer pool afresh each block — the `bufnum` input may
/// name a different buffer between one and the next.
enum Line<'a> {
    Private(&'a mut [f32]),
    Pool(&'a Buffer, usize),
}

impl Line<'_> {
    #[inline]
    fn len(&self) -> usize {
        match self {
            Line::Private(l) => l.len(),
            Line::Pool(b, _) => b.frames(),
        }
    }

    #[inline]
    fn get(&self, i: usize) -> f64 {
        match self {
            Line::Private(l) => l[i] as f64,
            Line::Pool(b, channel) => b.sample(i, *channel) as f64,
        }
    }

    #[inline]
    fn set(&mut self, i: usize, v: f32) {
        match self {
            Line::Private(l) => l[i] = v,
            Line::Pool(b, channel) => b.set_sample(i, *channel, v),
        }
    }
}

/// The sample `back` frames before the write head. `back == 0` is the most
/// recently written one.
#[inline]
fn tap(line: &Line, write: usize, back: usize) -> f64 {
    let len = line.len();
    line.get((write + len - back % len) % len)
}

/// Reads the line at a fractional distance behind the write head, with the
/// given interpolation.
#[inline]
fn read_at(line: &Line, write: usize, interp: Interp, back: f64) -> f64 {
    match interp {
        Interp::None => tap(line, write, back.round() as usize),
        Interp::Linear => {
            let i = back.floor();
            let frac = back - i;
            let i = i as usize;
            let (a, b) = (tap(line, write, i), tap(line, write, i + 1));
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
                tap(line, write, i - 1),
                tap(line, write, i),
                tap(line, write, i + 1),
                tap(line, write, i + 2),
            )
        }
    }
}

/// One delay line. Inputs for the private family: 0 the signal, 1 the delay
/// time in seconds, and — for the comb and allpass forms — 2 the decay time.
/// The `Buf*` family prepends 0 the buffer index and 1 the channel.
pub struct Delay {
    line: Storage,
    /// Next write position.
    write: usize,
    interp: Interp,
    feedback: Feedback,
    /// Longest delay a **private** instance can read, in frames — always at
    /// least three short of the line so a cubic tap's neighbours stay inside
    /// it. A pool line computes the same bound per block from the buffer it
    /// lands on, which is the only thing it cannot know at build.
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
            line: Storage::Private(vec![0.0; frames]),
            write: 0,
            interp,
            feedback,
            max_frames: (frames - 4) as f64,
        }
    }

    /// A line over whatever pool buffer and channel its first two inputs name
    /// — the `Buf*` family. Nothing is allocated: the buffer is the client's,
    /// and so is clearing it.
    pub fn over_buffer(interp: Interp, feedback: Feedback) -> Self {
        Self {
            line: Storage::Pool,
            write: 0,
            interp,
            feedback,
            max_frames: 0.0,
        }
    }

    /// Whether this instance reads its `bufnum`/`chan` from inputs 0 and 1 —
    /// which is also how many inputs the signal and the times are offset by.
    #[inline]
    fn buffered(&self) -> bool {
        matches!(self.line, Storage::Pool)
    }
}

/// Clamps a delay in frames into what the line can actually serve.
///
/// The lower bound is the interpolation's, not a safety margin: cubic needs a
/// sample on each side, and any feedback form needs at least one frame or the
/// loop has no delay in it at all and is not computable.
///
/// A free function rather than a method because the caller is already holding
/// the line out of `self`, and this needs to know nothing else.
#[inline]
fn clamp_frames(interp: Interp, feedback: Feedback, frames: f64, max: f64) -> f64 {
    let lo = if interp == Interp::Cubic || feedback != Feedback::None {
        1.0
    } else {
        0.0
    };
    frames.clamp(lo, max.max(lo))
}

impl UGen for Delay {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        // The `Buf*` family puts the buffer and the channel first, as every
        // other buffer UGen does; everything after them is the same list.
        let base = if self.buffered() { 2 } else { 0 };
        // Copied out before the line is borrowed: what the loop needs of
        // `self` besides the samples is two flags.
        let (interp, feedback) = (self.interp, self.feedback);
        let (sig, dtime) = (inputs[base], inputs[base + 1]);
        let decay = inputs.get(base + 2).copied();

        // The line for this block. A pool one is resolved afresh each block,
        // because the `bufnum` input may name a different buffer between one
        // and the next; a missing or empty one plays silence rather than
        // guessing, which is what every other buffer UGen does.
        let (mut line, max_frames) = match &mut self.line {
            Storage::Private(l) => {
                let max = self.max_frames;
                (Line::Private(l.as_mut_slice()), max)
            }
            Storage::Pool => {
                let index = inputs[0][0].max(0.0) as usize;
                let channel = inputs[1][0].max(0.0) as usize;
                let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
                    output.fill(0.0);
                    return;
                };
                if buf.frames() < 8 || channel >= buf.channels() {
                    output.fill(0.0);
                    return;
                }
                let max = (buf.frames() - 4) as f64;
                (Line::Pool(buf, channel), max)
            }
        };
        let len = line.len();

        // A scalar delay time means one clamp and one floor for the whole
        // block instead of one per sample; the same block-level fast path the
        // rest of the track uses.
        let const_frames = (dtime.len() == 1)
            .then(|| clamp_frames(interp, feedback, dtime[0] as f64 * sr, max_frames));

        let mut write = self.write % len;
        for (i, s) in output.iter_mut().enumerate() {
            let frames = const_frames.unwrap_or_else(|| {
                clamp_frames(interp, feedback, at(dtime, i) as f64 * sr, max_frames)
            });
            let x = at(sig, i) as f64;
            let y = match feedback {
                Feedback::None => {
                    // Store first, so a delay of zero frames reads back the
                    // sample just written rather than the far end of the line.
                    line.set(write, x as f32);
                    read_at(&line, write, interp, frames)
                }
                fb => {
                    let g =
                        feedback_gain(frames / sr, decay.map(|d| at(d, i) as f64).unwrap_or(0.0));
                    let delayed = read_at(&line, write, interp, frames);
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
                    line.set(write, stored as f32);
                    out
                }
            };
            write = (write + 1) % len;
            *s = y as f32;
        }
        self.write = write;
    }
}
