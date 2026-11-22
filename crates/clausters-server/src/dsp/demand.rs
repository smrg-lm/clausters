//! The demand family (`dr`): streams that are *pulled* rather than run.
//!
//! A demand UGen is not in the block-execution order. It is a **sub-graph its
//! driver owns**: the driver ([`Demand`], [`Duty`]) decides when a value is
//! needed and pulls one; the source ([`Dlist`], [`Dramp`], ...) yields the next
//! item of its stream, or `NaN` once it has none left. Between two pulls the
//! source does nothing at all -- a stream has no samples, only a next value.
//!
//! **Streams nest.** That is the whole point of the family, and it is what
//! shapes this module: a source's *inputs* may themselves be streams, so
//! `Dseq(1, Dwhite(2, 0, 1), 100)` yields two random numbers and then 100.
//! Everything a source reads therefore goes through [`DemandInputs`], where a
//! plain number and a nested `Dwhite` differ only in what `is_demand` answers.
//! The recursion itself lives in [`crate::synthdef::instance`], which is the
//! only place that can see the graph; its `Pull` walks strictly backwards
//! through the UGen vector, so the borrows can never alias and the depth is
//! capped at compile time. Nothing here allocates.
//!
//! **Who resets whom.** A parent that returns to a child it has already
//! drained resets it first, so the child's stream replays -- that is what makes
//! `Dseq(2, Dseries(3, 0, 1))` give `0 1 2 0 1 2` rather than `0 1 2` and
//! silence. The reset is **lazy** (marked when the child is left, performed
//! just before it is read again), because doing it eagerly would restart a
//! child the parent may never come back to. Reset propagation is per kind, not
//! a blanket rule: the list sources reset the child they move to, `Dstutter`
//! and `Dswitch1` reset their inputs outright, and the scalar sources
//! (`Dseries`, `Dwhite`, ...) propagate nothing, since they read their bounds
//! afresh on every pull anyway. This mirrors scsynth, where the same asymmetry
//! is visible in which `_next` functions call `RESETINPUT`.
//!
//! **`repeats` and the endless stream.** sclang says `inf`; a def cannot. Our
//! wire format rejects a non-finite constant (and JSON has no spelling for
//! one), so **`repeats <= 0` is the endless stream** here -- which is also what
//! a client would guess from a count. A positive count behaves exactly as
//! scsynth's, `inf` still works if a client manages to send it, and a `NaN`
//! (an exhausted stream feeding the count) still means zero, since there the
//! number is a value rather than a request.
//!
//! The cores, one per family, with the scsynth names on the wire:
//!
//! - [`Dramp`] -- `Dseries` (add) and `Dgeom` (multiply): the same walk with a
//!   different step operator.
//! - [`Drandom`] -- `Dwhite`/`Diwhite` (independent draws) and
//!   `Dbrown`/`Dibrown` (a bounded random walk), float or integer.
//! - [`Dlist`] -- `Dseq`, `Dser`, `Drand`, `Dwrand`, `Dxrand` and `Dshuf`:
//!   one traversal of a value list, differing only in what picks the next
//!   slot and in what `repeats` counts. This is the core that flattens nested
//!   streams, so all six inherit it.
//! - [`Dstutter`] (`Dstutter` and `Ddup`, one machine under two names),
//!   [`Dswitch1`], [`Dswitch`], [`Dconst`], [`Dreset`], [`Dpoll`],
//!   [`Dbufrd`], [`Dbufwr`] -- one machine each.
//! - [`Demand`], [`Duty`] and [`DemandEnvGen`] -- the drivers, which turn
//!   pulls into samples.

use clausters_core::builtins::fold;

use clausters_core::envshape::Segment;

use crate::dsp::rand::SynthRand;
use crate::dsp::registry::UGenConfig;
use crate::dsp::reply::ReplyBuffer;
use crate::dsp::trig::Edge;
use crate::dsp::{
    DemandInputs, DoneAction, MAX_UGEN_INPUTS, ProcessCtx, ReplyKind, ReplyMsg, UGen,
};

/// Input slot every source takes its repeat count in.
const I_REPEATS: usize = 0;

/// The shared `repeats` counter: read once per stream (on the first pull after
/// a reset), then counted against.
///
/// scsynth latches it the same way, which is why a `repeats` that is itself a
/// stream is consumed once rather than once per item.
struct Repeats {
    /// How many the stream may yield; `INFINITY` for an endless one.
    limit: f32,
    /// How many it has yielded (items or passes, depending on the kind).
    count: f32,
    /// False until the first pull latches `limit`.
    started: bool,
}

impl Repeats {
    fn new() -> Self {
        Self {
            limit: f32::INFINITY,
            count: 0.0,
            started: false,
        }
    }

    /// Latches the count on the first pull since the last reset. Returns
    /// whether this *was* that first pull -- the moment a source also latches
    /// whatever else it only reads once (a ramp's start value).
    fn begin(&mut self, inputs: &mut dyn DemandInputs) -> bool {
        if self.started {
            return false;
        }
        self.started = true;
        let r = inputs.pull(I_REPEATS);
        self.limit = if r.is_nan() {
            // Not a request but an exhausted stream: nothing to repeat.
            0.0
        } else if r <= 0.0 {
            f32::INFINITY
        } else {
            (r + 0.5).floor()
        };
        true
    }

    fn exhausted(&self) -> bool {
        self.count >= self.limit
    }

    fn advance(&mut self) {
        self.count += 1.0;
    }

    fn reset(&mut self) {
        self.limit = f32::INFINITY;
        self.count = 0.0;
        self.started = false;
    }
}

/// Rounds a demanded value to the integer index or count it stands for, the
/// way scsynth does (`floor(x + 0.5)`): a stream carries floats, and a count
/// of `2.9999998` is a count of three.
fn round(x: f32) -> f32 {
    (x + 0.5).floor()
}

