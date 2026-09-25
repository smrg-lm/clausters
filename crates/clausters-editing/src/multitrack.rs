//! **A multitrack as the props the multitrack widget is drawn with.**
//!
//! The projection over [`clausters_document::multitrack::picture`]: that module
//! says what a row and a box *are*, and this says how they reach a host. Every
//! payload here is a flat array, which is what the wire carries, and every one
//! of them was written three times before this existed -- once in each client
//! and once in the standalone host, which needs the same picture with no client
//! in the process at all.
//!
//! # What the caller brings, and why it is not in the document
//!
//! Two things, both [`Look`]. **The sample rate**, which puts the multitrack's
//! seconds on the shared frame axis by the core's one rule -- a position lands on
//! a whole sample and a length is the difference of its ends -- and no tempo is
//! involved, since a multitrack is placed in physical time. And
//! **which server buffer a source was read into**, which is a running server's
//! fact and never a document's.
//!
//! # What is here and what is the caller's
//!
//! Here: everything a multitrack has **from the document alone** -- the rows, the
//! boxes, the automations over both, their break-points, which of them are
//! hidden and which boxes loop. Not here: the position cursor (a window's), the
//! meter buses (a playback's, so the instance projection's) and the widget's
//! own chrome. The line is not taste -- it is that a projection is a function of
//! the structure, and anything that is a function of something else would drag
//! that something else in behind it.
//!
//! # The heights are here too, and that is deliberate
//!
//! A row is [`ROW_H`] tall and an automation lane [`CURVE_H`], and those are
//! written here rather than in each drawing because they were the same three
//! numbers in three files: a constant that agrees by coincidence is a constant
//! waiting to stop agreeing.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use clausters_core::tempoclock::{
    samples_to_secs, samples_to_secs_over, secs_to_samples, secs_to_samples_over,
};
use clausters_core::tempomap::TempoMap;
use clausters_document::multitrack::edit::MultitrackIntent;
use clausters_document::multitrack::nodes::{self, SourceInfo};
use clausters_document::multitrack::{Multitrack, picture};
use clausters_document::{NodeId, Opaque, Second, SourceId};

use crate::intake::{Intake, groups, number, text};

/// How tall a track's row is drawn.
pub const ROW_H: f64 = 96.0;

/// How tall an automation's own row under a track is drawn.
pub const CURVE_H: f64 = 40.0;

/// The value range a curve is drawn over when its target says nothing: unity,
/// which is what an unlabelled level means.
const UNIT: (f64, f64) = (0.0, 1.0);

/// **The tempo a multitrack that states none is drawn at**, in beats per
/// second: one, so a beat of its ruler is a second.
///
/// The reader's default and not the document's: a multitrack that said no
/// tempo did not say one. It only reaches a ruler -- nothing a multitrack
/// places is in beats.
pub const DEFAULT_TEMPO: f64 = 1.0;

/// What a caller knows that the document does not.
///
/// The rate that puts a second on the shared axis, and the table saying which
/// server buffer each source was read into. Both are the reason this is a
/// projection rather than a picture: the document describes the multitrack
/// and stops exactly where a *running* system begins.
pub struct Look<'a> {
    /// Frames per second on the shared axis.
    pub rate: f64,
    /// Which server buffer each source was read into. A source nobody answers
    /// for draws as a box over nothing, which is honest: a window onto notes, a
    /// composite, or samples nobody has read in yet.
    pub sources: &'a dyn Buffers,
}

/// **Which server buffer a source was read into.**
///
/// A trait rather than a map because the three callers hold it three ways and
/// none of them should have to build a fourth: a client keeps a table keyed by
/// source, the standalone host keeps its resolved takes, and the JSON door
/// parses one off the wire. What they share is the question, and this is the
/// question.
pub trait Buffers {
    /// The buffer `source` was read into, or `-1` for a source nobody read.
    fn bufnum(&self, source: SourceId) -> i64;

    /// **Every source this holds samples for**, in no particular order.
    ///
    /// What minting a source needs and a multitrack cannot answer: a source stops
    /// being named by the multitrack the moment nothing windows it, while whoever
    /// loaded it still holds the buffer. An id handed out off the multitrack alone
    /// can therefore already have samples behind it, and a box over it is then
    /// a window onto whatever that was.
    ///
    /// Defaulted to nothing so a caller that has no table is still a
    /// `Buffers` -- it means *I hold none*, which is the honest answer for one.
    fn taken(&self) -> Vec<SourceId> {
        Vec::new()
    }

    /// **The other direction**: the source a buffer number came from, or `None`
    /// for a buffer this multitrack knows nothing about.
    ///
    /// Both are here because a box is *drawn* from a buffer and *read back*
    /// into a source, and a caller that answered only one of them would have
    /// the other written beside it -- which is the second table this trait
    /// exists to prevent.
    fn source(&self, bufnum: i64) -> Option<SourceId>;

    /// **The segments a source is made of**, when it is a join this caller
    /// knows -- a take and a span of it, per part -- or `None` for a take.
    ///
    /// What lets a join over a box that is itself a join read through to the
    /// takes, so the source it mints is one flat list rather than a join of
    /// joins. Defaulted to none: a caller that knows no joins states none.
    fn parts(&self, _source: SourceId) -> Option<Vec<clausters_document::session::Part>> {
        None
    }

    /// **The rate `source`'s samples were written at**, when this caller knows
    /// -- or `None`, which means "assume the axis the box is measured on".
    ///
    /// A box is placed and drawn in the **view's** samples and filled with the
    /// **source's** frames, and the two are the same number only while the two
    /// rates are. It is asked here rather than carried on a box because it is a
    /// fact about the samples and not about the placement: six boxes over one
    /// 44.1 kHz take all read it at 44.1 kHz.
    fn rate(&self, _source: SourceId) -> Option<f64> {
        None
    }

    /// **How many frames `source` holds**, when this caller knows -- or `None`.
    ///
    /// What lets a join be refused as an edit instead of applied and left
    /// hollow: a box trimmed past the end of its take reads frames the take
    /// does not have, and a stitch asking for them is refused by the server
    /// after the multitrack already holds the joined box. Defaulted to unknown, which
    /// checks nothing.
    fn frames(&self, _source: SourceId) -> Option<u64> {
        None
    }
}

/// **A source table with the takes' lengths beside it**, as a request carries
/// one: `{"<id>": {"buffer", "channels", "frames"}}`.
pub struct Held {
    /// Which buffer each source was read into.
    pub buffers: HashMap<SourceId, i64>,
    /// How many frames each holds, where the table said.
    pub lengths: HashMap<SourceId, u64>,
    /// What rate each was written at, where the table said. A source that says
    /// none is read as one frame per sample of the view.
    pub rates: HashMap<SourceId, f64>,
}

impl Held {
    /// Both tables off one request value.
    pub fn of(sources: &Value) -> Self {
        Self {
            buffers: table(sources),
            lengths: lengths(sources),
            rates: source_rates(sources),
        }
    }
}

impl Buffers for Held {
    fn bufnum(&self, source: SourceId) -> i64 {
        self.buffers.bufnum(source)
    }

    fn taken(&self) -> Vec<SourceId> {
        self.buffers.taken()
    }

    fn source(&self, bufnum: i64) -> Option<SourceId> {
        self.buffers.source(bufnum)
    }

    fn frames(&self, source: SourceId) -> Option<u64> {
        self.lengths.get(&source).copied()
    }

    fn rate(&self, source: SourceId) -> Option<f64> {
        self.rates.get(&source).copied()
    }
}

impl Buffers for HashMap<SourceId, i64> {
    fn bufnum(&self, source: SourceId) -> i64 {
        self.get(&source).copied().unwrap_or(-1)
    }

    fn taken(&self) -> Vec<SourceId> {
        self.keys().copied().collect()
    }

    fn source(&self, bufnum: i64) -> Option<SourceId> {
        (bufnum >= 0)
            .then(|| {
                self.iter()
                    .find(|(_, held)| **held == bufnum)
                    .map(|(id, _)| *id)
            })
            .flatten()
    }
}

/// Nothing was resolved: every box is a box over nothing.
impl Buffers for () {
    fn bufnum(&self, _source: SourceId) -> i64 {
        -1
    }

    fn source(&self, _bufnum: i64) -> Option<SourceId> {
        None
    }
}

