//! **One geometry for every box that lives on a time axis**: a note in a roll
//! and a clip on a lane.
//!
//! A note's rectangle and a clip's rectangle are the same object with respect
//! to editing and positioning — both are a span `[offset, offset + dur)` on a
//! **row**, grabbed by one of three parts, snapped to a grid, floored at a
//! shortest length, and bounded by whatever domain they sit in. They differ in
//! what they *contain* and in what they *send*, and in nothing else. So the
//! arithmetic is written here once and both call it, on the crate's standing
//! rule that a model and its hit-test primitives are extracted once and reused
//! (`graphics::pianoroll`'s own module doc says the same about `bpf`).
//!
//! **What is unified is the behaviour, never the storage.** A note stays five
//! numbers in a contiguous `Vec<Note>`; a clip stays a widget in the tree.
//! Making either the other's shape was measured and rejected: a `Widget` is
//! 13.5x a `Note` and the layout pass that places it runs per frame *and* per
//! pointer event. Hence [`Placements`], an accessor over **indexed** storage
//! rather than a common item type — monomorphised over a slice of notes it
//! compiles to the contiguous access that is there today, and over a lane's
//! clip children it reads the tree.
//!
//! The pixel mappings stay with their renderers — `graphics::pianoroll` maps a
//! pitch to a row and `interact::coords` maps a cursor to a sample — and both
//! hand the *numbers* to this module.

/// Which part of a box a press grabbed: its body (move) or one of its edges
/// (resize).
///
/// The same three for a note and a clip. What differs is how the strips are
/// read off the picture — a clip's are the grips the renderer draws
/// ([`track::clip_grips`]), a note's are a margin at each end of the bar
/// ([`part_at`]) — and that is a drawing question, answered where the drawing
/// is.
///
/// [`track::clip_grips`]: super::graphics::track::clip_grips
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    Body,
    Start,
    End,
}

/// **Where a box sits, how long it is, and which part of its contents it
/// shows.**
///
/// The three move together and that is the whole of what an edge drag means: a
/// box is a window onto a segment of data, so pulling its **start** edge to the
/// right hides the contents's head rather than compressing it — the offset, the
/// duration and the window's `start` all advance by the same amount. Its **end**
/// edge changes only the duration, since the head of the window has not moved.
///
/// `start` is the source frame the box's own time zero reads. A note has no
/// source to window, so its `start` is always `0.0` and its accessor ignores
/// what a drag writes there.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Placement {
    pub offset: f64,
    pub dur: f64,
    /// The source frame the box's own time zero reads.
    pub start: f64,
}

/// What the contents behind a box allows a drag to do: how many frames there
/// are, and whether the window may run off them.
///
/// `total` is `None` for a box with no contents to run off — a roll, a bare
/// automation, **a note** — and then an edge drag is bounded by nothing but the
/// box's own floor and the domain it sits in.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Contents {
    pub total: Option<f64>,
    pub looping: bool,
}

impl Contents {
    /// Whether an edge may be pulled past the contents — a **looping** box's
    /// may, because past the end is the beginning again and before the start is
    /// the tail of the iteration before.
    fn unbounded(&self) -> bool {
        self.looping || self.total.is_none()
    }
}

/// The shortest a **drag** may leave a box: **one sample**, the smallest length
/// the axis addresses.
///
/// A box that can be dragged to nothing is gone for good — zero duration draws
/// no rectangle, so there is nothing left to press, and the piece keeps a clip
/// or a note nobody can see or reach. One sample is the whole of the floor: it
/// is a **length in the axis' own units, never a count of pixels** — the same
/// rule the time selection follows — so the same drag stops at the same place at
/// every zoom, and a reader zoomed in to the sample can keep trimming right down
/// to the grain.
///
/// It is deliberately **not the `snap` grid**, which was the first answer here
/// and was wrong in both directions: zoomed in it refused to trim below a grid
/// step the axis can plainly resolve, and zoomed out it made the shortest
/// possible box a different length on every lane. The grid is where an edge
/// *lands*, not how short a box may be. Keeping a box that short *visible* is
/// the drawing's job (`track::clip_x_range`).
pub const MIN_DUR: f64 = 1.0;

/// The device-pixel grab margin for a box's start/end edges, where the picture
/// draws no grip of its own.
pub const EDGE_PX: f32 = 4.0;