// ---------------------------------------------------------------------------
// Ramps: Dseries, Dgeom
// ---------------------------------------------------------------------------

/// Which operator advances a [`Dramp`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RampKind {
    /// `Dseries`: add the step (an arithmetic sequence).
    Series,
    /// `Dgeom`: multiply by it (a geometric one).
    Geom,
}

/// `Dseries(repeats, start, step)` / `Dgeom(repeats, start, grow)`: yields
/// `start`, then repeatedly applies the step, `repeats` times.
///
/// The step is read on **every** pull, so it may itself be a stream (a series
/// whose increment comes from a `Drand` is an ordinary thing to want); a `NaN`
/// there -- an exhausted step stream -- leaves the last step in place rather than
/// ending the ramp. `start` is read once, on the first pull.
pub struct Dramp {
    kind: RampKind,
    rep: Repeats,
    value: f32,
    step: f32,
}

impl Dramp {
    pub fn new(kind: RampKind) -> Self {
        Self {
            kind,
            rep: Repeats::new(),
            value: 0.0,
            step: match kind {
                RampKind::Series => 1.0,
                RampKind::Geom => 2.0,
            },
        }
    }
}

impl UGen for Dramp {
    fn reset(&mut self) {
        *self = Self::new(self.kind);
    }

    // Demand sources are skipped in block order; this is never called.
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let step = inputs.pull(2);
        if !step.is_nan() {
            self.step = step;
        }
        if self.rep.begin(inputs) {
            self.value = inputs.pull(1);
        }
        if self.rep.exhausted() {
            return f32::NAN;
        }
        self.rep.advance();
        let v = self.value;
        self.value = match self.kind {
            RampKind::Series => v + self.step,
            RampKind::Geom => v * self.step,
        };
        v
    }

    fn reset_demand(&mut self, _inputs: &mut dyn DemandInputs) {
        self.rep.reset();
    }
}

// ---------------------------------------------------------------------------
// Random sources: Dwhite, Diwhite, Dbrown, Dibrown
// ---------------------------------------------------------------------------

/// Which stochastic stream a [`Drandom`] yields.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RandKind {
    /// `Dwhite`: independent draws, uniform on `[lo, hi]`.
    White,
    /// `Diwhite`: the same, over the integers in `[lo, hi]` inclusive.
    IWhite,
    /// `Dbrown`: a random walk of at most `step`, folded into `[lo, hi]`.
    Brown,
    /// `Dibrown`: the same walk over the integers.
    IBrown,
}

impl RandKind {
    fn is_walk(self) -> bool {
        matches!(self, RandKind::Brown | RandKind::IBrown)
    }

    fn is_int(self) -> bool {
        matches!(self, RandKind::IWhite | RandKind::IBrown)
    }
}

/// `Dwhite(repeats, lo, hi)` / `Dbrown(repeats, lo, hi, step)` and their
/// integer siblings.
///
/// The bounds are read on every pull (so they may be streams, and a `NaN`
/// leaves the last value in place, as with a ramp's step). A walk is **folded**
/// rather than clipped, scsynth's choice and the better one: a walk that
/// clipped would pile up against the bound instead of turning around.
///
/// The draws are scsynth's, from the synth's generator
/// ([`crate::dsp::rand`]): `Dwhite` `lo + frand * (hi - lo)`, `Diwhite` an
/// integer in `[lo, hi]` with both bounds rounded, the walks starting at a
/// draw inside their range and stepping by `frand2 * step` (an integer step
/// of up to `step` either way for `Dibrown`, whose bounds truncate).
pub struct Drandom {
    kind: RandKind,
    rep: Repeats,
    rand: SynthRand,
    lo: f32,
    hi: f32,
    step: f32,
    /// Current position of a walk (unused by the independent draws).
    value: f32,
}

impl Drandom {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(kind: RandKind, seed: u64) -> Self {
        Self::new(kind, SynthRand::detached(seed))
    }

    pub fn new(kind: RandKind, rand: SynthRand) -> Self {
        Self {
            kind,
            rep: Repeats::new(),
            rand,
            lo: 0.0,
            hi: 1.0,
            step: 0.01,
            value: 0.0,
        }
    }

    /// A uniform draw on `[lo, hi)`, or on the integers `[lo, hi]` for an
    /// integer kind -- `Diwhite` rounds its bounds, `Dibrown` truncates them,
    /// as scsynth's do.
    fn draw(&mut self) -> f32 {
        let (lo, hi) = (self.lo, self.hi);
        match self.kind {
            RandKind::IWhite => {
                let lo = round(lo) as i32;
                let range = round(hi) as i32 - lo + 1;
                (self.rand.draw(|g| g.irand(range)) + lo) as f32
            }
            RandKind::IBrown => {
                let (lo, hi) = (lo as i32, hi as i32);
                (self.rand.draw(|g| g.irand(hi - lo + 1)) + lo) as f32
            }
            _ => self.rand.draw(|g| g.frand()) * (hi - lo) + lo,
        }
    }

    /// A walk's step: `frand2 * step`, or an integer of at most `step` either
    /// way.
    fn stride(&mut self) -> f32 {
        if self.kind.is_int() {
            let s = self.step as i32;
            self.rand.draw(|g| g.irand2(s)) as f32
        } else {
            self.rand.draw(|g| g.frand2()) * self.step
        }
    }
}