/// **The one rule a multitrack's seconds cross to a view's samples by**, and
/// back: the core's. A position lands on a whole sample
/// ([`secs_to_samples`]), and a length is the difference of its two ends
/// ([`secs_to_samples_over`]), so a box that ends where the next begins still
/// does after any number of round trips.
impl Look<'_> {
    /// The sample a position in seconds lands on.
    pub fn frame_at(&self, secs: f64) -> f64 {
        secs_to_samples(secs, self.rate) as f64
    }

    /// How many samples `length` seconds from `start` cover: the difference of
    /// the two ends, each landed on its sample.
    pub fn frames_over(&self, start: f64, length: f64) -> f64 {
        secs_to_samples_over(start, length, self.rate) as f64
    }

    /// The second a sample position falls on, the sample rounded first: the
    /// inverse of [`frame_at`](Self::frame_at), and the way an edit comes back.
    pub fn secs_at(&self, frame: f64) -> f64 {
        if self.rate <= 0.0 {
            return 0.0;
        }
        samples_to_secs(frame.round_ties_even() as i64, self.rate)
    }

    /// How many seconds `frames` samples from `at` cover: the difference of the
    /// two ends' seconds, for the reason [`frames_over`](Self::frames_over) is.
    pub fn secs_over(&self, at: f64, frames: f64) -> f64 {
        if self.rate <= 0.0 {
            return 0.0;
        }
        samples_to_secs_over(
            at.round_ties_even() as i64,
            frames.round_ties_even() as i64,
            self.rate,
        )
    }

    fn bufnum(&self, source: Option<SourceId>) -> i64 {
        source.map_or(-1, |id| self.sources.bufnum(id))
    }

    /// **The rate a source's samples were written at**, or this view's own
    /// where nobody says -- which makes one frame one sample, the answer for
    /// every source recorded at the rate the session runs at.
    pub fn source_rate(&self, source: Option<SourceId>) -> f64 {
        let rate = source
            .and_then(|id| self.sources.rate(id))
            .filter(|rate| *rate > 0.0)
            .unwrap_or(self.rate);
        // The same floor every crossing here takes: a rate of zero is no axis
        // at all, and frames and seconds are then the same number.
        if rate > 0.0 { rate } else { 1.0 }
    }
}

/// The value range a curve is drawn over, out of what it automates.
///
/// **The document says what a curve automates and never reads it**, so which
/// range that parameter has -- a gain over one, a pan over another -- is a fact
/// about the parameter and is stated where the parameter is.
fn domain(curve: &picture::Curve) -> (f64, f64) {
    let Value::Object(target) = &curve.target.0 else {
        return UNIT;
    };
    let read = |key: &str, default: f64| target.get(key).and_then(Value::as_f64).unwrap_or(default);
    (read("min", UNIT.0), read("max", UNIT.1))
}

/// The rows as the widget's flat sextuples: name, label, height, mute, solo,
/// gain.
pub fn lanes(multitrack: &Multitrack) -> Vec<Value> {
    let mut out = Vec::new();
    for row in picture::rows(multitrack) {
        out.extend([
            json!(row.track.0.to_string()),
            json!(row.label),
            json!(ROW_H),
            json!(row.mute),
            json!(row.solo),
            json!(row.gain),
            json!(row.curves),
        ]);
    }
    out
}

/// **The spans each join on screen is made of**, as the widget's flat
/// `box source start frames rate` quintuples: the take each span reads, the
/// frame of it the span starts at, how many frames of **the join** it
/// contributes, and how many frames of that take one frame of the join is --
/// one, unless the take was written at another rate, which a join reads
/// through rather than converting (`clausters_core`'s stitch says the same
/// thing to the server).
///
/// What lets a join be drawn from the takes it is spans of instead of from its
/// own buffer, which owns no samples and is built on the server after the edit
/// that made it: the picture is whole the moment the join is, and the join's
/// buffer is never downloaded. A box whose source is not a join this caller
/// knows is not named; nor is one with a span this caller cannot resolve to a
/// buffer and a length, which is then drawn from its own buffer as any box is.
pub fn segments(multitrack: &Multitrack, look: &Look<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    for box_ in picture::boxes(multitrack) {
        let Some(parts) = box_.source.and_then(|id| look.sources.parts(id)) else {
            continue;
        };
        let spans: Option<Vec<[Value; 5]>> = parts
            .iter()
            .map(|part| {
                let source = part.source.source;
                let bufnum = look.sources.bufnum(source);
                let (start, frames) = match &part.source.range {
                    Some(range) => (range.start, range.len()),
                    None => (0, look.sources.frames(source)?),
                };
                // The span reads `frames` of its take; what it contributes to
                // the join is that span crossed by the take's own rate, which
                // is the number the picture steps by.
                let rate = span_rate(look, part.source.source, box_.source);
                let contributed = (frames as f64 / rate).round().max(0.0) as u64;
                (bufnum >= 0 && frames > 0 && contributed > 0).then(|| {
                    [
                        json!(box_.region.0.to_string()),
                        json!(bufnum),
                        json!(start),
                        json!(contributed),
                        json!(rate),
                    ]
                })
            })
            .collect();
        if let Some(spans) = spans {
            out.extend(spans.into_iter().flatten());
        }
    }
    out
}

/// **How many frames of a box's source one sample of the box is**: the source's
/// own rate against the axis the box is measured on, times the box's playrate.
///
/// One number, because the picture, the edge a hand pulls and the reader that
/// sounds all ask the same question and any two of them answering it apart is
/// how a box comes to be drawn a different length than its samples. The
/// reader's half is `BufRateScale(buf) * rate` off the buffer itself
/// (`clausters_core::mixer`); this is the same product for everyone who has to
/// say it in the view's samples.
pub fn box_rate(box_: &picture::Box, look: &Look<'_>) -> f64 {
    let axis = if look.rate > 0.0 { look.rate } else { 1.0 };
    let rate = look.source_rate(box_.source) * box_.playrate / axis;
    if rate > 0.0 { rate } else { 1.0 }
}

/// The boxes whose samples are **not** one frame per sample, as flat
/// `name rate` pairs: how many frames of its source one sample of that box is.
///
/// A name list like `loops` rather than a field of the septuple, and for the
/// same two reasons: the septuple is a fixed width every reader chunks by, and
/// this is not a fact a hand can edit -- it follows from the source's rate and
/// the box's playrate. A box not named here reads one frame per sample, which
/// is every box of a session recorded at its own rate.
pub fn rates(multitrack: &Multitrack, look: &Look<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    for box_ in picture::boxes(multitrack) {
        let rate = box_rate(&box_, look);
        if (rate - 1.0).abs() > f64::EPSILON {
            out.extend([json!(box_.region.0.to_string()), json!(rate)]);
        }
    }
    out
}

/// **How many frames of a span's take one frame of the join is**: the take's
/// own rate against the join's, which is what a part of a join at another rate
/// is read through -- on the server by `dsp::stitch` and here by the drawing,
/// off the same two numbers.
fn span_rate(look: &Look<'_>, part: SourceId, join: Option<SourceId>) -> f64 {
    let take = look.source_rate(Some(part));
    let whole = look.source_rate(join);
    if take > 0.0 && whole > 0.0 {
        take / whole
    } else {
        1.0
    }
}

pub fn clips(multitrack: &Multitrack, look: &Look<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    for box_ in picture::boxes(multitrack) {
        out.extend([
            json!(box_.region.0.to_string()),
            json!(box_.row.0.to_string()),
            json!(look.frame_at(box_.position.0)),
            json!(look.frames_over(box_.position.0, box_.length.0)),
            // The window's start is a **second of the source**, so it crosses
            // to a frame at the source's own rate -- the same crossing the
            // reader makes with `BufSampleRate`, and not the view's.
            json!(box_.start * look.source_rate(box_.source)),
            json!(box_.label),
            json!(look.bufnum(box_.source)),
        ]);
    }
    out
}

/// The **track automations** as flat sextuples: a row of its own under the
/// track it names -- name, owner, label, low, high, height.
pub fn curves(multitrack: &Multitrack) -> Vec<Value> {
    let mut out = Vec::new();
    for curve in picture::curves(multitrack) {
        let (lo, hi) = domain(&curve);
        out.extend([
            json!(curve.automation.0.to_string()),
            json!(curve.owner.0.to_string()),
            json!(curve.label),
            json!(lo),
            json!(hi),
            json!(CURVE_H),
        ]);
    }
    out
}

/// The **region automations** as flat quintuples: a layer inside the box it
/// names, and no height, because it is as tall as that box.
pub fn layers(multitrack: &Multitrack) -> Vec<Value> {
    let mut out = Vec::new();
    for curve in picture::layers(multitrack) {
        let (lo, hi) = domain(&curve);
        out.extend([
            json!(curve.automation.0.to_string()),
            json!(curve.owner.0.to_string()),
            json!(curve.label),
            json!(lo),
            json!(hi),
        ]);
    }
    out
}

