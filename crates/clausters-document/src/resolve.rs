//! From a selection to the span of samples underneath it.
//!
//! A [`crate::Selection`] says what is selected on a *timeline*; an operation --
//! normalize, fade, copy, reverse -- needs the span of a **source**. Between them
//! sit three things the view knows and an algorithm does not: where the element
//! was placed, how much of the source it uses (its trim), and the bridge between
//! the arrangement's beats and the buffer's frames. This module is that
//! mapping, and only that: it hands back *which source, which frames*, and the
//! operation is performed by whoever owns it.
//!
//! # The tempo is the caller's; the arithmetic is here
//!
//! [`Mapping`] takes a **tempo map** and **frames per second**, both handed
//! over by the caller: which clock a tree is played on, and so what one of its
//! beats is, is nothing the tree says, and the sample rate is the device's. It
//! needs both because the tree measures its two kinds of length in two units:
//! an onset is in beats and a take's length is in seconds
//! ([`crate::Body::duration_unit`]), so the map places a clip and the rate
//! says how many frames a stretch of it is. It is the same line the rest of
//! the crate draws around a leaf's configuration: carry what is given, own
//! what is shared.
//!
//! **Nothing here multiplies a length by a tempo.** A beat is a position, so
//! the frames between two of them are the seconds between them, read off the
//! map at both ends -- which is what lets a selection start before an
//! accelerando and end after it and still land on the right frames at both
//! edges. A caller whose tempo never moves hands over a map of one segment
//! ([`TempoMap::new`]) and says so by doing it.
//!
//! # What a resolution has to include, and what it must not
//!
//! Trim and placement, both -- a selection at second three of a clip that starts
//! at second two and reads the take from second ten is at second eleven of the
//! take, and getting either term wrong is silent. **Clamping**, too: a selection
//! dragged past the end of a clip selects what the clip covers, not a span past
//! the end of a file. And the **generation**, because an operation reads
//! samples and a copy taken against an older one is the case the two counters
//! exist for.
//!
//! What it must not include is the operation. A numeric routine anything outside
//! a window would call belongs in `clausters-core`; a user-written function
//! belongs to the user. Neither belongs here.

use clausters_core::tempomap::TempoMap;
use serde::{Deserialize, Serialize};

use crate::{Beats, Body, Document, Member, Node, NodeId, Range, Selection, SourceId, TimeUnit};

/// Which unit a selection's numbers are in.
///
/// Not a field of [`Selection`], deliberately: a selection is a value that
/// travels, and tagging it would have broken the plain two-number form the wire
/// has always carried for nothing the caller does not already know. The reader
/// that resolves one knows which surface it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    /// Frames on the shared timeline axis -- what a view over samples reports.
    Frames,
    /// Beats of the arrangement -- what a view over placements reports.
    Beats,
}

/// How to get from a selection's numbers to a source's frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping<'a> {
    /// What a beat of the tree is: the map of the clock it is played on.
    /// Supplied rather than held, because the tree names no clock.
    pub tempo: &'a TempoMap,
    /// Frames of samples per second -- the sample rate, which with the map is
    /// what a stretch of the tree is measured in frames with.
    pub frames_per_second: f64,
    /// What the selection's numbers mean.
    pub unit: Unit,
}

impl<'a> Mapping<'a> {
    /// A selection in frames on the shared axis.
    pub fn frames(tempo: &'a TempoMap, frames_per_second: f64) -> Self {
        Self {
            tempo,
            frames_per_second,
            unit: Unit::Frames,
        }
    }

    /// A selection in beats.
    pub fn beats(tempo: &'a TempoMap, frames_per_second: f64) -> Self {
        Self {
            tempo,
            frames_per_second,
            unit: Unit::Beats,
        }
    }

    /// Whether the rate can measure anything. A degenerate one resolves
    /// nothing, rather than every span to frame zero.
    fn usable(self) -> bool {
        self.frames_per_second.is_finite() && self.frames_per_second > 0.0
    }

