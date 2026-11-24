//! The rest of scsynth's `PV_*` set, and the UGens that read and write a
//! frame's bins: parameterized implementations under the scsynth names, the
//! stance of `dsp::spectral` (decision "The PV set is curated and
//! parameterized, never a per-op catalog").
//!
//! - [`PvPhase`] -- `PV_Conj`, `PV_PhaseShift90`, `PV_PhaseShift270`,
//!   `PV_PhaseShift`: a rotation of every bin, exact in rectangular form.
//! - [`PvMask`] -- `PV_BrickWall`/`PV_BinWipe`, `PV_RectComb`/
//!   `PV_RectComb2`, `PV_RandComb`/`PV_RandWipe`: a set of bins chosen by a
//!   shape, zeroed on one chain or taken from chain B on two.
//! - [`PvBinScramble`], [`PvDiffuser`], [`PvMagNoise`]: the random
//!   rearrangements, drawn from the synth's generators in scsynth's order.
//! - [`PvLocalMax`], [`PvConformalMap`]: a bin against its neighbors, and a
//!   Moebius map of each bin.
//! - [`PvCopy`] -- `PV_Copy`: a second chain, a copy of the first, so one
//!   analysis feeds two branches.
//! - [`PackFft`], [`Unpack1Fft`]: a frame's bins from signals, and a bin to
//!   a signal.
//!
//! Like scsynth's, the rotations, the masks and the rearrangements leave
//! DC and Nyquist alone where moving them would make them complex: those two
//! bins are real. Each departure from scsynth's code is named where it is
//! made, with what scsynth does.

use std::f32::consts::TAU;

use crate::dsp::rand::SynthRand;
use crate::dsp::registry::UGenConfig;
use crate::dsp::spectral::{SpectralChain, bin_mag, get_bin, resolve_fft_size, set_bin, zero_bin};
use crate::dsp::{ProcessCtx, UGen, at};

/// The chain marker every spectral UGen writes on its wire: 1 on a slice
/// with a fresh frame.
fn mark(output: &mut [f32], ready: bool) {
    if let Some(o) = output.first_mut() {
        *o = if ready { 1.0 } else { 0.0 };
    }
}

/// A rising edge of a trigger read once per slice, scsynth's `PV_*` rule:
/// `prev <= 0` and now `> 0`.
#[derive(Default)]
struct Edge {
    prev: f32,
}

impl Edge {
    fn rose(&mut self, v: f32) -> bool {
        let rose = self.prev <= 0.0 && v > 0.0;
        self.prev = v;
        rose
    }
}

/// A uniform permutation of `0..order.len()` drawn from `rand`, one `frand`
/// per place as scsynth draws it. scsynth swaps place `i` with one drawn
/// from `0..len - i`, which is not a uniform permutation (the classic
/// shuffle draws from `i..len`); this draws from `i..len`.
fn shuffle(order: &mut [u32], rand: &SynthRand) {
    for (i, o) in order.iter_mut().enumerate() {
        *o = i as u32;
    }
    let len = order.len();
    rand.draw(|g| {
        for i in 0..len {
            let j = i + ((g.frand() * (len - i) as f32) as usize).min(len - i - 1);
            order.swap(i, j);
        }
    });
}

/// The rotation a [`PvPhase`] applies.
#[derive(Clone, Copy)]
pub enum PhaseOp {
    /// The complex conjugate (`PV_Conj`): the phase negated.
    Conj,
    /// A quarter turn forward (`PV_PhaseShift90`).
    Rot90,
    /// A quarter turn back (`PV_PhaseShift270`).
    Rot270,
    /// A turn by input 1, radians, summed frame to frame while input 2 is
    /// above 0 (`PV_PhaseShift`).
    Shift,
}

/// Rotates every bin's phase (`PV_Conj`, `PV_PhaseShift90`,
/// `PV_PhaseShift270`, `PV_PhaseShift`). The quarter turns and the
/// conjugate swap and negate components; `PV_PhaseShift` multiplies by
/// `e^{j shift}`, one `cos` and one `sin` per frame where scsynth converts
/// every bin to polar and back with its table approximations. DC and
/// Nyquist are left alone, as scsynth leaves them: a real bin turned a
/// quarter would be imaginary, which those two bins cannot hold.
pub struct PvPhase {
    op: PhaseOp,
    /// `PV_PhaseShift`'s running sum, kept in `[0, 2 pi)` as scsynth keeps
    /// it.
    integral: f32,
}

