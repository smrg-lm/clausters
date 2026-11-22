//! Xenakis's dynamic stochastic synthesis, scsynth's `Gendy1`, `Gendy2` and
//! `Gendy3` (Nick Collins's port, `GendynUGens.cpp`): a waveform of control
//! points, each a level and the time to the next, every point's two values
//! taking a random walk -- one step a cycle -- between mirrors.
//!
//! The memory of control points is sized when the synth is built, from the
//! def's `init_cps` (a constant; 12 where the def wires it to anything
//! else), as scsynth's constructor sizes it; `knum`, how many of them the
//! cycle runs through, may move at run time within it.

use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, UGen, at};

/// One of Xenakis's distributions, scsynth's: a uniform draw `f` in `[0, 1)`
/// shaped into `[-1, 1]` by distribution `which` with parameter `a`.
fn distribution(which: i32, a: f32, f: f32) -> f32 {
    let a = a.clamp(0.0001, 1.0);
    match which {
        // Cauchy.
        1 => {
            let c = (10.0 * a).atan();
            (1.0 / a) * (c * (2.0 * f - 1.0)).tan() * 0.1
        }
        // Logistic.
        2 => {
            let c = 0.5 + 0.499 * a;
            let c = ((1.0 - c) / c).ln();
            let f = (f - 0.5) * 0.998 * a + 0.5;
            ((1.0 - f) / f).ln() / c
        }
        // Hyperbolic cosine.
        3 => {
            let c = (1.569_225_5 * a).tan();
            let t = (1.569_225_5 * a * f).tan() / c;
            let t = (t * 0.999 + 0.001).ln() * -0.144_764_8;
            2.0 * t - 1.0
        }
        // Arcsine.
        4 => {
            let c = (std::f32::consts::FRAC_PI_2 * a).sin();
            (std::f32::consts::PI * (f - 0.5) * a).sin() / c
        }
        // Exponential.
        5 => {
            let c = (1.0 - 0.999 * a).ln();
            let t = (1.0 - f * 0.999 * a).ln() / c;
            2.0 * t - 1.0
        }
        // "Sinus": the parameter itself, so a signal can drive the walk.
        6 => 2.0 * a - 1.0,
        // Linear, and any other number.
        _ => 2.0 * f - 1.0,
    }
}

/// `x` reflected into `[lo, hi]` as many times as it takes.
#[inline]
fn fold(x: f32, lo: f32, hi: f32) -> f32 {
    if (lo..=hi).contains(&x) {
        return x;
    }
    let range = hi - lo;
    let y = (x - lo).rem_euclid(2.0 * range);
    lo + if y > range { 2.0 * range - y } else { y }
}

/// `Gendy1`'s mirror for a level, scsynth's own sequence of operations
/// (which reflects correctly within +/-5), and the general reflection past
/// it.
#[inline]
fn mirror_level(x: f32) -> f32 {
    if (-1.0..=1.0).contains(&x) {
        return x;
    }
    if !(-5.0..5.0).contains(&x) {
        return fold(x, -1.0, 1.0);
    }
    let mut y = if x < 0.0 { x + 4.0 } else { x };
    y %= 4.0;
    if y > 1.0 && y < 3.0 {
        2.0 - y
    } else if y > 1.0 {
        y - 4.0
    } else {
        y
    }
}

/// `Gendy1`'s mirror for a duration, scsynth's sequence within `(-1, 2)`.
#[inline]
fn mirror_duration(x: f32) -> f32 {
    if (0.0..=1.0).contains(&x) {
        return x;
    }
    if !(-1.0..2.0).contains(&x) {
        return fold(x, 0.0, 1.0);
    }
    let y = if x < 0.0 { x + 2.0 } else { x };
    2.0 - y % 2.0
}

/// Which of the three a [`Gendy`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GendyKind {
    /// `Gendy1`: each point's level and duration take a random walk.
    One,
    /// `Gendy2`: the walk's steps take a walk too (a second-order walk), the
    /// level's driven by a Lehmer congruence rather than the generator.
    Two,
}

/// `Gendy1(ampdist, durdist, adparam, ddparam, minfreq, maxfreq, ampscale,
/// durscale, init_cps, knum)` and `Gendy2(..., a, c)`: the waveform runs
/// through `knum` control points a cycle, linearly from one level to the
/// next; at each point the next level steps by `ampscale` times a draw from
/// distribution `ampdist` and the duration by `durscale` times one from
/// `durdist`, each reflected back into its range (levels +/-1, durations
/// `[0, 1]`, the duration mapping to a frequency between `minfreq` and
/// `maxfreq`).
///
/// scsynth's, its draws in its order. `Gendy2` reflects a value below a
/// range in scsynth with a formula that is right only for a range starting
/// at 0, so its levels leave +/-1; here every value is reflected into its
/// range.
pub struct Gendy {
    kind: GendyKind,
    rand: SynthRand,
    amp: Vec<f32>,
    dur: Vec<f32>,
    amp_step: Vec<f32>,
    dur_step: Vec<f32>,
    index: usize,
    phase: f64,
    level: f32,
    next_level: f32,
    speed: f32,
    started: bool,
}

