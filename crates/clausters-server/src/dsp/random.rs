//! The random values: one value drawn and held, or drawn again on a
//! trigger, and the two UGens that choose and reseed the generator a synth
//! draws from -- scsynth's `Rand` family, from its `NoiseUGens.cpp`.
//!
//! Every one draws from its synth's generator ([`crate::dsp::rand`]), with
//! scsynth's draws: the same seed gives scsynth's values.
//!
//! - **Drawn once** (`Rand`, `IRand`, `ExpRand`, `LinRand`, `NRand`): at the
//!   synth's start, then held for its life. They run at the initial rate, so
//!   the synth's init pass is the one time they compute.
//! - **Drawn on a trigger** (`TRand`, `TIRand`, `TExpRand`): a value at the
//!   start, and a new one at each rising edge of the trigger; `CoinGate` lets
//!   each trigger through with a probability.
//! - **The generator itself** (`RandID`, `RandSeed`): which of the engine's
//!   generators the synth draws from, and a reseed of it on a trigger.

use crate::dsp::rand::SynthRand;
use crate::dsp::{ProcessCtx, UGen, at};

/// Which once-drawn value a [`Once`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnceKind {
    /// `Rand(lo, hi)`: uniform in `[lo, hi)`.
    Uniform,
    /// `IRand(lo, hi)`: an integer in `[lo, hi]`, the bounds truncated.
    Integer,
    /// `ExpRand(lo, hi)`: `lo * (hi/lo)^u`, log-uniform; `lo` and `hi` of one
    /// sign and nonzero.
    Exponential,
    /// `LinRand(lo, hi, minmax)`: the lower of two uniforms (`minmax <= 0`),
    /// leaning to `lo`, or the higher, leaning to `hi`.
    Linear,
    /// `NRand(lo, hi, n)`: the mean of `n` uniforms, a bell for `n >= 3`.
    Mean,
}

/// `Rand`, `IRand`, `ExpRand`, `LinRand`, `NRand`: one value, drawn the first
/// time the synth runs (the `ir` init pass) and held forever after. Inputs 0
/// `lo`, 1 `hi`, and for the last two 2 `minmax` / `n`.
///
/// A recomputed `Rand` would give a new number every block, so it stays
/// constant only because the init pass runs it exactly once -- which makes the
/// family the sharpest test of that pass.
pub struct Once {
    kind: OnceKind,
    rand: SynthRand,
    value: Option<f32>,
}

impl Once {
    pub fn new(kind: OnceKind, rand: SynthRand) -> Self {
        Self {
            kind,
            rand,
            value: None,
        }
    }
}

impl UGen for Once {
    fn reset(&mut self) {
        // A restarted voice draws a new value; the stream goes on.
        self.value = None;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let value = match self.value {
            Some(v) => v,
            None => {
                let (lo, hi) = (inputs[0][0], inputs[1][0]);
                let kind = self.kind;
                let v = self.rand.draw(|g| match kind {
                    OnceKind::Uniform => g.frand() * (hi - lo) + lo,
                    OnceKind::Integer => {
                        let (lo, hi) = (lo as i32, hi as i32);
                        (g.irand(hi - lo + 1) + lo) as f32
                    }
                    OnceKind::Exponential => ((hi / lo) as f64).powf(g.frand() as f64) as f32 * lo,
                    OnceKind::Linear => {
                        let (a, b) = (g.frand(), g.frand());
                        let u = if inputs[2][0] as i32 <= 0 {
                            a.min(b)
                        } else {
                            a.max(b)
                        };
                        u * (hi - lo) + lo
                    }
                    OnceKind::Mean => {
                        let n = inputs[2][0] as i32;
                        let mut sum = 0.0f32;
                        for _ in 0..n.max(0) {
                            sum += g.frand();
                        }
                        (sum / n as f32) * (hi - lo) + lo
                    }
                });
                self.value = Some(v);
                v
            }
        };
        output.fill(value);
    }
}

/// Which triggered draw a [`Triggered`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggeredKind {
    /// `TRand`: uniform in `[lo, hi)`.
    Uniform,
    /// `TIRand`: an integer in `[lo, hi]`, the bounds truncated.
    Integer,
    /// `TExpRand`: log-uniform between `lo` and `hi`.
    Exponential,
}

/// `TRand`, `TIRand`, `TExpRand`: a random value, drawn again at each rising
/// edge of the trigger and held between. Inputs 0 `lo`, 1 `hi`, 2 `trig`.
///
/// scsynth's, including the start: the output is a draw from the first
/// sample, before any trigger, and a trigger already high at the start is not
/// an edge.
pub struct Triggered {
    kind: TriggeredKind,
    rand: SynthRand,
    value: f32,
    prev: f32,
    started: bool,
}

impl Triggered {
    pub fn new(kind: TriggeredKind, rand: SynthRand) -> Self {
        Self {
            kind,
            rand,
            value: 0.0,
            prev: 0.0,
            started: false,
        }
    }

    fn draw(&mut self, lo: f32, hi: f32) -> f32 {
        let kind = self.kind;
        self.rand.draw(|g| match kind {
            TriggeredKind::Uniform => g.frand() * (hi - lo) + lo,
            TriggeredKind::Integer => {
                let (lo, hi) = (lo as i32, hi as i32);
                (g.irand(hi - lo + 1) + lo) as f32
            }
            TriggeredKind::Exponential => ((hi / lo) as f64).powf(g.frand() as f64) as f32 * lo,
        })
    }
}