impl PvPhase {
    pub fn new(op: PhaseOp) -> Self {
        Self { op, integral: 0.0 }
    }
}

impl UGen for PvPhase {
    fn reset(&mut self) {
        self.integral = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let half = chain.winsize / 2;
            let f = &mut chain.frame;
            match self.op {
                PhaseOp::Conj => {
                    for b in 1..half {
                        f[2 * b + 1] = -f[2 * b + 1];
                    }
                }
                PhaseOp::Rot90 => {
                    for b in 1..half {
                        let (re, im) = (f[2 * b], f[2 * b + 1]);
                        f[2 * b] = -im;
                        f[2 * b + 1] = re;
                    }
                }
                PhaseOp::Rot270 => {
                    for b in 1..half {
                        let (re, im) = (f[2 * b], f[2 * b + 1]);
                        f[2 * b] = im;
                        f[2 * b + 1] = -re;
                    }
                }
                PhaseOp::Shift => {
                    let mut shift = inputs.get(1).map_or(0.0, |i| at(i, 0));
                    if inputs.get(2).is_some_and(|i| at(i, 0) > 0.0) {
                        shift += self.integral;
                        self.integral = shift % TAU;
                    }
                    let (s, c) = shift.sin_cos();
                    for b in 1..half {
                        let (re, im) = (f[2 * b], f[2 * b + 1]);
                        f[2 * b] = re * c - im * s;
                        f[2 * b + 1] = re * s + im * c;
                    }
                }
            }
        }
        mark(output, chain.ready);
    }
}

/// How a [`PvMask`] chooses its bins.
#[derive(Clone, Copy)]
pub enum MaskShape {
    /// A run of bins from one end: `wipe` in `-1..1`, above 0 from the
    /// bottom up (a high pass on one chain), below 0 from the top down
    /// (`PV_BrickWall`, `PV_BinWipe`).
    Wipe,
    /// The gaps of a comb over the bins: `teeth` teeth, the comb's `phase`
    /// at DC and the `width` of a tooth, both fractions of a period; a bin
    /// past the tooth is chosen (`PV_RectComb`, `PV_RectComb2`).
    Comb,
    /// A random `wipe` fraction of the bins, in an order drawn at the first
    /// frame and again at each trigger (`PV_RandComb`, `PV_RandWipe`).
    Random,
}

/// A set of bins chosen by a [`MaskShape`]: zeroed on one chain
/// (`PV_BrickWall`, `PV_RectComb`, `PV_RandComb`), taken from chain B into
/// chain A on two (`PV_BinWipe`, `PV_RectComb2`, `PV_RandWipe`). The shape's
/// inputs follow the chains.
///
/// Departures from scsynth's code: a comb's starting phase is wrapped into
/// a period (scsynth wraps one step at a time, so a phase of 1.5 starts a
/// tooth late); the random order is a uniform permutation (see
/// `shuffle`); and a random wipe of every bin takes DC and Nyquist from B
/// too, as the random comb already zeroes them, so a full `PV_RandWipe` is
/// chain B.
pub struct PvMask {
    shape: MaskShape,
    /// Two chains (A and B) or one.
    pair: bool,
    /// The random order over bins `1..half`, `half - 1` places.
    order: Vec<u32>,
    drawn: bool,
    edge: Edge,
    triggered: bool,
    rand: SynthRand,
}

impl PvMask {
    pub fn new(config: &UGenConfig, rand: SynthRand, shape: MaskShape, pair: bool) -> Self {
        let n = resolve_fft_size(config.fft_size);
        let places = if matches!(shape, MaskShape::Random) {
            n / 2 - 1
        } else {
            0
        };
        Self {
            shape,
            pair,
            order: vec![0; places],
            drawn: false,
            edge: Edge::default(),
            triggered: false,
            rand,
        }
    }

    /// The input the shape's parameters start at.
    fn first_param(&self) -> usize {
        if self.pair { 2 } else { 1 }
    }

    /// Reads the trigger of a random shape, once per slice.
    fn watch(&mut self, inputs: &[&[f32]]) {
        if matches!(self.shape, MaskShape::Random) {
            let trig = inputs.get(self.first_param() + 1).map_or(0.0, |i| at(i, 0));
            if self.edge.rose(trig) {
                self.triggered = true;
            }
        }
    }

