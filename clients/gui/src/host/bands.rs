//! **One vertical axis for a pitch row and a lane**: the stack of horizontal
//! bands a timeline view places its boxes on.
//!
//! A semitone row in a piano roll and a lane in a multitrack are the same
//! structure — a band of the vertical axis, holding boxes that share a time
//! axis with every other band — and they differ in exactly one thing: a
//! chromatic roll's bands are all the same height, and a multitrack's are not.
//! So this is an **enum with two arms** rather than two modules: [`Uniform`]
//! for a roll (arithmetic, no storage at all) and [`Table`] for lanes (prefix
//! sums, `4(n+1)` bytes).
//!
//! [`Uniform`]: Bands::Uniform
//! [`Table`]: Bands::Table
//!
//! # Why an enum and not a trait object
//!
//! [`band`](Bands::band) is the call inside a draw loop — once per row of
//! chrome, once per box — and it was measured at 1.4-2.2 ns in **both** arms,
//! against ~49 ns per widget for the layout pass that runs beside it. The
//! `match` hoists out of the loop once per pass and the `Uniform` arm compiles
//! to the arithmetic that was written by hand before this module existed. A
//! `dyn` call would be the one thing that could not be hoisted.
//!
//! [`index_at`](Bands::index_at) — the vertical hit-test, once per pointer
//! event — is 2.3 ns uniform and 6-15 ns tabulated, growing logarithmically:
//! 60,000x the bands cost 2.1x the lookup. Against the 9.5-112 us a pointer
//! event already pays to lay the window out, the vertical lookup is 0.02%.
//!
//! # The two questions it does not answer
//!
//! A band's **index is not its identity**. A roll's row is derived from a pitch
//! domain: it exists with nothing in it, and its index *is* data (the pitch
//! travels in the payload). A lane is a thing that exists, with a node behind
//! it. So moving a note between rows changes a number in the same list, while
//! moving a clip between lanes reparents — and that is the owner's business,
//! not this module's.
//!
//! And **the vertical is the container's, never the group's**. A roll linked to
//! a lane shares the time axis and nothing else: two views on one axis have
//! their own pitch windows, their own scroll and their own heights.

use std::ops::Range;

/// A stack of horizontal bands, measured from the stack's own zero (a caller
/// adds its rectangle's `y`).
#[derive(Debug, Clone, PartialEq)]
pub enum Bands {
    /// Every band the same height — the chromatic roll. Holds **nothing**: the
    /// count and one height are the whole of it.
    Uniform { n: usize, h: f32 },
    /// Bands of their own heights — the lanes of a multitrack. Stored as the
    /// `n + 1` **edges** rather than as `n` heights, so a band and a lookup are
    /// both a read instead of a running sum.
    Table { edges: Vec<f32> },
}

impl Bands {
    /// `n` bands of height `h`.
    pub fn uniform(n: usize, h: f32) -> Self {
        Bands::Uniform { n, h }
    }

    /// Bands of the given heights, in order. A negative height is read as zero:
    /// the edges must not run backwards, or every lookup below would be
    /// answering about a stack that cannot be drawn.
    pub fn table(heights: impl IntoIterator<Item = f32>) -> Self {
        let mut edges = vec![0.0];
        let mut y = 0.0;
        for h in heights {
            y += h.max(0.0);
            edges.push(y);
        }
        Bands::Table { edges }
    }

    /// How many bands there are.
    pub fn len(&self) -> usize {
        match self {
            Bands::Uniform { n, .. } => *n,
            Bands::Table { edges } => edges.len().saturating_sub(1),
        }
    }

    /// Whether the stack is empty — a view with no lanes, a roll with no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How tall the whole stack is.
    pub fn total(&self) -> f32 {
        match self {
            Bands::Uniform { n, h } => *n as f32 * h,
            Bands::Table { edges } => edges.last().copied().unwrap_or(0.0),
        }
    }

    /// Band `i` as `(y, height)` — the draw loop's call. An index past the end
    /// answers with the stack's end and no height, rather than panicking: a
    /// drawing pass that has fallen out of step should draw nothing, not stop.
    pub fn band(&self, i: usize) -> (f32, f32) {
        match self {
            Bands::Uniform { n, h } => {
                if i >= *n {
                    (*n as f32 * h, 0.0)
                } else {
                    (i as f32 * h, *h)
                }
            }
            Bands::Table { edges } => match (edges.get(i), edges.get(i + 1)) {
                (Some(a), Some(b)) => (*a, b - a),
                _ => (self.total(), 0.0),
            },
        }
    }

