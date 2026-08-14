//! What is selected, as a **value** rather than as a highlight.
//!
//! A selection is the thing an algorithm is handed: *normalize this*, *copy
//! this*, *erase this band here*. A pair of floats on a navigation group cannot
//! be handed to anything — it says where a rectangle is drawn and not what it
//! holds — so this is the type that closes the gap between a gesture and an
//! operation.
//!
//! # It is a value, and it is not in the tree
//!
//! A [`Selection`] is never a field of a [`crate::Document`]. The four layers
//! put a selection in flight under *screen state*, which is never persisted and
//! never logged, and nothing changes that: what this type adds is that the same
//! selection can also be **read out** as a value, crossed over a wire, kept in a
//! script's variable and handed to an operation. Where it lives while it is
//! being dragged is the view's business; what it *is* when someone asks is here.
//!
//! # One time span, and the axes that may restrict it
//!
//! Every selection is a span of time first — that is what makes an arrangement
//! selection, a sample selection and a spectral selection the same kind of thing
//! — and each further axis narrows it:
//!
//! - a **value range** on a container whose second axis measures something (a
//!   waveform's amplitude, a curve's value),
//! - a **bin range**, which is what makes the span a spectral region of frames ×
//!   bins,
//! - a **mask**, for a region no rectangle describes — the lasso.
//!
//! They are separate fields rather than one "second axis" because they mean
//! different things and are read by different code: a value range is in the
//! signal's own units and a bin range is in bins, and an operation that
//! understands one need not understand the other.
//!
//! # The unit is whatever the selected thing is measured in
//!
//! Frames over material, beats over an arrangement — and the crate does not
//! convert between them, because the beats↔samples bridge belongs to whoever
//! renders. Both travel as `f64`, which holds a frame index exactly past any
//! length a session will have, and which is what the wire already sends.
//!
//! # The round trip is the plain one
//!
//! A selection that is only a span serializes as exactly the two numbers the
//! `"selection"` payload has always carried, because every other field is
//! omitted when absent. So what a query gives back is what a set takes, and a
//! script reading the old two-number form keeps working.

use serde::{Deserialize, Serialize};

use crate::NodeId;

/// A range on an axis that measures something — amplitude, dB, a curve's value.
///
/// In the signal's **own** units, never in pixels: a selection that meant
/// screen coordinates could not be handed to an operation, which is the whole
/// reason this type exists.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ValueRange {
    /// The low edge.
    pub min: f64,
    /// The high edge.
    pub max: f64,
}

impl ValueRange {
    /// A range, with the edges put in order.
    pub fn new(a: f64, b: f64) -> Self {
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    /// Whether a value falls inside, edges included.
    pub fn contains(&self, value: f64) -> bool {
        value >= self.min && value <= self.max
    }
}

/// A half-open range of spectral bins — the second axis of a spectral region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinRange {
    /// First bin, inclusive.
    pub low: u32,
    /// One past the last bin.
    pub high: u32,
}

impl BinRange {
    /// A range, with the edges put in order.
    pub fn new(a: u32, b: u32) -> Self {
        Self {
            low: a.min(b),
            high: a.max(b),
        }
    }

    /// How many bins.
    pub fn len(&self) -> u32 {
        self.high.saturating_sub(self.low)
    }

    /// Whether it covers none.
    pub fn is_empty(&self) -> bool {
        self.high <= self.low
    }
}

/// A free-hand region, one bit per cell, row-major with the low bit first.
///
/// A **mask and not a polygon**, which is the design decision rather than an
/// encoding detail: the edge a hand drew is display-only and every operation
/// that reads a region wants to ask *is this cell in* rather than to
/// re-rasterize an outline. It also means an intersection or a union of two
/// regions needs no geometry.
///
/// The grid is the selection's own — `cols` cells along time, `rows` across the
/// other axis — so a mask means nothing without the selection that carries it.
/// A mask large enough to matter travels **beside** its JSON as bytes, by the
/// same bulk rule sample payloads follow; a selection is a value crossing a
/// wire, so how it is framed is the caller's choice and not the crate's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mask {
    /// Cells along the time axis.
    pub cols: u32,
    /// Cells across the other axis.
    pub rows: u32,
    /// The bits, row-major, low bit first.
    pub bits: Vec<u8>,
}

impl Mask {
    /// An empty mask of this size — nothing selected.
    pub fn new(cols: u32, rows: u32) -> Self {
        let cells = cols as usize * rows as usize;
        Self {
            cols,
            rows,
            bits: vec![0; cells.div_ceil(8)],
        }
    }

