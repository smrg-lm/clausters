//! Running statistics of a signal, scsynth's from its `TriggerUGens.cpp`
//! and `FeatureDetection.cpp`: the peak, maximum and minimum since a trigger,
//! a decaying peak, a running sum, the last value that changed, the input
//! that changed most or least, and the frequency of the zero crossings.

use crate::dsp::{ProcessCtx, Ramp, UGen, at};

/// Which running extreme a [`Running`] keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunningKind {
    /// `Peak`: the largest `|in|`.
    Peak,
    /// `RunningMax`: the largest `in`.
    Max,
    /// `RunningMin`: the smallest `in`.
    Min,
}

/// `Peak(in, trig)`, `RunningMax(in, trig)`, `RunningMin(in, trig)`: the
/// extreme of the input since the last rising edge of `trig`, which starts
/// it again from the input of the sample it rises on -- that sample's output
/// still the old extreme with it, as scsynth's is. A control-rate trigger
/// rises at the block's last sample, as scsynth's does.
///
/// At control rate over an audio-rate input the extreme covers every sample
/// of the block, scsynth's `Peak` semantics, which here `RunningMax` and
/// `RunningMin` share: scsynth's two read only the block's first sample there.
pub struct Running {
    kind: RunningKind,
    level: f32,
    prev: f32,
    started: bool,
}

impl Running {
    pub fn new(kind: RunningKind) -> Self {
        Self {
            kind,
            level: 0.0,
            prev: 0.0,
            started: false,
        }
    }

    #[inline]
    fn value(&self, x: f32) -> f32 {
        if self.kind == RunningKind::Peak {
            x.abs()
        } else {
            x
        }
    }

    #[inline]
    fn keep(&self, level: f32, v: f32) -> f32 {
        match self.kind {
            RunningKind::Min => level.min(v),
            _ => level.max(v),
        }
    }
}

impl UGen for Running {
    fn reset(&mut self) {
        self.started = false;
        self.prev = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (x, trig) = (inputs[0], inputs[1]);
        if !self.started {
            self.started = true;
            self.level = self.value(x[0]);
        }
        if x.len() > output.len() {
            // Control rate over audio: one value for the whole block.
            let mut level = self.level;
            if trig.len() == 1 {
                // scsynth's (its SIMD form, the one it runs): the block's
                // extreme from the level so far, then a rising trigger
                // starts over from the block's last sample.
                let t = trig[0];
                for &v in x {
                    level = self.keep(level, self.value(v));
                }
                output.fill(level);
                if self.prev <= 0.0 && t > 0.0 {
                    level = self.value(x[x.len() - 1]);
                }
                self.prev = t;
            } else {
                let mut out = None;
                for (i, &v) in x.iter().enumerate() {
                    let v = self.value(v);
                    level = self.keep(level, v);
                    let t = at(trig, i);
                    if self.prev <= 0.0 && t > 0.0 {
                        out = Some(level);
                        level = v;
                    }
                    self.prev = t;
                }
                output.fill(out.unwrap_or(level));
            }
            self.level = level;
            return;
        }
        let mut last = 0.0;
        for (i, s) in output.iter_mut().enumerate() {
            let v = self.value(at(x, i));
            last = v;
            self.level = self.keep(self.level, v);
            *s = self.level;
            if trig.len() > 1 {
                let t = trig[i];
                if self.prev <= 0.0 && t > 0.0 {
                    self.level = v;
                }
                self.prev = t;
            }
        }
        if trig.len() == 1 {
            let t = trig[0];
            if self.prev <= 0.0 && t > 0.0 {
                self.level = last;
            }
            self.prev = t;
        }
    }
}

/// `PeakFollower(in, decay)`: `|in|` held at each new peak and falling back
/// towards the input by `decay` a sample, `y = x + decay (y - x)`. A
/// control-rate `decay` is a line across the block.
///
/// scsynth's has three formulas for the fall by the sign of the decay and
/// moves the line only on falling samples, so it ends a block short of the
/// value asked; here the fall is the one formula and the line reaches it.
#[derive(Default)]
pub struct PeakFollower {
    level: Option<f32>,
    decay: Ramp,
}