impl UGen for Drandom {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
        self.rep = Repeats::new();
        self.lo = 0.0;
        self.hi = 1.0;
        self.step = 0.01;
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let lo = inputs.pull(1);
        if !lo.is_nan() {
            self.lo = lo;
        }
        let hi = inputs.pull(2);
        if !hi.is_nan() {
            self.hi = hi;
        }
        if self.kind.is_walk() {
            let step = inputs.pull(3);
            if !step.is_nan() {
                self.step = step;
            }
        }
        // A walk starts somewhere inside its range; the first pull is where
        // the bounds are known, so that is where it is placed.
        if self.rep.begin(inputs) && self.kind.is_walk() {
            self.value = self.draw();
        }
        if self.rep.exhausted() {
            return f32::NAN;
        }
        self.rep.advance();
        if self.kind.is_walk() {
            let v = self.value;
            let stepped = v + self.stride();
            self.value = if self.kind.is_int() {
                fold(stepped, (self.lo as i32) as f32, (self.hi as i32) as f32)
            } else {
                fold(stepped, self.lo, self.hi)
            };
            v
        } else {
            self.draw()
        }
    }

    fn reset_demand(&mut self, _inputs: &mut dyn DemandInputs) {
        self.rep.reset();
    }
}

// ---------------------------------------------------------------------------
// List sources: Dseq, Drand, Dxrand, Dshuf
// ---------------------------------------------------------------------------

/// How a [`Dlist`] picks the next slot of its value list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListOrder {
    /// `Dseq`: in order, over and over.
    Seq,
    /// `Dser`: in order, like `Dseq`, but `repeats` counts items rather than
    /// passes -- `Dser([1, 2, 3], 5)` is `1 2 3 1 2`.
    Ser,
    /// `Drand`: a fresh random slot every item.
    Rand,
    /// `Dwrand`: a fresh random slot every item, each slot as likely as its
    /// weight. The weights sit between the count and the values on the wire
    /// (`repeats, size, w0 .. w(size-1), v0 .. v(size-1)`, sclang's layout).
    Wrand,
    /// `Dxrand`: random, but never the slot just used.
    Xrand,
    /// `Dshuf`: one shuffled order, replayed as it stands.
    Shuf,
}

impl ListOrder {
    /// Whether `repeats` counts **passes over the list** (`Dseq`, `Dshuf`) or
    /// **items yielded** (`Drand`, `Dxrand`). scsynth's asymmetry, kept: a
    /// shuffle without a full pass would not be one, and a random pick has no
    /// pass to complete.
    fn counts_passes(self) -> bool {
        matches!(self, ListOrder::Seq | ListOrder::Shuf)
    }
}

/// `Dseq(repeats, v0, v1, ...)` and its three siblings: a value list traversed
/// `repeats` times (or endlessly), yielding one item per pull.
///
/// **A value may be a stream**, and then it is *drained* rather than taken
/// once: this core reads the slot until it answers `NaN`, and only then moves
/// on, resetting whatever it moves to. So a list flattens its nested streams,
/// which is what makes `Dseq` a sequencer of phrases rather than of numbers.
pub struct Dlist {
    order: ListOrder,
    rep: Repeats,
    rand: SynthRand,
    /// Position within the list, 0-based over the values (input `1 + index`).
    index: usize,
    /// Shuffled slot order (`Dshuf`), rebuilt on every reset.
    perm: [u8; MAX_UGEN_INPUTS],
    /// Whether the slot about to be read must be restarted first -- set when a
    /// slot is left, honoured just before it is read again.
    reset_child: bool,
}

impl Dlist {
    /// One drawing from a table of its own, seeded from `seed`.
    pub fn with_seed(order: ListOrder, seed: u64) -> Self {
        Self::new(order, SynthRand::detached(seed))
    }

    pub fn new(order: ListOrder, rand: SynthRand) -> Self {
        let mut perm = [0u8; MAX_UGEN_INPUTS];
        for (i, p) in perm.iter_mut().enumerate() {
            *p = i as u8;
        }
        Self {
            order,
            rep: Repeats::new(),
            rand,
            index: 0,
            perm,
            reset_child: false,
        }
    }

    /// The input slot the values start at: after the count, and for
    /// `Dwrand` after its size and its weights too.
    fn base(&self, inputs: &dyn DemandInputs) -> usize {
        match self.order {
            ListOrder::Wrand => 2 + weights(inputs),
            _ => 1,
        }
    }

    /// The input slot the current position reads: `Dshuf` goes through its
    /// permutation, everyone else straight at the list.
    fn slot(&self, base: usize, n: usize) -> usize {
        let i = match self.order {
            ListOrder::Shuf => self.perm[self.index.min(MAX_UGEN_INPUTS - 1)] as usize,
            _ => self.index,
        };
        base + i.min(n.saturating_sub(1))
    }

    /// `Dwrand`'s pick: one uniform draw, and the first slot whose running
    /// sum of weights reaches it.
    ///
    /// scsynth compares the draw with the bare weights, so a set that sums to
    /// less than one leaves the remainder to *whatever slot was picked last*
    /// (and to an uninitialized one on the first pick); here the draw is scaled
    /// by the weights' sum, so each slot is as likely as its share of it. A set
    /// that sums to one -- what sclang's `normalizeSum` gives -- draws exactly
    /// as scsynth does.
    fn pick_weighted(&self, n: usize, inputs: &dyn DemandInputs) -> usize {
        let w = weights(inputs).min(n);
        let total: f32 = (0..w).map(|i| inputs.at(2 + i)).sum();
        let r = self.rand.draw(|g| g.frand()) * total;
        let mut sum = 0.0;
        for i in 0..w {
            sum += inputs.at(2 + i);
            if sum >= r {
                return i;
            }
        }
        n - 1
    }

    /// Moves to the next position, counting a repeat where the order says one
    /// has been completed.
    fn step(&mut self, n: usize, inputs: &dyn DemandInputs) {
        match self.order {
            ListOrder::Seq | ListOrder::Shuf => self.index += 1,
            ListOrder::Ser => {
                self.index += 1;
                self.rep.advance();
            }
            ListOrder::Rand => {
                self.index = self.rand.draw(|g| g.irand(n as i32)) as usize;
                self.rep.advance();
            }
            ListOrder::Wrand => {
                self.index = self.pick_weighted(n, inputs);
                self.rep.advance();
            }
            ListOrder::Xrand => {
                // A uniform pick over the other `n - 1` slots: draw one of
                // them, then skip past the current slot if it is at or after
                // it. With one value there is no other slot to pick.
                if n > 1 {
                    let j = self.rand.draw(|g| g.irand(n as i32 - 1)) as usize;
                    self.index = if j < self.index { j } else { j + 1 };
                }
                self.rep.advance();
            }
        }
    }