/// The memory `init_cps` asks for, at least one point.
fn memory(init_cps: f32) -> usize {
    (init_cps as i64).max(1) as usize
}

impl Gendy {
    /// A Gendy whose def gives `init_cps` control points.
    pub fn new(kind: GendyKind, rand: SynthRand, init_cps: f32) -> Self {
        let n = memory(init_cps);
        let steps = if kind == GendyKind::Two { n } else { 0 };
        Self {
            kind,
            rand,
            amp: vec![0.0; n],
            dur: vec![0.0; n],
            amp_step: vec![0.0; steps],
            dur_step: vec![0.0; steps],
            index: 0,
            phase: 1.0,
            level: 0.0,
            next_level: 0.0,
            speed: 100.0,
            started: false,
        }
    }

    fn start(&mut self) {
        let two = self.kind == GendyKind::Two;
        let (amp, dur, amp_step, dur_step) = (
            &mut self.amp,
            &mut self.dur,
            &mut self.amp_step,
            &mut self.dur_step,
        );
        self.rand.draw(|g| {
            for i in 0..amp.len() {
                amp[i] = 2.0 * g.frand() - 1.0;
                dur[i] = g.frand();
                if two {
                    amp_step[i] = 2.0 * g.frand() - 1.0;
                    dur_step[i] = 2.0 * g.frand() - 1.0;
                }
            }
        });
        self.index = 0;
        self.phase = 1.0;
        self.level = 0.0;
        self.next_level = 0.0;
        self.speed = 100.0;
    }
}

impl UGen for Gendy {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            self.start();
        }
        let input = |k: usize| at(inputs[k], 0);
        let (which_amp, which_dur) = (input(0) as i32, input(1) as i32);
        let (a_amp, a_dur) = (input(2), input(3));
        let (min_freq, max_freq) = (input(4), input(5));
        let (scale_amp, scale_dur) = (input(6), input(7));
        let sample_dur = (1.0 / ctx.sample_rate as f64) as f32;
        let size = self.amp.len();
        for s in output.iter_mut() {
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                let num = input(9) as i64;
                let num = if num > size as i64 || num < 1 {
                    size
                } else {
                    num as usize
                };
                let index = (self.index + 1) % num;
                self.index = index;
                let rate = match self.kind {
                    GendyKind::One => {
                        self.level = self.next_level;
                        let f = self.rand.draw(|g| g.frand());
                        let next = self.amp[index] + scale_amp * distribution(which_amp, a_amp, f);
                        self.next_level = mirror_level(next);
                        self.amp[index] = self.next_level;
                        let f = self.rand.draw(|g| g.frand());
                        let r = self.dur[index] + scale_dur * distribution(which_dur, a_dur, f);
                        mirror_duration(r)
                    }
                    GendyKind::Two => {
                        let (a, c) = (input(10), input(11));
                        let lehmer = (self.level * a + c) % 1.0;
                        self.level = self.next_level;
                        let step =
                            self.amp_step[index] + distribution(which_amp, a_amp, lehmer.abs());
                        let step = fold(step, -1.0, 1.0);
                        self.amp_step[index] = step;
                        self.next_level = fold(self.amp[index] + scale_amp * step, -1.0, 1.0);
                        self.amp[index] = self.next_level;
                        let f = self.rand.draw(|g| g.frand());
                        let step = fold(
                            self.dur_step[index] + distribution(which_dur, a_dur, f),
                            -1.0,
                            1.0,
                        );
                        self.dur_step[index] = step;
                        fold(self.dur[index] + scale_dur * step, 0.0, 1.0)
                    }
                };
                self.dur[index] = rate;
                // A cycle runs through `num` points, so each segment is that
                // many times faster than the cycle.
                self.speed = (min_freq + (max_freq - min_freq) * rate) * sample_dur * num as f32;
            }
            let p = self.phase;
            *s = ((1.0 - p) * self.level as f64 + p * self.next_level as f64) as f32;
            self.phase += self.speed as f64;
        }
    }
}