    /// Calls `pick` with every chosen bin of a frame of `half + 1` bins.
    fn choose(&mut self, inputs: &[&[f32]], half: usize, mut pick: impl FnMut(usize)) {
        let p = self.first_param();
        let param = |k: usize, default: f32| inputs.get(p + k).map_or(default, |i| at(i, 0));
        // The bins between DC and Nyquist.
        let inner = half as i32 - 1;
        match self.shape {
            MaskShape::Wipe => {
                let wipe = (param(0, 0.0) * inner as f32) as i32;
                if wipe > 0 {
                    let m = wipe.min(inner);
                    pick(0);
                    (1..=m as usize).for_each(&mut pick);
                    if m == inner {
                        pick(half);
                    }
                } else if wipe < 0 {
                    let m = wipe.max(-inner);
                    if m == -inner {
                        pick(0);
                    }
                    ((inner + m + 1) as usize..half).for_each(&mut pick);
                    pick(half);
                }
            }
            MaskShape::Comb => {
                let teeth = param(0, 0.0);
                let width = param(2, 0.5);
                let freq = teeth / half as f32;
                let mut phase = param(1, 0.0).rem_euclid(1.0);
                for b in 0..=half {
                    if phase > width {
                        pick(b);
                    }
                    phase = (phase + freq).rem_euclid(1.0);
                }
            }
            MaskShape::Random => {
                if !self.drawn || self.triggered {
                    self.drawn = true;
                    self.triggered = false;
                    shuffle(&mut self.order, &self.rand);
                }
                let n = ((param(0, 0.0) * inner as f32) as i32).clamp(0, inner) as usize;
                for &o in &self.order[..n] {
                    pick(o as usize + 1);
                }
                if n == inner as usize {
                    pick(0);
                    pick(half);
                }
            }
        }
    }
}

impl UGen for PvMask {
    fn reset(&mut self) {
        self.drawn = false;
        self.edge = Edge::default();
        self.triggered = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        self.watch(inputs);
        if chain.ready {
            let half = chain.winsize / 2;
            let frame = &mut chain.frame;
            self.choose(inputs, half, |b| zero_bin(frame, b, half));
        }
        mark(output, chain.ready);
    }

    fn process_spectral_pair(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        a: &mut SpectralChain,
        b: &mut SpectralChain,
    ) {
        self.watch(inputs);
        if a.ready {
            let half = a.winsize / 2;
            let (fa, fb) = (&mut a.frame, &b.frame);
            self.choose(inputs, half, |k| {
                let (re, im) = get_bin(fb, k, half);
                set_bin(fa, k, half, re, im);
            });
        }
        mark(output, a.ready);
    }
}

/// Moves a random part of the bins to random nearby places
/// (`PV_BinScramble`). Inputs: `[chain, wipe, width, trig]`: the fraction of
/// the bins moved, how far one may come from (a fraction of the bins on each
/// side) and a trigger that draws the arrangement again (read at its next
/// frame, with `width` read then). The first frame draws it too. DC and
/// Nyquist stay.
///
/// Departures from scsynth's code: the places are a uniform permutation
/// (see `shuffle`), and a source is drawn over its whole range: scsynth's
/// draw never reaches the range's upper end.
pub struct PvBinScramble {
    /// Destination places, a permutation of the `half - 1` inner bins.
    to: Vec<u32>,
    /// Where each destination takes its bin from.
    from: Vec<u32>,
    /// The frame rebuilt, so every move reads the frame as it came.
    scratch: Vec<f32>,
    drawn: bool,
    edge: Edge,
    triggered: bool,
    rand: SynthRand,
}

impl PvBinScramble {
    pub fn new(config: &UGenConfig, rand: SynthRand) -> Self {
        let n = resolve_fft_size(config.fft_size);
        Self {
            to: vec![0; n / 2 - 1],
            from: vec![0; n / 2 - 1],
            scratch: vec![0.0; n],
            drawn: false,
            edge: Edge::default(),
            triggered: false,
            rand,
        }
    }