impl UGen for Triggered {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (lo, hi, trig) = (inputs[0], inputs[1], inputs[2]);
        if !self.started {
            self.started = true;
            self.value = self.draw(lo[0], hi[0]);
            self.prev = trig[0];
            output[0] = self.value;
            for s in output.iter_mut().skip(1) {
                *s = self.value;
            }
            if trig.len() == 1 {
                return;
            }
        }
        for (i, s) in output.iter_mut().enumerate() {
            let t = at(trig, i);
            if t > 0.0 && self.prev <= 0.0 {
                self.value = self.draw(at(lo, i), at(hi, i));
            }
            self.prev = t;
            *s = self.value;
        }
    }
}

/// `CoinGate(prob, trig)`: each rising edge of `trig` passes through (the
/// trigger's own value, for that sample) with probability `prob`, and
/// otherwise the output is 0. Inputs 0 `prob`, 1 `trig`.
pub struct CoinGate {
    rand: SynthRand,
    prev: f32,
}

impl CoinGate {
    pub fn new(rand: SynthRand) -> Self {
        Self { rand, prev: 0.0 }
    }
}

impl UGen for CoinGate {
    fn reset(&mut self) {
        self.prev = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (prob, trig) = (inputs[0][0], inputs[1]);
        for (i, s) in output.iter_mut().enumerate() {
            let t = at(trig, i);
            *s = if t > 0.0 && self.prev <= 0.0 && self.rand.draw(|g| g.frand()) < prob {
                t
            } else {
                0.0
            };
            self.prev = t;
        }
    }
}

/// `RandID(id)`: selects which of the engine's generators the synth's random
/// UGens draw from, from here on in the graph's order (0 until it runs). An
/// `id` past the table is ignored, as scsynth ignores it. Outputs 0.
///
/// Synths that draw from different generators are independent streams: they
/// can run in one parallel stage, and a reseed of one leaves the others alone.
pub struct RandId {
    rand: SynthRand,
}

impl RandId {
    pub fn new(rand: SynthRand) -> Self {
        Self { rand }
    }
}

impl UGen for RandId {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let id = inputs[0][0];
        if id >= 0.0 {
            self.rand.select(id as usize);
        }
        output.fill(0.0);
    }
}

/// `RandSeed(trig, seed)`: at each rising edge of `trig`, reseeds the
/// generator the synth draws from with `seed`, as scsynth's `RGen::init`
/// does -- so every synth drawing from that generator starts the same
/// sequence again, which is how a group's randomness is restarted on purpose.
/// At the initial rate it reseeds once, at the synth's start, when `trig` is
/// positive. Inputs 0 `trig`, 1 `seed`. Outputs 0.
pub struct RandSeed {
    rand: SynthRand,
    prev: f32,
}

impl RandSeed {
    pub fn new(rand: SynthRand) -> Self {
        Self { rand, prev: 0.0 }
    }
}

impl UGen for RandSeed {
    fn reset(&mut self) {
        self.prev = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let (trig, seed) = (inputs[0], inputs[1]);
        for i in 0..output.len() {
            let t = at(trig, i);
            if t > 0.0 && self.prev <= 0.0 {
                self.rand.reseed(at(seed, i) as i32 as u32);
            }
            self.prev = t;
        }
        output.fill(0.0);
    }
}

/// `TWindex(trig, normalize, weights...)`: at each rising edge of `trig`, an
/// index drawn with the weights' probabilities -- the weighted choice
/// `Select` then reads a list by. The weights sum to 1, or, with
/// `normalize` 1, are divided by their sum. A draw is made at the start too,
/// and a trigger already high there is not an edge, as scsynth's is.
///
/// Where unnormalized weights sum to less than 1, the probability left over
/// goes to the last index; scsynth answers an index past the weights there,
/// which `Select` clips to the same last one.
pub struct TWindex {
    rand: SynthRand,
    index: f32,
    prev: f32,
    started: bool,
}

/// `TWindex`'s inputs before its weights.
const TWINDEX_HEAD: usize = 2;

impl TWindex {
    pub fn new(rand: SynthRand) -> Self {
        Self {
            rand,
            index: 0.0,
            prev: 0.0,
            started: false,
        }
    }

    fn choose(&mut self, inputs: &[&[f32]], i: usize) -> f32 {
        let weights = &inputs[TWINDEX_HEAD..];
        if weights.is_empty() {
            return 0.0;
        }
        let total = if at(inputs[1], i) == 1.0 {
            weights.iter().map(|w| at(w, i)).sum()
        } else {
            1.0
        };
        let target = total * self.rand.draw(|g| g.frand());
        let mut sum = 0.0;
        for (k, w) in weights.iter().enumerate() {
            sum += at(w, i);
            if sum >= target {
                return k as f32;
            }
        }
        (weights.len() - 1) as f32
    }
}

impl UGen for TWindex {
    fn reset(&mut self) {
        self.started = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        if !self.started {
            self.started = true;
            self.index = self.choose(inputs, 0);
            self.prev = 1.0;
        }
        for (i, s) in output.iter_mut().enumerate() {
            let t = at(inputs[0], i);
            if t > 0.0 && self.prev <= 0.0 {
                self.index = self.choose(inputs, i);
            }
            self.prev = t;
            *s = self.index;
        }
    }
}