    /// Fisher-Yates over the first `n` slots, so a `Dshuf` plays one order for
    /// the life of its stream and a new one after a reset. scsynth's order of
    /// draws: forward, slot `i` swapped with one of `i..n`.
    fn shuffle(&mut self, n: usize) {
        let n = n.min(MAX_UGEN_INPUTS);
        for i in 0..n.saturating_sub(1) {
            let j = i + self.rand.draw(|g| g.irand((n - i) as i32)) as usize;
            self.perm.swap(i, j);
        }
    }
}

impl UGen for Dlist {
    fn reset(&mut self) {
        // The generator's stream goes on: a restarted voice draws new numbers.
        self.rep = Repeats::new();
        self.index = 0;
        for (i, p) in self.perm.iter_mut().enumerate() {
            *p = i as u8;
        }
        self.reset_child = false;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let base = self.base(inputs);
        let n = inputs.len().saturating_sub(base);
        if n == 0 {
            return f32::NAN;
        }
        if self.rep.begin(inputs) {
            match self.order {
                ListOrder::Shuf => self.shuffle(n),
                // A random order is random from its *first* item too -- starting
                // at slot 0 and only then drawing would make the head of every
                // `Drand` predictable.
                ListOrder::Rand | ListOrder::Xrand => {
                    self.index = self.rand.draw(|g| g.irand(n as i32)) as usize;
                }
                ListOrder::Wrand => self.index = self.pick_weighted(n, inputs),
                ListOrder::Seq | ListOrder::Ser => {}
            }
        }
        // Every slot that answers `NaN` costs one attempt; `n + 1` of them in a
        // row means the whole list is empty (each was restarted before being
        // read), and the loop must end rather than spin on the audio thread.
        // scsynth only warns here; a callback cannot afford to find out later.
        let mut attempts = n + 1;
        loop {
            if self.index >= n {
                self.index = 0;
                if self.order.counts_passes() {
                    self.rep.advance();
                }
            }
            if self.rep.exhausted() {
                self.index = 0;
                return f32::NAN;
            }
            let k = self.slot(base, n);
            if !inputs.is_demand(k) {
                let x = inputs.pull(k);
                self.step(n, inputs);
                self.reset_child = true;
                return x;
            }
            if self.reset_child {
                self.reset_child = false;
                inputs.reset(k);
            }
            let x = inputs.pull(k);
            if !x.is_nan() {
                return x;
            }
            // That slot is spent: move on, and restart whatever we move to.
            self.step(n, inputs);
            self.reset_child = true;
            attempts -= 1;
            if attempts == 0 {
                return f32::NAN;
            }
        }
    }

    fn reset_demand(&mut self, _inputs: &mut dyn DemandInputs) {
        self.rep.reset();
        self.index = 0;
        // The child a reset stream lands on is restarted before it is read,
        // like any other slot this core moves to.
        self.reset_child = true;
    }
}

/// How many weights a `Dwrand` carries: its size input, which sclang makes
/// the value list's length, held to the inputs there are.
fn weights(inputs: &dyn DemandInputs) -> usize {
    let size = round(inputs.at(1)).max(0.0) as usize;
    size.min(inputs.len().saturating_sub(2) / 2)
}

// ---------------------------------------------------------------------------
// Dstutter, Dswitch1, Dswitch, Dconst, Dreset, Dpoll, Dbufrd, Dbufwr
// ---------------------------------------------------------------------------

/// `Dstutter(n, value)` and `Ddup(n, value)`: repeats each item of `value`
/// `n` times. sclang renamed the first to the second and scsynth keeps both,
/// with the same code, so here they are one machine under two names.
///
/// Both inputs are pulled -- `n` per item, so the repeat count can itself vary --
/// and either running out ends the stream. Unlike the list sources it resets
/// both inputs outright when it is reset, since it has only the one value
/// stream and no notion of moving on.
pub struct Dstutter {
    /// How many times the current value is to be yielded; negative until the
    /// first pull, so the first call fetches one.
    repeats: f32,
    count: f32,
    value: f32,
}

impl Dstutter {
    pub fn new() -> Self {
        Self {
            repeats: -1.0,
            count: 0.0,
            value: 0.0,
        }
    }
}

impl Default for Dstutter {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Dstutter {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        if self.count >= self.repeats {
            let value = inputs.pull(1);
            let repeats = inputs.pull(0);
            if value.is_nan() || repeats.is_nan() {
                return f32::NAN;
            }
            self.value = value;
            self.repeats = round(repeats);
            self.count = 0.0;
        }
        self.count += 1.0;
        self.value
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        self.repeats = -1.0;
        self.count = 0.0;
        inputs.reset(0);
        inputs.reset(1);
    }
}

/// `Dswitch1(index, v0, v1, ...)`: yields **one** item of the stream `index`
/// picks, then picks again on the next pull.
///
/// The `1` in the name is the count: unlike a list source it never drains a
/// slot, so an unselected stream stays exactly where it was. The index wraps
/// into range, so a modulating index cannot fall off the list, and an exhausted
/// index stream ends this one.
pub struct Dswitch1;

impl UGen for Dswitch1 {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let n = inputs.len().saturating_sub(1);
        if n == 0 {
            return f32::NAN;
        }
        let which = inputs.pull(0);
        if which.is_nan() {
            return f32::NAN;
        }
        let i = (round(which) as i64).rem_euclid(n as i64) as usize;
        inputs.pull(1 + i)
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        // Every branch restarts, the selected one included: a switch has no
        // position of its own to rewind, only its inputs'.
        for k in 0..inputs.len() {
            inputs.reset(k);
        }
    }
}

