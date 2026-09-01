//! From a selection to the span of samples underneath it.
//!
//! A [`crate::Selection`] says what is selected on a *timeline*; an operation —
//! normalize, fade, copy, reverse — needs the span of a **source**. Between them
//! sit three things the view knows and an algorithm does not: where the element
//! was placed, how much of the source it uses (its trim), and the bridge between
//! the arrangement's beats and the buffer's frames. This module is that
//! mapping, and only that: it hands back *which source, which frames*, and the
//! operation is performed by whoever owns it.
//!
//! # The tempo is the caller's; the arithmetic is here
//!
//! [`Mapping`] takes **frames per beat** and **frames per second** rather than
//! a tempo and a sample rate, which keeps the crate out of a policy it has no
//! business in while still doing the conversion once instead of in every
//! client. It needs both because the tree measures its two kinds of length in
//! two units: an onset is in beats and a take's length is in seconds
//! ([`crate::Body::duration_unit`]), so one ratio can place a clip and the
//! other says how long it is. It is the same line the rest of the crate draws
//! around a leaf's configuration: carry what is given, own what is shared.
//!
//! # What a resolution has to include, and what it must not
//!
//! Trim and placement, both — a selection at second three of a clip that starts
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
    /// Frames on the shared timeline axis — what a view over samples reports.
    Frames,
    /// Beats of the arrangement — what a view over placements reports.
    Beats,
}

/// How to get from a selection's numbers to a source's frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    /// Frames of samples per beat of the arrangement. Supplied rather than
    /// derived: tempo and sample rate are the caller's.
    pub frames_per_beat: f64,
    /// Frames of samples per second — the sample rate, and what a length that
    /// is already in seconds is measured with.
    pub frames_per_second: f64,
    /// What the selection's numbers mean.
    pub unit: Unit,
}

impl Mapping {
    /// A selection in frames on the shared axis.
    pub fn frames(frames_per_beat: f64, frames_per_second: f64) -> Self {
        Self {
            frames_per_beat,
            frames_per_second,
            unit: Unit::Frames,
        }
    }

    /// A selection in beats.
    pub fn beats(frames_per_beat: f64, frames_per_second: f64) -> Self {
        Self {
            frames_per_beat,
            frames_per_second,
            unit: Unit::Beats,
        }
    }

    /// Beats per second — the tempo the two ratios imply. Zero when the caller
    /// gave a degenerate pair, which every reader here already guards for.
    fn tempo(self) -> f64 {
        if self.frames_per_beat > 0.0 {
            self.frames_per_second / self.frames_per_beat
        } else {
            0.0
        }
    }

    /// A length in its own unit, as beats of the arrangement.
    ///
    /// A `Mapping` states its own `frames_per_beat`, so its tempo is a
    /// **constant by construction** and the multiplication is the right one
    /// here — which is why this does not take the piece's converter. A
    /// selection resolved across a tempo change is a wider question than this
    /// mapping expresses, and it is written down in the plan rather than
    /// assumed away.
    fn length_in_beats(self, length: f64, unit: TimeUnit) -> Beats {
        match unit {
            TimeUnit::Beats => length,
            TimeUnit::Seconds => length * self.tempo(),
        }
    }

    /// A position in the selection's own unit, as beats.
    fn to_beats(self, position: f64) -> Beats {
        match self.unit {
            Unit::Beats => position,
            Unit::Frames => {
                if self.frames_per_beat > 0.0 {
                    position / self.frames_per_beat
                } else {
                    0.0
                }
            }
        }
    }

    /// A length in beats, as frames.
    fn to_frames(self, beats: Beats) -> f64 {
        beats * self.frames_per_beat
    }
}

/// One piece of samples a selection landed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The element the span belongs to.
    pub node: NodeId,
    /// Its samples.
    pub source: SourceId,
    /// Which generation of those samples this was resolved against — what an
    /// operation names so a stale read is detectable rather than silent.
    pub generation: u64,
    /// The span **within the source**, in frames: trim and placement both
    /// applied.
    pub range: Range,
    /// Where this piece starts inside the selection, in frames from the
    /// selection's own start.
    ///
    /// What a copy of several takes needs in order to lay them back down in the
    /// right places — without it, a multi-element selection resolves to a bag
    /// of spans with no way to reassemble them.
    pub at: u64,
}