    /// Where something that starts at beat `at` and lasts `length` of `unit`
    /// ends, in beats.
    ///
    /// The start is part of the question: a length in seconds reaches a
    /// different beat depending on where it begins, so there is no "length in
    /// beats" to hand back, only an end.
    fn end_of(self, at: Beats, length: f64, unit: TimeUnit) -> Beats {
        match unit {
            TimeUnit::Beats => at + length,
            TimeUnit::Seconds => at + self.tempo.span_beats(at, length),
        }
    }

    /// A position in the selection's own unit, as beats.
    fn to_beats(self, position: f64) -> Beats {
        match self.unit {
            Unit::Beats => position,
            Unit::Frames => self.tempo.beats_at(position / self.frames_per_second),
        }
    }

    /// How many frames lie between two beats: the seconds between them, at
    /// the rate. Both ends, because the same stretch of beats lasts
    /// differently depending on where it sits.
    fn frames_between(self, from: Beats, to: Beats) -> u64 {
        (self.tempo.span_secs(from, to) * self.frames_per_second)
            .round()
            .max(0.0) as u64
    }
}

/// One multitrack of samples a selection landed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The element the span belongs to.
    pub node: NodeId,
    /// Its samples.
    pub source: SourceId,
    /// Which generation of those samples this was resolved against -- what an
    /// operation names so a stale read is detectable rather than silent.
    pub generation: u64,
    /// The span **within the source**, in frames: trim and placement both
    /// applied.
    pub range: Range,
    /// Where this multitrack starts inside the selection, in frames from the
    /// selection's own start.
    ///
    /// What a copy of several takes needs in order to lay them back down in the
    /// right places -- without it, a multi-element selection resolves to a bag
    /// of spans with no way to reassemble them.
    pub at: u64,
}

/// Every multitrack of samples a selection lands on, in tree order.
///
/// A selection may cross several elements -- that is what a marquee over a
/// multitrack *is* -- so this returns all of them. `selection.nodes` narrows it
/// when the selection named what it was of; an empty list means the shared
/// axis, and then everything under it resolves.
///
/// Elements the selection touches but that hold no samples are skipped rather
/// than reported: an aggregate and a generator have no span to give, and the caller
/// asked what is underneath, not what is in the way.
pub fn resolve(document: &Document, selection: &Selection, mapping: &Mapping) -> Vec<Resolved> {
    if selection.is_empty() || !mapping.usable() {
        return Vec::new();
    }
    let start = mapping.to_beats(selection.start);
    let end = mapping.to_beats(selection.end());
    let mut out = Vec::new();
    walk(
        &document.root,
        0.0,
        selection,
        mapping,
        start,
        end,
        &mut out,
    );
    out
}

/// The span of the source one placed element would give for this selection, or
/// `None` when it gives none -- the single-element form of [`resolve`], for a
/// caller that already knows which element it is asking about.
pub fn resolve_node(
    document: &Document,
    node: NodeId,
    selection: &Selection,
    mapping: &Mapping,
) -> Option<Resolved> {
    let narrowed = Selection {
        nodes: vec![node],
        ..selection.clone()
    };
    resolve(document, &narrowed, mapping).into_iter().next()
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: &Node,
    base: Beats,
    selection: &Selection,
    mapping: &Mapping,
    start: Beats,
    end: Beats,
    out: &mut Vec<Resolved>,
) {
    for member in node.members() {
        let at = base + member.offset;
        if (selection.nodes.is_empty() || selection.nodes.contains(&member.node.id))
            && let Some(resolved) = multitrack(member, at, mapping, start, end)
        {
            out.push(resolved);
        }
        // **Assembled samples resolves per window**: one entry per segment the
        // selection reaches, because each of them is a different part of a
        // different source and a caller that copied them as one span would be
        // copying frames nobody placed there.
        parts_of_segments(member, at, mapping, start, end, out);
        walk(&member.node, at, selection, mapping, start, end, out);
    }
}