/// `Dswitch(index, v0, v1, ...)`: yields the **whole** stream `index` picks,
/// then picks again -- where [`Dswitch1`] takes one item, this one drains.
///
/// Two departures from scsynth, both where its code contradicts what the UGen
/// is for:
///
/// - **A plain number is a stream of one item**, as it is to every list
///   source. scsynth reads a number again on every pull and never sees it end,
///   so a switch that once picks a number stays on it for good.
/// - **The stream picked is restarted when it is entered**, not when it is
///   left. scsynth pulls the newly picked stream first and resets the old one
///   after, so picking the same stream twice in a row reads it drained and
///   ends the switch; here a stream picked again plays again. (Its constructor
///   and its reset also wrap the first index into one slot too many, which can
///   read past the last input; here every pick wraps into the list.)
pub struct Dswitch {
    /// The input slot playing, `0` before the first pick.
    index: usize,
    /// Whether the slot playing has given all it had (a number, after its
    /// one item).
    spent: bool,
}

impl Dswitch {
    pub fn new() -> Self {
        Self {
            index: 0,
            spent: false,
        }
    }

    /// Pulls the next index and enters the stream it names, restarting it.
    /// False once the index stream is exhausted.
    fn pick(&mut self, inputs: &mut dyn DemandInputs, n: usize) -> bool {
        let which = inputs.pull(0);
        if which.is_nan() {
            return false;
        }
        self.index = 1 + (round(which) as i64).rem_euclid(n as i64) as usize;
        self.spent = false;
        inputs.reset(self.index);
        true
    }

    /// One item of the slot playing; a number is spent once read.
    fn take(&mut self, inputs: &mut dyn DemandInputs) -> f32 {
        if !inputs.is_demand(self.index) {
            self.spent = true;
        }
        inputs.pull(self.index)
    }
}

impl Default for Dswitch {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Dswitch {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let n = inputs.len().saturating_sub(1);
        if n == 0 {
            return f32::NAN;
        }
        let mut fresh = false;
        if self.index == 0 || self.spent {
            if !self.pick(inputs, n) {
                return f32::NAN;
            }
            fresh = true;
        }
        let x = self.take(inputs);
        if !x.is_nan() || fresh {
            // A stream entered just now that has nothing in it ends the
            // switch, as in scsynth: one pick per pull, so an index stream
            // naming only empty streams cannot spin the audio thread.
            return x;
        }
        if !self.pick(inputs, n) {
            return f32::NAN;
        }
        self.take(inputs)
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        self.index = 0;
        self.spent = false;
        inputs.reset(0);
    }
}

/// `Dconst(sum, in, tolerance)`: yields `in` until the items add up to `sum`,
/// the last one cut to land on it -- a stream of durations that fills a bar
/// exactly. Within `tolerance` of the sum counts as there, so a sum of floats
/// that misses by a rounding does not leave a sliver of an item behind.
///
/// `sum` and `tolerance` are read once per stream, on its first pull.
pub struct Dconst {
    total: f32,
    tolerance: f32,
    latched: bool,
    /// What has been yielded so far; `NaN` once the sum is reached.
    sum: f32,
}

impl Dconst {
    pub fn new() -> Self {
        Self {
            total: 0.0,
            tolerance: 0.0,
            latched: false,
            sum: 0.0,
        }
    }
}

impl Default for Dconst {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Dconst {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        if self.sum.is_nan() {
            return f32::NAN;
        }
        if !self.latched {
            let total = inputs.pull(0);
            let tolerance = inputs.pull(2);
            if total.is_nan() || tolerance.is_nan() {
                return f32::NAN;
            }
            self.total = total;
            self.tolerance = tolerance;
            self.latched = true;
        }
        let x = inputs.pull(1);
        if x.is_nan() {
            return f32::NAN;
        }
        let sum = self.sum + x;
        if sum > self.total || (self.total - sum).abs() <= self.tolerance {
            let last = self.total - self.sum;
            self.sum = f32::NAN;
            last
        } else {
            self.sum = sum;
            x
        }
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        self.latched = false;
        self.sum = 0.0;
        for k in 0..3 {
            inputs.reset(k);
        }
    }
}

/// `Dreset(in, reset)`: passes `in` through, and restarts it when the `reset`
/// stream rises -- a reset that is itself a sequence, so a phrase can be cut
/// short on a pattern of its own. The restart takes effect on the next pull:
/// the item read alongside the rising value is still the old stream's.
pub struct Dreset {
    prev: f32,
}

impl Dreset {
    pub fn new() -> Self {
        Self { prev: 0.0 }
    }
}

impl Default for Dreset {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Dreset {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let x = inputs.pull(0);
        if x.is_nan() {
            return f32::NAN;
        }
        let reset = inputs.pull(1);
        if reset > 0.0 && self.prev <= 0.0 {
            inputs.reset(0);
        }
        self.prev = reset;
        x
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        inputs.reset(0);
    }
}

/// `Dpoll(in, trigid, run)`: passes `in` through and, while `run` is above
/// zero, posts `label: value` for every item and, when `trigid >= 0`, sends
/// `/node_trigger nodeID trigid value` -- [`Poll`](crate::dsp::reply::Poll)
/// for a stream, on the same reply path. `run` is pulled with each item, so
/// it may be a stream too.
///
/// scsynth sends the trigger whether `run` is set or not, and gates only the
/// post; `run` is documented as making the UGen inactive, so here it gates
/// both. The label is a static field, as `Poll`'s is, rather than characters
/// spread over the inputs.
pub struct Dpoll {
    label: String,
    buf: ReplyBuffer,
}

impl Dpoll {
    pub fn new(config: &UGenConfig) -> Self {
        let label = config
            .label
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "dpoll".to_string());
        Self {
            label,
            buf: ReplyBuffer::new(),
        }
    }
}

