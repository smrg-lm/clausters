//! The demand family (`dr`): streams that are *pulled* rather than run.
//!
//! A demand UGen is not in the block-execution order. It is a **sub-graph its
//! driver owns**: the driver ([`Demand`], [`Duty`]) decides when a value is
//! needed and pulls one; the source ([`Dlist`], [`Dramp`], …) yields the next
//! item of its stream, or `NaN` once it has none left. Between two pulls the
//! source does nothing at all — a stream has no samples, only a next value.
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
//! drained resets it first, so the child's stream replays — that is what makes
//! `Dseq(2, Dseries(3, 0, 1))` give `0 1 2 0 1 2` rather than `0 1 2` and
//! silence. The reset is **lazy** (marked when the child is left, performed
//! just before it is read again), because doing it eagerly would restart a
//! child the parent may never come back to. Reset propagation is per kind, not
//! a blanket rule: the list sources reset the child they move to, `Dstutter`
//! and `Dswitch1` reset their inputs outright, and the scalar sources
//! (`Dseries`, `Dwhite`, …) propagate nothing, since they read their bounds
//! afresh on every pull anyway. This mirrors scsynth, where the same asymmetry
//! is visible in which `_next` functions call `RESETINPUT`.
//!
//! **`repeats` and the endless stream.** sclang says `inf`; a def cannot. Our
//! wire format rejects a non-finite constant (and JSON has no spelling for
//! one), so **`repeats <= 0` is the endless stream** here — which is also what
//! a client would guess from a count. A positive count behaves exactly as
//! scsynth's, `inf` still works if a client manages to send it, and a `NaN`
//! (an exhausted stream feeding the count) still means zero, since there the
//! number is a value rather than a request.
//!
//! The cores, one per family, with the scsynth names on the wire:
//!
//! - [`Dramp`] — `Dseries` (add) and `Dgeom` (multiply): the same walk with a
//!   different step operator.
//! - [`Drandom`] — `Dwhite`/`Diwhite` (independent draws) and
//!   `Dbrown`/`Dibrown` (a bounded random walk), float or integer.
//! - [`Dlist`] — `Dseq`, `Drand`, `Dxrand` and `Dshuf`: one traversal of a
//!   value list, differing only in what picks the next slot. This is the core
//!   that flattens nested streams, so all four inherit it.
//! - [`Dstutter`], [`Dswitch1`], [`Dbufrd`] — one machine each.
//! - [`Demand`] and [`Duty`] — the drivers, which turn pulls into samples.

use clausters_core::builtins::fold;
use clausters_core::rng;

use crate::dsp::noise::next_seed;
use crate::dsp::trig::Edge;
use crate::dsp::{DemandInputs, DoneAction, MAX_UGEN_INPUTS, ProcessCtx, UGen};

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
    /// whether this *was* that first pull — the moment a source also latches
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

