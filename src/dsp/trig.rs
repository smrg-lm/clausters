//! Triggers and control (U5): the UGens that read a **rising edge** rather than
//! a waveform, and the two envelope followers that go with them.
//!
//! Everything here is built on [`Edge`], the one definition of what a trigger
//! *is* — a signal crossing from `<= 0` up to `> 0`. That definition was
//! already duplicated in three places (`SendTrig`, `SendReply`/`Poll`, the
//! `Demand` driver) before this module existed; they now share this one, so
//! "trigger" means exactly the same thing everywhere in the server, including
//! for kinds added later.
//!
//! The rows are grouped by the **state machine** behind them, not by their
//! names: one counter serves `Trig`/`Trig1`/`TDelay`, one held value serves
//! `Latch`/`Gate`, one accumulator serves `Timer`/`Sweep`, one leaky
//! integrator serves `Decay`/`Decay2`. Where two scsynth names are genuinely
//! different machines (`Stepper` against `PulseCount`) they stay apart —
//! grouping by affinity is not grouping by force.
//!
//! **Every row here defaults to `ar`**, including the counters, whose output
//! can only move when a trigger does. That looks wasteful and is deliberate: a
//! `kr` UGen reads **one sample per block** from an `ar` input, so a `kr`
//! counter driven by an `ar` impulse train silently misses every trigger that
//! does not land on a block boundary — 63 out of 64. Defaulting to `ar` makes
//! the cheap-and-wrong pairing something you have to ask for. `kr` is
//! available on every row and is the right choice *when its trigger is also
//! `kr`*; the saving is then real and the arithmetic is unchanged, since a
//! duration means seconds at either rate.
//!
//! **Sub-sample crossings.** `Timer` and `Sweep` measure *time*, so they
//! interpolate where between two samples the input actually crossed zero
//! (`frac = -prev / (cur - prev)`) instead of rounding to the sample. For a
//! trigger built from an impulse this is exactly zero and costs nothing; for
//! one built from a slow oscillator it is the difference between a period
//! measured to the sample and one measured to a fraction of it. scsynth does
//! the same, for the same reason.

use crate::dsp::{DoneAction, ProcessCtx, UGen, at};

/// `ln(0.001)`: a decay time is the time to fall 60 dB, as everywhere else in
/// the server (`lag.rs`, the comb's feedback gain). Kept in `f64` here because
/// [`Decay`]'s pole sits at `exp(ln(0.001) / (t·sr))`, which for a long decay
/// is close enough to 1 that an `f32` coefficient quantizes the decay time
/// visibly — the U-track precision policy, applied to the one place in this
/// module that has a pole at all.
const LOG001: f64 = -6.907_755_278_982_137;

/// A **rising edge**: the signal crossing from `<= 0` up to `> 0`.
///
/// One definition, shared by every UGen that takes a trigger. It holds the
/// previous sample, so an instance belongs to one input of one UGen — a kind
/// with `trig` and `reset` carries two.
#[derive(Clone, Copy, Debug, Default)]
pub struct Edge {
    prev: f32,
}

impl Edge {
    /// Whether `cur` completes a rising edge. Updates the stored sample.
    #[inline]
    pub fn rose(&mut self, cur: f32) -> bool {
        let fired = self.prev <= 0.0 && cur > 0.0;
        self.prev = cur;
        fired
    }

    /// Like [`rose`](Self::rose), but reports **where between the two samples**
    /// the crossing happened, as a fraction in `[0, 1)`: 0 when the previous
    /// sample was already at zero (an impulse train, the common case), rising
    /// toward 1 the closer the crossing sits to the current sample. Only the
    /// UGens that measure time pay for this.
    #[inline]
    pub fn cross(&mut self, cur: f32) -> Option<f32> {
        let prev = self.prev;
        self.prev = cur;
        if prev > 0.0 || cur <= 0.0 {
            return None;
        }
        let span = cur - prev;
        Some(if span > 0.0 {
            (-prev / span).clamp(0.0, 1.0)
        } else {
            0.0
        })
    }
}

