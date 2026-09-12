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
use clausters_document::SourceId;
use clausters_document::multitrack::nodes::SourceInfo;
use clausters_document::multitrack::{Multitrack, picture};

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
}

impl Buffers for HashMap<SourceId, i64> {
    fn bufnum(&self, source: SourceId) -> i64 {
        self.get(&source).copied().unwrap_or(-1)
    }
}

/// Nothing was resolved: every box is a box over nothing.
impl Buffers for () {
    fn bufnum(&self, _source: SourceId) -> i64 {
        -1
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
    let where_: HashMap<u64, f64> = picture::boxes(piece)
        .iter()
        .map(|b| (b.region.0, b.position.0))
        .collect();
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
        write(&curve, where_.get(&curve.owner.0).copied().unwrap_or(0.0));
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
    let table: HashMap<SourceId, i64> =
        serde_json::from_str::<HashMap<String, SourceInfo>>(sources)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, info)| {
                id.parse::<u64>()
                    .ok()
                    .map(|id| (SourceId(id), i64::from(info.buffer)))
            })
            .collect();
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
                data: Opaque::none(),
            },
            Point {
                at,
                value: 1.0,
                data: Opaque::none(),
            },
        ];
        a
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
}