/// The far edge of the **domain** a box lives in, or `None` for a domain with
/// no far edge.
///
/// A roll standing on its own has none — its content is what it spans, so a
/// note dragged rightwards simply lengthens it, the axis has somewhere further
/// to go and nothing is lost. A roll drawn as a **clip's body** has one: the
/// clip's own `dur`, past which a note would still exist and be drawn by no
/// pixel, since the body is clipped to the rectangle. What is edited has to
/// stay visible, so the note stops at the edge and the clip's length stays what
/// its own edge says it is — content does not silently lengthen the thing
/// containing it. A clip on a lane has none, for the same reason the roll does
/// not: the lane is as long as its clips.
pub type Limit = Option<f64>;

/// What bounds one drag: the grid its edges land on, the shortest it may leave
/// the box, and the far edge of the **domain** the box lives in.
///
/// `limit` is not the contents: [`Contents`] is what lies *behind* the box and
/// [`Limit`] is how far the thing *containing* it lets it go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// The drag grid an edge lands on; `0` means whole samples.
    pub grid: f64,
    /// The shortest a drag may leave the box.
    pub min_dur: f64,
    /// The far edge of the domain the box lives in.
    pub limit: Limit,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            grid: 0.0,
            min_dur: MIN_DUR,
            limit: None,
        }
    }
}

/// Snaps a timeline sample value to a drag grid: to the nearest multiple of
/// `grid` when it is positive, else to a whole sample.
///
/// The axis' unit is the sample, so "no grid" is still a grid — the finest one
/// there is. Two spellings of this used to exist, and the roll's returned the
/// raw value while its own doc said whole units.
pub fn snap(v: f64, grid: f64) -> f64 {
    if grid > 0.0 {
        (v / grid).round() * grid
    } else {
        v.round()
    }
}

/// The part of a box spanning pixels `[x0, x1]` that `x` fell on — edges before
/// the body, and only when the bar is wide enough to carry two edge zones and
/// still have a body between them.
pub fn part_at(x0: f32, x1: f32, x: f32) -> Part {
    if x1 - x0 < 3.0 * EDGE_PX {
        return Part::Body;
    }
    if x - x0 <= EDGE_PX {
        Part::Start
    } else if x1 - x <= EDGE_PX {
        Part::End
    } else {
        Part::Body
    }
}

/// **The placement one drag step produces**, against the press-time snapshot
/// (`orig`) so a clamped edge never drifts.
///
/// `target` is where the grabbed part is being pulled to, on the box's own
/// axis: the new **offset** for a body or a start drag, the new **end** for an
/// end drag. A body drag moves the offset, an edge drag trims — never below the
/// floor, never past the contents unless the box loops, never past the domain's
/// far edge, and the start stays within `[0, end]`.
pub fn drag(
    part: Part,
    target: f64,
    orig: Placement,
    contents: Contents,
    bounds: Bounds,
) -> Placement {
    let end = orig.offset + orig.dur;
    // A box already shorter than the floor is not *grown* to it — a drag moves
    // the edge it was given hold of, and snapping the far end out to a minimum
    // nobody asked for is an edit of its own. It simply cannot shrink further.
    let floor = bounds.min_dur.min(orig.dur.max(0.0));
    match part {
        Part::Body => Placement {
            offset: place_body(snap(target, bounds.grid), orig.dur, bounds.limit),
            ..orig
        },
        Part::End => {
            let mut new_end = snap(target, bounds.grid);
            // The domain runs out where the picture does: a note in a clip's
            // body stops at the clip's own length, because past it the note is
            // still in the list and drawn by no pixel.
            if let Some(limit) = bounds.limit {
                new_end = new_end.min(limit);
            }
            // The contents runs out where the window does: without a loop the
            // end edge stops at the last frame, because past it there is
            // nothing to show and nothing to play.
            if let Some(total) = contents.total.filter(|_| !contents.unbounded()) {
                new_end = new_end.min(orig.offset + (total - orig.start).max(floor));
            }
            Placement {
                dur: new_end.max(orig.offset + floor) - orig.offset,
                ..orig
            }
        }
        Part::Start => {
            // A start drag holds the end still, so it needs no far edge of its
            // own: an edge already inside stays inside.
            let mut new_off = snap(target, bounds.grid).min(end - floor).max(0.0);
            // ...and the same at the head: the window cannot begin before the
            // contents does unless the box loops, where what lies before frame
            // zero is the tail of the iteration before it.
            if !contents.unbounded() {
                new_off = new_off.max(orig.offset - orig.start);
            }
            Placement {
                offset: new_off,
                dur: end - new_off,
                // The trim: the window's head travels with the edge, which is
                // what makes an edge drag a trim and not a squeeze.
                start: orig.start + (new_off - orig.offset),
            }
        }
    }
}