/// A duration in seconds as a whole number of samples, never less than one:
/// a trigger that lasts zero samples is a trigger nobody can see, so a zero
/// duration still emits one. (scsynth rounds the same way and lets zero fall
/// through to nothing; this is the one deliberate difference, and it only
/// changes a case that had no use.)
#[inline]
fn dur_samples(dur: f32, sr: f32) -> i64 {
    ((dur * sr + 0.5) as i64).max(1)
}

/// Which of the three timed pulses a [`TrigPulse`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrigMode {
    /// `Trig`: hold the **input's level at the trigger** for `dur`.
    Level,
    /// `Trig1`: hold `1` for `dur`.
    Unit,
    /// `TDelay`: nothing for `dur`, then one sample of `1`.
    Delay,
}

/// `Trig`, `Trig1` and `TDelay`: one countdown, read three ways. Inputs
/// 0 `signal`, 1 `dur`.
///
/// `TDelay` **ignores a trigger while one is already pending**, which is what
/// keeps a delayed trigger from turning a burst into a pile-up; scsynth does
/// the same.
pub struct TrigPulse {
    edge: Edge,
    counter: i64,
    level: f32,
    mode: TrigMode,
}

impl TrigPulse {
    pub fn new(mode: TrigMode) -> Self {
        Self {
            edge: Edge::default(),
            counter: 0,
            level: 0.0,
            mode,
        }
    }
}

impl UGen for TrigPulse {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            match self.mode {
                // The held pulse *includes* the triggering sample, so a
                // duration of n samples covers `t ..= t+n-1`.
                TrigMode::Level | TrigMode::Unit => {
                    if self.edge.rose(x) {
                        self.level = if self.mode == TrigMode::Level { x } else { 1.0 };
                        self.counter = dur_samples(at(inputs[1], i), sr);
                    }
                    *out = if self.counter > 0 {
                        self.counter -= 1;
                        self.level
                    } else {
                        0.0
                    };
                }
                // The delay does not: `n` samples later is the sample at
                // `t+n`. Advancing the pending pulse **before** looking at the
                // trigger is what makes that exact, and it also decides the
                // boundary case — a trigger landing on the very sample the
                // pending pulse fires re-arms rather than being swallowed, so
                // a regular stream of triggers comes out regular instead of
                // limping.
                TrigMode::Delay => {
                    let emit = if self.counter > 0 {
                        self.counter -= 1;
                        self.counter == 0
                    } else {
                        false
                    };
                    if self.edge.rose(x) && self.counter <= 0 {
                        self.counter = dur_samples(at(inputs[1], i), sr);
                    }
                    *out = if emit { 1.0 } else { 0.0 };
                }
            }
        }
    }
}

/// Which of the two sample-and-holds a [`Hold`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldMode {
    /// `Latch`: sample on the rising edge, hold until the next one.
    Latch,
    /// `Gate`: follow the input while the gate is open, freeze when it closes.
    Gate,
}

/// `Latch` and `Gate`: one held value, updated on an edge or on a level.
/// Inputs 0 `signal`, 1 `trig`.
///
/// The difference is exactly the one their names claim, and it matters: a
/// `Latch` takes one sample per trigger however long the trigger lasts, while
/// a `Gate` is transparent for as long as it is open.
pub struct Hold {
    edge: Edge,
    level: f32,
    mode: HoldMode,
}

impl Hold {
    pub fn new(mode: HoldMode) -> Self {
        Self {
            edge: Edge::default(),
            level: 0.0,
            mode,
        }
    }
}

impl UGen for Hold {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            let trig = at(inputs[1], i);
            match self.mode {
                HoldMode::Latch => {
                    if self.edge.rose(trig) {
                        self.level = x;
                    }
                }
                HoldMode::Gate => {
                    if trig > 0.0 {
                        self.level = x;
                    }
                }
            }
            *out = self.level;
        }
    }
}

