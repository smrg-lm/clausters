//! Envelope followers and gain control, scsynth's from its
//! `FilterUGens.cpp`: `Amplitude`, `Compander`, and the look-ahead
//! `Normalizer` and `Limiter`.
//!
//! The followers' times are scsynth's: the time to come within a tenth (20
//! dB) of a new level -- its code's `log(0.1)`, where its documentation
//! says 60 dB. A ported def keeps the envelope it had.

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// The one-pole coefficient that comes within a tenth in `time` seconds at
/// `rate`, scsynth's; 0 (no smoothing) for a time of 0.
#[inline]
fn coef(time: f32, rate: f32) -> f32 {
    if time == 0.0 {
        0.0
    } else {
        (0.1f64.ln() / (time as f64 * rate as f64)).exp() as f32
    }
}

/// One step of the attack/release follower over `|x|`.
#[inline]
fn follow(prev: f32, x: f32, attack: f32, release: f32) -> f32 {
    let v = x.abs();
    if v < prev {
        v + (prev - v) * release
    } else {
        v + (prev - v) * attack
    }
}

/// `Amplitude(in, attack, release)`: an envelope follower, `|in|` smoothed
/// by one coefficient rising and another falling.
///
/// At control rate over an audio-rate input it follows every sample and
/// outputs the block's last. scsynth computes that form's coefficients from
/// the control rate while it runs them every audio sample, so its times
/// come out as many times longer as there are samples in a block; here they
/// are the times given.
#[derive(Default)]
pub struct Amplitude {
    prev: Option<f32>,
}

impl UGen for Amplitude {
    fn reset(&mut self) {
        self.prev = None;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let x = inputs[0];
        // The rate the follower steps at: the input's samples.
        let rate = if x.len() > output.len() {
            ctx.full_sample_rate
        } else {
            ctx.sample_rate
        };
        let attack = coef(at(inputs[1], 0), rate);
        let release = coef(at(inputs[2], 0), rate);
        let mut prev = *self.prev.get_or_insert(x[0].abs());
        if x.len() > output.len() {
            for &v in x {
                prev = follow(prev, v, attack, release);
            }
            output.fill(prev);
        } else {
            for (i, s) in output.iter_mut().enumerate() {
                prev = follow(prev, at(x, i), attack, release);
                *s = prev;
            }
        }
        self.prev = Some(prev);
    }
}

/// `Compander(in, control, thresh, slope_below, slope_above, clamp_time,
/// relax_time)`: a compressor, expander or gate. The control's envelope
/// (followed as `Amplitude` does) sets a gain once a block -- `(env /
/// thresh)^(slope - 1)` with the slope below or above the threshold -- and
/// the gain moves to it across the block, as scsynth's does.
///
/// scsynth's first block fades in from a gain of 0; here it starts at the
/// gain the control asks for.
#[derive(Default)]
pub struct Compander {
    env: f32,
    gain: Ramp,
}

impl UGen for Compander {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let rate = ctx.sample_rate;
        let thresh = at(inputs[2], 0);
        let (below, above) = (at(inputs[3], 0), at(inputs[4], 0));
        let attack = coef(at(inputs[5], 0), rate);
        let release = coef(at(inputs[6], 0), rate);
        let control = inputs[1];
        for i in 0..output.len() {
            self.env = follow(self.env, at(control, i), attack, release);
        }
        let env = self.env;
        let target = if env < thresh {
            if below == 1.0 {
                1.0
            } else {
                let g = (env / thresh).powf(below - 1.0);
                // scsynth's guard where the expansion blows up.
                let a = g.abs();
                if a < 1e-15 {
                    0.0
                } else if a > 1e15 {
                    1.0
                } else {
                    g
                }
            }
        } else if above == 1.0 {
            1.0
        } else {
            (env / thresh).powf(above - 1.0)
        };
        let (start, step) = self.gain.toward(target as f64, output.len());
        for (i, s) in output.iter_mut().enumerate() {
            *s = at(inputs[0], i) * (start + step * i as f64) as f32;
        }
    }
}

/// Which look-ahead gain a [`Lookahead`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookaheadKind {
    /// `Normalizer`: the gain that brings each window's peak to `level`.
    Normalize,
    /// `Limiter`: that gain where the peak is above `level`, 1 where not.
    Limit,
}

/// `Normalizer(in, level, dur)` and `Limiter(in, level, dur)`: the input
/// `2 dur` late, under a gain that moves linearly across each window of
/// `dur` to the one the window's peak and the next one's call for -- so the
/// output never exceeds `level`, and the gain is a line rather than a
/// step. `dur` sizes the windows at the synth's start (scsynth's default is
/// 10 ms), the memory allocated when the synth is built.
///
/// scsynth's constructor writes the first sample into the window once more,
/// so its windows fall a sample later; here they start at the first sample.
pub struct Lookahead {
    kind: LookaheadKind,
    /// Three windows: the one filling, the middle one, the one playing.
    table: Vec<f32>,
    size: usize,
    /// Which third of the table each window is.
    fill: usize,
    middle: usize,
    play: usize,
    pos: usize,
    flips: u32,
    /// The gain and its step, in `f64` so a window's line ends where it
    /// aims: scsynth's `f32` line drifts past the ceiling by its rounding.
    level: f64,
    slope: f64,
    prev_peak: f32,
    peak: f32,
}

impl Lookahead {
    /// A look-ahead over windows of `dur` seconds at `sample_rate`.
    pub fn new(kind: LookaheadKind, dur: f32, sample_rate: f32) -> Self {
        let size = ((dur as f64 * sample_rate as f64).ceil() as usize).max(1);
        Self {
            kind,
            table: vec![0.0; size * 3],
            size,
            fill: 0,
            middle: 1,
            play: 2,
            pos: 0,
            flips: 0,
            level: 1.0,
            slope: 0.0,
            prev_peak: 0.0,
            peak: 0.0,
        }
    }
}

impl UGen for Lookahead {
    fn reset(&mut self) {
        self.table.fill(0.0);
        self.fill = 0;
        self.middle = 1;
        self.play = 2;
        self.pos = 0;
        self.flips = 0;
        self.level = 1.0;
        self.slope = 0.0;
        self.prev_peak = 0.0;
        self.peak = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let amp = at(inputs[1], 0);
        let n = self.size;
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            self.table[self.fill * n + self.pos] = x;
            *s = if self.flips >= 2 {
                (self.level * self.table[self.play * n + self.pos] as f64) as f32
            } else {
                0.0
            };
            self.level += self.slope;
            self.peak = self.peak.max(x.abs());
            self.pos += 1;
            if self.pos >= n {
                self.pos = 0;
                let peak = self.prev_peak.max(self.peak);
                self.prev_peak = self.peak;
                self.peak = 0.0;
                let next = match self.kind {
                    LookaheadKind::Normalize => {
                        if peak <= 0.00001 {
                            100_000.0 * amp
                        } else {
                            amp / peak
                        }
                    }
                    LookaheadKind::Limit => {
                        if peak > amp {
                            amp / peak
                        } else {
                            1.0
                        }
                    }
                };
                self.slope = (next as f64 - self.level) / n as f64;
                // The middle window plays next, the one filled becomes the
                // middle, and the one played fills.
                (self.fill, self.middle, self.play) = (self.play, self.fill, self.middle);
                self.flips += 1;
            }
        }
    }
}
