//! **A piece as the props the multitrack widget is drawn with.**
//!
//! The projection over [`clausters_document::multitrack::picture`]: that module
//! says what a row and a box *are*, and this says how they reach a host. Every
//! payload here is a flat array, which is what the wire carries, and every one
//! of them was written three times before this existed — once in each client
//! and once in the standalone host, which needs the same picture with no client
//! in the process at all.
//!
//! # What the caller brings, and why it is not in the document
//!
//! Two things, both [`Look`]. **Where a beat lands**, which is the piece's own
//! tempo map and a sample rate: a position is `secs_at(beat) × rate` and a
//! *length* is the difference of two of those, because how long four beats last
//! depends on where they start. And **which server buffer a source was read
//! into**, which is a running server's fact and never a document's.
//!
//! # What is here and what is the caller's
//!
//! Here: everything a piece has **from the document alone** — the rows, the
//! boxes, the automations over both, their break-points, which of them are
//! hidden and which boxes loop. Not here: the position cursor (a window's), the
//! meter buses (a playback's, so the instance projection's) and the widget's
//! own chrome. The line is not taste — it is that a projection is a function of
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

use clausters_core::tempomap::TempoMap;
use clausters_document::multitrack::edit::MultitrackIntent;
use clausters_document::multitrack::nodes::SourceInfo;
use clausters_document::multitrack::{Multitrack, picture};
use clausters_document::{Beat, NodeId, Opaque, SourceId};

use crate::intake::{Intake, groups, number, text};

/// How tall a track's row is drawn.
pub const ROW_H: f64 = 96.0;

/// How tall an automation's own row under a track is drawn.
pub const CURVE_H: f64 = 40.0;

/// The value range a curve is drawn over when its target says nothing: unity,
/// which is what an unlabelled level means.
const UNIT: (f64, f64) = (0.0, 1.0);

/// What a caller knows that the document does not.
///
/// The tempo map and rate that put a beat on the shared axis, and the table
/// saying which server buffer each source was read into. Both are the reason
/// this is a projection rather than a picture: the document describes the
/// piece and stops exactly where a *running* system begins.
pub struct Look<'a> {
    /// The piece's own map, so the boxes and the readers cannot disagree about
    /// where a beat is.
    pub tempo: &'a TempoMap,
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
    /// What minting a source needs and a piece cannot answer: a source stops
    /// being named by the piece the moment nothing windows it, while whoever
    /// loaded it still holds the buffer. An id handed out off the piece alone
    /// can therefore already have samples behind it, and a box over it is then
    /// a window onto whatever that was.
    ///
    /// Defaulted to nothing so a caller that has no table is still a
    /// `Buffers` — it means *I hold none*, which is the honest answer for one.
    fn taken(&self) -> Vec<SourceId> {
        Vec::new()
    }

    /// **The other direction**: the source a buffer number came from, or `None`
    /// for a buffer this piece knows nothing about.
    ///
    /// Both are here because a box is *drawn* from a buffer and *read back*
    /// into a source, and a caller that answered only one of them would have
    /// the other written beside it — which is the second table this trait
    /// exists to prevent.
    fn source(&self, bufnum: i64) -> Option<SourceId>;
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

impl Look<'_> {
    /// The frame a beat lands on.
    pub fn frame_at(&self, beats: f64) -> f64 {
        self.tempo.secs_at(beats) * self.rate
    }

    /// How many frames `length` beats take **starting at** `start` — the
    /// difference of two positions, never a ratio.
    pub fn frames_over(&self, start: f64, length: f64) -> f64 {
        self.frame_at(start + length) - self.frame_at(start)
    }

    /// Where a beat measured **from `base`** falls, in frames **from `base`** —
    /// what a box's own axis counts in.
    ///
    /// A layer is drawn inside its box, so its break-points are the box's own
    /// time and not the timeline's. That is a *length* from the box's start,
    /// which is why it goes through [`frames_over`](Self::frames_over) rather
    /// than through [`frame_at`](Self::frame_at): four beats are not one length
    /// under a tempo that moves.
    pub fn frame_in(&self, base: f64, at: f64) -> f64 {
        self.frames_over(base, at)
    }

    /// The beat a frame falls on: the inverse of
    /// [`frame_at`](Self::frame_at), and the way an edit comes back.
    pub fn beat_at(&self, frame: f64) -> f64 {
        self.tempo
            .beats_at(frame / if self.rate == 0.0 { 1.0 } else { self.rate })
    }

    /// The beat, measured **from `base`**, that a frame from `base` falls on —
    /// the inverse of [`frame_in`](Self::frame_in).
    pub fn beat_in(&self, base: f64, frame: f64) -> f64 {
        self.beat_at(self.frame_at(base) + frame) - base
    }

    fn bufnum(&self, source: Option<SourceId>) -> i64 {
        source.map_or(-1, |id| self.sources.bufnum(id))
    }
}

