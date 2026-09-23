//! **A join's parts, as a list an edit is made of.**
//!
//! A join ([`Location::Segments`](crate::session::Location::Segments)) is a
//! flat list of [`Part`]s: spans of takes, read back to back. That is what the
//! multitrack mints when boxes are joined, and it is also what an audio editor
//! holds while a take is edited: a cut, a paste, a deletion or a new take put
//! in over a span all leave a new list and move no samples. So the arithmetic
//! is one -- which parts a span of the list is, and what the list is with a span
//! taken out, put in or replaced -- and it is written here, once, beside the
//! part it counts.
//!
//! # Every position is a frame of the list
//!
//! A part contributes the frames of its `range`, and the list's frame `n` is
//! wherever the parts before it leave off. That is only true while every part
//! is at the list's own rate, which is the case this module serves: an edited
//! take writes its new takes at its own rate. A list mixing rates is the
//! multitrack's, whose parts state frames of the source and are read at a
//! ratio the server works out; it is not widened into this one.
//!
//! # A part must say its frames
//!
//! A part with no `range` means "the whole take", and how long that is only the
//! holder of the samples knows. Every function here refuses one with the
//! reason rather than guessing a length, so a caller resolves the ranges first.
//!
//! # A seam is where the list is cut, and nowhere else
//!
//! A part's fades are the ones an editor put on a cut. Taking a span out of a
//! part keeps each fade only where that end of the part is kept, and two parts
//! that read on from each other -- the same take, the next frame, one channel
//! map and no fade between them -- are one part again. The second rule is what
//! makes a list come back to itself: cutting a span and pasting it back where
//! it was leaves the list it started from, not three parts over one take.

use crate::session::Part;
use crate::{Range, SourceId};

/// Why a list could not be read.
pub const UNRANGED: &str = "a part of this join does not say which frames it is";

/// **How many frames the list is**, or the refusal when a part does not say.
pub fn length(parts: &[Part]) -> Result<u64, &'static str> {
    parts.iter().try_fold(0u64, |total, part| {
        part.source
            .range
            .map(|range| total + range.len())
            .ok_or(UNRANGED)
    })
}

/// **The parts frames `from`..`to` of the list are**, each cut to the span.
///
/// A span running past the end is cut at the end, and an empty one is no parts:
/// both are answers, since whether reading nothing is an error is the caller's
/// to say.
pub fn span(parts: &[Part], from: u64, to: u64) -> Result<Vec<Part>, &'static str> {
    let mut out = Vec::new();
    let mut at = 0u64;
    for part in parts {
        let range = part.source.range.ok_or(UNRANGED)?;
        let (lo, hi) = (at, at + range.len());
        at = hi;
        let (a, b) = (from.max(lo), to.min(hi));
        if a >= b {
            continue;
        }
        let mut cut = part.clone();
        cut.source.range = Some(Range {
            start: range.start + (a - lo),
            end: range.start + (b - lo),
        });
        if a != lo {
            cut.fade_in = 0;
        }
        if b != hi {
            cut.fade_out = 0;
        }
        out.push(cut);
    }
    Ok(out)
}

/// **The list with frames `from`..`to` replaced by `with`** -- what a paste over
/// a selection, a take drawn over a span and a deletion (`with` empty) all are.
///
/// A position past the end is the end, so an insertion there appends.
pub fn replace(
    parts: &[Part],
    from: u64,
    to: u64,
    with: &[Part],
) -> Result<Vec<Part>, &'static str> {
    let total = length(parts)?;
    length(with)?;
    let from = from.min(total);
    let to = to.clamp(from, total);
    let mut out = span(parts, 0, from)?;
    out.extend(with.iter().cloned());
    out.extend(span(parts, to, total)?);
    Ok(merged(out))
}

/// **The list with frames `from`..`to` taken out.**
pub fn remove(parts: &[Part], from: u64, to: u64) -> Result<Vec<Part>, &'static str> {
    replace(parts, from, to, &[])
}

/// **The list with `inserted` put in at frame `at`**, which moves everything
/// after it later.
pub fn insert(parts: &[Part], at: u64, inserted: &[Part]) -> Result<Vec<Part>, &'static str> {
    replace(parts, at, at, inserted)
}

/// **Every take the list reads**, once each, in the order first read.
///
/// What a history and a clipboard state they hold: a take outlives an edit for
/// as long as some list still names it.
pub fn sources(parts: &[Part]) -> Vec<SourceId> {
    let mut out: Vec<SourceId> = Vec::new();
    for part in parts {
        if !out.contains(&part.source.source) {
            out.push(part.source.source);
        }
    }
    out
}

/// **The list read through to its takes**: a part over a source `parts_of`
/// knows as a join is replaced by the spans of that join it covers.
///
/// What keeps every list flat -- a join is a list of takes, never of joins --
/// so the server's limit on stitching joins over joins is never approached by
/// editing. A join over a join over a take reads through both.
pub fn flatten(
    parts: &[Part],
    parts_of: &dyn Fn(SourceId) -> Option<Vec<Part>>,
) -> Result<Vec<Part>, &'static str> {
    let mut out = Vec::new();
    for part in parts {
        match parts_of(part.source.source) {
            None => out.push(part.clone()),
            Some(inner) => {
                let inner = flatten(&inner, parts_of)?;
                let range = part.source.range.ok_or(UNRANGED)?;
                let mut read = span(&inner, range.start, range.end)?;
                if let Some(first) = read.first_mut()
                    && part.fade_in > 0
                {
                    first.fade_in = part.fade_in;
                }
                if let Some(last) = read.last_mut()
                    && part.fade_out > 0
                {
                    last.fade_out = part.fade_out;
                }
                out.extend(read);
            }
        }
    }
    Ok(merged(out))
}

/// The list with every two parts that read on from each other made one.
fn merged(parts: Vec<Part>) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::with_capacity(parts.len());
    for part in parts {
        if part.source.range.is_some_and(|r| r.is_empty()) {
            continue;
        }
        if let Some(last) = out.last_mut()
            && continues(last, &part)
        {
            let (Some(a), Some(b)) = (last.source.range, part.source.range) else {
                unreachable!("continues() reads both ranges");
            };
            last.source.range = Some(Range {
                start: a.start,
                end: b.end,
            });
            last.fade_out = part.fade_out;
            continue;
        }
        out.push(part);
    }
    out
}

/// Whether `next` is `last`'s next frames of the same take, read the same way
/// and with no fade on the seam between them.
fn continues(last: &Part, next: &Part) -> bool {
    let (Some(a), Some(b)) = (last.source.range, next.source.range) else {
        return false;
    };
    last.source.source == next.source.source
        && last.source.lifetime == next.source.lifetime
        && last.source.generation == next.source.generation
        && last.channels == next.channels
        && a.end == b.start
        && last.fade_out == 0
        && next.fade_in == 0
}

#[cfg(test)]
mod tests;