    /// The band a position falls in — **the vertical hit-test**. `None` above
    /// the first band or below the last.
    pub fn index_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 || y >= self.total() {
            return None;
        }
        match self {
            Bands::Uniform { h, .. } => (*h > 0.0).then(|| (y / h) as usize),
            // The first edge strictly past `y`, less one: the band that holds
            // it. Binary, which is why a stack of half a million bands costs
            // twice a stack of eight rather than sixty thousand times.
            Bands::Table { edges } => edges.partition_point(|e| *e <= y).checked_sub(1),
        }
        .filter(|i| *i < self.len())
    }

    /// The half-open range of bands any part of `[y0, y1)` touches — what a
    /// drawing pass iterates, so the chrome, the dividers and the labels are
    /// the same set of bands. Clamped to the stack; a reversed range comes back
    /// empty.
    pub fn window(&self, y0: f32, y1: f32) -> Range<usize> {
        let (y0, y1) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        let n = self.len();
        let first = match self {
            Bands::Uniform { h, .. } if *h > 0.0 => (y0 / h).floor().max(0.0) as usize,
            Bands::Uniform { .. } => 0,
            Bands::Table { edges } => edges.partition_point(|e| *e <= y0).saturating_sub(1),
        }
        .min(n);
        let last = match self {
            Bands::Uniform { h, .. } if *h > 0.0 => (y1 / h).ceil().max(0.0) as usize,
            Bands::Uniform { .. } => 0,
            Bands::Table { edges } => edges.partition_point(|e| *e < y1),
        }
        .min(n);
        first..last.max(first)
    }

    /// The position of a **fractional** index: `at(0.0)` is the top of the
    /// first band, `at(1.5)` the middle of the second.
    ///
    /// This is what a continuous axis over the stack needs — a note's pitch is
    /// a number before it is a row, and a bar half a semitone off is drawn half
    /// a row off. Clamped to the stack's own span at both ends.
    pub fn at(&self, i: f32) -> f32 {
        match self {
            Bands::Uniform { n, h } => (i * h).clamp(0.0, *n as f32 * h),
            Bands::Table { .. } => {
                let n = self.len();
                if n == 0 {
                    return 0.0;
                }
                let whole = i.floor().clamp(0.0, (n - 1) as f32);
                let (y, h) = self.band(whole as usize);
                (y + (i - whole) * h).clamp(0.0, self.total())
            }
        }
    }

    /// The inverse of [`at`](Self::at): the fractional index a position falls
    /// at. Clamped to `[0, len]`.
    pub fn index_of(&self, y: f32) -> f32 {
        match self {
            Bands::Uniform { n, h } if *h > 0.0 => (y / h).clamp(0.0, *n as f32),
            Bands::Uniform { .. } => 0.0,
            Bands::Table { .. } => {
                let n = self.len();
                if n == 0 || y <= 0.0 {
                    return 0.0;
                }
                if y >= self.total() {
                    return n as f32;
                }
                let i = self.index_at(y).unwrap_or(n - 1);
                let (top, h) = self.band(i);
                if h > 0.0 {
                    i as f32 + (y - top) / h
                } else {
                    i as f32
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two arms answer the same questions about the same stack — which is
    /// the whole claim: a roll's rows and a multitrack's lanes differ in
    /// storage and in nothing a caller asks.
    #[test]
    fn the_two_arms_agree_on_a_stack_they_can_both_describe() {
        let uniform = Bands::uniform(4, 10.0);
        let table = Bands::table([10.0; 4]);
        for i in 0..4 {
            assert_eq!(uniform.band(i), table.band(i), "band {i}");
        }
        for y in [-1.0, 0.0, 9.9, 10.0, 25.0, 39.9, 40.0, 100.0] {
            assert_eq!(uniform.index_at(y), table.index_at(y), "index_at {y}");
            assert!((uniform.index_of(y) - table.index_of(y)).abs() < 1e-4);
            assert_eq!(uniform.at(y / 10.0), table.at(y / 10.0));
        }
        assert_eq!(uniform.window(12.0, 31.0), table.window(12.0, 31.0));
        assert_eq!(uniform.total(), table.total());
    }

    /// A band is `[y, y + h)`: the position on an edge belongs to the band
    /// below it, and the bottom of the stack belongs to nothing.
    #[test]
    fn a_band_holds_its_own_top_edge_and_not_the_next_one() {
        let b = Bands::table([20.0, 5.0, 40.0]);
        assert_eq!(b.band(0), (0.0, 20.0));
        assert_eq!(b.band(1), (20.0, 5.0));
        assert_eq!(b.band(2), (25.0, 40.0));
        assert_eq!(b.index_at(0.0), Some(0));
        assert_eq!(b.index_at(19.99), Some(0));
        assert_eq!(b.index_at(20.0), Some(1));
        assert_eq!(b.index_at(25.0), Some(2));
        assert_eq!(b.index_at(-0.01), None);
        assert_eq!(b.index_at(65.0), None, "the stack's own end is past it");
        assert_eq!(b.total(), 65.0);
    }

    /// The window is what a drawing pass iterates: every band any part of the
    /// span touches, and nothing outside the stack.
    #[test]
    fn the_window_takes_every_band_the_span_touches() {
        let b = Bands::table([20.0, 5.0, 40.0]);
        assert_eq!(b.window(0.0, 65.0), 0..3);
        assert_eq!(b.window(21.0, 22.0), 1..2);
        assert_eq!(b.window(19.0, 26.0), 0..3, "half of three bands");
        assert_eq!(b.window(-100.0, 1.0), 0..1);
        assert_eq!(b.window(100.0, 200.0), 3..3);
        // A reversed span is a sweep dragged upwards, and means the same thing.
        assert_eq!(b.window(26.0, 19.0), b.window(19.0, 26.0));
    }

    /// A fractional index and a position are inverses, in both arms — what a
    /// pitch axis needs, since a pitch is a number before it is a row.
    #[test]
    fn a_fractional_index_and_a_position_are_inverses() {
        for b in [Bands::uniform(8, 12.5), Bands::table([5.0, 30.0, 65.0])] {
            for i in [0.0, 0.5, 1.0, 1.75, 2.0] {
                let round = b.index_of(b.at(i));
                assert!((round - i).abs() < 1e-3, "{b:?}: {i} -> {round}");
            }
        }
    }

    /// A stack with nothing in it answers every question without panicking:
    /// a view whose lanes have not arrived yet is drawn empty, not crashed.
    #[test]
    fn an_empty_stack_answers_everything() {
        for b in [Bands::uniform(0, 10.0), Bands::table([])] {
            assert!(b.is_empty());
            assert_eq!(b.total(), 0.0);
            assert_eq!(b.band(0), (0.0, 0.0));
            assert_eq!(b.index_at(0.0), None);
            assert_eq!(b.window(0.0, 100.0), 0..0);
            assert_eq!(b.at(3.0), 0.0);
            assert_eq!(b.index_of(3.0), 0.0);
        }
    }
}