/// Where a box of `dur` may sit inside a domain: past zero, and near enough the
/// far edge that its **tail** still lands inside.
///
/// A box is clamped whole rather than by its onset, which is the difference
/// between one that stops at the edge and one whose head stops there while the
/// rest of it goes over — the part that would vanish being exactly the part
/// being dragged. A box longer than the whole domain pins to zero: its tail
/// cannot fit, so the near edge is the one that can be honoured.
fn place_body(offset: f64, dur: f64, limit: Limit) -> f64 {
    let last = limit.map_or(f64::INFINITY, |l| l - dur.max(0.0));
    offset.min(last).max(0.0)
}

/// **Cut a box in two** at `at`, a position on the box's own axis strictly
/// inside its span. `None` when the cut falls on an edge or outside: that would
/// leave a box of nothing beside the one that was already there, which is not a
/// cut.
///
/// The **window travels with the second half**, the same rule an edge trim
/// follows: the halves are two windows over one source, and the later one reads
/// further into it by exactly the length of the earlier one. So cutting and
/// joining back leaves what was there.
pub fn split_at(p: Placement, at: f64) -> Option<(Placement, Placement)> {
    if at <= p.offset || at >= p.offset + p.dur {
        return None;
    }
    Some((
        Placement {
            dur: at - p.offset,
            ..p
        },
        Placement {
            offset: at,
            dur: p.offset + p.dur - at,
            start: p.start + (at - p.offset),
        },
    ))
}

/// **Join two boxes into the one that spans both**: from the earlier onset to
/// the later end, reading its source from where the earlier one read.
///
/// The inverse of [`split_at`] on two halves it produced, and a merge of an
/// overlap otherwise — a join is stated over what is there, not over what the
/// halves were.
pub fn merge(a: Placement, b: Placement) -> Placement {
    let (first, second) = if a.offset <= b.offset { (a, b) } else { (b, a) };
    let end = (first.offset + first.dur).max(second.offset + second.dur);
    Placement {
        offset: first.offset,
        dur: end - first.offset,
        start: first.start,
    }
}

/// Whether `b` begins where `a` ends, within `tol` — what "two juxtaposed
/// boxes" means on an axis whose positions are snapped and whose lengths are
/// floats. An overlap counts: boxes that share pixels are not two boxes to a
/// reader.
pub fn adjacent(a: Placement, b: Placement, tol: f64) -> bool {
    let (first, second) = if a.offset <= b.offset { (a, b) } else { (b, a) };
    second.offset <= first.offset + first.dur.max(0.0) + tol
}

/// **Indexed access to a run of boxes**, whatever they are stored as.
///
/// Not a common item type and not a slice: notes live contiguously in a
/// `Vec<Note>` and clips live in the widget tree, and the whole point of this
/// plan is that neither moves. Every block operation below is written over this
/// and monomorphised, so a note block edit is the same generated code it was
/// before the trait existed.
///
/// These calls happen **per gesture** — one press, one drag step, over the
/// selection — never per item per frame, so the indirection is off every hot
/// path there is.
pub trait Placements {
    fn len(&self) -> usize;
    /// Whether there is nothing to edit.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn placement(&self, i: usize) -> Placement;
    fn set_placement(&mut self, i: usize, p: Placement);
    /// The row the box sits on: a MIDI pitch in a roll, a lane index on a
    /// multitrack.
    fn row(&self, i: usize) -> f32;
    fn set_row(&mut self, i: usize, r: f32);
    /// What lies behind the box, for the edges to stop at.
    fn contents(&self, _i: usize) -> Contents {
        Contents::default()
    }
}