/// Every curve's break-points as flat quintuples, each naming the curve it is
/// on -- one list for the rows and the layers alike.
///
/// **What each curve's time is measured from** is the one thing that differs
/// between the two: a track automation runs the timeline and is measured from
/// the origin, and a clip envelope is drawn inside its box and is measured from
/// where that box starts. Both are seconds, and a length of seconds is the same
/// frames wherever it starts, so the difference reaches no number here.
pub fn points(multitrack: &Multitrack, look: &Look<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    for curve in picture::curves(multitrack)
        .iter()
        .chain(&picture::layers(multitrack))
    {
        for point in &curve.points {
            let data = point.data.0.as_object();
            let read = |key: &str, default: f64| {
                data.and_then(|d| d.get(key))
                    .and_then(Value::as_f64)
                    .unwrap_or(default)
            };
            out.extend([
                json!(curve.automation.0.to_string()),
                json!(look.frame_at(point.at)),
                json!(point.value),
                json!(read("shape", 1.0)),
                json!(read("curve", 0.0)),
            ]);
        }
    }
    out
}

/// The automations a hand has folded away, by name -- **read out of the multitrack**,
/// because which curves a person had showing is part of reopening the multitrack as
/// they left it.
pub fn hidden(multitrack: &Multitrack) -> String {
    let mut names = Vec::new();
    for curve in picture::curves(multitrack)
        .iter()
        .chain(&picture::layers(multitrack))
    {
        if !curve.visible {
            names.push(curve.automation.0.to_string());
        }
    }
    names.join(" ")
}

/// Which boxes wrap, by name. A box that loops has always more past its end,
/// which is what an edge drag may do and how the samples draw under a box
/// longer than they are.
pub fn loops(multitrack: &Multitrack) -> String {
    picture::boxes(multitrack)
        .iter()
        .filter(|b| b.looping)
        .map(|b| b.region.0.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// **What the multitrack calls its rows and its boxes**, by the names the wire
/// carries them under.
///
/// The minting correction's half that is a fact about the multitrack. A gesture is
/// normally answered with an acknowledgement and nothing else, because the
/// report described the result: the host drew what it sent and the multitrack
/// agreed. The cases where it does not are the ones where the host **makes**
/// something -- a track from a double click, a box from a split or a paste.
/// There the host mints the word (`track 1`, `white 2`) and the document mints
/// the id, so until the picture goes back the two are naming the same thing
/// differently.
///
/// And a name the multitrack does not know is not ignored: it is read as something
/// *new*. So the next report about that row or that box mints it again, and
/// again after that -- a split box took a fresh id on every drag, losing
/// whatever was hung on it, and a box dropped on a new track landed on a track
/// nobody had.
///
/// So a view keeps what it was last told and compares. It is here rather than
/// read off the props by striding them because a stride is a flat array's
/// shape restated at the call site, and the shape is this module's.
pub fn names(multitrack: &Multitrack) -> Value {
    json!({
        "rows": picture::rows(multitrack)
            .iter()
            .map(|row| row.track.0.to_string())
            .collect::<Vec<_>>(),
        "boxes": picture::boxes(multitrack)
            .iter()
            .map(|box_| box_.region.0.to_string())
            .collect::<Vec<_>>(),
        // **And the curves** *(found 2026-09-12 by the user: "al crear un track
        // y activar A no hace nada, al crear otro track aparece la
        // automatizacion del anterior")*. A curve the owner made is the same
        // fact a box the owner made is: the host cannot have drawn it, because
        // it did not make it. Left out, the toggle that asks a track for its
        // gain automation worked the whole way down and changed nothing on
        // screen -- until the next gesture that added a *row* fired the
        // correction, which then carried the previous track's curve with it.
        // That is why the two lists were never enough: they are not "what the
        // multitrack is called", they are "what the host was told", and the host is
        // told about rows, boxes **and** curves.
        "curves": picture::curves(multitrack)
            .iter()
            .chain(&picture::layers(multitrack))
            .map(|curve| curve.automation.0.to_string())
            .collect::<Vec<_>>(),
    })
}

/// [`names`] against a multitrack given as JSON.
pub fn names_json(multitrack: &str) -> String {
    let Ok(multitrack) = serde_json::from_str::<Multitrack>(multitrack) else {
        return r#"{"rows":[],"boxes":[],"curves":[]}"#.into();
    };
    names(&multitrack).to_string()
}

/// Every prop a multitrack has **from the document alone**, in one object.
///
/// What a caller adds is what is a function of something other than the multitrack:
/// the position cursor, the meter buses, and the widget's own chrome.
pub fn props(multitrack: &Multitrack, look: &Look<'_>) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("lanes".into(), Value::Array(lanes(multitrack)));
    out.insert("clips".into(), Value::Array(clips(multitrack, look)));
    out.insert("curves".into(), Value::Array(curves(multitrack)));
    out.insert("layers".into(), Value::Array(layers(multitrack)));
    out.insert("points".into(), Value::Array(points(multitrack, look)));
    out.insert("hidden".into(), json!(hidden(multitrack)));
    out.insert("loops".into(), json!(loops(multitrack)));
    out.insert("rates".into(), Value::Array(rates(multitrack, look)));
    out.insert("segments".into(), Value::Array(segments(multitrack, look)));
    out
}

/// [`props`] against a multitrack and a source table given as JSON, which is how the
/// two client doors carry them.
///
/// `sources` is **the same table the instance plan takes** -- source id to
/// `{"buffer", "channels"}` -- rather than a second one shaped for drawing: a
/// client that had to keep two would eventually keep two that disagree, and
/// what a box is drawn from and what it is played from are the same samples.
///
/// An unreadable multitrack answers an empty object rather than an error: a
/// projection has nothing to refuse.
pub fn props_json(multitrack: &str, rate: f64, sources: &str) -> String {
    let Ok(multitrack) = serde_json::from_str::<Multitrack>(multitrack) else {
        return "{}".into();
    };
    let table = table(&serde_json::from_str::<Value>(sources).unwrap_or(Value::Null));
    let look = Look {
        rate,
        sources: &table,
    };
    Value::Object(props(&multitrack, &look)).to_string()
}

/// The tempo map a multitrack holds, with [`DEFAULT_TEMPO`] where it states
/// none: what its ruler draws beats and bars from. It places nothing.
pub fn tempo_map(multitrack: &Multitrack) -> TempoMap {
    nodes::tempo_map(multitrack, DEFAULT_TEMPO)
}

/// What the `lanes` prop takes and reports: flat `name label height mute solo
/// gain curves` septuples -- the same width the `clips` prop happens to be, and
/// a different seven fields.
pub const LANE_FIELDS: usize = 7;

/// What the `clips` prop takes and reports: flat `name lane at duration start
/// label source` septuples.
pub const SEPTUPLE: usize = 7;

/// What the `points` prop takes and reports: flat `curve t v shape amount`
/// quintuples, each naming the curve it is on.
pub const POINT_QUINTUPLE: usize = 5;

/// The flat `clips` report as the crate's boxes: names as they came, and every
/// number in seconds.
///
/// A row is named by its **track's id** and never renamed, so a name that is
/// not one names no row this multitrack has and the box on it is dropped rather than
/// placed somewhere it was not.
fn placed(values: &[Value], look: &Look<'_>) -> Vec<picture::Placed> {
    let mut out = Vec::new();
    for group in groups(values, SEPTUPLE) {
        let Ok(row) = text(&group[1]).parse::<u64>() else {
            continue;
        };
        let (at, dur) = (number(&group[2]), number(&group[3]));
        let position = look.secs_at(at);
        let length = look.secs_over(at, dur);
        out.push(picture::Placed {
            name: text(&group[0]),
            row: NodeId(row),
            position: Second(position),
            length: Second(length),
            // A box's start comes back as a **frame of its source**, so it
            // crosses back at that source's own rate and not at the view's.
            start: {
                let source = look.sources.source(number(&group[6]) as i64);
                number(&group[4]) / look.source_rate(source)
            },
            // How much a **new** box shows: the stretch it occupies.
            content: length,
            source: look.sources.source(number(&group[6]) as i64),
        });
    }
    out
}

/// The flat `points` report as the crate's curves: one entry per curve named,
/// its break-points back in seconds.
///
/// The widget reports **every** curve there is, in one list, so they are
/// gathered by name here -- the reader says nothing about the ones that did not
/// move.
fn curved(values: &[Value], look: &Look<'_>) -> Vec<picture::Curved> {
    let mut order: Vec<String> = Vec::new();
    let mut found: HashMap<String, Vec<clausters_document::points::Point>> = HashMap::new();
    for group in groups(values, POINT_QUINTUPLE) {
        let name = text(&group[0]);
        // A layer's time is its box's own, so a break-point inside one comes
        // back as seconds from that box's start, as it went out.
        let points = found.entry(name.clone()).or_insert_with(|| {
            order.push(name.clone());
            Vec::new()
        });
        points.push(clausters_document::points::Point {
            at: look.secs_at(number(&group[1])),
            value: number(&group[2]),
            // **What a shape is stays the client's**: the document carries a
            // point's data and never reads it, which is what keeps an undo from
            // putting a bent curve back straight.
            data: Opaque(json!({
                "shape": number(&group[3]) as i64,
                "curve": number(&group[4]),
            })),
        });
    }
    order
        .into_iter()
        .map(|name| picture::Curved {
            points: found.remove(&name).unwrap_or_default(),
            name,
        })
        .collect()
}

/// The flat `lanes` report as the crate's strips.
///
/// The label and the height are **dropped rather than reported**: a row's label
/// is the track's name where it has one and a made-up one where it has not, and
/// its height is this window's. Neither is a fact about the multitrack.
fn strips(values: &[Value]) -> Vec<picture::Strip> {
    groups(values, LANE_FIELDS)
        .map(|group| picture::Strip {
            name: text(&group[0]),
            mute: number(&group[3]) != 0.0,
            solo: number(&group[4]) != 0.0,
            gain: number(&group[5]),
            curves: number(&group[6]) != 0.0,
        })
        .collect()
}

/// **What an undo menu calls each of the multitrack's verbs.**
///
/// One table, because a menu entry a hand reads is part of what an edit *is* to
/// the person who made it -- and a verb named two ways in two clients is the same
/// divergence as a verb applied two ways, only quieter.
pub fn label(intent: &MultitrackIntent) -> &'static str {
    match intent {
        MultitrackIntent::PlaceRegion { .. } => "move a clip",
        MultitrackIntent::TrimRegion { .. } => "trim a clip",
        MultitrackIntent::SetLane { .. } => "edit the clips",
        MultitrackIntent::SetTracks { .. } => "mix a track",
        MultitrackIntent::SplitRegion { .. } => "split a clip",
        MultitrackIntent::JoinRegions { .. } => "join the clips",
        MultitrackIntent::SetAutomation { .. } => "draw a curve",
        _ => "edit the multitrack",
    }
}

/// **The words this domain answers for**, and the only place they are listed.
///
/// A tag is a domain's vocabulary, so *which* tags are the multitrack's is a fact
/// about the multitrack and not about whoever is routing a report to it. It was
/// written twice -- here, and in the GUI host's own dispatch, which knew about
/// `clips` and `lanes` and had never heard of the other two -- and the second
/// list was two tags short: a curve dragged in a host with no client attached
/// reached nobody, and so did a `join`. A caller asks; nobody restates.
pub fn answers(tag: &str) -> bool {
    matches!(tag, "clips" | "lanes" | "points" | "join")
}

/// **What a report came to**: the edits, or the reason there are none.
///
/// The two are one answer because a caller has to tell them apart: no edits
/// because the hand changed nothing, and no edits because the multitrack **refused**,
/// are the same empty list and opposite things to say to the person who made the
/// gesture. Everything that reads a report goes through here, so neither door
/// can quietly drop the half the other keeps.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reading {
    /// The edits, in the multitrack's own vocabulary.
    pub intents: Vec<MultitrackIntent>,
    /// Why there are none, when the multitrack refused the verb rather than finding
    /// nothing to do.
    pub refusal: Option<&'static str>,
}