impl UGen for Dpoll {
    // Buffered replies still leave with this block's drain.
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let x = inputs.pull(0);
        if inputs.pull(2) > 0.0 {
            let mut msg = ReplyMsg::new(ReplyKind::Poll, inputs.at(1) as i32, &self.label);
            msg.push_value(x);
            self.buf.push(msg);
        }
        x
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        inputs.reset(0);
    }

    fn is_reply(&self) -> bool {
        true
    }

    fn drain_replies(&mut self, node_id: i32, sink: &mut dyn FnMut(ReplyMsg)) {
        self.buf.drain(node_id, sink);
    }
}

/// The frame a demand buffer UGen addresses: `phase` wrapped into the buffer
/// when `looping`, held to it otherwise (scsynth's `sc_loop`).
fn frame_of(phase: f32, frames: usize, looping: bool) -> usize {
    let pos = if looping {
        phase.rem_euclid(frames as f32)
    } else {
        phase.clamp(0.0, frames as f32 - 1.0)
    };
    (pos as usize).min(frames - 1)
}

/// `Dbufrd(bufnum, phase, loop, channel)`: reads one frame of a buffer at the
/// frame index `phase` yields.
///
/// The natural companion to a demand phase source -- `Dbufrd` with a `Dseries`
/// phase walks a buffer as a step sequence. `channel` sits last so the sclang
/// argument order still reads correctly; it exists because every other buffer
/// reader in this catalog takes one. Out of range it wraps when `loop` is set
/// and clamps when it is not (scsynth also raises its done flag there, which
/// nothing on this side of the demand boundary could read).
pub struct Dbufrd;

impl UGen for Dbufrd {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let bufnum = inputs.pull(0);
        let phase = inputs.pull(1);
        if bufnum.is_nan() || phase.is_nan() {
            return f32::NAN;
        }
        let looping = inputs.at(2) != 0.0;
        let channel = inputs.at(3).max(0.0) as usize;
        let index = bufnum.max(0.0) as usize;
        let Some(buf) = ctx.buffer(index) else {
            return 0.0;
        };
        let frames = buf.frames();
        if frames == 0 {
            return 0.0;
        }
        buf.sample(frame_of(phase, frames, looping), channel)
    }
}

/// `Dbufwr(bufnum, phase, signal, loop, channel)`: writes each item of the
/// `signal` stream into the buffer frame the `phase` stream names, and yields
/// it -- the writing twin of [`Dbufrd`], so a sequence can record what it
/// plays. Either stream running out ends this one; out of range the frame
/// wraps when `loop` is set and clamps when it is not. `channel` sits last,
/// as `Dbufrd`'s does, where scsynth writes the first channel only.
pub struct Dbufwr;

impl UGen for Dbufwr {
    fn reset(&mut self) {}

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let bufnum = inputs.pull(0);
        if bufnum.is_nan() {
            return f32::NAN;
        }
        let phase = inputs.pull(1);
        if phase.is_nan() {
            return f32::NAN;
        }
        let value = inputs.pull(2);
        if value.is_nan() {
            return f32::NAN;
        }
        let looping = inputs.at(3) != 0.0;
        let channel = inputs.at(4).max(0.0) as usize;
        let Some(buf) = ctx.buffer(bufnum.max(0.0) as usize) else {
            return 0.0;
        };
        let frames = buf.frames();
        if frames == 0 {
            return 0.0;
        }
        buf.set_sample(frame_of(phase, frames, looping), channel, value);
        value
    }

    fn reset_demand(&mut self, inputs: &mut dyn DemandInputs) {
        inputs.reset(1);
        inputs.reset(2);
    }
}

// ---------------------------------------------------------------------------
// Drivers: Demand, Duty, TDuty
// ---------------------------------------------------------------------------

/// Input slot of [`Demand`]'s source. Its own business, not a shared rule --
/// the other drivers name streams in more than one slot.
const DEMAND_SOURCE: usize = 2;

/// `Demand(trig, reset, source)`: pulls one value on each rising edge of
/// `trig` and holds it until the next; a rising edge of `reset` restarts the
/// source. The output is `0` before the first trigger, and holds the last
/// value once the source is exhausted.
pub struct Demand {
    held: f32,
    prev_trig: Edge,
    prev_reset: Edge,
}

impl Demand {
    pub fn new() -> Self {
        Self {
            held: 0.0,
            prev_trig: Edge::default(),
            prev_reset: Edge::default(),
        }
    }
}

impl Default for Demand {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for Demand {
    fn reset(&mut self) {
        *self = Self::new();
    }

    // The synth drives this via `drive`; `process` is never called.
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn drive(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs, output: &mut [f32]) {
        for (i, out) in output.iter_mut().enumerate() {
            inputs.seek(i);
            if self.prev_reset.rose(inputs.at(1)) {
                inputs.reset(DEMAND_SOURCE);
            }
            if self.prev_trig.rose(inputs.at(0)) {
                let v = inputs.pull(DEMAND_SOURCE);
                if v.is_finite() {
                    self.held = v; // NaN = exhausted: hold the last value
                }
            }
            *out = self.held;
        }
    }
}

/// What a [`Duty`] does with the value it pulls.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DutyKind {
    /// `Duty`: hold it until the next one is due.
    Hold,
    /// `TDuty`: emit it on that one sample and nothing in between -- a stream
    /// of triggers whose amplitudes come from `level`.
    Trigger,
}