/// `Schmidt(signal, lo, hi)`: a comparator with hysteresis — 1 once the input
/// rises above `hi`, 0 once it falls below `lo`, and *unchanged* in between.
///
/// The gap is the point. A plain `signal > threshold` chatters when a noisy
/// input sits on its threshold; here the input has to cross the whole band to
/// change the answer.
#[derive(Default)]
pub struct Schmidt {
    level: f32,
}

impl UGen for Schmidt {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            if self.level > 0.0 {
                if x < at(inputs[1], i) {
                    self.level = 0.0;
                }
            } else if x > at(inputs[2], i) {
                self.level = 1.0;
            }
            *out = self.level;
        }
    }
}

/// Which of the two flip-flops a [`FlipFlop`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlipFlopMode {
    /// `ToggleFF`: one input; every trigger inverts the output.
    Toggle,
    /// `SetResetFF`: `trig` raises it, `reset` lowers it.
    SetReset,
}

/// `ToggleFF(trig)` and `SetResetFF(trig, reset)`: one bit of state.
///
/// A `SetResetFF` that sees both edges on the same sample ends at 0 — reset is
/// applied second, so the safe outcome wins. `ToggleFF` is not a divider by
/// two of the *signal*, it is a divider by two of the *triggers*: what it halves
/// is the rate at which the triggers arrive.
pub struct FlipFlop {
    set: Edge,
    reset: Edge,
    level: f32,
    mode: FlipFlopMode,
}

impl FlipFlop {
    pub fn new(mode: FlipFlopMode) -> Self {
        Self {
            set: Edge::default(),
            reset: Edge::default(),
            level: 0.0,
            mode,
        }
    }
}

impl UGen for FlipFlop {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            if self.set.rose(at(inputs[0], i)) {
                self.level = match self.mode {
                    FlipFlopMode::Toggle => 1.0 - self.level,
                    FlipFlopMode::SetReset => 1.0,
                };
            }
            if self.mode == FlipFlopMode::SetReset && self.reset.rose(at(inputs[1], i)) {
                self.level = 0.0;
            }
            *out = self.level;
        }
    }
}

/// Which of the two trigger counters a [`Counter`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CounterMode {
    /// `PulseCount(trig, reset)`: report the running count.
    Count,
    /// `PulseDivider(trig, div, start)`: emit one trigger every `div` of them.
    Divide,
}

/// `PulseCount` and `PulseDivider`: one count of triggers, reported or divided.
///
/// `PulseDivider`'s counter starts at `start` and is read **once**, on the
/// first block — it is an initial condition, not a signal, and re-reading it
/// would make the divider jump whenever the value moved. Counting up and
/// firing on reaching `div` (rather than on zero) is what makes `start = div-1`
/// fire on the very first trigger, which is how a divider is phased.
pub struct Counter {
    trig: Edge,
    reset: Edge,
    count: i64,
    primed: bool,
    mode: CounterMode,
}

impl Counter {
    pub fn new(mode: CounterMode) -> Self {
        Self {
            trig: Edge::default(),
            reset: Edge::default(),
            count: 0,
            primed: false,
            mode,
        }
    }
}

impl UGen for Counter {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.primed {
            self.primed = true;
            if self.mode == CounterMode::Divide {
                self.count = (at(inputs[2], 0) + 0.5).floor() as i64;
            }
        }
        for (i, out) in output.iter_mut().enumerate() {
            let fired = self.trig.rose(at(inputs[0], i));
            match self.mode {
                CounterMode::Count => {
                    if self.reset.rose(at(inputs[1], i)) {
                        self.count = 0;
                    } else if fired {
                        self.count += 1;
                    }
                    *out = self.count as f32;
                }
                CounterMode::Divide => {
                    let div = (at(inputs[1], i) as i64).max(1);
                    *out = if fired {
                        self.count += 1;
                        if self.count >= div {
                            self.count = 0;
                            1.0
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    };
                }
            }
        }
    }
}

/// Wraps `x` into `[lo, hi]` **inclusive of both ends**, the way a step
/// sequencer wraps: `hi` is a position, not a limit you stop before.
#[inline]
fn wrap_i64(x: i64, lo: i64, hi: i64) -> i64 {
    let span = hi - lo + 1;
    if span <= 0 {
        return lo;
    }
    lo + (x - lo).rem_euclid(span)
}

