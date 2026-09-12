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
    })
}

/// [`names`] against a piece given as JSON.
pub fn names_json(piece: &str) -> String {
    let Ok(piece) = serde_json::from_str::<Multitrack>(piece) else {
        return r#"{"rows":[],"boxes":[]}"#.into();
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
/// gain` sextuples.
pub const SEXTUPLE: usize = 6;

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
    groups(values, SEXTUPLE)
        .map(|group| picture::Strip {
            name: text(&group[0]),
            mute: number(&group[3]) != 0.0,
            solo: number(&group[4]) != 0.0,
            gain: number(&group[5]),
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

/// **What a gesture over a piece means**, in the piece's own vocabulary.
///
/// Three tags, and each of them reports the **whole** structure rather than the
/// gesture: every box, every row, every break-point. So a move, a block drag, a
/// trim, a split, a delete and a paste all arrive the same way and telling them
/// apart is one rule, [`clausters_document::multitrack::picture`]'s, written
/// once — and what comes back is the *difference*, which is why a hand that
/// looked without editing produces nothing at all.
///
/// The label is the first payload's, because the payloads of one report are one
/// thing a hand did and go into the pile as one entry.
pub fn read(
    piece: &Multitrack,
    tag: &str,
    values: &[Value],
    look: &Look<'_>,
) -> Vec<MultitrackIntent> {
    match tag {
        "clips" => picture::read(piece, &placed(values, look), picture::fresh_id(piece)),
        "lanes" => picture::read_rows(piece, &strips(values)),
        "points" => picture::read_points(piece, &curved(piece, values, look)),
        _ => Vec::new(),
    }
}

/// [`read`] as the payloads and the label an endpoint carries.
///
/// The label is the **first** intent's, because the intents of one report are
/// one thing a hand did and go into the pile as one entry.
pub fn intake(piece: &Multitrack, tag: &str, values: &[Value], look: &Look<'_>) -> Intake {
    if !matches!(tag, "clips" | "lanes" | "points") {
        return Intake::nothing();
    }
    let intents = read(piece, tag, values, look);
    let named = intents
        .first()
        .map_or("edit the piece", |first| label(first));
    let payloads = intents
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

    /// The two flat payloads, in the shapes the widget takes: a row is six
    /// values and a box seven.
    #[test]
    fn a_row_is_six_values_and_a_box_is_seven() {
        let piece = piece();
        let tempo = tempo_map(&piece, 60.0);
        let table = HashMap::new();
        let look = look(&tempo, &table);

        let lanes = lanes(&piece);
        assert_eq!(lanes.len(), 6);
        assert_eq!(lanes[0], json!("1"), "a row is named by its track's id");
        assert_eq!(lanes[1], json!("drums"));
        assert_eq!(lanes[2], json!(ROW_H));
        assert_eq!(lanes[5], json!(0.5));

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
    #[test]
    fn a_piece_says_what_it_calls_its_rows_and_boxes() {
        let named = names(&piece());
        assert_eq!(named["rows"], json!(["1"]));
        assert_eq!(named["boxes"], json!(["3"]));
        assert_eq!(
            serde_json::from_str::<Value>(&names_json("not a piece")).expect("JSON"),
            json!({ "rows": [], "boxes": [] })
        );
    }
}