/// The value range a curve is drawn over, out of what it automates.
///
/// **The document says what a curve automates and never reads it**, so which
/// range that parameter has — a gain over one, a pan over another — is a fact
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
pub fn lanes(piece: &Multitrack) -> Vec<Value> {
    let mut out = Vec::new();
    for row in picture::rows(piece) {
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

/// The boxes as the widget's flat septuples: name, lane, at, duration, the
/// frame of its source its own zero reads, label, buffer.
pub fn clips(piece: &Multitrack, look: &Look<'_>) -> Vec<Value> {
    let mut out = Vec::new();
    for box_ in picture::boxes(piece) {
        out.extend([
            json!(box_.region.0.to_string()),
            json!(box_.row.0.to_string()),
            json!(look.frame_at(box_.position.0)),
            json!(look.frames_over(box_.position.0, box_.length.0)),
            json!(box_.start * look.rate),
            json!(box_.label),
            json!(look.bufnum(box_.source)),
        ]);
    }
    out
}

/// The **track automations** as flat sextuples: a row of its own under the
/// track it names — name, owner, label, low, high, height.
pub fn curves(piece: &Multitrack) -> Vec<Value> {
    let mut out = Vec::new();
    for curve in picture::curves(piece) {
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
pub fn layers(piece: &Multitrack) -> Vec<Value> {
    let mut out = Vec::new();
    for curve in picture::layers(piece) {
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
/// on — one list for the rows and the layers alike.
///
/// **What each curve's time is measured from** is the one thing that differs
/// between the two: a track automation runs the timeline and is measured from
/// the origin, and a clip envelope is drawn inside its box and is measured from
/// where that box starts.
pub fn points(piece: &Multitrack, look: &Look<'_>) -> Vec<Value> {
    let bases = bases(piece);
    let mut out = Vec::new();
    let mut write = |curve: &picture::Curve, base: f64| {
        for point in &curve.points {
            let data = point.data.0.as_object();
            let read = |key: &str, default: f64| {
                data.and_then(|d| d.get(key))
                    .and_then(Value::as_f64)
                    .unwrap_or(default)
            };
            out.extend([
                json!(curve.automation.0.to_string()),
                json!(look.frame_in(base, point.at)),
                json!(point.value),
                json!(read("shape", 1.0)),
                json!(read("curve", 0.0)),
            ]);
        }
    };
    for curve in picture::curves(piece) {
        write(&curve, 0.0);
    }
    for curve in picture::layers(piece) {
        write(
            &curve,
            bases
                .get(&curve.automation.0.to_string())
                .copied()
                .unwrap_or(0.0),
        );
    }
    out
}

/// The automations a hand has folded away, by name — **read out of the piece**,
/// because which curves a person had showing is part of reopening the piece as
/// they left it.
pub fn hidden(piece: &Multitrack) -> String {
    let mut names = Vec::new();
    for curve in picture::curves(piece).iter().chain(&picture::layers(piece)) {
        if !curve.visible {
            names.push(curve.automation.0.to_string());
        }
    }
    names.join(" ")
}

/// Which boxes wrap, by name. A box that loops has always more past its end,
/// which is what an edge drag may do and how the samples draw under a box
/// longer than they are.
pub fn loops(piece: &Multitrack) -> String {
    picture::boxes(piece)
        .iter()
        .filter(|b| b.looping)
        .map(|b| b.region.0.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// **What the piece calls its rows and its boxes**, by the names the wire
/// carries them under.
///
/// The minting correction's half that is a fact about the piece. A gesture is
/// normally answered with an acknowledgement and nothing else, because the
/// report described the result: the host drew what it sent and the piece
/// agreed. The cases where it does not are the ones where the host **makes**
/// something — a track from a double click, a box from a split or a paste.
/// There the host mints the word (`track 1`, `white 2`) and the document mints
/// the id, so until the picture goes back the two are naming the same thing
/// differently.
///
/// And a name the piece does not know is not ignored: it is read as something
/// *new*. So the next report about that row or that box mints it again, and
/// again after that — a split box took a fresh id on every drag, losing
/// whatever was hung on it, and a box dropped on a new track landed on a track
/// nobody had.
///
/// So a view keeps what it was last told and compares. It is here rather than
/// read off the props by striding them because a stride is a flat array's
/// shape restated at the call site, and the shape is this module's.
pub fn names(piece: &Multitrack) -> Value {
    json!({
        "rows": picture::rows(piece)
            .iter()
            .map(|row| row.track.0.to_string())
            .collect::<Vec<_>>(),
        "boxes": picture::boxes(piece)
            .iter()
            .map(|box_| box_.region.0.to_string())
            .collect::<Vec<_>>(),
        // **And the curves** *(found 2026-09-12 by the user: "al crear un track
        // y activar A no hace nada, al crear otro track aparece la
        // automatizacion del anterior")*. A curve the owner made is the same
        // fact a box the owner made is: the host cannot have drawn it, because
        // it did not make it. Left out, the toggle that asks a track for its
        // gain automation worked the whole way down and changed nothing on
        // screen — until the next gesture that added a *row* fired the
        // correction, which then carried the previous track's curve with it.
        // That is why the two lists were never enough: they are not "what the
        // piece is called", they are "what the host was told", and the host is
        // told about rows, boxes **and** curves.
        "curves": picture::curves(piece)
            .iter()
            .chain(&picture::layers(piece))
            .map(|curve| curve.automation.0.to_string())
            .collect::<Vec<_>>(),
    })
}

/// [`names`] against a piece given as JSON.
pub fn names_json(piece: &str) -> String {
    let Ok(piece) = serde_json::from_str::<Multitrack>(piece) else {
        return r#"{"rows":[],"boxes":[],"curves":[]}"#.into();
    };
    names(&piece).to_string()
}

/// Every prop a piece has **from the document alone**, in one object.
///
/// What a caller adds is what is a function of something other than the piece:
/// the position cursor, the meter buses, and the widget's own chrome.
pub fn props(piece: &Multitrack, look: &Look<'_>) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("lanes".into(), Value::Array(lanes(piece)));
    out.insert("clips".into(), Value::Array(clips(piece, look)));
    out.insert("curves".into(), Value::Array(curves(piece)));
    out.insert("layers".into(), Value::Array(layers(piece)));
    out.insert("points".into(), Value::Array(points(piece, look)));
    out.insert("hidden".into(), json!(hidden(piece)));
    out.insert("loops".into(), json!(loops(piece)));
    out
}

/// [`props`] against a piece and a source table given as JSON, which is how the
/// two client doors carry them.
///
/// `sources` is **the same table the instance plan takes** — source id to
/// `{"buffer", "channels"}` — rather than a second one shaped for drawing: a
/// client that had to keep two would eventually keep two that disagree, and
/// what a box is drawn from and what it is played from are the same samples.
///
/// An unreadable piece answers an empty object rather than an error: a
/// projection has nothing to refuse.
pub fn props_json(piece: &str, rate: f64, default_bpm: f64, sources: &str) -> String {
    let Ok(piece) = serde_json::from_str::<Multitrack>(piece) else {
        return "{}".into();
    };
    let table = table(&serde_json::from_str::<Value>(sources).unwrap_or(Value::Null));
    let tempo = tempo_map(&piece, default_bpm);
    let look = Look {
        tempo: &tempo,
        rate,
        sources: &table,
    };
    Value::Object(props(&piece, &look)).to_string()
}

/// The piece's tempo map, as the shared core holds one.
///
/// The document writes beats per **minute**, as a score does; every tempo in
/// the map is per second.
pub fn tempo_map(piece: &Multitrack, default_bpm: f64) -> TempoMap {
    let changes: Vec<clausters_core::tempomap::TempoChange> = piece
        .tempo
        .iter()
        .map(|t| clausters_core::tempomap::TempoChange {
            beats: t.at.0,
            tempo: t.bpm / 60.0,
            ramp: t.ramp,
        })
        .collect();
    let default = default_bpm / 60.0;
    TempoMap::from_changes(&changes, default).unwrap_or_else(|_| TempoMap::new(default))
}

/// What the `lanes` prop takes and reports: flat `name label height mute solo
/// gain curves` septuples — the same width the `clips` prop happens to be, and
/// a different seven fields.
pub const LANE_FIELDS: usize = 7;

/// What the `clips` prop takes and reports: flat `name lane at duration start
/// label source` septuples.
pub const SEPTUPLE: usize = 7;

/// What the `points` prop takes and reports: flat `curve t v shape amount`
/// quintuples, each naming the curve it is on.
pub const POINT_QUINTUPLE: usize = 5;

/// **What each curve's time is measured from**, by curve name.
///
/// A track automation runs the timeline, so it is measured from the origin; a
/// clip envelope is drawn inside its box and is measured from where that box
/// starts. It is the one thing that differs between the two on the wire, and
/// both [`points`] and [`intake`] read it from here so a break-point cannot go
/// out against one base and come back against another.
fn bases(piece: &Multitrack) -> HashMap<String, f64> {
    let where_: HashMap<u64, f64> = picture::boxes(piece)
        .iter()
        .map(|b| (b.region.0, b.position.0))
        .collect();
    let mut out: HashMap<String, f64> = picture::curves(piece)
        .iter()
        .map(|c| (c.automation.0.to_string(), 0.0))
        .collect();
    for curve in picture::layers(piece) {
        let base = where_.get(&curve.owner.0).copied().unwrap_or(0.0);
        out.insert(curve.automation.0.to_string(), base);
    }
    out
}

/// The flat `clips` report as the crate's boxes: names as they came, positions
/// in beats, the window's own numbers in seconds.
///
/// A row is named by its **track's id** and never renamed, so a name that is
/// not one names no row this piece has and the box on it is dropped rather than
/// placed somewhere it was not.
fn placed(values: &[Value], look: &Look<'_>) -> Vec<picture::Placed> {
    let mut out = Vec::new();
    for group in groups(values, SEPTUPLE) {
        let Ok(row) = text(&group[1]).parse::<u64>() else {
            continue;
        };
        let (at, dur) = (number(&group[2]), number(&group[3]));
        let position = look.beat_at(at);
        let length = look.beat_at(at + dur) - position;
        out.push(picture::Placed {
            name: text(&group[0]),
            row: NodeId(row),
            position: Beat(position),
            length: Beat(length),
            start: number(&group[4]) / if look.rate == 0.0 { 1.0 } else { look.rate },
            // How much a **new** box shows: the stretch it occupies, crossed to
            // the wall clock the only way a length may be.
            content: look.tempo.span_secs(position, position + length),
            source: look.sources.source(number(&group[6]) as i64),
        });
    }
    out
}

/// The flat `points` report as the crate's curves: one entry per curve named,
/// its break-points back on the musical axis.
///
/// The widget reports **every** curve there is, in one list, so they are
/// gathered by name here — the reader says nothing about the ones that did not
/// move.
fn curved(piece: &Multitrack, values: &[Value], look: &Look<'_>) -> Vec<picture::Curved> {
    let bases = bases(piece);
    let mut order: Vec<String> = Vec::new();
    let mut found: HashMap<String, Vec<clausters_document::points::Point>> = HashMap::new();
    for group in groups(values, POINT_QUINTUPLE) {
        let name = text(&group[0]);
        // **Against the same base the picture was drawn from**: a layer's time
        // is its box's own, so a break-point inside one comes back as a beat
        // from that box's start.
        let base = bases.get(&name).copied().unwrap_or(0.0);
        let points = found.entry(name.clone()).or_insert_with(|| {
            order.push(name.clone());
            Vec::new()
        });
        points.push(clausters_document::points::Point {
            at: look.beat_in(base, number(&group[1])),
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
/// its height is this window's. Neither is a fact about the piece.
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

/// **What an undo menu calls each of the piece's verbs.**
///
/// One table, because a menu entry a hand reads is part of what an edit *is* to
/// the person who made it — and a verb named two ways in two clients is the same
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
        _ => "edit the piece",
    }
}

/// **The words this domain answers for**, and the only place they are listed.
///
/// A tag is a domain's vocabulary, so *which* tags are the piece's is a fact
/// about the piece and not about whoever is routing a report to it. It was
/// written twice — here, and in the GUI host's own dispatch, which knew about
/// `clips` and `lanes` and had never heard of the other two — and the second
/// list was two tags short: a curve dragged in a host with no client attached
/// reached nobody, and so did a `join`. A caller asks; nobody restates.
pub fn answers(tag: &str) -> bool {
    matches!(tag, "clips" | "lanes" | "points" | "join")
}

/// **What a report came to**: the edits, or the reason there are none.
///
/// The two are one answer because a caller has to tell them apart: no edits
/// because the hand changed nothing, and no edits because the piece **refused**,
/// are the same empty list and opposite things to say to the person who made the
/// gesture. Everything that reads a report goes through here, so neither door
/// can quietly drop the half the other keeps.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reading {
    /// The edits, in the piece's own vocabulary.
    pub intents: Vec<MultitrackIntent>,
    /// Why there are none, when the piece refused the verb rather than finding
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

    /// A verb the piece refused, and why.
    fn refused(why: &'static str) -> Self {
        Reading {
            intents: Vec::new(),
            refusal: Some(why),
        }
    }

    /// The label one entry in the pile takes: the **first** payload's, because
    /// the payloads of one report are one thing a hand did.
    pub fn label(&self) -> &'static str {
        self.intents.first().map_or("edit the piece", label)
    }
}

/// **What a gesture over a piece means**, in the piece's own vocabulary.
///
/// Four tags, and three of them report the **whole** structure rather than the
/// gesture: every box, every row, every break-point. So a move, a block drag, a
/// trim, a split, a delete and a paste all arrive the same way and telling them
/// apart is one rule, [`clausters_document::multitrack::picture`]'s, written
/// once — and what comes back is the *difference*, which is why a hand that
/// looked without editing produces nothing at all.
pub fn reading(piece: &Multitrack, tag: &str, values: &[Value], look: &Look<'_>) -> Reading {
    match tag {
        "clips" => Reading::of(picture::read(
            piece,
            &placed(values, look),
            picture::fresh_id(piece),
        )),
        "lanes" => Reading::of(picture::read_rows(piece, &strips(values))),
        "points" => Reading::of(picture::read_points(piece, &curved(piece, values, look))),
        // **The one verb that is stated rather than differenced**, and the one
        // that can be refused on the *material*: a join and a "delete one,
        // lengthen the other" leave a lane holding the same thing, and a box in
        // a `clips` report names one source and one start -- so fragments
        // joined into one box have no report that describes them. A gap it
        // cannot state as silence and an overlap it cannot state as a mix are
        // refusals, and they are the reason this function answers with more
        // than a list.
        "join" => {
            match picture::read_join(piece, &held(values), look.rate, &look.sources.taken()) {
                Ok(intents) => Reading::of(intents),
                Err(why) => Reading::refused(why),
            }
        }
        _ => Reading::default(),
    }
}

/// [`reading`]'s edits alone, for a caller with nothing to say about a refusal.
pub fn read(
    piece: &Multitrack,
    tag: &str,
    values: &[Value],
    look: &Look<'_>,
) -> Vec<MultitrackIntent> {
    reading(piece, tag, values, look).intents
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
pub fn intake(piece: &Multitrack, tag: &str, values: &[Value], look: &Look<'_>) -> Intake {
    if !answers(tag) {
        return Intake::nothing();
    }
    let reading = reading(piece, tag, values, look);
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

/// [`intake`] against a piece and a source table given as JSON values, which
/// is how the one door carries them.
///
/// An unreadable piece answers [`Intake::nothing`]: there is no piece to say
/// what the gesture meant, and inventing one would write an edit against a
/// structure nobody has.
pub fn intake_value(
    piece: &Value,
    tag: &str,
    values: &[Value],
    rate: f64,
    default_bpm: f64,
    sources: &Value,
) -> Intake {
    let Ok(piece) = serde_json::from_value::<Multitrack>(piece.clone()) else {
        return Intake::nothing();
    };
    let table = table(sources);
    let tempo = tempo_map(&piece, default_bpm);
    let look = Look {
        tempo: &tempo,
        rate,
        sources: &table,
    };
    intake(&piece, tag, values, &look)
}

/// The instance plan's source table as the buffer question this crate asks.
fn table(sources: &Value) -> HashMap<SourceId, i64> {
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
        Beat, Lifetime, NodeId, Opaque, SegmentRef, SegmentSource, SourceRef,
    };

    /// One track at half gain with one box on it, a track automation over the
    /// timeline and an envelope inside the box.
    fn piece() -> Multitrack {
        let mut region = Region::new(
            NodeId(3),
            Beat(4.0),
            Beat(4.0),
            Content::Unknown(Value::Null),
        );
        region.automation.push(curve(NodeId(5), 2.0));
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.name = Some("drums".into());
        track.level = 0.5;
        track.lanes[0].regions.push(region);
        track.automation.push(curve(NodeId(4), 1.0));
        let mut piece = Multitrack::default();
        piece.tracks.push(track);
        piece
    }

    /// A curve with one point at the origin and one `at` beats along.
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
    /// which is the case a real piece is in after its first edit.
    fn shape() -> Opaque {
        Opaque(json!({ "shape": 1, "curve": 0.0 }))
    }

    fn look<'a>(tempo: &'a TempoMap, sources: &'a HashMap<SourceId, i64>) -> Look<'a> {
        Look {
            tempo,
            rate: 48_000.0,
            sources,
        }
    }

    /// The two flat payloads, in the shapes the widget takes: a row is seven
    /// values and a box seven, and they are not the same seven.
    #[test]
    fn a_row_is_seven_values_and_a_box_is_seven() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        let look = look(&tempo, &table);

        let lanes = lanes(&piece);
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

        let clips = clips(&piece, &look);
        assert_eq!(clips.len(), 7);
        assert_eq!(clips[0], json!("3"), "and a box by its region's");
        assert_eq!(clips[1], json!("1"), "on the row it is on");
        // A beat a second, so four beats in is four seconds in.
        assert_eq!(clips[2], json!(4.0 * 48_000.0));
        assert_eq!(clips[3], json!(4.0 * 48_000.0));
        assert_eq!(clips[6], json!(-1), "over a source nobody read");
    }

    /// **A box over a source that was read draws from that buffer**, and one
    /// over a source nobody read is honest about it rather than empty.
    #[test]
    fn a_box_names_the_buffer_its_source_was_read_into() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let mut table = HashMap::new();
        table.insert(SourceId(77), 12);
        assert_eq!(clips(&piece, &look(&tempo, &table))[6], json!(-1));

        let mut piece = piece;
        piece.tracks[0].lanes[0].regions[0].content = Content::Window {
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
        assert_eq!(clips(&piece, &look(&tempo, &table))[6], json!(12));
    }

    /// **A layer's break-points are its box's own time, and a row's are the
    /// timeline's.** The one thing that differs between the two on the wire,
    /// and getting it wrong puts a clip envelope where its box is rather than
    /// at its start — silently, since both are valid positions.
    #[test]
    fn a_layer_is_measured_from_its_box_and_a_row_from_the_origin() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        let points = points(&piece, &look(&tempo, &table));

        // Five values a point, the track's curve first, then the box's.
        let at = |i: usize| points[i * 5..i * 5 + 5].to_vec();
        assert_eq!(at(0)[0], json!("4"));
        assert_eq!(at(0)[1], json!(0.0), "the row's first point is the origin");
        assert_eq!(at(1)[1], json!(1.0 * 48_000.0), "and one beat along it");
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
        let mut piece = piece();
        assert_eq!(curves(&piece)[3..5], [json!(0.0), json!(1.0)]);

        piece.tracks[0].automation[0].target = Opaque(json!({"min": -1.0, "max": 1.0}));
        let drawn = curves(&piece);
        assert_eq!(drawn[3..5], [json!(-1.0), json!(1.0)]);
        assert_eq!(drawn[5], json!(CURVE_H), "and it has a row of its own");
        assert_eq!(layers(&piece).len(), 5, "a layer has no height");
    }

    /// The JSON door is the same answer, and an unreadable piece is an empty
    /// object rather than an error: a projection has nothing to refuse.
    #[test]
    fn the_door_answers_the_same_thing_and_refuses_nothing() {
        let piece = piece();
        let body = serde_json::to_string(&piece).expect("a piece");
        let answer: Map<String, Value> =
            serde_json::from_str(&props_json(&body, 48_000.0, 60.0, "{}")).expect("JSON");
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        assert_eq!(answer, props(&piece, &look(&tempo, &table)));

        assert_eq!(props_json("not a piece", 48_000.0, 60.0, "{}"), "{}");
    }

    /// **A gesture goes out and comes back on the same axis.** The props are
    /// read, one box is moved four beats along in the widget's own frames, and
    /// what comes back names the beat it was moved to — the round trip that was
    /// written once per client before this existed.
    #[test]
    fn a_box_dragged_in_frames_comes_back_in_beats() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        let look = look(&tempo, &table);
        let mut drawn = clips(&piece, &look);
        drawn[2] = json!(number(&drawn[2]) + 4.0 * 48_000.0);

        let taken = intake(&piece, "clips", &drawn, &look);
        assert_eq!(taken.payloads.len(), 1);
        assert_eq!(taken.label, "move a clip");
        let moved = &taken.payloads[0];
        assert_eq!(moved["intent"], json!("placeregion"));
        assert_eq!(
            moved["position"],
            json!(8.0),
            "four beats past the four it was at"
        );
    }

    /// **A layer's break-point goes back against the base it was drawn
    /// against.** It is the divergence the view projection surfaced, seen from
    /// the other direction: a curve read out and reported back unchanged is no
    /// edit at all, and it only is if both halves measure from the box.
    #[test]
    fn a_curve_reported_back_unchanged_is_not_an_edit() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        let look = look(&tempo, &table);
        let drawn = points(&piece, &look);

        let taken = intake(&piece, "points", &drawn, &look);
        assert!(
            taken.payloads.is_empty(),
            "a hand that looked without editing moved nothing: {:?}",
            taken.payloads
        );

        // And one that did move a layer's point names that layer alone.
        let mut dragged = drawn.clone();
        dragged[3 * 5 + 2] = json!(0.75);
        let taken = intake(&piece, "points", &dragged, &look);
        assert_eq!(taken.payloads.len(), 1);
        assert_eq!(taken.payloads[0]["intent"], json!("setautomation"));
        assert_eq!(taken.payloads[0]["automation"], json!(5));
        assert_eq!(taken.label, "draw a curve");
    }

    /// A tag no hand over a piece makes is nothing, and a row named by
    /// something that is no track's id places no box.
    #[test]
    fn a_report_this_piece_cannot_place_is_dropped_and_not_guessed_at() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        let look = look(&tempo, &table);
        assert_eq!(intake(&piece, "meters", &[], &look), Intake::nothing());

        let stray = vec![
            json!("n9"),
            json!("not-a-track"),
            json!(0.0),
            json!(48_000.0),
            json!(0.0),
            json!("stray"),
            json!(-1),
        ];
        let taken = intake(&piece, "clips", &stray, &look);
        assert_eq!(
            taken.payloads[0]["intent"],
            json!("setlane"),
            "the piece lost the box it had and gained none"
        );
    }

    /// The names a view keeps to tell a minted word from the piece's own id.
    ///
    /// **The curves are in it** *(found 2026-09-12 by the user)*: an automation
    /// the owner made is one the host cannot have drawn, so a view that
    /// compared only the rows and the boxes never answered with the picture
    /// when a curve appeared — and the toggle that asks a track for its gain
    /// automation changed nothing on screen until some later gesture added a
    /// row and carried the curve back with it.
    #[test]
    fn a_piece_says_what_it_calls_its_rows_boxes_and_curves() {
        let named = names(&piece());
        assert_eq!(named["rows"], json!(["1"]));
        assert_eq!(named["boxes"], json!(["3"]));
        assert_eq!(
            named["curves"],
            json!(["4", "5"]),
            "the track's row and the box's layer, which are one question"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&names_json("not a piece")).expect("JSON"),
            json!({ "rows": [], "boxes": [], "curves": [] })
        );
    }

    /// A piece with one take cut in two on one lane: the head reads the take's
    /// first second, the tail its second, laid out in that order.
    fn halves() -> Multitrack {
        let mut piece = Multitrack::default();
        let mut track = Track::new(NodeId(1), NodeId(2));
        for (id, at, start) in [(NodeId(10), 0.0, 0.0), (NodeId(11), 1.0, 1.0)] {
            let mut region = Region::new(id, Beat(at), Beat(1.0), Content::Unknown(Value::Null));
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
        piece.tracks.push(track);
        piece
    }

    /// **The halves of a cut put back in order are the join they always were**:
    /// one window over one run, and nothing minted. A source made of one span of
    /// one take says nothing the take does not.
    #[test]
    fn halves_that_read_on_from_each_other_join_without_minting_anything() {
        let piece = halves();
        let tempo = TempoMap::new(1.0);
        let sources = HashMap::new();
        let intents = read(
            &piece,
            "join",
            &[json!("10"), json!("11")],
            &look(&tempo, &sources),
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

    /// The two halves with the tail moved in front of the head: the gesture
    /// the user reported, and the shape every join test below is over.
    fn swapped() -> Multitrack {
        let mut piece = halves();
        let regions = &mut piece.tracks[0].lanes[0].regions;
        regions[0].position = Beat(1.0);
        regions[1].position = Beat(0.0);
        piece
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
        let piece = swapped();
        let tempo = TempoMap::new(1.0);
        let sources = HashMap::new();
        let intents = read(
            &piece,
            "join",
            &[json!("10"), json!("11")],
            &look(&tempo, &sources),
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

    /// **The header's toggle makes the automation it is asked to show** *(asked
    /// for by the user 2026-09-12: "lo que te estoy pidiendo es que agregue/cree
    /// una automatizacion de gain para cualquier pista y que se pueda ocultar
    /// con toggle")*.
    ///
    /// The same shape a double click on a header has: the verb makes the thing
    /// rather than opening a question about it. A track with no curve reports
    /// its automation as not shown, so asking to see it is asking for one —
    /// and the second press hides what the first made rather than making a
    /// second.
    #[test]
    fn asking_a_bare_track_to_show_its_automation_makes_one() {
        let mut piece = Multitrack::default();
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0].regions.push(Region::new(
            NodeId(3),
            Beat(0.0),
            Beat(4.0),
            Content::Unknown(Value::Null),
        ));
        piece.tracks.push(track);
        let tempo = TempoMap::new(1.0);
        let sources = HashMap::new();
        let look = look(&tempo, &sources);

        // As drawn: no automation, so the toggle reads as off.
        let drawn = lanes(&piece);
        assert_eq!(drawn[6], json!(false));
        assert!(
            read(&piece, "lanes", &drawn, &look).is_empty(),
            "the rows as they were drawn are not an edit"
        );

        // The toggle goes on, and the piece gains the curve.
        let mut asked = drawn.clone();
        asked[6] = json!(true);
        let edits = read(&piece, "lanes", &asked, &look);
        let [MultitrackIntent::SetTracks { tracks }] = edits.as_slice() else {
            panic!("one settracks");
        };
        let made = &tracks[0].automation;
        assert_eq!(made.len(), 1, "one curve, made here");
        assert_eq!(made[0].name.as_deref(), Some("gain"));
        assert_eq!(made[0].target.0["port"], json!("gain"));
        assert!(made[0].visible && made[0].enabled);
        // **Flat at unity across the piece**, so there is a line to grab and
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
        let mut piece = piece.clone();
        piece.tracks = tracks.clone();
        let plan =
            clausters_document::multitrack::nodes::plan(&piece, 48_000.0, 60.0, &HashMap::new());
        assert_eq!(plan.tracks[0].curves.len(), 1, "the server gets the curve");
        assert_eq!(plan.tracks[0].curves[0].port, "gain");

        // And the second press hides it rather than making a second.
        let drawn = lanes(&piece);
        assert_eq!(drawn[6], json!(true), "shown now");
        let mut asked = drawn.clone();
        asked[6] = json!(false);
        let edits = read(&piece, "lanes", &asked, &look);
        let [MultitrackIntent::SetTracks { tracks }] = edits.as_slice() else {
            panic!("one settracks");
        };
        assert_eq!(tracks[0].automation.len(), 1, "the same one");
        assert!(!tracks[0].automation[0].visible);
        // Hidden is a view's word: it still sounds.
        let mut hidden = piece.clone();
        hidden.tracks = tracks.clone();
        let plan =
            clausters_document::multitrack::nodes::plan(&hidden, 48_000.0, 60.0, &HashMap::new());
        assert_eq!(
            plan.tracks[0].curves.len(),
            1,
            "a curve nobody is looking at is a curve that is still applied"
        );
    }

    /// **A source the piece stopped naming is still a source** *(found
    /// 2026-09-12 by the user: a second join left an empty box)*.
    ///
    /// A join's id was minted off the piece alone, and the piece stops naming a
    /// source the moment nothing windows it — an undo, a box deleted, a joined
    /// box cut back up. The client still holds the buffer it made, so the next
    /// join was handed an id that already had samples behind it: the client saw
    /// an id it knew, made nothing, and the box became a window onto **the
    /// previous join**. So the table says what it holds, and the mint clears
    /// both.
    #[test]
    fn a_join_never_mints_a_source_whoever_holds_the_samples_is_already_using() {
        let piece = swapped();
        let tempo = TempoMap::new(1.0);
        let held = [json!("10"), json!("11")];

        // Nobody holding anything: clear of the piece, which names 7.
        let none: HashMap<SourceId, i64> = HashMap::new();
        assert_eq!(minted(&piece, &held, &look(&tempo, &none)), SourceId(8));

        // The client holds 8 already -- the join it made a moment ago, which
        // this piece no longer names because the box was undone.
        let mut sources: HashMap<SourceId, i64> = HashMap::new();
        sources.insert(SourceId(8), 1);
        assert_eq!(minted(&piece, &held, &look(&tempo, &sources)), SourceId(9));
    }

    /// The source one `join` report mints.
    fn minted(piece: &Multitrack, values: &[Value], look: &Look<'_>) -> SourceId {
        let intents = read(piece, "join", values, look);
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

    /// **A box the hand holds and the piece does not have is refused.**
    ///
    /// Joining the rest would leave that one where it is, under the box that
    /// now spans over it. Every bug this seam has produced has had this shape:
    /// a verb quietly acting on less than it was given.
    #[test]
    fn a_join_over_a_box_the_piece_does_not_have_is_refused_rather_than_partial() {
        let piece = swapped();
        let tempo = TempoMap::new(1.0);
        let sources = HashMap::new();
        assert_eq!(
            intake(
                &piece,
                "join",
                &[json!("10"), json!("11"), json!("a 2")],
                &look(&tempo, &sources)
            )
            .to_json()["refusal"],
            json!("one of these boxes is not one the piece has")
        );
    }

    /// **What a join is not, said out loud.** A gap and an overlap are the two
    /// cases `/buffer_stitch` cannot state, so they are refused rather than
    /// joined into something that plays material nobody placed -- and the
    /// refusal travels, which is the difference between a verb that is right
    /// and a key that looks dead.
    #[test]
    fn a_join_that_cannot_be_stated_says_which_of_the_cases_it_is() {
        let tempo = TempoMap::new(1.0);
        let sources = HashMap::new();
        let held = [json!("10"), json!("11")];

        let mut piece = halves();
        piece.tracks[0].lanes[0].regions[1].position = Beat(2.0);
        assert_eq!(
            intake(&piece, "join", &held, &look(&tempo, &sources)).to_json()["refusal"],
            json!("there is a gap between these boxes, and a join cannot state silence yet")
        );

        let mut piece = halves();
        piece.tracks[0].lanes[0].regions[1].position = Beat(0.5);
        assert_eq!(
            intake(&piece, "join", &held, &look(&tempo, &sources)).to_json()["refusal"],
            json!("these boxes overlap, and a join cannot state a mix yet")
        );

        let piece = halves();
        assert_eq!(
            intake(&piece, "join", &[json!("10")], &look(&tempo, &sources)).to_json()["refusal"],
            json!("a join needs two boxes or more in hand")
        );
    }
}