impl Reading {
    /// A reading that came to these edits.
    fn of(intents: Vec<MultitrackIntent>) -> Self {
        Reading {
            intents,
            refusal: None,
        }
    }

    /// A verb the multitrack refused, and why.
    fn refused(why: &'static str) -> Self {
        Reading {
            intents: Vec::new(),
            refusal: Some(why),
        }
    }

    /// The label one entry in the pile takes: the **first** payload's, because
    /// the payloads of one report are one thing a hand did.
    pub fn label(&self) -> &'static str {
        self.intents.first().map_or("edit the multitrack", label)
    }
}

/// **What a gesture over a multitrack means**, in the multitrack's own vocabulary.
///
/// Four tags, and three of them report the **whole** structure rather than the
/// gesture: every box, every row, every break-point. So a move, a block drag, a
/// trim, a split, a delete and a paste all arrive the same way and telling them
/// apart is one rule, [`clausters_document::multitrack::picture`]'s, written
/// once -- and what comes back is the *difference*, which is why a hand that
/// looked without editing produces nothing at all.
pub fn reading(multitrack: &Multitrack, tag: &str, values: &[Value], look: &Look<'_>) -> Reading {
    match tag {
        "clips" => Reading::of(picture::read(
            multitrack,
            &placed(values, look),
            picture::fresh_id(multitrack),
            &|source| {
                let rate = look.source_rate(Some(source));
                look.sources
                    .frames(source)
                    .filter(|_| rate > 0.0)
                    .map(|frames| frames as f64 / rate)
            },
        )),
        "lanes" => Reading::of(picture::read_rows(multitrack, &strips(values))),
        "points" => Reading::of(picture::read_points(multitrack, &curved(values, look))),
        // **The one verb that is stated rather than differenced**, and the one
        // that can be refused on the *material*: a join and a "delete one,
        // lengthen the other" leave a lane holding the same thing, and a box in
        // a `clips` report names one source and one start -- so fragments
        // joined into one box have no report that describes them. A gap it
        // cannot state as silence and an overlap it cannot state as a mix are
        // refusals, and they are the reason this function answers with more
        // than a list.
        "join" => {
            match picture::read_join(
                multitrack,
                &held(values),
                look.rate,
                &look.sources.taken(),
                &|source| look.sources.parts(source),
                &|source| look.sources.frames(source),
            ) {
                Ok(intents) => Reading::of(intents),
                Err(why) => Reading::refused(why),
            }
        }
        _ => Reading::default(),
    }
}

/// [`reading`]'s edits alone, for a caller with nothing to say about a refusal.
pub fn read(
    multitrack: &Multitrack,
    tag: &str,
    values: &[Value],
    look: &Look<'_>,
) -> Vec<MultitrackIntent> {
    reading(multitrack, tag, values, look).intents
}

/// The flat `join` report: the boxes to join, by the names the picture gave
/// them.
fn held(values: &[Value]) -> Vec<String> {
    values.iter().map(text).collect()
}

/// [`read`] as the payloads and the label an endpoint carries.
///
/// The label is the **first** intent's, because the intents of one report are
/// one thing a hand did and go into the pile as one entry.
pub fn intake(multitrack: &Multitrack, tag: &str, values: &[Value], look: &Look<'_>) -> Intake {
    if !answers(tag) {
        return Intake::nothing();
    }
    let reading = reading(multitrack, tag, values, look);
    // **A refusal that reaches nobody is indistinguishable from a key that does
    // not work**, which is why the reading carries one and this passes it on.
    if let Some(why) = reading.refusal {
        return Intake::refused(why);
    }
    let named = reading.label();
    let payloads = reading
        .intents
        .iter()
        .map(|intent| serde_json::to_value(intent).unwrap_or(Value::Null))
        .collect();
    Intake::edits(payloads, named)
}

/// [`intake`] against a multitrack and a source table given as JSON values, which
/// is how the one door carries them.
///
/// An unreadable multitrack answers [`Intake::nothing`]: there is no multitrack to say
/// what the gesture meant, and inventing one would write an edit against a
/// structure nobody has.
pub fn intake_value(
    multitrack: &Value,
    tag: &str,
    values: &[Value],
    rate: f64,
    sources: &Value,
) -> Intake {
    let Ok(multitrack) = serde_json::from_value::<Multitrack>(multitrack.clone()) else {
        return Intake::nothing();
    };
    let held = Held::of(sources);
    let look = Look {
        rate,
        sources: &held,
    };
    intake(&multitrack, tag, values, &look)
}

/// **What rate each source's samples were written at**, off the same table
/// [`table`] reads: an entry's `rate`, where it states a positive one. A source
/// that states none is read at the view's own rate, which is one frame per
/// sample.
pub fn source_rates(sources: &Value) -> HashMap<SourceId, f64> {
    let Some(entries) = sources.as_object() else {
        return HashMap::new();
    };
    entries
        .iter()
        .filter_map(|(id, entry)| {
            let id = id.parse::<u64>().ok()?;
            let rate = entry.get("rate")?.as_f64().filter(|r| *r > 0.0)?;
            Some((SourceId(id), rate))
        })
        .collect()
}