    /// Whether this cell is in. Out of range reads as out, so a mask that
    /// arrived short or oversized answers rather than panicking.
    pub fn get(&self, col: u32, row: u32) -> bool {
        let Some(index) = self.index(col, row) else {
            return false;
        };
        self.bits
            .get(index / 8)
            .is_some_and(|byte| byte & (1 << (index % 8)) != 0)
    }

    /// Puts a cell in or out. Out of range does nothing.
    pub fn set(&mut self, col: u32, row: u32, on: bool) {
        let Some(index) = self.index(col, row) else {
            return;
        };
        let Some(byte) = self.bits.get_mut(index / 8) else {
            return;
        };
        if on {
            *byte |= 1 << (index % 8);
        } else {
            *byte &= !(1 << (index % 8));
        }
    }

    /// How many cells are in.
    pub fn count(&self) -> u64 {
        (0..self.cols)
            .flat_map(|c| (0..self.rows).map(move |r| (c, r)))
            .filter(|(c, r)| self.get(*c, *r))
            .count() as u64
    }

    /// Whether the mask selects nothing.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Whether the bit vector matches the declared size — what a reader checks
    /// on something that arrived over a wire.
    pub fn is_well_formed(&self) -> bool {
        let cells = self.cols as usize * self.rows as usize;
        self.bits.len() == cells.div_ceil(8)
    }

    fn index(&self, col: u32, row: u32) -> Option<usize> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        Some(row as usize * self.cols as usize + col as usize)
    }
}

/// What is selected: a span of time, and whatever narrows it.
///
/// See the module docs. In short: it is a **value** a script can read back and
/// hand to an operation, it is not part of the document tree, its unit is
/// whatever the selected thing is measured in, and a selection that is only a
/// span is on the wire exactly the two numbers it always was.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Selection {
    /// Where it starts, in the selected thing's own unit.
    pub start: f64,
    /// How long it is, in the same unit. Zero is a **cursor** rather than a
    /// selection, and reads as empty.
    pub len: f64,
    /// What it is a selection *of*, when it is of something in particular.
    /// Empty means the shared time axis — the case a selection dragged across
    /// a multitrack's lanes is in, where the span is the whole of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeId>,
    /// The second axis, where it measures a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ValueRange>,
    /// The second axis, where it is spectral. With this present the selection
    /// is a region of frames × bins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bins: Option<BinRange>,
    /// A free-hand region inside the span, where a rectangle is not the shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
}

impl Selection {
    /// A plain time span — the two numbers the wire has always carried.
    pub fn span(start: f64, len: f64) -> Self {
        Self {
            start,
            len,
            ..Self::default()
        }
    }

    /// A cursor: a position with no extent.
    pub fn cursor(at: f64) -> Self {
        Self::span(at, 0.0)
    }

    /// Restricts it to these elements.
    pub fn of(mut self, nodes: impl IntoIterator<Item = NodeId>) -> Self {
        self.nodes = nodes.into_iter().collect();
        self
    }

    /// Restricts it on an axis that measures a value.
    pub fn with_value(mut self, value: ValueRange) -> Self {
        self.value = Some(value);
        self
    }

    /// Makes it a spectral region.
    pub fn with_bins(mut self, bins: BinRange) -> Self {
        self.bins = Some(bins);
        self
    }

    /// Gives it a free-hand shape inside the span.
    pub fn with_mask(mut self, mask: Mask) -> Self {
        self.mask = Some(mask);
        self
    }

    /// One past the end.
    pub fn end(&self) -> f64 {
        self.start + self.len
    }

    /// Whether it holds nothing — a cursor, or a span of no length.
    ///
    /// A mask of all zeros counts as empty too: a lasso that closed on nothing
    /// selected nothing, whatever rectangle bounds it.
    pub fn is_empty(&self) -> bool {
        if self.len <= 0.0 {
            return true;
        }
        self.mask.as_ref().is_some_and(Mask::is_empty)
    }

    /// Whether a position on the time axis falls inside the span. The narrowing
    /// axes are not consulted — they restrict *what* is selected, not *when*.
    pub fn contains(&self, position: f64) -> bool {
        position >= self.start && position < self.end()
    }

    /// Whether this selection names one element in particular.
    pub fn is_of(&self, node: NodeId) -> bool {
        self.nodes.contains(&node)
    }

    /// Whether it is the plain two-number form — no narrowing, no target.
    ///
    /// What a reader checks before taking the short path: a script that only
    /// understands spans should not silently treat a spectral region as if it
    /// were the whole band.
    pub fn is_plain(&self) -> bool {
        self.nodes.is_empty() && self.value.is_none() && self.bins.is_none() && self.mask.is_none()
    }
}

#[cfg(test)]
mod tests;
