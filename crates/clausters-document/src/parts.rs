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
//! A part's `range` is in frames of its take, and the list's frame `n` is
//! wherever the parts before it leave off. A take at the list's own rate
//! contributes its range as it is; one at another rate is read at the ratio
//! of the two (`docs/decisions.md`, "A source at another rate is resampled as
//! it plays"), so a second of it is a second of the list ([`contributed`]).
//! [`Rates`] says the list's rate and each take's; [`Rates::SAME`] is a list
//! whose takes are all at its rate. A span that starts or ends inside a part
//! at another rate is cut at the nearest frame of its take.
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

use serde_json::json;

use crate::session::{Part, SeamShape};
use crate::{Opaque, Range, SourceId};

/// **The domain name a take made of parts is registered under** in a history.
///
/// Its one verb states the result, as every vocabulary here does: *the take is
/// now these parts*. So an entry's forward half is the list after an edit and
/// its backward half the list before it, and both are small whatever the take
/// is -- the samples stay in the takes the lists name.
pub const PARTS: &str = "parts";

/// An edited take: its immutable sample spans and its non-destructive curves.
/// Automation uses seconds, exactly as a region's automation does.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Take {
    /// The sample spans, in reading order.
    pub parts: Vec<Part>,
    /// Curves over the take, from its origin.
    #[serde(default)]
    pub automation: Vec<crate::multitrack::Automation<crate::timebase::Second>>,
}

impl Take {
    /// A snapshot in the existing parts vocabulary, including its curves.
    pub fn payload(&self) -> Opaque {
        Opaque(json!({"intent": PARTS, "parts": self.parts, "automation": self.automation}))
    }

    /// Read a snapshot; older parts-only payloads have no curves.
    pub fn read(value: &Opaque) -> Option<Self> {
        Some(Self {
            parts: read(value)?,
            automation: serde_json::from_value(
                value
                    .0
                    .get("automation")
                    .cloned()
                    .unwrap_or_else(|| json!([])),
            )
            .ok()?,
        })
    }

    /// The same SetAutomation intent a region or track accepts.
    pub fn set_automation(
        &mut self,
        intent: &crate::multitrack::edit::MultitrackIntent,
    ) -> Result<bool, &'static str> {
        let crate::multitrack::edit::MultitrackIntent::SetAutomation { automation, points } =
            intent
        else {
            return Err("not a setautomation intent");
        };
        let curve = self
            .automation
            .iter_mut()
            .find(|a| a.id == *automation)
            .ok_or("no such automation")?;
        if curve.points == *points {
            return Ok(false);
        }
        curve.points.clone_from(points);
        Ok(true)
    }
}

/// **An edit in the parts vocabulary**: `{"intent": "parts", "parts": [...]}`.
pub fn payload(parts: &[Part]) -> Opaque {
    Opaque(json!({ "intent": PARTS, "parts": parts }))
}

/// The list a [`payload`] states, or `None` for a payload of another
/// vocabulary.
pub fn read(payload: &Opaque) -> Option<Vec<Part>> {
    if payload.0.get("intent").and_then(|i| i.as_str()) != Some(PARTS) {
        return None;
    }
    serde_json::from_value(payload.0.get("parts")?.clone()).ok()
}

/// Why a list could not be read.
pub const UNRANGED: &str = "a part of this join does not say which frames it is";

/// **What a take's rate is**, where it is not the list's: a function of the
/// source, or the table a holder keeps of the takes at another rate.
pub trait RateOf {
    /// The rate of `source`, or `None` for one at the list's rate.
    fn rate_of(&self, source: SourceId) -> Option<f64>;
}

impl<F: Fn(SourceId) -> Option<f64>> RateOf for F {
    fn rate_of(&self, source: SourceId) -> Option<f64> {
        self(source)
    }
}

impl RateOf for std::collections::BTreeMap<SourceId, f64> {
    fn rate_of(&self, source: SourceId) -> Option<f64> {
        self.get(&source).copied()
    }
}

/// No take at another rate.
struct NoRates;

impl RateOf for NoRates {
    fn rate_of(&self, _source: SourceId) -> Option<f64> {
        None
    }
}

/// **The rates a list is counted at**: its own, and each take's.
#[derive(Clone, Copy)]
pub struct Rates<'a> {
    /// The list's rate; `0` counts every part at its take's frames.
    pub join: f64,
    /// Each take's rate, where it is not the list's.
    pub of: &'a dyn RateOf,
}