/// A non-negative integer width for a draw, capped well short of where `u64`
/// arithmetic would wrap. A range wider than a billion is past anything an
/// integer stream means, and the cap is what keeps a nonsense `hi` from
/// overflowing the draw rather than merely being useless.
fn span(width: f32) -> u64 {
    width.clamp(0.0, 1e9) as u64
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
/// there — an exhausted step stream — leaves the last step in place rather than
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
/// Randomness comes from [`clausters_core::rng`], seeded per instance from the
/// same shared counter the noise generators use — two `Dwhite`s in one graph
/// must not draw the same stream — and reproducible from an explicit seed.
pub struct Drandom {
    kind: RandKind,
    rep: Repeats,
    rng: rng::Rng,
    lo: f32,
    hi: f32,
    step: f32,
    /// Current position of a walk (unused by the independent draws).
    value: f32,
}

impl Drandom {
    pub fn new(kind: RandKind) -> Self {
        Self::with_seed(kind, next_seed())
    }

    pub fn with_seed(kind: RandKind, seed: u64) -> Self {
        Self {
            kind,
            rep: Repeats::new(),
            rng: rng::Rng::from_seed(seed),
            lo: 0.0,
            hi: 1.0,
            step: 0.01,
            value: 0.0,
        }
    }

    /// A uniform draw on `[lo, hi]` — over the integers when the kind is an
    /// integer one, where both ends are included.
    fn draw(&mut self) -> f32 {
        if self.kind.is_int() {
            let lo = self.lo.floor();
            let hi = self.hi.floor();
            lo + self.rng.next_below(span(hi - lo) + 1) as f32
        } else {
            self.rng.uniform(self.lo as f64, self.hi as f64) as f32
        }
    }

    /// A step of at most `step` in either direction, integer or not.
    fn stride(&mut self) -> f32 {
        if self.kind.is_int() {
            let s = span(self.step.abs());
            self.rng.next_below(s * 2 + 1) as f32 - s as f32
        } else {
            (self.rng.next_f64() * 2.0 - 1.0) as f32 * self.step
        }
    }
}

impl UGen for Drandom {
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
            self.value = fold(stepped, self.lo, self.hi);
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
    /// `Drand`: a fresh random slot every item.
    Rand,
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

/// `Dseq(repeats, v0, v1, …)` and its three siblings: a value list traversed
/// `repeats` times (or endlessly), yielding one item per pull.
///
/// **A value may be a stream**, and then it is *drained* rather than taken
/// once: this core reads the slot until it answers `NaN`, and only then moves
/// on, resetting whatever it moves to. So a list flattens its nested streams,
/// which is what makes `Dseq` a sequencer of phrases rather than of numbers.
pub struct Dlist {
    order: ListOrder,
    rep: Repeats,
    rng: rng::Rng,
    /// Position within the list, 0-based over the values (input `1 + index`).
    index: usize,
    /// Shuffled slot order (`Dshuf`), rebuilt on every reset.
    perm: [u8; MAX_UGEN_INPUTS],
    /// Whether the slot about to be read must be restarted first — set when a
    /// slot is left, honoured just before it is read again.
    reset_child: bool,
}

impl Dlist {
    pub fn new(order: ListOrder) -> Self {
        Self::with_seed(order, next_seed())
    }

    pub fn with_seed(order: ListOrder, seed: u64) -> Self {
        let mut perm = [0u8; MAX_UGEN_INPUTS];
        for (i, p) in perm.iter_mut().enumerate() {
            *p = i as u8;
        }
        Self {
            order,
            rep: Repeats::new(),
            rng: rng::Rng::from_seed(seed),
            index: 0,
            perm,
            reset_child: false,
        }
    }

    /// The input slot the current position reads: `Dshuf` goes through its
    /// permutation, everyone else straight at the list.
    fn slot(&self, n: usize) -> usize {
        let i = match self.order {
            ListOrder::Shuf => self.perm[self.index.min(MAX_UGEN_INPUTS - 1)] as usize,
            _ => self.index,
        };
        1 + i.min(n.saturating_sub(1))
    }

    /// Moves to the next position, counting a repeat where the order says one
    /// has been completed.
    fn step(&mut self, n: usize) {
        match self.order {
            ListOrder::Seq | ListOrder::Shuf => self.index += 1,
            ListOrder::Rand => {
                self.index = self.rng.next_below(n as u64) as usize;
                self.rep.advance();
            }
            ListOrder::Xrand => {
                // A uniform pick over the other `n - 1` slots: draw one of
                // them, then skip past the current slot if it is at or after
                // it. With one value there is no other slot to pick.
                if n > 1 {
                    let j = self.rng.next_below(n as u64 - 1) as usize;
                    self.index = if j < self.index { j } else { j + 1 };
                }
                self.rep.advance();
            }
        }
    }

    /// Fisher-Yates over the first `n` slots, so a `Dshuf` plays one order for
    /// the life of its stream and a new one after a reset.
    fn shuffle(&mut self, n: usize) {
        for i in (1..n.min(MAX_UGEN_INPUTS)).rev() {
            let j = self.rng.next_below(i as u64 + 1) as usize;
            self.perm.swap(i, j);
        }
    }
}

impl UGen for Dlist {
    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn demand(&mut self, _ctx: &ProcessCtx, inputs: &mut dyn DemandInputs) -> f32 {
        let n = inputs.len().saturating_sub(1);
        if n == 0 {
            return f32::NAN;
        }
        if self.rep.begin(inputs) {
            match self.order {
                ListOrder::Shuf => self.shuffle(n),
                // A random order is random from its *first* item too — starting
                // at slot 0 and only then drawing would make the head of every
                // `Drand` predictable.
                ListOrder::Rand | ListOrder::Xrand => {
                    self.index = self.rng.next_below(n as u64) as usize;
                }
                ListOrder::Seq => {}
            }
        }
        // Every slot that answers `NaN` costs one attempt; `n + 1` of them in a
        // row means the whole list is empty (each was restarted before being
        // read), and the loop must end rather than spin on the audio thread.
        // scsynth only warns here; a callback cannot afford to find out later.
        let mut attempts = n + 1;
        loop {
            if self.order.counts_passes() && self.index >= n {
                self.index = 0;
                self.rep.advance();
            }
            if self.rep.exhausted() {
                self.index = 0;
                return f32::NAN;
            }
            let k = self.slot(n);
            if !inputs.is_demand(k) {
                let x = inputs.pull(k);
                self.step(n);
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
            self.step(n);
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

// ---------------------------------------------------------------------------
// Dstutter, Dswitch1, Dbufrd
// ---------------------------------------------------------------------------

/// `Dstutter(n, value)`: repeats each item of `value` `n` times.
///
/// Both inputs are pulled — `n` per item, so the repeat count can itself vary —
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

/// `Dswitch1(index, v0, v1, …)`: yields **one** item of the stream `index`
/// picks, then picks again on the next pull.
///
/// The `1` in the name is the count: unlike a list source it never drains a
/// slot, so an unselected stream stays exactly where it was. The index wraps
/// into range, so a modulating index cannot fall off the list, and an exhausted
/// index stream ends this one.
pub struct Dswitch1;

impl UGen for Dswitch1 {
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

/// `Dbufrd(bufnum, phase, loop, channel)`: reads one frame of a buffer at the
/// frame index `phase` yields.
///
/// The natural companion to a demand phase source — `Dbufrd` with a `Dseries`
/// phase walks a buffer as a step sequence. `channel` sits last so the sclang
/// argument order still reads correctly; it exists because every other buffer
/// reader in this catalog takes one. Out of range it wraps when `loop` is set
/// and clamps when it is not (scsynth also raises its done flag there, which
/// nothing on this side of the demand boundary could read).
pub struct Dbufrd;

impl UGen for Dbufrd {
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
        let Some(buf) = ctx.buffers.get(index).and_then(|b| b.as_deref()) else {
            return 0.0;
        };
        let frames = buf.frames();
        if frames == 0 {
            return 0.0;
        }
        let last = frames as f32 - 1.0;
        let pos = if looping {
            phase.rem_euclid(frames as f32)
        } else {
            phase.clamp(0.0, last)
        };
        buf.sample((pos as usize).min(frames - 1), channel)
    }
}

// ---------------------------------------------------------------------------
// Drivers: Demand, Duty, TDuty
// ---------------------------------------------------------------------------

/// Input slot of [`Demand`]'s source. Its own business, not a shared rule —
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
    /// `TDuty`: emit it on that one sample and nothing in between — a stream
    /// of triggers whose amplitudes come from `level`.
    Trigger,
}

/// `Duty(dur, reset, level, done_action)` and
/// `TDuty(dur, reset, level, done_action, gap_first)`: a driver with its own
/// clock. Every `dur` seconds it pulls one `level`, so it needs no external
/// trigger — where [`Demand`] is told when to pull, this one decides.
///
/// Both `dur` and `level` are pulled, and that is the point: a `Dseq` of
/// durations against a `Dseq` of pitches is a sequencer, in two streams that
/// need not be the same length. When either runs out the stream ends and
/// `done_action` fires (`Done`/`FreeSelf`, U4's set).
///
/// The countdown is kept in `f64` and carries its remainder across pulls
/// (`count += dur * sr` rather than `count = dur * sr`), so a duration that is
/// not a whole number of samples does not drift: a hundred sixteenths of a
/// beat land where the hundredth sixteenth belongs, not a sample short.
pub struct Duty {
    kind: DutyKind,
    /// Samples remaining before the next pull.
    count: f64,
    /// Last level pulled — what `Duty` holds between pulls.
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
            // `TDuty` is silent. A finished stream stops pulling entirely —
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