/// `Duty(dur, reset, level, done_action)` and
/// `TDuty(dur, reset, level, done_action, gap_first)`: a driver with its own
/// clock. Every `dur` seconds it pulls one `level`, so it needs no external
/// trigger -- where [`Demand`] is told when to pull, this one decides.
///
/// Both `dur` and `level` are pulled, and that is the point: a `Dseq` of
/// durations against a `Dseq` of pitches is a sequencer, in two streams that
/// need not be the same length. When either runs out the stream ends and
/// `done_action` fires (`Done`/`FreeSelf`).
///
/// The countdown is kept in `f64` and carries its remainder across pulls
/// (`count += dur * sr` rather than `count = dur * sr`), so a duration that is
/// not a whole number of samples does not drift: a hundred sixteenths of a
/// beat land where the hundredth sixteenth belongs, not a sample short.
pub struct Duty {
    kind: DutyKind,
    /// Samples remaining before the next pull.
    count: f64,
    /// Last level pulled -- what `Duty` holds between pulls.
    level: f32,
    prev_reset: Edge,
    /// Set once a stream has ended; cleared by a reset.
    finished: bool,
    /// False until the first slice, where `gap_first` is first readable.
    primed: bool,
    /// Action to take on the node when the stream ends.
    action: DoneAction,
    /// `TDuty` only: the next pull is a silent one, opening with a gap.
    gap_pending: bool,
}

impl Duty {
    pub fn new(kind: DutyKind) -> Self {
        Self {
            kind,
            count: 0.0,
            level: 0.0,
            prev_reset: Edge::default(),
            finished: false,
            primed: false,
            action: DoneAction::None,
            gap_pending: false,
        }
    }

    /// Starts (or restarts) the clock. `gap_first` is read here rather than
    /// latched at build: it is an ordinary input like any other, so a reset
    /// re-reads it and a def may map it to a control.
    fn restart(&mut self, inputs: &mut dyn DemandInputs) {
        inputs.reset(0);
        inputs.reset(2);
        self.count = 0.0;
        self.finished = false;
        self.gap_pending = self.kind == DutyKind::Trigger && inputs.at(4) != 0.0;
    }
}

impl UGen for Duty {
    fn reset(&mut self) {
        *self = Self::new(self.kind);
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn drive(&mut self, ctx: &ProcessCtx, inputs: &mut dyn DemandInputs, output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        for (i, out) in output.iter_mut().enumerate() {
            inputs.seek(i);
            // `gap_first` is only readable once a slice is running, so the
            // opening gap is armed on the first sample rather than at build.
            if !self.primed {
                self.primed = true;
                self.gap_pending = self.kind == DutyKind::Trigger && inputs.at(4) != 0.0;
            }
            if self.prev_reset.rose(inputs.at(1)) {
                self.restart(inputs);
            }
            // Between pulls (and after the last one) `Duty` holds its level and
            // `TDuty` is silent. A finished stream stops pulling entirely --
            // otherwise a `NaN` duration would ask for a value every sample.
            if self.count > 0.0 || self.finished {
                self.count -= 1.0;
                *out = match self.kind {
                    DutyKind::Hold => self.level,
                    DutyKind::Trigger => 0.0,
                };
                continue;
            }
            self.action = DoneAction::from_u8(inputs.at(3).max(0.0) as u8);
            let dur = inputs.pull(0);
            if dur.is_nan() {
                self.finished = true;
            } else {
                self.count += (dur as f64 * sr).max(0.0);
            }
            // The gap `TDuty`'s `gap_first` asks for: one duration spent before
            // the first level is pulled at all.
            if self.gap_pending {
                self.gap_pending = false;
                self.count -= 1.0;
                *out = 0.0;
                continue;
            }
            let level = inputs.pull(2);
            if level.is_nan() {
                self.finished = true;
            } else {
                self.level = level;
            }
            self.count -= 1.0;
            *out = match self.kind {
                // A trigger is the level on its own sample, and nothing once
                // the stream has ended.
                DutyKind::Trigger if self.finished => 0.0,
                _ => self.level,
            };
        }
    }

    fn done(&self) -> DoneAction {
        if self.finished {
            self.action
        } else {
            DoneAction::None
        }
    }

    fn is_done(&self) -> bool {
        self.finished
    }
}

// ---------------------------------------------------------------------------
// DemandEnvGen
// ---------------------------------------------------------------------------

/// `DemandEnvGen`'s input slots: four streams, then what it reads as values.
const DE_LEVEL: usize = 0;
const DE_DUR: usize = 1;
const DE_SHAPE: usize = 2;
const DE_CURVE: usize = 3;
const DE_GATE: usize = 4;
const DE_RESET: usize = 5;
const DE_SCALE: usize = 6;
const DE_BIAS: usize = 7;
const DE_TIME: usize = 8;
const DE_DONE: usize = 9;

/// The custom-curvature shape number, which a curvature this close to zero
/// turns into a line, as `EnvGen` does.
const SHAPE_CURVE: i32 = 5;
const SHAPE_LINEAR: i32 = 1;

/// `DemandEnvGen(level, dur, shape, curve, gate, reset, levelScale,
/// levelBias, timeScale, doneAction)`: an envelope whose breakpoints are
/// streams. Each segment pulls its duration, its shape, its curvature and its
/// end level, and runs from wherever the output is to that level in that
/// shape -- a sequencer of glides, `EnvGen`'s segments with no list to end.
///
/// - `gate` at or above one runs; between zero and one releases at the end of
///   the segment playing (the output holds there, and the done action fires,
///   until the gate is back at one); at or below zero holds where it is.
/// - A rising `reset` restarts the four streams. Up to one it discards their
///   first level and glides on from the current one; above one it jumps there.
/// - When the duration or the level stream ends, the envelope holds its last
///   level and its done action fires.
///
/// A segment's shape is `EnvGen`'s, evaluated by the same function
/// (`clausters_core::envshape`), and the output follows `EnvGen`'s convention:
/// a segment's first sample is its start level and its end level is the next
/// segment's first. The departures from scsynth's code, each a fault of it:
///
/// - scsynth advances before it writes and its constructor runs a sample of
///   its own, so its output starts two steps into the first segment and never
///   shows the first level.
/// - Its segments step by `1 / length` with a fractional length while running
///   a whole number of samples, so every shape but the custom curve overshoots
///   its end level by up to a step; here a segment of `n` samples lands on its
///   end level exactly.
/// - The first level, and the level a reset jumps to, are scaled and biased
///   like every other here; scsynth leaves those two raw.
/// - A control-rate `reset` restarts once per rise; scsynth restarts on every
///   sample of the block it rises in. A control-rate `gate` acts on the block
///   it arrives in; scsynth acts on it a block late.
/// - The duration stream ending ends the envelope; scsynth turns it into a
///   segment of the largest float's length, which never ends.
/// - The hold shape holds and then reaches its end level, as `EnvGen`'s does;
///   scsynth has no case for it and never leaves the start level.
/// - Its rule that a segment shorter than two samples is a line uses the
///   duration before `timeScale`; here no shape needs one, since every shape
///   lands on its end level in however many samples it has.
pub struct DemandEnvGen {
    /// False until the first slice, which reads the first level.
    primed: bool,
    /// The output now.
    level: f32,
    /// The segment playing, its end level, how far in and how long it is.
    segment: Segment,
    end: f32,
    active: bool,
    pos: f64,
    len: f64,
    /// Samples left before the next segment, carrying the fraction a
    /// duration leaves so that a run of segments keeps time.
    phase: f64,
    /// The last shape and curvature pulled, kept when their streams end.
    shape: i32,
    curve: f32,
    prev_reset: Edge,
    /// A gate between zero and one asked for a release at the segment's end.
    release_pending: bool,
    /// Held at a release, until the gate is back at one.
    released: bool,
    /// A stream ended.
    finished: bool,
    action: DoneAction,
}

impl DemandEnvGen {
    pub fn new() -> Self {
        Self {
            primed: false,
            level: 0.0,
            segment: Segment::new(SHAPE_LINEAR, 0.0, 0.0, 0.0),
            end: 0.0,
            active: false,
            pos: 0.0,
            len: 1.0,
            phase: 0.0,
            shape: SHAPE_LINEAR,
            curve: 0.0,
            prev_reset: Edge::default(),
            release_pending: false,
            released: false,
            finished: false,
            action: DoneAction::None,
        }
    }