impl Rates<'_> {
    /// A list whose takes are all at its own rate.
    pub const SAME: Rates<'static> = Rates {
        join: 0.0,
        of: &NoRates,
    };

    /// `part`'s take's rate and the list's, when the two differ.
    fn differ(&self, part: &Part) -> Option<(f64, f64)> {
        match (self.join, self.of.rate_of(part.source.source)) {
            (join, Some(take)) if join > 0.0 && take > 0.0 && take != join => Some((take, join)),
            _ => None,
        }
    }
}

/// **How many frames of a list at `join` a span of `frames` of a take at
/// `take` fills**: the same count when the two rates agree, else the span's
/// duration at the list's rate, to the nearest frame. One rule for the
/// list's arithmetic and for the stitch that reads it.
pub fn contributed(frames: u64, take: f64, join: f64) -> u64 {
    if take > 0.0 && join > 0.0 && take != join {
        ((frames as f64) * join / take).round() as u64
    } else {
        frames
    }
}

/// The frames of the list `part` fills.
fn filled(part: &Part, range: Range, rates: &Rates) -> u64 {
    match rates.differ(part) {
        Some((take, join)) => contributed(range.len(), take, join),
        None => range.len(),
    }
}

/// **How many frames the list is**, or the refusal when a part does not say.
pub fn length(parts: &[Part], rates: &Rates) -> Result<u64, &'static str> {
    parts.iter().try_fold(0u64, |total, part| {
        part.source
            .range
            .map(|range| total + filled(part, range, rates))
            .ok_or(UNRANGED)
    })
}

/// **The parts frames `from`..`to` of the list are**, each cut to the span.
///
/// A span running past the end is cut at the end, and an empty one is no parts:
/// both are answers, since whether reading nothing is an error is the caller's
/// to say.
pub fn span(parts: &[Part], from: u64, to: u64, rates: &Rates) -> Result<Vec<Part>, &'static str> {
    let mut out = Vec::new();
    let mut at = 0u64;
    for part in parts {
        let range = part.source.range.ok_or(UNRANGED)?;
        let (lo, hi) = (at, at + filled(part, range, rates));
        at = hi;
        let (a, b) = (from.max(lo), to.min(hi));
        if a >= b {
            continue;
        }
        // A frame of the list, as the frame of the take it reads: the same
        // number at the list's rate, the nearest one at another.
        let into = |offset: u64| {
            let frame = match rates.differ(part) {
                Some((take, join)) => ((offset as f64) * take / join).round() as u64,
                None => offset,
            };
            (range.start + frame).min(range.end)
        };
        let mut cut = part.clone();
        cut.source.range = Some(Range {
            start: into(a - lo),
            end: if b == hi { range.end } else { into(b - lo) },
        });
        if a != lo {
            cut.fade_in = 0;
            cut.fade_in_shape = SeamShape::default();
        }
        if b != hi {
            cut.fade_out = 0;
            cut.fade_out_shape = SeamShape::default();
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
    rates: &Rates,
) -> Result<Vec<Part>, &'static str> {
    let total = length(parts, rates)?;
    length(with, rates)?;
    let from = from.min(total);
    let to = to.clamp(from, total);
    let mut out = span(parts, 0, from, rates)?;
    out.extend(with.iter().cloned());
    out.extend(span(parts, to, total, rates)?);
    Ok(merged(out))
}

/// **The list with frames `from`..`to` taken out.**
pub fn remove(
    parts: &[Part],
    from: u64,
    to: u64,
    rates: &Rates,
) -> Result<Vec<Part>, &'static str> {
    replace(parts, from, to, &[], rates)
}

/// **The list with `inserted` put in at frame `at`**, which moves everything
/// after it later.
pub fn insert(
    parts: &[Part],
    at: u64,
    inserted: &[Part],
    rates: &Rates,
) -> Result<Vec<Part>, &'static str> {
    replace(parts, at, at, inserted, rates)
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
                let mut read = span(&inner, range.start, range.end, &Rates::SAME)?;
                if let Some(first) = read.first_mut()
                    && part.fade_in > 0
                {
                    first.fade_in = part.fade_in;
                    first.fade_in_shape = part.fade_in_shape;
                }
                if let Some(last) = read.last_mut()
                    && part.fade_out > 0
                {
                    last.fade_out = part.fade_out;
                    last.fade_out_shape = part.fade_out_shape;
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
            last.fade_out_shape = part.fade_out_shape;
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