/// The indices of the boxes intersecting the time span `[t0, t1)` whose row
/// touches the band `[r_lo, r_hi]` (a box's row spans half a unit either side
/// of its own). Either range may come reversed — a marquee drags both ways.
pub fn in_rect<P: Placements + ?Sized>(
    p: &P,
    t0: f64,
    t1: f64,
    r_lo: f32,
    r_hi: f32,
) -> Vec<usize> {
    let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
    let (r_lo, r_hi) = if r_lo <= r_hi {
        (r_lo, r_hi)
    } else {
        (r_hi, r_lo)
    };
    (0..p.len())
        .filter(|&i| {
            let b = p.placement(i);
            let r = p.row(i);
            b.offset < t1 && b.offset + b.dur.max(0.0) > t0 && r + 0.5 >= r_lo && r - 0.5 <= r_hi
        })
        .collect()
}

/// Move a block of boxes rigidly from a press-time snapshot: `orig` is
/// `(index, offset, row)` per selected box, `dt`/`dr` the drag deltas.
///
/// The deltas are clamped **as one** — no offset below zero, no tail past
/// `limit`, no row outside `rows` — so the block stops at an edge instead of
/// folding against it. Durations are kept: a block move never resizes anything.
pub fn move_block<P: Placements + ?Sized>(
    p: &mut P,
    orig: &[(usize, f64, f32)],
    dt: f64,
    dr: f32,
    rows: (f32, f32),
    limit: Limit,
) {
    if orig.is_empty() {
        return;
    }
    let (lo, hi) = rows;
    let min_start = orig
        .iter()
        .map(|(_, s, _)| *s)
        .fold(f64::INFINITY, f64::min);
    // The block's far end is its last **tail**, so it stops where a single box
    // would. Read from the snapshot's offsets and the boxes' current durations:
    // a block move never touches a duration.
    let max_end = orig
        .iter()
        .filter(|(i, _, _)| *i < p.len())
        .map(|(i, s, _)| s + p.placement(*i).dur.max(0.0))
        .fold(f64::NEG_INFINITY, f64::max);
    // The near edge is applied last, so a block longer than the whole domain
    // pins to zero rather than to a negative offset — the same choice a single
    // over-long box makes.
    let dt = match limit {
        Some(l) if max_end.is_finite() => dt.min(l - max_end).max(-min_start),
        _ => dt.max(-min_start),
    };
    let (min_r, max_r) = orig
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), (_, _, r)| {
            (a.min(r.round()), b.max(r.round()))
        });
    // A block already wider than the window cannot move rigidly across rows.
    let dr = if lo - min_r <= hi - max_r {
        dr.round().clamp(lo - min_r, hi - max_r)
    } else {
        0.0
    };
    for (i, s, r) in orig {
        if *i < p.len() {
            let b = p.placement(*i);
            p.set_placement(
                *i,
                Placement {
                    offset: s + dt,
                    ..b
                },
            );
            p.set_row(*i, (r.round() + dr).clamp(lo, hi));
        }
    }
}

/// Quantize box onsets to the `grid` (timeline samples): each offset snaps to
/// the nearest grid line, durations untouched. `indices` picks the boxes (the
/// selection); empty quantizes them all. A zero/negative grid is a no-op.
/// Returns whether anything moved.
pub fn quantize<P: Placements + ?Sized>(p: &mut P, indices: &[usize], grid: f64) -> bool {
    if grid <= 0.0 {
        return false;
    }
    let mut moved = false;
    let mut apply = |p: &mut P, i: usize| {
        if i >= p.len() {
            return;
        }
        let b = p.placement(i);
        let offset = snap(b.offset, grid).max(0.0);
        if offset != b.offset {
            moved = true;
            p.set_placement(i, Placement { offset, ..b });
        }
    };
    if indices.is_empty() {
        for i in 0..p.len() {
            apply(p, i);
        }
    } else {
        for &i in indices {
            apply(p, i);
        }
    }
    moved
}

/// The selection re-mapped after the box at `removed` left the list: the
/// removed index drops out, higher indices shift down one.
pub fn selection_after_removal(selected: &[usize], removed: usize) -> Vec<usize> {
    selected
        .iter()
        .filter(|&&i| i != removed)
        .map(|&i| if i > removed { i - 1 } else { i })
        .collect()
}

/// Toggle a box in or out of the selection (Alt+click: a non-rectangular
/// selection built one box at a time).
pub fn toggle_selected(selected: &mut Vec<usize>, index: usize) {
    match selected.iter().position(|&i| i == index) {
        Some(p) => {
            selected.remove(p);
        }
        None => selected.push(index),
    }
}
