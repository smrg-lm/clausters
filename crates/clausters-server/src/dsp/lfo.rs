//! The modulators that are more than a phase and a shape: `Vibrato`, a
//! frequency wobbling by a randomized depth and rate, and `LFGauss`, a
//! repeating (or single) Gaussian bell -- scsynth's, from its `LFUGens.cpp`.

use crate::dsp::rand::SynthRand;
use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// scsynth's `Vibrato(freq, rate, depth, delay, onset, rate_variation,
/// depth_variation, iphase, trig)`: `freq` times `1 + depth * w`, `w` a
/// parabolic approximation of a sine at `rate` Hz.
///
/// Each cycle draws its own rate and the depth of each of its two halves,
/// `rate * (1 + rate_variation * r)` and `depth * (1 + depth_variation * r)`
/// with `r` uniform in `[-1, 1)` from the synth's generator, scsynth's draws
/// in scsynth's order -- which is what makes it sound sung rather than
/// mechanical. The vibrato starts after `delay` seconds and fades in over
/// `onset`; a rising `trig` starts it again from `iphase` (in cycles), with a
/// fresh draw, delay and onset.
///
/// The bounds the cycle reads (`rate`, `depth`, the variations) are taken at
/// the start of each cycle, as scsynth takes them. The trigger is read every
/// sample, where scsynth reads its first sample per block: the same at
/// control rate, and an audio-rate trigger keeps its timing.
pub struct Vibrato {
    rand: SynthRand,
    /// scsynth's phase, four units a cycle, over `[-1, 3)`.
    phase: f64,
    /// This cycle's increment per sample, in those units.
    step: f64,
    scale_a: f32,
    scale_b: f32,
    delay: i64,
    attack: i64,
    attack_slope: f64,
    attack_level: f64,
    prev_trig: f32,
    started: bool,
}

impl Vibrato {
    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            phase: 0.0,
            step: 0.0,
            scale_a: 0.0,
            scale_b: 0.0,
            delay: 0,
            attack: 0,
            attack_slope: 0.0,
            attack_level: 0.0,
            prev_trig: 0.0,
            started: false,
        }
    }

    /// A new cycle's rate and depths, from the inputs at sample `i`.
    fn draw(&mut self, inputs: &[&[f32]], i: usize, sr: f32) {
        let rate = at(inputs[1], i) * (4.0 / sr);
        let depth = at(inputs[2], i);
        let (rv, dv) = (at(inputs[5], i), at(inputs[6], i));
        let (f, a, b) = self.rand.draw(|g| {
            let f = rate * (1.0 + rv * g.frand2());
            let a = depth * (1.0 + dv * g.frand2());
            let b = depth * (1.0 + dv * g.frand2());
            (f, a, b)
        });
        self.step = f as f64;
        self.scale_a = a;
        self.scale_b = b;
    }

    /// The start: the phase, the draws, the delay and the onset.
    fn start(&mut self, inputs: &[&[f32]], i: usize, sr: f32) {
        let iphase = at(inputs[7], i);
        self.phase = 4.0 * (iphase - iphase.floor()) as f64 - 1.0;
        self.draw(inputs, i, sr);
        self.delay = (at(inputs[3], i) * sr) as i64;
        self.attack = (at(inputs[4], i) * sr) as i64;
        self.attack_slope = 1.0 / (1 + self.attack.max(0)) as f64;
        self.attack_level = self.attack_slope;
    }
}

impl UGen for Vibrato {
    fn reset(&mut self) {
        self.started = false;
        self.prev_trig = 0.0;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        if !self.started {
            self.started = true;
            self.start(inputs, 0, sr);
        }
        for (i, s) in output.iter_mut().enumerate() {
            let trig = at(inputs[8], i);
            if self.prev_trig <= 0.0 && trig > 0.0 {
                self.start(inputs, i, sr);
            }
            self.prev_trig = trig;
            let x = at(inputs[0], i);
            if self.delay > 0 {
                self.delay -= 1;
                *s = x;
                continue;
            }
            let level = if self.attack > 0 {
                self.attack -= 1;
                let l = self.attack_level as f32;
                self.attack_level += self.attack_slope;
                l
            } else {
                1.0
            };
            let w = if self.phase < 1.0 {
                let z = self.phase as f32;
                self.scale_a * (1.0 - z * z)
            } else if self.phase < 3.0 {
                let z = (self.phase - 2.0) as f32;
                self.scale_b * (z * z - 1.0)
            } else {
                self.phase -= 4.0;
                self.draw(inputs, i, sr);
                let z = self.phase as f32;
                self.scale_a * (1.0 - z * z)
            };
            *s = x * (1.0 + level * w);
            self.phase += self.step;
        }
    }
}

/// scsynth's `LFGauss(duration, width, iphase, loop, done_action)`: the bell
/// `exp(-x^2 / (2 width^2))` with `x` running from -1 to 1 over `duration`
/// seconds -- a grain's window, or a repeating pulse when `loop` is on.
///
/// `iphase` shifts the bell along `x` (in `x`'s units, `[-1, 1]`) and is a
/// running offset, read every block. Without `loop`, the bell is played once:
/// `done_action` is taken when `x` passes 1, and the output goes on down the
/// bell's tail, as scsynth's does.
pub struct LfGauss {
    /// `x + iphase`, kept so a moving `iphase` shifts the bell rather than
    /// restarting it.
    pos: f64,
    done_action: DoneAction,
    finished: bool,
}

impl LfGauss {
    pub fn new() -> Self {
        Self {
            pos: -1.0,
            done_action: DoneAction::None,
            finished: false,
        }
    }
}

impl Default for LfGauss {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for LfGauss {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (dur, width) = (inputs[0], inputs[1]);
        let b = at(inputs[2], 0) as f64;
        let looping = at(inputs[3], 0) != 0.0;
        self.done_action = DoneAction::from_i32(at(inputs[4], 0) as i32);
        let sr = ctx.sample_rate;
        let mut x = self.pos - b;
        for (i, s) in output.iter_mut().enumerate() {
            if x > 1.0 {
                if looping {
                    x -= 2.0;
                } else {
                    self.finished = true;
                }
            }
            // A full cycle covers -1 to 1, so the step is twice the rate.
            let step = 2.0 / (at(dur, i) * sr);
            let c = at(width, i);
            let factor = -1.0 / (2.0 * c * c);
            *s = (x * x * factor as f64).exp() as f32;
            x += step as f64;
        }
        self.pos = x + b;
    }

    fn done(&self) -> DoneAction {
        if self.finished {
            self.done_action
        } else {
            DoneAction::None
        }
    }

    fn is_done(&self) -> bool {
        self.finished
    }
}