/// `Stepper(trig, reset, min, max, step, resetval)`: a counter that walks a
/// range and wraps, one step per trigger.
///
/// It sits at `resetval` before the first trigger, so the first trigger lands
/// on `resetval + step` — a stepper is defined by its *transitions*, and the
/// alternative (the first trigger producing the value it already shows) makes
/// the first step invisible. A negative `step` walks backwards through the same
/// wrap.
#[derive(Default)]
pub struct Stepper {
    trig: Edge,
    reset: Edge,
    level: i64,
    primed: bool,
}

impl UGen for Stepper {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            let lo = at(inputs[2], i) as i64;
            let hi = at(inputs[3], i) as i64;
            let resetval = at(inputs[5], i) as i64;
            if !self.primed {
                self.primed = true;
                self.level = wrap_i64(resetval, lo, hi);
            }
            if self.reset.rose(at(inputs[1], i)) {
                self.level = wrap_i64(resetval, lo, hi);
            } else if self.trig.rose(at(inputs[0], i)) {
                self.level = wrap_i64(self.level + at(inputs[4], i) as i64, lo, hi);
            }
            *out = self.level as f32;
        }
    }
}

/// Which of the two elapsed-time readings an [`Elapsed`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElapsedMode {
    /// `Timer(trig)`: the interval between the last two triggers, held.
    Timer,
    /// `Sweep(trig, rate)`: a ramp rising at `rate` per second, restarted at
    /// each trigger.
    Sweep,
}

/// `Timer` and `Sweep`: one accumulator restarted at every rising edge, read
/// either as the interval it measured or as the ramp it is tracing.
///
/// Both count from the node's birth, not from the first trigger: `Sweep` is
/// already rising before anything triggers it (so `Sweep(0, 1)` is simply the
/// node's age in seconds), and `Timer` reports 0 until it has two edges to
/// measure between.
pub struct Elapsed {
    edge: Edge,
    /// Seconds (`Timer`) or output units (`Sweep`) since the last crossing.
    acc: f64,
    /// Sub-sample position of the previous crossing, so an interval is not
    /// rounded twice.
    prev_frac: f64,
    level: f64,
    mode: ElapsedMode,
}

impl Elapsed {
    pub fn new(mode: ElapsedMode) -> Self {
        Self {
            edge: Edge::default(),
            // Zero, and incremented at the *end* of each sample: an edge on
            // the node's very first sample then measures zero elapsed time
            // rather than one sample of it.
            acc: 0.0,
            prev_frac: 0.0,
            level: 0.0,
            mode,
        }
    }
}

impl UGen for Elapsed {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        for (i, out) in output.iter_mut().enumerate() {
            let inc = match self.mode {
                ElapsedMode::Timer => 1.0 / sr,
                ElapsedMode::Sweep => at(inputs[1], i) as f64 / sr,
            };
            if let Some(frac) = self.edge.cross(at(inputs[0], i)) {
                match self.mode {
                    ElapsedMode::Timer => {
                        self.level = self.acc + (frac as f64 - self.prev_frac) * inc;
                        self.prev_frac = frac as f64;
                        self.acc = 0.0;
                    }
                    ElapsedMode::Sweep => self.acc = frac as f64 * inc,
                }
            }
            *out = match self.mode {
                ElapsedMode::Timer => self.level as f32,
                ElapsedMode::Sweep => self.acc as f32,
            };
            self.acc += inc;
        }
    }
}

/// `Changed(signal, threshold)`: 1 on any sample where the input moved.
///
/// It reports `|(x[n] - x[n-1]) / 2| > threshold` — the halved difference,
/// because sclang builds this from `HPZ1`, whose gain is 0.5, and a def ported
/// from there must not change value. Worth knowing when picking a threshold:
/// a step of 0.2 registers against a threshold of 0.09, not of 0.19.
#[derive(Default)]
pub struct Changed {
    prev: f32,
}