    fn draw(&mut self, width: f32) {
        shuffle(&mut self.to, &self.rand);
        let inner = self.to.len();
        let width = (width * inner as f32) as i64;
        let (to, from) = (&self.to, &mut self.from);
        self.rand.draw(|g| {
            for (f, &k) in from.iter_mut().zip(to) {
                let lo = (k as i64 - width).max(0);
                let hi = (k as i64 + width).min(inner as i64 - 1);
                let span = (hi - lo + 1) as f32;
                *f = (lo + ((g.frand() * span) as i64).min(hi - lo)) as u32;
            }
        });
    }
}

impl UGen for PvBinScramble {
    fn reset(&mut self) {
        self.drawn = false;
        self.edge = Edge::default();
        self.triggered = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        let param = |k: usize| inputs.get(k).map_or(0.0, |i| at(i, 0));
        if self.edge.rose(param(3)) {
            self.triggered = true;
        }
        if chain.ready {
            if !self.drawn || self.triggered {
                self.drawn = true;
                self.triggered = false;
                self.draw(param(2));
            }
            let half = chain.winsize / 2;
            let inner = half - 1;
            let moved = (inner as f32 * param(1).clamp(0.0, 1.0)) as usize;
            self.scratch.copy_from_slice(&chain.frame);
            for j in 0..inner {
                let dest = self.to[j] as usize + 1;
                let src = if j < moved {
                    self.from[j] as usize + 1
                } else {
                    dest
                };
                let (re, im) = get_bin(&chain.frame, src, half);
                set_bin(&mut self.scratch, dest, half, re, im);
            }
            chain.frame.copy_from_slice(&self.scratch);
        }
        mark(output, chain.ready);
    }
}

/// Turns each bin's phase by its own random amount (`PV_Diffuser`). Inputs:
/// `[chain, trig]`; the amounts are drawn at the first frame and again at
/// each trigger. Every bin but DC and Nyquist is turned, by a rotation whose
/// `cos`/`sin` are computed when the amounts are drawn.
///
/// Departure from scsynth's code: scsynth turns only the first `trig *
/// bins` bins -- it reads its trigger input as a count -- so with the
/// trigger at 0 it diffuses nothing.
pub struct PvDiffuser {
    /// `(cos, sin)` of each inner bin's turn.
    turns: Vec<(f32, f32)>,
    drawn: bool,
    edge: Edge,
    triggered: bool,
    rand: SynthRand,
}

impl PvDiffuser {
    pub fn new(config: &UGenConfig, rand: SynthRand) -> Self {
        let n = resolve_fft_size(config.fft_size);
        Self {
            turns: vec![(1.0, 0.0); n / 2 - 1],
            drawn: false,
            edge: Edge::default(),
            triggered: false,
            rand,
        }
    }
}

impl UGen for PvDiffuser {
    fn reset(&mut self) {
        self.drawn = false;
        self.edge = Edge::default();
        self.triggered = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if self.edge.rose(inputs.get(1).map_or(0.0, |i| at(i, 0))) {
            self.triggered = true;
        }
        if chain.ready {
            if !self.drawn || self.triggered {
                self.drawn = true;
                self.triggered = false;
                let turns = &mut self.turns;
                self.rand.draw(|g| {
                    for t in turns.iter_mut() {
                        let (s, c) = (g.frand() * TAU).sin_cos();
                        *t = (c, s);
                    }
                });
            }
            let f = &mut chain.frame;
            for (k, &(c, s)) in self.turns.iter().enumerate() {
                let b = k + 1;
                let (re, im) = (f[2 * b], f[2 * b + 1]);
                f[2 * b] = re * c - im * s;
                f[2 * b + 1] = re * s + im * c;
            }
        }
        mark(output, chain.ready);
    }
}

/// Multiplies each bin by its own random factor in `[-1, 1)` every frame
/// (`PV_MagNoise`): input `[chain]`. The inner bins first, then DC, then
/// Nyquist, the order scsynth draws them in, so a seeded synth draws the
/// same factors.
pub struct PvMagNoise {
    rand: SynthRand,
}

impl PvMagNoise {
    pub fn new(rand: SynthRand) -> Self {
        Self { rand }
    }
}