    /// A pulled level, scaled and biased.
    fn scaled(inputs: &dyn DemandInputs, x: f32) -> f32 {
        x * inputs.at(DE_SCALE) + inputs.at(DE_BIAS)
    }

    /// Pulls the next segment and starts it from the output. False when the
    /// duration or the level stream has ended.
    fn next_segment(&mut self, inputs: &mut dyn DemandInputs, sr: f64) -> bool {
        let dur = inputs.pull(DE_DUR);
        if dur.is_nan() {
            return false;
        }
        self.phase += dur as f64 * inputs.at(DE_TIME) as f64 * sr;
        let shape = inputs.pull(DE_SHAPE);
        if !shape.is_nan() {
            self.shape = shape as i32;
        }
        let curve = inputs.pull(DE_CURVE);
        if !curve.is_nan() {
            self.curve = curve;
        }
        let level = inputs.pull(DE_LEVEL);
        if level.is_nan() {
            return false;
        }
        self.end = Self::scaled(inputs, level);
        let shape = if self.shape == SHAPE_CURVE && self.curve.abs() < 0.001 {
            SHAPE_LINEAR
        } else {
            self.shape
        };
        self.segment = Segment::new(shape, self.curve, self.level, self.end);
        self.len = self.phase.ceil().max(1.0);
        self.pos = 0.0;
        self.active = true;
        true
    }
}

impl Default for DemandEnvGen {
    fn default() -> Self {
        Self::new()
    }
}

impl UGen for DemandEnvGen {
    fn reset(&mut self) {
        *self = Self::new();
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn drive(&mut self, ctx: &ProcessCtx, inputs: &mut dyn DemandInputs, output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        if !self.primed {
            self.primed = true;
            inputs.seek(0);
            let first = inputs.pull(DE_LEVEL);
            self.level = if first.is_nan() {
                0.0
            } else {
                Self::scaled(inputs, first)
            };
            self.end = self.level;
        }
        for (i, out) in output.iter_mut().enumerate() {
            inputs.seek(i);
            let reset = inputs.at(DE_RESET);
            if self.prev_reset.rose(reset) {
                for k in [DE_LEVEL, DE_DUR, DE_SHAPE, DE_CURVE] {
                    inputs.reset(k);
                }
                let first = inputs.pull(DE_LEVEL);
                if reset > 1.0 && !first.is_nan() {
                    self.level = Self::scaled(inputs, first);
                }
                self.active = false;
                self.phase = 0.0;
                self.release_pending = false;
                self.released = false;
                self.finished = false;
            }
            let gate = inputs.at(DE_GATE);
            if gate >= 1.0 {
                self.released = false;
            } else if gate > 0.0 {
                self.release_pending = true;
            }
            self.action = DoneAction::from_i32(inputs.at(DE_DONE) as i32);
            let running = gate > 0.0 && !self.released && !self.finished;
            if running && self.phase <= 0.0 {
                // A release waits for a segment to end: one asked for before
                // the first segment (or right after a reset) lets that segment
                // play.
                let landed = self.active;
                if landed {
                    self.level = self.end;
                    self.active = false;
                }
                if landed && self.release_pending {
                    self.release_pending = false;
                    self.released = true;
                } else if !self.next_segment(inputs, sr) {
                    self.finished = true;
                }
            }
            if self.active {
                self.level = self.segment.at((self.pos / self.len) as f32);
                if running {
                    self.pos += 1.0;
                    self.phase -= 1.0;
                }
            }
            *out = self.level;
        }
    }

    fn done(&self) -> DoneAction {
        if self.finished || self.released {
            self.action
        } else {
            DoneAction::None
        }
    }

    fn is_done(&self) -> bool {
        self.finished || self.released
    }
}