/// **How many frames each source holds**, off the same table [`table`] reads:
/// an entry's `frames`, where it states a positive one. A zero is what a
/// client writes for a length it does not know, so it is left out rather than
/// read as an empty take.
pub fn lengths(sources: &Value) -> HashMap<SourceId, u64> {
    let Some(entries) = sources.as_object() else {
        return HashMap::new();
    };
    entries
        .iter()
        .filter_map(|(id, entry)| {
            let id = id.parse::<u64>().ok()?;
            let frames = entry.get("frames")?.as_u64().filter(|f| *f > 0)?;
            Some((SourceId(id), frames))
        })
        .collect()
}

/// The instance plan's source table as the buffer question this crate asks.
///
/// Public because an application reads the same table off the same request: a
/// window over a multitrack is drawn from the buffers the multitrack is played from.
pub fn table(sources: &Value) -> HashMap<SourceId, i64> {
    serde_json::from_value::<HashMap<String, SourceInfo>>(sources.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, info)| {
            id.parse::<u64>()
                .ok()
                .map(|id| (SourceId(id), i64::from(info.buffer)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::{Automation, Content, Region, Track};
    use clausters_document::points::Point;
    use clausters_document::session::Location;
    use clausters_document::{
        Lifetime, NodeId, Opaque, Second, SegmentRef, SegmentSource, SourceRef,
    };

    /// One track at half gain with one box on it, a track automation over the
    /// timeline and an envelope inside the box.
    fn multitrack() -> Multitrack {
        let mut region = Region::new(
            NodeId(3),
            Second(4.0),
            Second(4.0),
            Content::Unknown(Value::Null),
        );
        region.automation.push(curve(NodeId(5), 2.0));
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.name = Some("drums".into());
        track.level = 0.5;
        track.lanes[0].regions.push(region);
        track.automation.push(curve(NodeId(4), 1.0));
        let mut multitrack = Multitrack::default();
        multitrack.tracks.push(track);
        multitrack
    }

    /// **A box over a source written at another rate says so, and its window
    /// is in that source's frames.** The multitrack's axis is the session's
    /// samples and the samples behind a box are its source's frames, and the
    /// two are the same number only while the rates are: a 44.1 kHz take on a
    /// 48 kHz session is `0.91875` frames of source per sample of box. The
    /// picture, the edge a hand pulls and the reader that sounds all cross by
    /// that one number.
    #[test]
    fn a_source_written_at_another_rate_states_its_own() {
        let source = SourceId(7);
        let mut multitrack = multitrack();
        let region = &mut multitrack.tracks[0].lanes[0].regions[0];
        region.content = Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source,
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                // Half a second into the take, in the take's own seconds.
                start: 0.5,
                duration: 4.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        };
        let held = Held {
            buffers: HashMap::from([(source, 0)]),
            lengths: HashMap::from([(source, 44_100)]),
            rates: HashMap::from([(source, 44_100.0)]),
        };
        let look = Look {
            rate: 48_000.0,
            sources: &held,
        };

        let rates = rates(&multitrack, &look);
        assert_eq!(rates[0], json!("3"), "the box it is about");
        assert!(
            (rates[1].as_f64().expect("a number") - 0.91875).abs() < 1e-12,
            "44100 frames of source per 48000 samples of box: {:?}",
            rates[1]
        );

        // And its window's start is a frame of **that** take: half a second in
        // is 22050 frames, not the 24000 the session's rate would have said.
        let clips = clips(&multitrack, &look);
        assert_eq!(clips[4], json!(22_050.0));

        // The way back is the same crossing, so what the hand did not move
        // comes back where it was.
        let read = placed(&clips, &look);
        assert!((read[0].start - 0.5).abs() < 1e-12, "{:?}", read[0].start);
    }

    /// A box over a source at the session's own rate states nothing: one frame
    /// per sample is what `rates` leaving it out means, and every session
    /// recorded at its own rate is that case.
    #[test]
    fn a_source_at_the_sessions_own_rate_is_not_named() {
        let source = SourceId(7);
        let mut multitrack = multitrack();
        let region = &mut multitrack.tracks[0].lanes[0].regions[0];
        region.content = Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source,
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start: 0.5,
                duration: 4.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        };
        let held = Held {
            buffers: HashMap::from([(source, 0)]),
            lengths: HashMap::from([(source, 48_000)]),
            rates: HashMap::from([(source, 48_000.0)]),
        };
        let look = Look {
            rate: 48_000.0,
            sources: &held,
        };
        assert!(rates(&multitrack, &look).is_empty());
        assert_eq!(clips(&multitrack, &look)[4], json!(24_000.0));
    }

    /// A curve with one point at the origin and one `at` seconds along.
    fn curve(id: NodeId, at: f64) -> Automation {
        let mut a = Automation::new(id, Opaque::none());
        a.name = Some(format!("curve {}", id.0));
        a.points = vec![
            Point {
                at: 0.0,
                value: 0.0,
                data: shape(),
            },
            Point {
                at,
                value: 1.0,
                data: shape(),
            },
        ];
        a
    }

    /// What the wire says about a segment, which is what the widget reports
    /// back: linear, with no bend. A point that carries it round-trips exactly,
    /// which is the case a real multitrack is in after its first edit.
    fn shape() -> Opaque {
        Opaque(json!({ "shape": 1, "curve": 0.0 }))
    }

    fn look<'a>(sources: &'a HashMap<SourceId, i64>) -> Look<'a> {
        Look {
            rate: 48_000.0,
            sources,
        }
    }

    /// The two flat payloads, in the shapes the widget takes: a row is seven
    /// values and a box seven, and they are not the same seven.
    #[test]
    fn a_row_is_seven_values_and_a_box_is_seven() {
        let multitrack = multitrack();
        let table = HashMap::new();
        let look = look(&table);

        let lanes = lanes(&multitrack);
        assert_eq!(lanes.len(), LANE_FIELDS);
        assert_eq!(lanes[0], json!("1"), "a row is named by its track's id");
        assert_eq!(lanes[1], json!("drums"));
        assert_eq!(lanes[2], json!(ROW_H));
        assert_eq!(lanes[5], json!(0.5));
        assert_eq!(
            lanes[6],
            json!(false),
            "and says whether its automation is shown -- this curve is not"
        );

        let clips = clips(&multitrack, &look);
        assert_eq!(clips.len(), 7);
        assert_eq!(clips[0], json!("3"), "and a box by its region's");
        assert_eq!(clips[1], json!("1"), "on the row it is on");
        // Four seconds in, in frames.
        assert_eq!(clips[2], json!(4.0 * 48_000.0));
        assert_eq!(clips[3], json!(4.0 * 48_000.0));
        assert_eq!(clips[6], json!(-1), "over a source nobody read");
    }

    /// **A box over a source that was read draws from that buffer**, and one
    /// over a source nobody read is honest about it rather than empty.
    #[test]
    fn a_box_names_the_buffer_its_source_was_read_into() {
        let multitrack = multitrack();
        let mut table = HashMap::new();
        table.insert(SourceId(77), 12);
        assert_eq!(clips(&multitrack, &look(&table))[6], json!(-1));

        let mut multitrack = multitrack;
        multitrack.tracks[0].lanes[0].regions[0].content = Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(77),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start: 0.0,
                duration: 4.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        };
        assert_eq!(clips(&multitrack, &look(&table))[6], json!(12));
    }

    /// **A layer's break-points are its box's own time, and a row's are the
    /// timeline's.** The one thing that differs between the two on the wire,
    /// and getting it wrong puts a clip envelope where its box is rather than
    /// at its start -- silently, since both are valid positions.
    #[test]
    fn a_layer_is_measured_from_its_box_and_a_row_from_the_origin() {
        let multitrack = multitrack();
        let table = HashMap::new();
        let points = points(&multitrack, &look(&table));

        // Five values a point, the track's curve first, then the box's.
        let at = |i: usize| points[i * 5..i * 5 + 5].to_vec();
        assert_eq!(at(0)[0], json!("4"));
        assert_eq!(at(0)[1], json!(0.0), "the row's first point is the origin");
        assert_eq!(at(1)[1], json!(1.0 * 48_000.0), "and one second along it");
        assert_eq!(at(2)[0], json!("5"));
        assert_eq!(
            at(2)[1],
            json!(0.0),
            "the layer's first point is its box's start, not the timeline's"
        );
        assert_eq!(at(3)[1], json!(2.0 * 48_000.0));
    }

    /// A curve says what range it is drawn over through what it automates; a
    /// curve that says nothing is drawn over unity, which is what an
    /// unlabelled level means.
    #[test]
    fn a_curve_is_drawn_over_the_range_its_target_states() {
        let mut multitrack = multitrack();
        assert_eq!(curves(&multitrack)[3..5], [json!(0.0), json!(1.0)]);

        multitrack.tracks[0].automation[0].target = Opaque(json!({"min": -1.0, "max": 1.0}));
        let drawn = curves(&multitrack);
        assert_eq!(drawn[3..5], [json!(-1.0), json!(1.0)]);
        assert_eq!(drawn[5], json!(CURVE_H), "and it has a row of its own");
        assert_eq!(layers(&multitrack).len(), 5, "a layer has no height");
    }

    /// The JSON door is the same answer, and an unreadable multitrack is an empty
    /// object rather than an error: a projection has nothing to refuse.
    #[test]
    fn the_door_answers_the_same_thing_and_refuses_nothing() {
        let multitrack = multitrack();
        let body = serde_json::to_string(&multitrack).expect("a multitrack");
        let answer: Map<String, Value> =
            serde_json::from_str(&props_json(&body, 48_000.0, "{}")).expect("JSON");
        let table = HashMap::new();
        assert_eq!(answer, props(&multitrack, &look(&table)));

        assert_eq!(props_json("not a multitrack", 48_000.0, "{}"), "{}");
    }

    /// **A gesture goes out and comes back on the same axis.** The props are
    /// read, one box is moved four seconds along in the widget's own frames, and
    /// what comes back names the second it was moved to -- the round trip that was
    /// written once per client before this existed.
    #[test]
    fn a_box_dragged_in_frames_comes_back_in_seconds() {
        let multitrack = multitrack();
        let table = HashMap::new();
        let look = look(&table);
        let mut drawn = clips(&multitrack, &look);
        drawn[2] = json!(number(&drawn[2]) + 4.0 * 48_000.0);

        let taken = intake(&multitrack, "clips", &drawn, &look);
        assert_eq!(taken.payloads.len(), 1);
        assert_eq!(taken.label, "move a clip");
        let moved = &taken.payloads[0];
        assert_eq!(moved["intent"], json!("placeregion"));
        assert_eq!(
            moved["position"],
            json!(8.0),
            "four seconds past the four it was at"
        );
    }

    /// **A layer's break-point goes back against the base it was drawn
    /// against.** It is the divergence the view projection surfaced, seen from
    /// the other direction: a curve read out and reported back unchanged is no
    /// edit at all, and it only is if both halves measure from the box.
    #[test]
    fn a_curve_reported_back_unchanged_is_not_an_edit() {
        let multitrack = multitrack();
        let table = HashMap::new();
        let look = look(&table);
        let drawn = points(&multitrack, &look);

        let taken = intake(&multitrack, "points", &drawn, &look);
        assert!(
            taken.payloads.is_empty(),
            "a hand that looked without editing moved nothing: {:?}",
            taken.payloads
        );

        // And one that did move a layer's point names that layer alone.
        let mut dragged = drawn.clone();
        dragged[3 * 5 + 2] = json!(0.75);
        let taken = intake(&multitrack, "points", &dragged, &look);
        assert_eq!(taken.payloads.len(), 1);
        assert_eq!(taken.payloads[0]["intent"], json!("setautomation"));
        assert_eq!(taken.payloads[0]["automation"], json!(5));
        assert_eq!(taken.label, "draw a curve");
    }

    /// A tag no hand over a multitrack makes is nothing, and a row named by
    /// something that is no track's id places no box.
    #[test]
    fn a_report_this_multitrack_cannot_place_is_dropped_and_not_guessed_at() {
        let multitrack = multitrack();
        let table = HashMap::new();
        let look = look(&table);
        assert_eq!(intake(&multitrack, "meters", &[], &look), Intake::nothing());

        let stray = vec![
            json!("n9"),
            json!("not-a-track"),
            json!(0.0),
            json!(48_000.0),
            json!(0.0),
            json!("stray"),
            json!(-1),
        ];
        let taken = intake(&multitrack, "clips", &stray, &look);
        assert_eq!(
            taken.payloads[0]["intent"],
            json!("setlane"),
            "the multitrack lost the box it had and gained none"
        );
    }

    /// The names a view keeps to tell a minted word from the multitrack's own id.
    ///
    /// **The curves are in it** *(found 2026-09-12 by the user)*: an automation
    /// the owner made is one the host cannot have drawn, so a view that
    /// compared only the rows and the boxes never answered with the picture
    /// when a curve appeared -- and the toggle that asks a track for its gain
    /// automation changed nothing on screen until some later gesture added a
    /// row and carried the curve back with it.
    #[test]
    fn a_multitrack_says_what_it_calls_its_rows_boxes_and_curves() {
        let named = names(&multitrack());
        assert_eq!(named["rows"], json!(["1"]));
        assert_eq!(named["boxes"], json!(["3"]));
        assert_eq!(
            named["curves"],
            json!(["4", "5"]),
            "the track's row and the box's layer, which are one question"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&names_json("not a multitrack")).expect("JSON"),
            json!({ "rows": [], "boxes": [], "curves": [] })
        );
    }

    /// A multitrack with one take cut in two on one lane: the head reads the take's
    /// first second, the tail its second, laid out in that order.
    fn halves() -> Multitrack {
        let mut multitrack = Multitrack::default();
        let mut track = Track::new(NodeId(1), NodeId(2));
        for (id, at, start) in [(NodeId(10), 0.0, 0.0), (NodeId(11), 1.0, 1.0)] {
            let mut region =
                Region::new(id, Second(at), Second(1.0), Content::Unknown(Value::Null));
            region.content = Content::window(SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(7),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start,
                duration: 1.0,
            });
            track.lanes[0].regions.push(region);
        }
        multitrack.tracks.push(track);
        multitrack
    }

    /// **The halves of a cut put back in order are the join they always were**:
    /// one window over one run, and nothing minted. A source made of one span of
    /// one take says nothing the take does not.
    #[test]
    fn halves_that_read_on_from_each_other_join_without_minting_anything() {
        let multitrack = halves();
        let sources = HashMap::new();
        let intents = read(
            &multitrack,
            "join",
            &[json!("10"), json!("11")],
            &look(&sources),
        );
        assert!(matches!(
            intents.as_slice(),
            [MultitrackIntent::JoinRegions {
                content: None,
                source: None,
                ..
            }]
        ));
    }

    /// **Boxes that meet on a sample still meet after the round trip, and
    /// join** *(found 2026-09-17 by the user: a snap that sometimes missed and a
    /// join refused)*. The host reports samples; a length crossed on its own
    /// (`dur / rate`) and its end crossed as `(at + dur) / rate` differ in their
    /// last bit for most sample counts, which the join read as a gap. Over many
    /// arbitrary cuts, the halves a report places must end and begin on one
    /// sample, draw back to the samples they were reported at, and join.
    #[test]
    fn halves_placed_at_any_sample_draw_back_there_and_join() {
        use clausters_document::multitrack::edit::apply;
        use clausters_document::{Against, Rules};
        let sources = HashMap::new();
        let look = look(&sources);
        for step in 0..200u32 {
            let at = 7_919.0 * f64::from(step) + 3.0;
            let cut = 13_331.0 + 97.0 * f64::from(step);
            let tail = 48_000.0 - cut;
            let mut multitrack = halves();
            let report = [
                json!("10"),
                json!("1"),
                json!(at),
                json!(cut),
                json!(0.0),
                json!(""),
                json!(-1),
                json!("11"),
                json!("1"),
                json!(at + cut),
                json!(tail),
                json!(cut),
                json!(""),
                json!(-1),
            ];
            for intent in read(&multitrack, "clips", &report, &look) {
                apply(
                    &mut multitrack,
                    &intent,
                    &Against::default(),
                    &Rules::none(),
                );
            }
            let drawn = clips(&multitrack, &look);
            assert_eq!(
                (number(&drawn[2]), number(&drawn[3]), number(&drawn[9])),
                (at, cut, at + cut),
                "drawn back at the samples it was reported at (step {step})"
            );
            let joined = reading(&multitrack, "join", &[json!("10"), json!("11")], &look);
            assert_eq!(joined.refusal, None, "step {step}: {joined:?}");
            assert_eq!(joined.intents.len(), 1);
        }
    }

    /// The two halves with the tail moved in front of the head: the gesture
    /// the user reported, and the shape every join test below is over.
    fn swapped() -> Multitrack {
        let mut multitrack = halves();
        let regions = &mut multitrack.tracks[0].lanes[0].regions;
        regions[0].position = Second(1.0);
        regions[1].position = Second(0.0);
        multitrack
    }

    /// **The gesture the user reported twice** *(2026-09-11 and 2026-09-12)*:
    /// the same two halves with the tail moved in front of the head.
    ///
    /// They touch exactly and they read one take, and there is still no region
    /// that describes them -- a region is one window onto one source, and these
    /// are its seconds in an order the take does not have. So the join makes the
    /// source: two spans, in the order the boxes show them, with a seam where
    /// the material is cut and none where it is not.
    #[test]
    fn halves_put_back_in_the_other_order_mint_the_source_they_are_a_window_onto() {
        // The tail at the front, the head behind it: what a hand does with a
        // drag and the proximity snap.
        let multitrack = swapped();
        let sources = HashMap::new();
        let intents = read(
            &multitrack,
            "join",
            &[json!("10"), json!("11")],
            &look(&sources),
        );
        let [
            MultitrackIntent::JoinRegions {
                regions,
                into,
                content: Some(content),
                source: Some(minted),
            },
        ] = intents.as_slice()
        else {
            panic!("a join that mints its source: {intents:?}");
        };
        // **In the order they are shown**, which is the whole of what the join
        // says: the tail first.
        assert_eq!(regions, &[NodeId(11), NodeId(10)]);
        assert_eq!(*into, NodeId(11), "the box in front keeps its identity");

        // The box is a plain window onto the new source, from its zero.
        let window = content.as_window().expect("a window");
        assert_eq!(
            window.source.samples().map(|s| s.source),
            Some(minted.id),
            "onto the source the join made"
        );
        assert_eq!((window.start, window.duration), (0.0, 2.0));
        assert_eq!(
            minted.id,
            SourceId(8),
            "minted clear of the take it is over"
        );

        // And the source is the two spans, in frames, with one seam.
        let Location::Segments { parts } = &minted.source.location else {
            panic!("a join is segments: {:?}", minted.source.location);
        };
        let seam = (picture::SEAM * 48_000.0) as u64;
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0].source.range,
            Some(clausters_document::Range {
                start: 48_000,
                end: 96_000
            }),
            "the take's second second is read first"
        );
        assert_eq!(
            parts[1].source.range,
            Some(clausters_document::Range {
                start: 0,
                end: 48_000
            })
        );
        assert_eq!((parts[0].fade_in, parts[0].fade_out), (0, seam));
        assert_eq!((parts[1].fade_in, parts[1].fade_out), (seam, 0));
        assert_eq!(minted.source.frames, Some(96_000));
    }

    /// **A join reads what each box shows, not what its window claims**
    /// *(found 2026-09-13, in a standalone host's log: `part 0: buffer 1 has
    /// 96000 frames and the part asks for 134434`, and a joined box that was
    /// neither seen nor heard)*. A left-hand trim slides a window's start and
    /// leaves its duration, so a trimmed box's window claims more of its take
    /// than the box plays; built from that claim a part ran past the end of the
    /// take, and the server refused the whole stitch.
    #[test]
    fn a_trimmed_box_joins_as_what_it_shows() {
        let mut multitrack = swapped();
        // The box in front, its left edge pulled in by half a second: it plays
        // from 1.5 s to the end of the take, while its window still says one
        // second from 1.5 s -- up to 2.5 s of a take that is two.
        let front = multitrack.tracks[0].lanes[0]
            .regions
            .iter_mut()
            .find(|r| r.id == NodeId(11))
            .expect("the tail, in front");
        front.position = Second(0.5);
        front.length = Second(0.5);
        if let Content::Window { window, .. } = &mut front.content {
            window.start = 1.5;
        }
        let sources = HashMap::new();
        let intents = read(
            &multitrack,
            "join",
            &[json!("10"), json!("11")],
            &look(&sources),
        );
        let [
            MultitrackIntent::JoinRegions {
                content: Some(content),
                source: Some(minted),
                ..
            },
        ] = intents.as_slice()
        else {
            panic!("a join that mints its source: {intents:?}");
        };
        let Location::Segments { parts } = &minted.source.location else {
            panic!("a join is segments: {:?}", minted.source.location);
        };
        assert_eq!(
            parts[0].source.range,
            Some(clausters_document::Range {
                start: 72_000,
                end: 96_000
            }),
            "the half second the trimmed box plays, and nothing past the take"
        );
        assert_eq!(
            parts[1].source.range,
            Some(clausters_document::Range {
                start: 0,
                end: 48_000
            })
        );
        assert_eq!(
            minted.source.frames,
            Some(72_000),
            "as long as what the boxes show"
        );
        assert_eq!(content.as_window().map(|w| w.duration), Some(1.5));
    }

    /// **A join over a box that reads past its take is refused as an edit**
    /// (found 2026-09-13). A trim is not bounded by its take, so a box can
    /// play past the end of the samples; minted, that join was a source the
    /// server refused to stitch, and the multitrack held a joined box over nothing.
    /// A caller that knows the take's length refuses it with its reason; one
    /// that does not checks nothing, as before.
    #[test]
    fn a_join_past_the_end_of_a_take_is_refused() {
        let mut multitrack = swapped();
        // The box in front plays from 1.5 s for three quarters of a second,
        // up to 2.25 s of a take that is two.
        let front = multitrack.tracks[0].lanes[0]
            .regions
            .iter_mut()
            .find(|r| r.id == NodeId(11))
            .expect("the tail, in front");
        let mut back = front.position;
        front.length = Second(0.75);
        back.0 += 0.75;
        let source = match &mut front.content {
            Content::Window { window, .. } => {
                window.start = 1.5;
                window.source.samples().map(|s| s.source)
            }
            _ => None,
        }
        .expect("a window onto a take");
        multitrack.tracks[0].lanes[0]
            .regions
            .iter_mut()
            .find(|r| r.id == NodeId(10))
            .expect("the head, behind")
            .position = back;
        let held = Held {
            buffers: HashMap::new(),
            lengths: HashMap::from([(source, 96_000)]),
            rates: HashMap::new(),
        };
        let known = Look {
            rate: 48_000.0,
            sources: &held,
        };
        let refused = reading(&multitrack, "join", &[json!("10"), json!("11")], &known);
        assert!(refused.intents.is_empty(), "{:?}", refused.intents);
        assert_eq!(
            refused.refusal,
            Some("one of these boxes reads past the end of its take")
        );

        // Not knowing the length checks nothing.
        let sources = HashMap::new();
        let joined = read(
            &multitrack,
            "join",
            &[json!("10"), json!("11")],
            &look(&sources),
        );
        assert_eq!(joined.len(), 1, "joined as before: {joined:?}");
    }

    /// **Lengths are read off the table a request carries**, and a zero -- a
    /// client's word for a length it does not know -- is left out.
    #[test]
    fn a_table_states_the_lengths_it_knows() {
        let sources = json!({
            "1": {"buffer": 4, "channels": 1, "frames": 96000},
            "2": {"buffer": 5, "channels": 2, "frames": 0},
            "3": {"buffer": 6, "channels": 2},
        });
        assert_eq!(lengths(&sources), HashMap::from([(SourceId(1), 96_000)]));
        assert_eq!(table(&sources).len(), 3);
    }

    /// **A join over a joined box is flat** *(asked for by the user
    /// 2026-09-13, after a join of joins was refused as nested more than four
    /// deep: segments are not to be nested as a data structure, being
    /// two-dimensional pointers into a buffer or a file, and a join of segments
    /// makes one new segments object)*. A box over a join reads through to
    /// the takes it is made of, so every part the new join states names a take.
    #[test]
    fn a_join_over_a_joined_box_names_only_takes() {
        use clausters_document::session::Part;

        // Source 8 is the swapped halves joined: the take's second second, then
        // its first, with a seam between.
        let part = |start: u64, end: u64, fade_in: u64, fade_out: u64| Part {
            source: SourceRef {
                source: SourceId(7),
                lifetime: Lifetime::Session,
                generation: 0,
                range: Some(clausters_document::Range { start, end }),
            },
            fade_in,
            fade_out,
            channels: None,
        };
        struct Table(HashMap<SourceId, i64>, Vec<Part>);
        impl Buffers for Table {
            fn bufnum(&self, source: SourceId) -> i64 {
                self.0.bufnum(source)
            }
            fn source(&self, bufnum: i64) -> Option<SourceId> {
                self.0.source(bufnum)
            }
            fn parts(&self, source: SourceId) -> Option<Vec<Part>> {
                (source == SourceId(8)).then(|| self.1.clone())
            }
        }
        let table = Table(
            HashMap::new(),
            vec![part(48_000, 96_000, 0, 480), part(0, 48_000, 480, 0)],
        );
        // A box over the join reading across its seam (0.5 s to 1.5 s of it),
        // and beside it a box over the take.
        let window = |source: u64, start: f64| {
            Content::window(SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start,
                duration: 1.0,
            })
        };
        let mut track = Track::new(NodeId(1), NodeId(2));
        let mut over_join = Region::new(
            NodeId(20),
            Second(0.0),
            Second(1.0),
            Content::Unknown(Value::Null),
        );
        over_join.content = window(8, 0.5);
        let mut over_take = Region::new(
            NodeId(21),
            Second(1.0),
            Second(1.0),
            Content::Unknown(Value::Null),
        );
        over_take.content = window(7, 0.0);
        track.lanes[0].regions = vec![over_join, over_take];
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let look = Look {
            rate: 48_000.0,
            sources: &table,
        };
        let intents = read(&multitrack, "join", &[json!("20"), json!("21")], &look);
        let [
            MultitrackIntent::JoinRegions {
                source: Some(minted),
                ..
            },
        ] = intents.as_slice()
        else {
            panic!("a join that mints its source: {intents:?}");
        };
        let Location::Segments { parts } = &minted.source.location else {
            panic!("a join is segments: {:?}", minted.source.location);
        };
        assert!(
            parts.iter().all(|p| p.source.source == SourceId(7)),
            "every part names the take, none the join: {parts:?}"
        );
        assert_eq!(
            parts,
            &vec![
                // The half second of the join's first part the box shows.
                part(72_000, 96_000, 0, 480),
                // The half second of its second, its own seam kept, and cut
                // where the next box starts.
                part(0, 24_000, 480, 480),
                // The box over the take, cut where it meets the join's box.
                part(0, 48_000, 480, 0),
            ]
        );
    }

    /// **The header's toggle makes the automation it is asked to show** *(asked
    /// for by the user 2026-09-12: "lo que te estoy pidiendo es que agregue/cree
    /// una automatizacion de gain para cualquier pista y que se pueda ocultar
    /// con toggle")*.
    ///
    /// The same shape a double click on a header has: the verb makes the thing
    /// rather than opening a question about it. A track with no curve reports
    /// its automation as not shown, so asking to see it is asking for one --
    /// and the second press hides what the first made rather than making a
    /// second.
    #[test]
    fn asking_a_bare_track_to_show_its_automation_makes_one() {
        let mut multitrack = Multitrack::default();
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0].regions.push(Region::new(
            NodeId(3),
            Second(0.0),
            Second(4.0),
            Content::Unknown(Value::Null),
        ));
        multitrack.tracks.push(track);
        let sources = HashMap::new();
        let look = look(&sources);

        // As drawn: no automation, so the toggle reads as off.
        let drawn = lanes(&multitrack);
        assert_eq!(drawn[6], json!(false));
        assert!(
            read(&multitrack, "lanes", &drawn, &look).is_empty(),
            "the rows as they were drawn are not an edit"
        );

        // The toggle goes on, and the multitrack gains the curve.
        let mut asked = drawn.clone();
        asked[6] = json!(true);
        let edits = read(&multitrack, "lanes", &asked, &look);
        let [MultitrackIntent::SetTracks { tracks }] = edits.as_slice() else {
            panic!("one settracks");
        };
        let made = &tracks[0].automation;
        assert_eq!(made.len(), 1, "one curve, made here");
        assert_eq!(made[0].name.as_deref(), Some("gain"));
        assert_eq!(made[0].target.0["port"], json!("gain"));
        assert!(made[0].visible && made[0].enabled);
        // **Flat at unity across the multitrack**, so there is a line to grab and
        // nothing is heard differently for having asked.
        assert_eq!(
            made[0]
                .points
                .iter()
                .map(|p| (p.at, p.value))
                .collect::<Vec<_>>(),
            vec![(0.0, 1.0), (4.0, 1.0)]
        );
        assert_ne!(made[0].id, NodeId(3), "and an id nothing else is using");

        // It is **heard**: a curve with a port and points reaches the plan,
        // which is what makes the toggle a document edit rather than a view's.
        let mut multitrack = multitrack.clone();
        multitrack.tracks = tracks.clone();
        let plan =
            clausters_document::multitrack::nodes::plan(&multitrack, 48_000.0, &HashMap::new());
        assert_eq!(plan.tracks[0].curves.len(), 1, "the server gets the curve");
        assert_eq!(plan.tracks[0].curves[0].port, "gain");

        // And the second press hides it rather than making a second.
        let drawn = lanes(&multitrack);
        assert_eq!(drawn[6], json!(true), "shown now");
        let mut asked = drawn.clone();
        asked[6] = json!(false);
        let edits = read(&multitrack, "lanes", &asked, &look);
        let [MultitrackIntent::SetTracks { tracks }] = edits.as_slice() else {
            panic!("one settracks");
        };
        assert_eq!(tracks[0].automation.len(), 1, "the same one");
        assert!(!tracks[0].automation[0].visible);
        // Hidden is a view's word: it still sounds.
        let mut hidden = multitrack.clone();
        hidden.tracks = tracks.clone();
        let plan = clausters_document::multitrack::nodes::plan(&hidden, 48_000.0, &HashMap::new());
        assert_eq!(
            plan.tracks[0].curves.len(),
            1,
            "a curve nobody is looking at is a curve that is still applied"
        );
    }

    /// **A source the multitrack stopped naming is still a source** *(found
    /// 2026-09-12 by the user: a second join left an empty box)*.
    ///
    /// A join's id was minted off the multitrack alone, and the multitrack stops naming a
    /// source the moment nothing windows it -- an undo, a box deleted, a joined
    /// box cut back up. The client still holds the buffer it made, so the next
    /// join was handed an id that already had samples behind it: the client saw
    /// an id it knew, made nothing, and the box became a window onto **the
    /// previous join**. So the table says what it holds, and the mint clears
    /// both.
    #[test]
    fn a_join_never_mints_a_source_whoever_holds_the_samples_is_already_using() {
        let multitrack = swapped();
        let held = [json!("10"), json!("11")];

        // Nobody holding anything: clear of the multitrack, which names 7.
        let none: HashMap<SourceId, i64> = HashMap::new();
        assert_eq!(minted(&multitrack, &held, &look(&none)), SourceId(8));

        // The client holds 8 already -- the join it made a moment ago, which
        // this multitrack no longer names because the box was undone.
        let mut sources: HashMap<SourceId, i64> = HashMap::new();
        sources.insert(SourceId(8), 1);
        assert_eq!(minted(&multitrack, &held, &look(&sources)), SourceId(9));
    }

    /// The source one `join` report mints.
    fn minted(multitrack: &Multitrack, values: &[Value], look: &Look<'_>) -> SourceId {
        let intents = read(multitrack, "join", values, look);
        match intents.as_slice() {
            [
                MultitrackIntent::JoinRegions {
                    source: Some(minted),
                    ..
                },
            ] => minted.id,
            other => panic!("a join that mints its source: {other:?}"),
        }
    }

    /// **A box the hand holds and the multitrack does not have is refused.**
    ///
    /// Joining the rest would leave that one where it is, under the box that
    /// now spans over it. Every bug this seam has produced has had this shape:
    /// a verb quietly acting on less than it was given.
    #[test]
    fn a_join_over_a_box_the_multitrack_does_not_have_is_refused_rather_than_partial() {
        let multitrack = swapped();
        let sources = HashMap::new();
        assert_eq!(
            intake(
                &multitrack,
                "join",
                &[json!("10"), json!("11"), json!("a 2")],
                &look(&sources)
            )
            .to_json()["refusal"],
            json!("one of these boxes is not one the multitrack has")
        );
    }

    /// **What a join is not, said out loud.** A gap and an overlap are the two
    /// cases `/buffer_stitch` cannot state, so they are refused rather than
    /// joined into something that plays material nobody placed -- and the
    /// refusal travels, which is the difference between a verb that is right
    /// and a key that looks dead.
    #[test]
    fn a_join_that_cannot_be_stated_says_which_of_the_cases_it_is() {
        let sources = HashMap::new();
        let held = [json!("10"), json!("11")];

        let mut multitrack = halves();
        multitrack.tracks[0].lanes[0].regions[1].position = Second(2.0);
        assert_eq!(
            intake(&multitrack, "join", &held, &look(&sources)).to_json()["refusal"],
            json!("there is a gap between these boxes, and a join cannot state silence yet")
        );

        let mut multitrack = halves();
        multitrack.tracks[0].lanes[0].regions[1].position = Second(0.5);
        assert_eq!(
            intake(&multitrack, "join", &held, &look(&sources)).to_json()["refusal"],
            json!("these boxes overlap, and a join cannot state a mix yet")
        );

        let multitrack = halves();
        assert_eq!(
            intake(&multitrack, "join", &[json!("10")], &look(&sources)).to_json()["refusal"],
            json!("a join needs two boxes or more in hand")
        );
    }
}