impl UGen for PvMagNoise {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let half = chain.winsize / 2;
            let f = &mut chain.frame;
            self.rand.draw(|g| {
                for b in 1..half {
                    let r = g.frand2();
                    f[2 * b] *= r;
                    f[2 * b + 1] *= r;
                }
                f[0] *= g.frand2();
                f[1] *= g.frand2();
            });
        }
        mark(output, chain.ready);
    }
}

/// Keeps the bins that are a local maximum of magnitude and at least
/// `threshold`, zeroing the rest (`PV_LocalMax`). Input: `[chain,
/// threshold]`. A bin is a maximum when no neighbor is louder; DC and
/// Nyquist have one neighbor each.
///
/// Departure from scsynth's code: scsynth compares each bin with its left
/// neighbor *after* zeroing that neighbor, so a bin beside a zeroed one
/// passes whatever it is; here every bin is compared with the frame as it
/// came.
pub struct PvLocalMax {
    mags: Vec<f32>,
}

impl PvLocalMax {
    pub fn new(config: &UGenConfig) -> Self {
        let n = resolve_fft_size(config.fft_size);
        Self {
            mags: vec![0.0; n / 2 + 1],
        }
    }
}

impl UGen for PvLocalMax {
    fn reset(&mut self) {
        self.mags.fill(0.0);
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let thresh = inputs.get(1).map_or(0.0, |i| at(i, 0));
            let half = chain.winsize / 2;
            for b in 0..=half {
                self.mags[b] = bin_mag(&chain.frame, b, half);
            }
            for b in 0..=half {
                let m = self.mags[b];
                let left = b > 0 && m < self.mags[b - 1];
                let right = b < half && m < self.mags[b + 1];
                if m < thresh || left || right {
                    zero_bin(&mut chain.frame, b, half);
                }
            }
        }
        mark(output, chain.ready);
    }
}

/// Maps each bin `z` to `(z - a) / (1 - conj(a) z)` (`PV_ConformalMap`), a
/// Moebius map of the complex plane. Inputs: `[chain, areal, aimag]`. A
/// denominator whose squared modulus is under 0.001 is held there, as
/// scsynth holds it. DC and Nyquist stay.
///
/// Departure from scsynth's code: scsynth multiplies by the conjugate of the
/// denominator with the real part it has just overwritten, so its imaginary
/// part is not the map's; this computes the map.
pub struct PvConformalMap;

impl UGen for PvConformalMap {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let ar = inputs.get(1).map_or(0.0, |i| at(i, 0));
            let ai = inputs.get(2).map_or(0.0, |i| at(i, 0));
            let half = chain.winsize / 2;
            let f = &mut chain.frame;
            for b in 1..half {
                let (zr, zi) = (f[2 * b], f[2 * b + 1]);
                let (nr, ni) = (zr - ar, zi - ai);
                let dr = 1.0 - (zr * ar + zi * ai);
                let di = zr * ai - ar * zi;
                let d2 = (dr * dr + di * di).max(0.001);
                f[2 * b] = (nr * dr + ni * di) / d2;
                f[2 * b + 1] = (ni * dr - nr * di) / d2;
            }
        }
        mark(output, chain.ready);
    }
}

/// A second chain that copies the first (`PV_Copy`): input `[chain]`, and
/// its output wire is the new chain. scsynth copies into a buffer the def
/// names; here the compiler opens the chain, with its source's shape, so a
/// branch that changes one leaves the other as it was analysed. It copies
/// on every slice the source has a fresh frame, in its place in the graph:
/// a `PV_*` the def orders before it on the source chain is in the copy.
pub struct PvCopy;

impl UGen for PvCopy {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    /// `a` is the copy, `b` its source.
    fn process_spectral_pair(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        a: &mut SpectralChain,
        b: &mut SpectralChain,
    ) {
        a.ready = b.ready;
        if b.ready {
            a.frame.copy_from_slice(&b.frame);
            a.pos = b.pos;
        }
        a.window = b.window;
        a.window_from = b.window_from;
        a.prev_window = b.prev_window;
        mark(output, a.ready);
    }
}