impl UGen for Changed {
    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            let slope = 0.5 * (x - self.prev);
            self.prev = x;
            *out = if slope.abs() > at(inputs[1], i) {
                1.0
            } else {
                0.0
            };
        }
    }
}

/// A one-pole decay coefficient: the pole that falls 60 dB in `time` seconds.
/// `time <= 0` gives 0, which turns the filter into a pass-through of the
/// current sample.
#[inline]
fn decay_coeff(time: f32, sr: f32) -> f64 {
    if time <= 0.0 || sr <= 0.0 {
        0.0
    } else {
        (LOG001 / (time as f64 * sr as f64)).exp()
    }
}

/// `Decay(signal, decaytime)` and `Decay2(signal, attacktime, decaytime)`: an
/// impulse turned into an envelope.
///
/// `Decay` is the leaky integrator `y[n] = x[n] + b·y[n-1]`, so a single
/// impulse becomes an exponential falling 60 dB in `decaytime`. Its jump is
/// instantaneous, which clicks; `Decay2` subtracts a second, faster decay from
/// the first, giving a rounded attack. The two are one struct because the
/// second *is* the first, twice.
pub struct Decay {
    y1: f64,
    y2: f64,
    two_stage: bool,
}

impl Decay {
    pub fn new(two_stage: bool) -> Self {
        Self {
            y1: 0.0,
            y2: 0.0,
            two_stage,
        }
    }
}

impl UGen for Decay {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        // The decay time is the last input either way; a two-stage decay reads
        // its attack from the slot before it.
        let (attack_in, decay_in) = if self.two_stage {
            (Some(inputs[1]), inputs[2])
        } else {
            (None, inputs[1])
        };
        // Block-level fast path: a scalar time means one `exp` per block
        // instead of one per sample. The transcendental is the whole cost of
        // this UGen, so the branch is the difference between cheap and not.
        let const_decay = (decay_in.len() == 1).then(|| decay_coeff(decay_in[0], sr));
        let const_attack = attack_in.and_then(|a| (a.len() == 1).then(|| decay_coeff(a[0], sr)));
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i) as f64;
            let bd = const_decay.unwrap_or_else(|| decay_coeff(at(decay_in, i), sr));
            self.y1 = x + bd * self.y1;
            *out = if let Some(a) = attack_in {
                let ba = const_attack.unwrap_or_else(|| decay_coeff(at(a, i), sr));
                self.y2 = x + ba * self.y2;
                (self.y1 - self.y2) as f32
            } else {
                self.y1 as f32
            };
        }
    }
}

/// `DetectSilence(signal, amp, time, done_action)`: 1 once the input has stayed
/// within `±amp` for `time` seconds, and the `done_action` with it.
///
/// The counter resets on the first sample that exceeds `amp`, so what it
/// measures is *uninterrupted* silence. Like the envelope family it raises a
/// **done flag**, so `Done`/`FreeSelfWhenDone` can watch it — which is the
/// point: it exists to notice that a voice has nothing left to say and let
/// something else decide what to do about that.
pub struct DetectSilence {
    counter: i64,
    silent: bool,
    action: DoneAction,
}

impl Default for DetectSilence {
    fn default() -> Self {
        Self {
            counter: 0,
            silent: false,
            action: DoneAction::None,
        }
    }
}

impl UGen for DetectSilence {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        self.action = DoneAction::from_i32(at(inputs[3], 0) as i32);
        let sr = ctx.sample_rate;
        for (i, out) in output.iter_mut().enumerate() {
            let x = at(inputs[0], i);
            let end = dur_samples(at(inputs[2], i), sr);
            if x.abs() > at(inputs[1], i) {
                self.counter = 0;
                self.silent = false;
            } else {
                self.counter += 1;
                self.silent = self.counter >= end;
            }
            *out = if self.silent { 1.0 } else { 0.0 };
        }
    }

    fn done(&self) -> DoneAction {
        if self.silent {
            self.action
        } else {
            DoneAction::None
        }
    }

    fn is_done(&self) -> bool {
        self.silent
    }
}