/// One placed member against the selection's span.
fn multitrack(
    member: &Member,
    at: Beats,
    mapping: &Mapping,
    start: Beats,
    end: Beats,
) -> Option<Resolved> {
    let Body::Vector { source, .. } = &member.node.body else {
        // No samples, no span. An aggregate or a generator is in the way of the
        // selection, not underneath it.
        return None;
    };
    // The trim: which part of the source this element uses. Absent means all of
    // it, and then the placement's own length is what bounds the read.
    let trim = source.range;
    let placed_end = placed_end(member, at, trim, mapping)?;

    // The overlap, in the tree's beats.
    let from = start.max(at);
    let to = end.min(placed_end);
    if to <= from {
        return None;
    }

    // Into the source: the trim's own start, plus how far into the element the
    // selection begins. Getting either term wrong is silent, which is why they
    // are one expression rather than two steps.
    let trim_start = trim.map_or(0, |r| r.start);
    let into = mapping.frames_between(at, from);
    let length = mapping.frames_between(from, to);
    if length == 0 {
        return None;
    }
    let range_start = trim_start + into;
    // Clamped to the trim, so a selection dragged past the end of a clip
    // resolves to what the clip covers and never past the end of a file.
    let range_end = match trim {
        Some(r) => (range_start + length).min(r.end),
        None => range_start + length,
    };
    if range_end <= range_start {
        return None;
    }
    Some(Resolved {
        node: member.node.id,
        source: source.source,
        generation: source.generation,
        range: Range {
            start: range_start,
            end: range_end,
        },
        at: mapping.frames_between(start, from),
    })
}

/// Every window of a [`Body::Segments`] the selection lands on, in reading
/// order.
///
/// The same arithmetic [`multitrack`] does, once per segment, against the stretch of
/// the placement that segment occupies -- and bounded by the placement, which is
/// a window onto the element like every other placement here.
fn parts_of_segments(
    member: &Member,
    at: Beats,
    mapping: &Mapping,
    start: Beats,
    end: Beats,
    out: &mut Vec<Resolved>,
) {
    let Body::Segments { segments, .. } = &member.node.body else {
        return;
    };
    // The placement's length and each window's are in the unit their source
    // measures -- seconds over samples, beats over a node -- and the axis they
    // are laid on is beats, so each one is laid from where it starts: the next
    // window begins where this one ended.
    let placed = member
        .length()
        .map(|d| mapping.end_of(at, d, member.duration_unit()));
    let mut from_beat = at;
    for segment in segments {
        let to_beat = mapping.end_of(from_beat, segment.duration, segment.source.unit());
        let this = from_beat;
        from_beat = to_beat;
        // Past what the placement shows: the rest of the samples is there and
        // is not being played, so it is not under anything.
        let to_beat = match placed {
            Some(end) if to_beat > end => end,
            _ => to_beat,
        };
        if to_beat <= this {
            break;
        }
        let (from, to) = (start.max(this), end.min(to_beat));
        if to <= from {
            continue;
        }
        let into = mapping.frames_between(this, from);
        let length = mapping.frames_between(from, to);
        if length == 0 {
            continue;
        }
        // What this resolves is **samples**, so a window onto a node of the
        // document contributes none: its contents are nodes, and what reads
        // those is the tree walk, not a range of frames.
        let Some(source) = segment.source.samples() else {
            continue;
        };
        // A window opens at a **second** of its source ([`crate::SegmentRef`]),
        // and what is handed back is frames.
        let opens = (segment.start.max(0.0) * mapping.frames_per_second).round() as u64;
        let range_start = opens + into;
        out.push(Resolved {
            node: member.node.id,
            source: source.source,
            generation: source.generation,
            range: Range {
                start: range_start,
                end: range_start + length,
            },
            at: mapping.frames_between(start, from),
        });
    }
}

/// Where the placement ends, in beats: what was written on it, or what the
/// trim implies when nothing was.
fn placed_end(member: &Member, at: Beats, trim: Option<Range>, mapping: &Mapping) -> Option<Beats> {
    if let Some(dur) = member.length() {
        return (dur > 0.0).then(|| mapping.end_of(at, dur, member.duration_unit()));
    }
    let trim = trim?;
    if trim.is_empty() {
        return None;
    }
    // A trim is frames of the source, which is a length in seconds.
    let seconds = trim.len() as f64 / mapping.frames_per_second;
    Some(mapping.end_of(at, seconds, TimeUnit::Seconds))
}

#[cfg(test)]
mod tests;