/// Writes a run of bins from signals (`PackFFT`). Inputs: `[chain, frombin,
/// tobin, zeroothers, mag0, phase0, mag1, phase1, ...]`: pair `j` sets bin
/// `frombin + j`, up to `tobin` and to the pairs given, each read at the
/// frame; with `zeroothers` above 0 every other bin is zeroed. Usually on a
/// chain an `FFTTrigger` opened, so the frame is all the def writes; on an
/// analysed one it replaces the bins it names.
///
/// Departures from scsynth's code: DC and Nyquist, which are real, take a
/// pair's projection `mag * cos(phase)` (scsynth takes the magnitude and
/// drops the phase); and Nyquist reads its own pair whatever `frombin` is
/// (scsynth reads it `2 * frombin` inputs early). The values are signals
/// read at the frame, not demand-rate pulls.
pub struct PackFft;

impl UGen for PackFft {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let half = chain.winsize / 2;
            let read = |k: usize| inputs.get(k).map_or(0.0, |i| at(i, 0));
            let pairs = inputs.len().saturating_sub(4) / 2;
            let from = read(1).max(0.0) as usize;
            let to = (read(2).max(0.0) as usize)
                .min(half)
                .min((from + pairs).saturating_sub(1));
            if read(3) > 0.0 {
                for b in (0..from.min(half + 1)).chain(to + 1..=half) {
                    zero_bin(&mut chain.frame, b, half);
                }
            }
            if pairs > 0 && from <= to {
                for b in from..=to {
                    let j = 4 + 2 * (b - from);
                    let (mag, phase) = (read(j), read(j + 1));
                    let (s, c) = phase.sin_cos();
                    set_bin(&mut chain.frame, b, half, mag * c, mag * s);
                }
            }
        }
        mark(output, chain.ready);
    }
}

/// One bin of a frame as a signal (`Unpack1FFT`): inputs `[chain, binindex,
/// whichmeasure]`, the magnitude (`whichmeasure` 0) or the phase (1) of bin
/// `binindex`, updated at each fresh frame and held between them. It reads
/// the chain and leaves it as it is; its output is a value, not a chain.
///
/// Departure from scsynth's code: the magnitude of DC or Nyquist is its
/// absolute value and its phase 0 or pi (scsynth gives the signed value and
/// a phase of 0). scsynth's is a demand-rate UGen a `Demand` pulls at each
/// frame; this is a control-rate one, read directly.
pub struct Unpack1Fft {
    value: f32,
}

impl Unpack1Fft {
    pub fn new() -> Self {
        Self { value: 0.0 }
    }
}

impl Default for Unpack1Fft {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Unpack1Fft {
    fn reset(&mut self) {
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let half = chain.winsize / 2;
            let read = |k: usize| inputs.get(k).map_or(0.0, |i| at(i, 0));
            let b = read(1).max(0.0) as usize;
            if b <= half {
                let (re, im) = get_bin(&chain.frame, b, half);
                self.value = if read(2) == 0.0 {
                    (re * re + im * im).sqrt()
                } else {
                    im.atan2(re)
                };
            }
        }
        output.fill(self.value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over many draws every value lands in every place about as often:
    /// the shuffle is uniform, where scsynth's (each place swapped with one
    /// drawn from the first `len - i`) is not.
    #[test]
    fn the_shuffle_is_uniform() {
        let rand = SynthRand::detached(7);
        let mut order = [0u32; 4];
        let mut counts = [[0u32; 4]; 4];
        for _ in 0..40_000 {
            shuffle(&mut order, &rand);
            for (place, &v) in order.iter().enumerate() {
                counts[place][v as usize] += 1;
            }
        }
        for row in counts {
            for c in row {
                assert!((c as f64 - 10_000.0).abs() < 400.0, "{counts:?}");
            }
        }
    }

    /// A scrambled bin comes from its whole range, both ends included.
    #[test]
    fn a_scrambled_bin_comes_from_its_whole_range() {
        let config = UGenConfig {
            fft_size: Some(64),
            ..Default::default()
        };
        let mut s = PvBinScramble::new(&config, SynthRand::detached(3));
        let inner = 31i64;
        let (mut low, mut high) = (false, false);
        for _ in 0..400 {
            s.draw(0.1);
            let w = (0.1 * inner as f32) as i64;
            for (&f, &k) in s.from.iter().zip(&s.to) {
                let (lo, hi) = ((k as i64 - w).max(0), (k as i64 + w).min(inner - 1));
                assert!((lo..=hi).contains(&(f as i64)));
                low |= f as i64 == lo;
                high |= f as i64 == hi;
            }
        }
        assert!(low && high, "both ends of a range are drawn");
    }
}