/// `Gendy3(ampdist, durdist, adparam, ddparam, freq, ampscale, durscale,
/// init_cps, knum)`: the points' walks of `Gendy1`, but the cycle's length
/// is fixed at `1/freq` and the durations are its proportions, drawn
/// together at the start of each cycle -- a pitch, where `Gendy1` and
/// `Gendy2` wander. The first point stays at 0, so each cycle starts and
/// ends at the same level; a point shorter than a sample is dropped.
///
/// scsynth truncates each segment's interpolation factor to an integer, so
/// its lines miss the next point and jump, and it crosses one point a
/// sample, so a segment shorter than a step is extrapolated past; here each
/// line reaches its point and every point passed is crossed. Its
/// reflection of a duration below its floor of 0.01 also misses the floor;
/// here it is reflected into `[0.01, 1]`.
pub struct Gendy3 {
    rand: SynthRand,
    amp: Vec<f32>,
    dur: Vec<f32>,
    /// The cycle's points: their levels and their spans, as a proportion of
    /// the cycle, with a guard after the last.
    levels: Vec<f32>,
    spans: Vec<f64>,
    index: usize,
    phase: f64,
    last_phase: f64,
    next_phase: f64,
    level: f32,
    next_level: f32,
    slope: f64,
    speed: f64,
    started: bool,
}

impl Gendy3 {
    /// A Gendy3 whose def gives `init_cps` control points.
    pub fn new(rand: SynthRand, init_cps: f32) -> Self {
        let n = memory(init_cps);
        Self {
            rand,
            amp: vec![0.0; n],
            dur: vec![0.0; n],
            // Two guards: the sums of the spans may fall a rounding short of
            // the cycle, and the last segment then reads past the first
            // guard, where scsynth reads past its memory.
            levels: vec![0.0; n + 2],
            spans: vec![1.0; n + 2],
            index: 0,
            phase: 1.0,
            last_phase: 0.0,
            next_phase: 0.0,
            level: 0.0,
            next_level: 0.0,
            slope: 1.0,
            speed: 100.0,
            started: false,
        }
    }
}

impl UGen for Gendy3 {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            let (amp, dur, levels) = (&mut self.amp, &mut self.dur, &mut self.levels);
            self.rand.draw(|g| {
                for i in 0..amp.len() {
                    amp[i] = 2.0 * g.frand() - 1.0;
                    dur[i] = g.frand();
                    levels[i] = 2.0 * g.frand() - 1.0;
                }
            });
            self.amp[0] = 0.0;
            self.phase = 1.0;
            self.last_phase = 0.0;
            self.next_phase = 0.0;
            self.level = 0.0;
            self.next_level = 0.0;
            self.slope = 1.0;
        }
        let input = |k: usize| at(inputs[k], 0);
        let (which_amp, which_dur) = (input(0) as i32, input(1) as i32);
        let (a_amp, a_dur) = (input(2), input(3));
        let freq = input(4);
        let (scale_amp, scale_dur) = (input(5), input(6));
        let sample_dur = (1.0 / ctx.sample_rate as f64) as f32;
        let size = self.amp.len();
        for s in output.iter_mut() {
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                let num = input(8) as i64;
                let num = if num > size as i64 || num < 1 {
                    size
                } else {
                    num as usize
                };
                let mut sum = 0.0f32;
                for j in 0..num {
                    if j > 0 {
                        let f = self.rand.draw(|g| g.frand());
                        let a = self.amp[j] + scale_amp * distribution(which_amp, a_amp, f);
                        self.amp[j] = fold(a, -1.0, 1.0);
                    }
                    let f = self.rand.draw(|g| g.frand());
                    let d = self.dur[j] + scale_dur * distribution(which_dur, a_dur, f);
                    self.dur[j] = fold(d, 0.01, 1.0);
                    sum += self.dur[j];
                }
                let inv = 1.0 / sum;
                self.speed = (freq * sample_dur) as f64;
                let mut active = 0;
                for j in 0..num {
                    let d = self.dur[j] * inv;
                    if d >= sample_dur {
                        self.levels[active] = self.amp[j];
                        self.spans[active] = d as f64;
                        active += 1;
                    }
                }
                self.levels[active] = 0.0;
                self.spans[active] = 2.0;
                self.levels[active + 1] = 0.0;
                self.spans[active + 1] = 2.0;
                self.next_phase = 0.0;
                self.next_level = self.levels[0];
                self.index = usize::MAX;
            }
            // Every boundary the sample passed, where scsynth takes one a
            // sample and extrapolates past a segment shorter than the step.
            while self.phase >= self.next_phase {
                self.index = self.index.wrapping_add(1);
                self.level = self.next_level;
                self.last_phase = self.next_phase;
                self.next_phase = self.last_phase + self.spans[self.index];
                self.next_level = self.levels[self.index + 1];
                self.slope = 1.0 / (self.next_phase - self.last_phase);
            }
            let t = ((self.phase - self.last_phase) * self.slope) as f32;
            *s = (1.0 - t) * self.level + t * self.next_level;
            self.phase += self.speed;
        }
    }
}