/// Every piece of samples a selection lands on, in tree order.
///
/// A selection may cross several elements — that is what a marquee over a
/// multitrack *is* — so this returns all of them. `selection.nodes` narrows it
/// when the selection named what it was of; an empty list means the shared
/// axis, and then everything under it resolves.
///
/// Elements the selection touches but that hold no samples are skipped rather
/// than reported: an aggregate and a generator have no span to give, and the caller
/// asked what is underneath, not what is in the way.
pub fn resolve(document: &Document, selection: &Selection, mapping: &Mapping) -> Vec<Resolved> {
    if selection.is_empty() {
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
/// `None` when it gives none — the single-element form of [`resolve`], for a
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
            && let Some(resolved) = piece(member, at, mapping, start, end)
        {
            out.push(resolved);
        }
        // **Assembled samples resolves per window**: one entry per segment the
        // selection reaches, because each of them is a different part of a
        // different source and a caller that copied them as one span would be
        // copying frames nobody placed there.
        pieces_of_segments(member, at, mapping, start, end, out);
        walk(&member.node, at, selection, mapping, start, end, out);
    }
}

/// One placed member against the selection's span.
fn piece(
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
    let extent = placed_extent(member, trim, mapping)?;

    // The overlap, in the arrangement's beats.
    let from = start.max(at);
    let to = end.min(at + extent);
    if to <= from {
        return None;
    }

    // Into the source: the trim's own start, plus how far into the element the
    // selection begins. Getting either term wrong is silent, which is why they
    // are one expression rather than two steps.
    let trim_start = trim.map_or(0, |r| r.start);
    let into = mapping.to_frames(from - at).round().max(0.0) as u64;
    let length = mapping.to_frames(to - from).round().max(0.0) as u64;
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
        at: mapping.to_frames(from - start).round().max(0.0) as u64,
    })
}

/// Every window of a [`Body::Segments`] the selection lands on, in reading
/// order.
///
/// The same arithmetic [`piece`] does, once per segment, against the stretch of
/// the placement that segment occupies — and bounded by the placement, which is
/// a window onto the element like every other placement here.
fn pieces_of_segments(
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
    // The placement's length and each window's are both in seconds here (these
    // are samples), and the axis they are laid on is beats, so each one crosses
    // once, on the way out.
    let placed = member
        .length()
        .map(|d| mapping.length_in_beats(d, TimeUnit::Seconds));
    let mut cursor = 0.0;
    for segment in segments {
        let length = mapping.length_in_beats(segment.duration, TimeUnit::Seconds);
        let (from_beat, to_beat) = (at + cursor, at + cursor + length);
        cursor += length;
        // Past what the placement shows: the rest of the samples is there and
        // is not being played, so it is not under anything.
        let to_beat = match placed {
            Some(dur) if to_beat > at + dur => at + dur,
            _ => to_beat,
        };
        if to_beat <= from_beat {
            break;
        }
        let (from, to) = (start.max(from_beat), end.min(to_beat));
        if to <= from {
            continue;
        }
        let into = mapping.to_frames(from - from_beat).round().max(0.0) as u64;
        let length = mapping.to_frames(to - from).round().max(0.0) as u64;
        if length == 0 {
            continue;
        }
        let range_start = segment.start.max(0.0).round() as u64 + into;
        out.push(Resolved {
            node: member.node.id,
            source: segment.source.source,
            generation: segment.source.generation,
            range: Range {
                start: range_start,
                end: range_start + length,
            },
            at: mapping.to_frames(from - start).round().max(0.0) as u64,
        });
    }
}

/// How long the placement is, in beats: what was written on it, or what the
/// trim implies when nothing was.
fn placed_extent(member: &Member, trim: Option<Range>, mapping: &Mapping) -> Option<Beats> {
    if let Some(dur) = member.length() {
        return (dur > 0.0).then(|| mapping.length_in_beats(dur, member.duration_unit()));
    }
    let trim = trim?;
    if trim.is_empty() || mapping.frames_per_beat <= 0.0 {
        return None;
    }
    Some(trim.len() as f64 / mapping.frames_per_beat)
}

#[cfg(test)]
mod tests;