impl UGen for PeakFollower {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let x = inputs[0];
        let mut level = *self.level.get_or_insert(x[0].abs());
        let d = inputs[1];
        let (mut decay, step) = if d.len() == 1 {
            self.decay.toward(d[0] as f64, output.len())
        } else {
            (0.0, 0.0)
        };
        for (i, s) in output.iter_mut().enumerate() {
            let k = if d.len() == 1 { decay as f32 } else { d[i] };
            let v = at(x, i).abs();
            level = if v >= level { v } else { v + k * (level - v) };
            *s = level;
            decay += step;
        }
        self.level = Some(level);
    }
}

/// `RunningSum(in, numsamp)`: the sum of the last `numsamp` samples, its
/// window sized at the synth's start. The sum is restarted from the exact
/// sum of the window at every window's end, scsynth's guard against the
/// rounding a running sum gathers.
pub struct RunningSum {
    window: Vec<f32>,
    count: usize,
    sum: f32,
    next: f32,
}

impl RunningSum {
    pub fn new(numsamp: f32) -> Self {
        Self {
            window: vec![0.0; (numsamp as i64).max(1) as usize],
            count: 0,
            sum: 0.0,
            next: 0.0,
        }
    }
}

impl UGen for RunningSum {
    fn reset(&mut self) {
        self.window.fill(0.0);
        self.count = 0;
        self.sum = 0.0;
        self.next = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let n = self.window.len();
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            self.sum -= self.window[self.count];
            self.window[self.count] = x;
            self.sum += x;
            self.next += x;
            *s = self.sum;
            self.count += 1;
            if self.count == n {
                self.count = 0;
                self.sum = self.next;
                self.next = 0.0;
            }
        }
    }
}

/// `LastValue(in, delta)`: the value the input had before it last moved by
/// at least `delta`.
#[derive(Default)]
pub struct LastValue {
    state: Option<(f32, f32)>,
}

impl UGen for LastValue {
    fn reset(&mut self) {
        self.state = None;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let x0 = at(inputs[0], 0);
        let (mut prev, mut curr) = *self.state.get_or_insert((x0, x0));
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if (x - curr).abs() >= at(inputs[1], i) {
                prev = curr;
                curr = x;
            }
            *s = prev;
        }
        self.state = Some((prev, curr));
    }
}

/// `MostChange(a, b)` and `LeastChange(a, b)`: whichever input moved more (or
/// less) since the last sample, and on a tie the one chosen last.
pub struct Change {
    most: bool,
    prev: [f32; 2],
    recent_b: bool,
}

impl Change {
    pub fn new(most: bool) -> Self {
        Self {
            most,
            prev: [0.0; 2],
            recent_b: true,
        }
    }
}

impl UGen for Change {
    fn reset(&mut self) {
        *self = Self::new(self.most);
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, s) in output.iter_mut().enumerate() {
            let (a, b) = (at(inputs[0], i), at(inputs[1], i));
            let mut diff = (a - self.prev[0]).abs() - (b - self.prev[1]).abs();
            if !self.most {
                diff = -diff;
            }
            if diff > 0.0 {
                self.recent_b = false;
            } else if diff < 0.0 {
                self.recent_b = true;
            }
            *s = if self.recent_b { b } else { a };
            self.prev = [a, b];
        }
    }
}

/// `ZeroCrossing(in)`: the frequency of the input's rising zero crossings,
/// each placed within its sample by the line between the two samples either
/// side, and held from one crossing to the next. Crossings closer than five
/// samples are not counted, scsynth's guard against noise.
#[derive(Default)]
pub struct ZeroCrossing {
    prev: Option<f32>,
    prev_frac: f32,
    level: f32,
    counter: i64,
}

impl UGen for ZeroCrossing {
    fn reset(&mut self) {
        *self = Self::default();
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let mut prev = *self.prev.get_or_insert(at(inputs[0], 0));
        for (i, s) in output.iter_mut().enumerate() {
            self.counter += 1;
            let x = at(inputs[0], i);
            if self.counter > 4 && prev <= 0.0 && x > 0.0 {
                let frac = -prev / (x - prev);
                self.level = ctx.sample_rate / (frac + self.counter as f32 - self.prev_frac);
                self.prev_frac = frac;
                self.counter = 0;
            }
            *s = self.level;
            prev = x;
        }
        self.prev = Some(prev);
    }
}
