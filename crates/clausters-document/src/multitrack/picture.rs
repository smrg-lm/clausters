//! **The piece as a multitrack view holds it**: rows and boxes, and a hand's
//! answer read back.
//!
//! The mapping between the model and the picture, in one place because there is
//! one of it. A multitrack view — the standalone host's, the Python client's,
//! the web client's — draws a **row** per track and a **box** per region, and
//! reports the whole list after any gesture. Which verb that list stands for is
//! not obvious (a move and a trim are different edits; a box the piece has no
//! region for is a new one), and a reader written per client is a reader that
//! disagrees per client.
//!
//! # It is in beats and seconds, and never in frames
//!
//! A timeline axis counts sample frames and this crate has no sample rate and
//! no tempo map — [`Tempo`](super::Tempo) says what the piece *states*, and
//! turning that into a function of time is `clausters_core::tempomap`'s. So
//! everything here is in the units the document itself is written in, and a
//! caller crosses to its axis with the two calls that crate already exposes.
//! That split is deliberate: the **shape** is the format's and the **time** is
//! the tempo map's, and neither is copied into the other.
//!
//! # A row is a track, showing the lane it plays
//!
//! A track holds several lanes because that is what comping is made of, and
//! which one plays is the track's own choice ([`Track::active`]). So a row is a
//! track showing its active lane; the others are the takes behind it, and
//! showing them is an expansion a view keeps state for.
//!
//! # A name is an id
//!
//! A row is named by its track's id and a box by its region's, so a payload is
//! read with no map on the side. The one exception is the reason this module
//! has a `read` at all: **a box may come back under a name that is no id**,
//! because a split names its halves after the box they came from — and that is
//! exactly how a new box is told from a moved one.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::multitrack::edit::MultitrackIntent;
use crate::multitrack::{Automation, Content, Lane, Multitrack, Region, Track};
use crate::{Beat, NodeId, Opaque, Point, SourceId};

/// One row of the view: a track, and the strip that is drawn beside it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// The track it draws — its identity, and its name on the wire.
    pub track: NodeId,
    /// The lane of that track whose regions it shows: what a box joins when it
    /// lands here.
    pub lane: NodeId,
    /// What the header draws.
    pub label: String,
    /// Silenced.
    pub mute: bool,
    /// Soloed.
    pub solo: bool,
    /// The fader, read out of the track's own table — the document holds no
    /// mixer, so a level is a key a client wrote and this only carries it.
    pub gain: f64,
}

/// One box: a region, where it sits and what it is a window onto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Box {
    /// The region it draws — its identity, and its name on the wire.
    pub region: NodeId,
    /// The row it sits on ([`Row::track`]).
    pub row: NodeId,
    /// Where it starts, in beats.
    pub position: Beat,
    /// How long it occupies, in beats.
    pub length: Beat,
    /// The frame of the source its own zero reads, **in seconds** — a
    /// recording's units, which no tempo scales.
    pub start: f64,
    /// How long the window lasts, in seconds. A region may be placed for longer
    /// than this; past it there is nothing to read.
    pub content: f64,
    /// What it is a window onto, when it is a window onto samples at all. A
    /// window onto notes and a composite both draw as a named box.
    pub source: Option<SourceId>,
    /// What the box draws as its name.
    pub label: String,
    /// Silenced by hand, and the region's own rather than its track's.
    pub muted: bool,
    /// Whether the window **wraps**: past the end of the source it begins
    /// again. What a box longer than what it reads means, and the only one of
    /// the three answers to that question which changes what *sounds* — so it
    /// is the piece's and travels with the box.
    pub looping: bool,
}

/// The key a client's fader is kept under in a track's opaque table.
///
/// The key a track's fader was carried under before it was a field.
///
/// Kept because a piece written by an older build has it in [`Track::config`],
/// and [`level_of`] still reads it when the field is at unity -- what a file
/// said is what a file meant. Nothing writes it any more: the fader is
/// [`Track::level`].
pub const LEVEL: &str = "level";

/// The rows a piece draws as, top to bottom.
pub fn rows(piece: &Multitrack) -> Vec<Row> {
    piece
        .tracks
        .iter()
        .filter_map(|track| {
            let lane = active_lane(track)?;
            Some(Row {
                track: track.id,
                lane: lane.id,
                label: track
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("track {}", track.id.0)),
                mute: track.muted,
                solo: track.soloed,
                gain: level_of(track),
            })
        })
        .collect()
}

/// The boxes a piece draws as, in the order the rows hold them.
pub fn boxes(piece: &Multitrack) -> Vec<Box> {
    let mut out = Vec::new();
    for track in &piece.tracks {
        let Some(lane) = active_lane(track) else {
            continue;
        };
        for region in &lane.regions {
            let (source, start, content, looping) = window_of(region);
            out.push(Box {
                region: region.id,
                row: track.id,
                position: region.position,
                length: region.length,
                start,
                content,
                source,
                label: region
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("region {}", region.id.0)),
                muted: region.muted,
                looping,
            });
        }
    }
    out
}

/// One **curve** of the view: an automation, and where it hangs.
///
/// The same shape for both places a curve lives, because it is the same curve:
/// a track's automation is drawn as a **row of its own** under that track and
/// runs the whole timeline, a region's is drawn as a **layer inside that box**
/// and runs as long as the box does. `owner` says which — a track's id for a
/// row, a region's for a layer — and the two are handed out by two calls
/// ([`curves`] and [`layers`]) rather than one with a flag, since a caller
/// draws them in two different places and never mixes them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    /// The automation it draws — its identity, and its name on the wire.
    pub automation: NodeId,
    /// What it hangs from: a track (a row) or a region (a layer).
    pub owner: NodeId,
    /// What is drawn on it.
    pub label: String,
    /// **What it automates**, in the client's own terms and never read here.
    ///
    /// It travels because the value *domain* is decided from it and the domain
    /// is the client's: a gain runs over one range and a pan over another, and
    /// which is which is a fact about the parameter, not about the curve.
    #[serde(default, skip_serializing_if = "Opaque::is_empty")]
    pub target: Opaque,
    /// The break-points. `at` is on the musical axis, like every placement here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Point>,
    /// Whether the row or layer is shown — the view's own state, kept in the
    /// piece because which curves a person had open is part of reopening it as
    /// they left it.
    pub visible: bool,
    /// Whether the curve is being applied.
    pub enabled: bool,
}

/// The **track automations**: one row of its own under each track that has one.
pub fn curves(piece: &Multitrack) -> Vec<Curve> {
    piece
        .tracks
        .iter()
        .flat_map(|track| track.automation.iter().map(|a| curve(a, track.id)))
        .collect()
}

/// The **region automations**: one layer inside each box that has one.
///
/// Only the boxes that are drawn — the active lane's — because a layer with no
/// box under it has nowhere to be.
pub fn layers(piece: &Multitrack) -> Vec<Curve> {
    piece
        .tracks
        .iter()
        .filter_map(active_lane)
        .flat_map(|lane| &lane.regions)
        .flat_map(|region| region.automation.iter().map(|a| curve(a, region.id)))
        .collect()
}

fn curve(automation: &Automation, owner: NodeId) -> Curve {
    Curve {
        automation: automation.id,
        owner,
        label: automation
            .name
            .clone()
            .unwrap_or_else(|| format!("automation {}", automation.id.0)),
        target: automation.target.clone(),
        points: automation.points.clone(),
        visible: automation.visible,
        enabled: automation.enabled,
    }
}

/// A curve as a hand left it — what [`read_points`] is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Curved {
    /// The name it came back under, which is its automation's id.
    pub name: String,
    /// Its break-points, in order.
    #[serde(default)]
    pub points: Vec<Point>,
}

/// **What a `"points"` report means**, as edits in the piece's own vocabulary.
///
/// The report is every curve there is, rows and layers alike, for the same
/// reason a box report is every box: applying what came back is the identity.
/// So what comes out is the difference — a
/// [`MultitrackIntent::SetAutomation`] per curve whose points actually moved,
/// and nothing at all for a hand that looked without editing.
///
/// A name that is no automation's id is dropped rather than minted: a curve is
/// declared by whoever holds the piece, and a hand that dragged a break-point
/// made no new one.
pub fn read_points(piece: &Multitrack, reported: &[Curved]) -> Vec<MultitrackIntent> {
    reported
        .iter()
        .filter_map(|curve| {
            let id = curve.name.parse::<u64>().ok().map(NodeId)?;
            let held = piece.automation(id)?;
            (!same_points(&held.points, &curve.points)).then(|| MultitrackIntent::SetAutomation {
                automation: id,
                points: curve.points.clone(),
            })
        })
        .collect()
}

/// Whether two runs of break-points say the same thing.
///
/// **A JSON number compares by value and not by spelling**, which derived
/// equality on [`Opaque`] cannot do. A point's `data` is opaque and travels
/// through whichever serializer the endpoint has, and JavaScript writes `0.0`
/// as `0` — so a curve reported back exactly as it was drawn came out as an
/// *edit* in the page and as nothing in a script, which is one report meaning
/// two things. Everything else compares as it always did.
fn same_points(held: &[Point], reported: &[Point]) -> bool {
    held.len() == reported.len()
        && held
            .iter()
            .zip(reported)
            .all(|(a, b)| a.at == b.at && a.value == b.value && same_json(&a.data.0, &b.data.0))
}

/// Two opaque values, compared with numbers read as numbers.
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_json(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, x)| y.get(key).is_some_and(|y| same_json(x, y)))
        }
        _ => a == b,
    }
}

/// A row as a hand left it — what [`read_rows`] is given, and the same shape
/// [`rows`] hands out with the two fields a picture answers for itself dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Strip {
    /// The name it came back under. **An id where the row is one the view was
    /// given, and anything at all where a hand made one**: a track added by a
    /// gesture is named by whoever added it, which is how a new row is told
    /// from a moved one — the same rule a box's name follows.
    pub name: String,
    /// Silenced.
    #[serde(default)]
    pub mute: bool,
    /// Soloed.
    #[serde(default)]
    pub solo: bool,
    /// The fader, in the client's own key of the track's table.
    #[serde(default = "unity")]
    pub gain: f64,
}

fn unity() -> f64 {
    1.0
}

/// **What a `"rows"` report means**, as edits in the piece's own vocabulary.
///
/// The report is the *piece* — every row, in the order they are shown — for the
/// same reason a box report is every box: applying what came back is the
/// identity, and there is no gesture to ask about. What comes out is the
/// difference, and it is **one** [`MultitrackIntent::SetTracks`] whatever
/// changed, because the tracks are one list and a hand that adds, removes,
/// reorders or mutes did one thing to it:
///
/// - a name that is a track's id is **that track**, with the strip's mute, solo
///   and level written onto it;
/// - a name that is no track's id is a **new track**, minted here with one
///   empty lane, since a track that could hold nothing is not one;
/// - a track the report does not name is **gone**, and its lanes and regions
///   with it — which is what makes deleting a track one edit rather than a
///   removal per box on it;
/// - the order is the report's, so the rows are the tracks and moving one moves
///   the other.
///
/// The minting walks up from [`fresh_id`], two at a time: a track and its lane.
/// Unlike [`read`] this asks the piece for that itself — a row report carries
/// no ids a caller had to reserve, so there is nothing for one to say.
///
/// **The label is not read.** A row's label is the track's name where it has
/// one and a made-up `track N` where it has not ([`rows`]), so believing a
/// report would write that made-up string into the document the first time
/// anything else on the row moved. Renaming a track is its own verb, and the
/// wire has no gesture for it yet.
pub fn read_rows(piece: &Multitrack, reported: &[Strip]) -> Vec<MultitrackIntent> {
    let mut next = fresh_id(piece);
    let mut tracks: Vec<Track> = Vec::with_capacity(reported.len());
    for strip in reported {
        let held = strip
            .name
            .parse::<u64>()
            .ok()
            .map(NodeId)
            .and_then(|id| piece.tracks.iter().find(|t| t.id == id));
        let mut track = match held {
            Some(track) => track.clone(),
            None => {
                let made = Track::new(NodeId(next), NodeId(next + 1));
                next += 2;
                made
            }
        };
        track.muted = strip.mute;
        track.soloed = strip.solo;
        // **Only when it moved.** A track that never named a level sits at
        // unity, so writing one unconditionally would make a piece that
        // changed nothing look edited.
        let gain = strip.gain.max(0.0);
        if (gain - level_of(&track)).abs() > f64::EPSILON {
            track.level = gain;
            // What an older piece carried in the table is now the field's, and
            // leaving it would be two answers to one question.
            if let Some(table) = track.config.0.as_object_mut() {
                table.remove(LEVEL);
            }
        }
        tracks.push(track);
    }
    if tracks == piece.tracks {
        return Vec::new();
    }
    vec![MultitrackIntent::SetTracks { tracks }]
}

/// A box as a hand left it — what [`read`] is given, and the same shape
/// [`boxes`] hands out with the two fields a picture cannot answer for dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// The name it came back under. **An id where the box is one the view was
    /// given, and anything at all where a hand made it**: a split names its
    /// halves after the box they came from, which is how a new box is told from
    /// a moved one.
    pub name: String,
    /// The row it is on, by its track's id. A row is never renamed, so this is
    /// always an id.
    pub row: NodeId,
    /// Where it now starts.
    pub position: Beat,
    /// How long it now occupies.
    pub length: Beat,
    /// The frame of the source its zero reads, in seconds.
    pub start: f64,
    /// How much of the source it shows, in seconds — its own length crossed to
    /// the wall clock by whoever holds the tempo map, which is not this crate.
    pub content: f64,
    /// What it is a window onto, where the caller could resolve one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
}

/// **What a `"boxes"` report means**, as edits in the piece's own vocabulary.
///
/// The report is the *piece*, not the gesture — a move, a block drag, a trim, a
/// split, a delete and a paste all arrive as one list — so nothing here asks
/// which gesture ran. What comes out is the difference:
///
/// - a box that stayed on its row and changed length is a
///   [`MultitrackIntent::TrimRegion`], which is the verb that changes what a
///   region reads;
/// - a box that moved, or changed row, is a
///   [`MultitrackIntent::PlaceRegion`] — one verb for both, which is why a
///   crossing is not a second mechanism;
/// - a row that gained a box the piece has no region for, or lost one the
///   report no longer names, is stated **whole**
///   ([`MultitrackIntent::SetLane`]), which is the piece's own verb for a
///   lane's contents and what a split, a join and a paste all invert to.
///
/// `next_id` is where minted region ids start; a caller with nothing better to
/// say passes [`fresh_id`]. Ids are the piece's and a hand that made a box has
/// none to offer.
pub fn read(piece: &Multitrack, placed: &[Placed], next_id: u64) -> Vec<MultitrackIntent> {
    let rows = rows(piece);
    let mut out = Vec::new();
    let mut seen: Vec<NodeId> = Vec::new();
    let mut fresh: Vec<(NodeId, Placed)> = Vec::new();
    for box_ in placed {
        // A box naming a row the piece has none of is **kept where it is**: the
        // view hands back what it could not place so it can be re-homed, and
        // acting on it would be moving a region onto a track that is not there.
        let Some(row) = rows.iter().find(|r| r.track == box_.row) else {
            continue;
        };
        let found = box_
            .name
            .parse::<u64>()
            .ok()
            .map(NodeId)
            .and_then(|id| find_region(piece, id).map(|f| (id, f)));
        let Some((region_id, (track, region))) = found else {
            fresh.push((row.lane, box_.clone()));
            continue;
        };
        seen.push(region_id);
        let crossed = track != box_.row;
        let moved = (box_.position - region.position).0.abs() > f64::EPSILON;
        let resized = (box_.length - region.length).0.abs() > f64::EPSILON;
        // **What the box reads, and from where.** A trim of the *left* edge
        // slides the window over the source -- that is what makes an edge drag
        // a trim and not a squeeze -- so a report whose `start` moved is saying
        // the box reads from somewhere else now, and dropping that left the
        // picture and the document disagreeing about which samples a box is
        // over.
        let (_, start, ..) = window_of(region);
        let rewound = (box_.start - start).abs() > f64::EPSILON;
        if resized || rewound {
            out.push(MultitrackIntent::TrimRegion {
                region: region_id,
                position: box_.position,
                length: box_.length,
                // Only what actually moved: the window's own **duration** is
                // left as the piece states it, since a flat report says how
                // long the box is and not how much of the source is behind it.
                content: rewound.then(|| rewound_content(region, box_)),
            });
        }
        if crossed || (moved && !resized && !rewound) {
            out.push(MultitrackIntent::PlaceRegion {
                region: region_id,
                track: box_.row,
                lane: row.lane,
                position: box_.position,
                layer: region.layer,
            });
        }
    }
    out.extend(lane_lists(piece, &rows, &seen, &fresh, next_id, placed));
    out
}

/// An id past everything the piece already names — its tracks, its lanes, its
/// regions **and its automations**, which share one id space.
///
/// The curves are in the count because they are in the space: a piece looks an
/// id up by number ([`Multitrack::automation`]) and does not ask what kind of
/// thing it expected, so handing out an id a curve already holds is how two
/// things come to answer to one name.
pub fn fresh_id(piece: &Multitrack) -> u64 {
    piece
        .tracks
        .iter()
        .flat_map(|track| {
            std::iter::once(track.id.0)
                .chain(track.automation.iter().map(|a| a.id.0))
                .chain(track.lanes.iter().flat_map(|lane| {
                    std::iter::once(lane.id.0).chain(lane.regions.iter().flat_map(|r| {
                        std::iter::once(r.id.0).chain(r.automation.iter().map(|a| a.id.0))
                    }))
                }))
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// **What each lane now holds**, for the two changes a placement cannot state:
/// a region the report no longer names, and a box the piece has no region for.
fn lane_lists(
    piece: &Multitrack,
    rows: &[Row],
    seen: &[NodeId],
    fresh: &[(NodeId, Placed)],
    mut next: u64,
    placed: &[Placed],
) -> Vec<MultitrackIntent> {
    let mut out = Vec::new();
    for row in rows {
        let Some(lane) = piece
            .tracks
            .iter()
            .find(|t| t.id == row.track)
            .and_then(active_lane)
        else {
            continue;
        };
        // **A box over samples nobody resolved is not invented**: the document
        // would name a source that cannot be opened, and a piece that will not
        // reopen is worse than a box that did not stick. Dropped here rather
        // than while building, so a lane that gained only such boxes is not
        // rewritten to say nothing.
        let added: Vec<&Placed> = fresh
            .iter()
            .filter(|(id, box_)| *id == lane.id && box_.source.is_some())
            .map(|(_, box_)| box_)
            .collect();
        let gone = lane.regions.iter().any(|r| !seen.contains(&r.id));
        if added.is_empty() && !gone {
            continue;
        }
        // **A kept region is kept as the report left it**, not as the piece
        // still holds it. A lane stated whole is stated *last*, so a clone of
        // what the piece says would undo the trim and the move the same report
        // asked for a moment earlier — which is what made a split leave its
        // first half at full length, playing over the second.
        let mut regions: Vec<Region> = lane
            .regions
            .iter()
            .filter(|r| seen.contains(&r.id))
            .map(|region| {
                let mut region = region.clone();
                if let Some(box_) = placed
                    .iter()
                    .find(|b| b.name.parse::<u64>().ok() == Some(region.id.0))
                {
                    region.position = box_.position;
                    region.length = box_.length;
                    if let Content::Window { window, .. } = &mut region.content {
                        window.start = box_.start;
                    }
                }
                region
            })
            .collect();
        for box_ in added {
            let Some(source) = box_.source else { continue };
            regions.push(Region::new(
                NodeId(next),
                box_.position,
                box_.length,
                Content::Window {
                    window: crate::SegmentRef {
                        source: crate::SegmentSource::Samples(crate::SourceRef {
                            source,
                            lifetime: crate::Lifetime::Session,
                            generation: 0,
                            range: None,
                        }),
                        start: box_.start,
                        duration: box_.content,
                    },
                    playrate: 1.0,
                    args: crate::Opaque::none(),
                    // A box a hand made does not loop: looping is what a box
                    // longer than its source means, and a new one is exactly
                    // as long as what it was given.
                    looping: false,
                },
            ));
            next += 1;
        }
        out.push(MultitrackIntent::SetLane {
            lane: lane.id,
            regions,
        });
    }
    out
}

/// The lane a track plays, which is the one a row draws.
fn active_lane(track: &Track) -> Option<&Lane> {
    track.active_lane().or_else(|| track.lanes.first())
}

/// The fader a track is at: its own field, falling back to the key an older
/// piece carried it under (see [`LEVEL`]).
pub fn level_of(track: &Track) -> f64 {
    if track.level != 1.0 {
        return track.level;
    }
    track
        .config
        .0
        .get(LEVEL)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0)
}

/// **The same content, over the part of the source the report names** — the
/// window slid to the reported `start`, looping as the report says, and every
/// other field of it untouched.
///
/// A region whose content is not a window (a composite) is handed back
/// unchanged: there is no window to slide, and inventing one would replace what
/// the box actually holds.
fn rewound_content(region: &Region, box_: &Placed) -> Content {
    let mut content = region.content.clone();
    if let Content::Window { window, .. } = &mut content {
        window.start = box_.start;
    }
    content
}

/// The samples a region is a window onto, where it opens, how much there is,
/// and whether the window wraps.
fn window_of(region: &Region) -> (Option<SourceId>, f64, f64, bool) {
    let Content::Window {
        window, looping, ..
    } = &region.content
    else {
        return (None, 0.0, 0.0, false);
    };
    (
        window.source.samples().map(|s| s.source),
        window.start,
        window.duration,
        *looping,
    )
}

/// The track a region is on, and the region itself.
fn find_region(piece: &Multitrack, region: NodeId) -> Option<(NodeId, &Region)> {
    piece.tracks.iter().find_map(|track| {
        track
            .lanes
            .iter()
            .find_map(|lane| lane.regions.iter().find(|r| r.id == region))
            .map(|found| (track.id, found))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multitrack::{Automation, Track};

    /// A piece of one track with one box on it, a track automation and a box
    /// envelope.
    fn piece() -> Multitrack {
        let mut region = Region::new(
            NodeId(3),
            Beat(0.0),
            Beat(4.0),
            Content::Unknown(serde_json::Value::Null),
        );
        region.automation.push(curve_at(NodeId(5), 0.25));
        let mut track = Track::new(NodeId(1), NodeId(2));
        track.lanes[0].regions.push(region);
        track.automation.push(curve_at(NodeId(4), 0.5));
        let mut piece = Multitrack::default();
        piece.tracks.push(track);
        piece
    }

    fn curve_at(id: NodeId, value: f64) -> Automation {
        let mut a = Automation::new(id, Opaque::none());
        a.name = Some(format!("curve {}", id.0));
        a.points = vec![Point {
            at: 0.0,
            value,
            data: Opaque::none(),
        }];
        a
    }

    /// **The same curve in two places, handed out by two calls** — a track's is
    /// a row of its own, a region's a layer inside its box, and a caller draws
    /// them somewhere different, so it never has to tell them apart.
    #[test]
    fn a_track_curve_is_a_row_and_a_region_curve_is_a_layer() {
        let piece = piece();
        let rows = curves(&piece);
        let layers = layers(&piece);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].automation, NodeId(4));
        assert_eq!(rows[0].owner, NodeId(1), "the track it is under");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].owner, NodeId(3), "the box it is inside");
        assert_eq!(rows[0].label, "curve 4");
    }

    /// A curve with no name is still addressable: the label falls back to
    /// something a header can draw, and the identity stays the id.
    #[test]
    fn a_nameless_curve_is_labelled_by_its_id() {
        let mut piece = piece();
        piece.tracks[0].automation[0].name = None;
        assert_eq!(curves(&piece)[0].label, "automation 4");
    }

    /// **What a report of the points means**: one edit per curve that actually
    /// moved, and nothing at all for a hand that looked without editing.
    #[test]
    fn the_points_report_is_read_as_the_difference() {
        let piece = piece();
        let same: Vec<Curved> = curves(&piece)
            .into_iter()
            .chain(layers(&piece))
            .map(|c| Curved {
                name: c.automation.0.to_string(),
                points: c.points,
            })
            .collect();
        assert!(read_points(&piece, &same).is_empty(), "nothing moved");

        let mut moved = same.clone();
        moved[0].points[0].value = 0.9;
        let intents = read_points(&piece, &moved);
        assert_eq!(intents.len(), 1, "the one that moved");
        assert!(matches!(
            &intents[0],
            MultitrackIntent::SetAutomation { automation, points }
                if *automation == NodeId(4) && points[0].value == 0.9
        ));
    }

    /// A name that is no automation's id is dropped rather than minted: a
    /// curve is declared by whoever holds the piece.
    /// **A trim of the left edge slides the window over the source.** That is
    /// what makes an edge drag a trim and not a squeeze, so a report whose
    /// `start` moved is saying the box reads from somewhere else now — and
    /// dropping it left the picture and the document disagreeing about which
    /// samples a box is over, which is heard as both halves of a split playing
    /// the beginning.
    #[test]
    fn a_report_that_moved_the_window_says_what_the_box_now_reads() {
        let mut piece = piece();
        piece.tracks[0].lanes[0].regions[0].content = Content::window(crate::SegmentRef {
            source: crate::SegmentSource::Samples(crate::SourceRef {
                source: crate::SourceId(1),
                lifetime: crate::Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 8.0,
        });
        let held = boxes(&piece)[0].clone();
        let placed = |start: f64| Placed {
            name: held.region.0.to_string(),
            row: held.row,
            position: held.position,
            length: held.length,
            start,
            content: held.content,
            source: held.source,
        };
        // What is already true is no edit at all.
        let out = read(&piece, &[placed(0.0)], fresh_id(&piece));
        assert!(out.is_empty(), "nothing moved: {out:?}");

        // The window slid: the box reads from two seconds in.
        let out = read(&piece, &[placed(2.0)], fresh_id(&piece));
        let [MultitrackIntent::TrimRegion { content, .. }] = &out[..] else {
            panic!("one trim, carrying what it now reads: {out:?}");
        };
        let Some(Content::Window { window, .. }) = content else {
            panic!("the window, slid");
        };
        assert_eq!(window.start, 2.0);
        assert_eq!(window.duration, 8.0, "and nothing else");
    }

    /// **A lane stated whole is stated as the report left it**, not as the
    /// piece still holds it.
    ///
    /// A lane's whole list is the *last* intent a report produces, so a clone
    /// of what the piece says undoes the trim and the move the same report
    /// asked for a moment earlier. That is what made a split leave its first
    /// half at full length, playing over the second — the picture was right and
    /// the document was not.
    ///
    /// Found by use 2026-09-10.
    #[test]
    fn a_split_leaves_the_first_half_short() {
        let mut piece = piece();
        piece.tracks[0].lanes[0].regions[0].content = Content::window(crate::SegmentRef {
            source: crate::SegmentSource::Samples(crate::SourceRef {
                source: crate::SourceId(1),
                lifetime: crate::Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 8.0,
        });
        let held = boxes(&piece)[0].clone();
        let same = |name: &str, at: f64, len: f64, start: f64| Placed {
            name: name.into(),
            row: held.row,
            position: Beat(at),
            length: Beat(len),
            start,
            content: len,
            source: held.source,
        };
        // The report a split sends: the original shortened, and a tail beside
        // it under a name that is no region's id.
        let out = read(
            &piece,
            &[same("3", 0.0, 2.0, 0.0), same("3 2", 2.0, 2.0, 2.0)],
            fresh_id(&piece),
        );
        for intent in &out {
            crate::multitrack::edit::apply(
                &mut piece,
                intent,
                &Default::default(),
                &Default::default(),
            );
        }
        let lane = &piece.tracks[0].lanes[0];
        assert_eq!(
            lane.regions.len(),
            2,
            "the half that stayed and the new one"
        );
        let first = lane
            .regions
            .iter()
            .find(|r| r.id == NodeId(3))
            .expect("the original");
        assert_eq!(
            first.length,
            Beat(2.0),
            "shortened, and it stayed shortened"
        );
        let tail = lane
            .regions
            .iter()
            .find(|r| r.id != NodeId(3))
            .expect("the tail");
        assert_eq!(tail.position, Beat(2.0));
        assert_eq!(tail.content.as_window().map(|w| w.start), Some(2.0));
    }

    /// **A curve holds an id like anything else does.** `fresh_id` answers
    /// with an id past everything the piece names, and a piece looks one up by
    /// number without asking what kind of thing it expected — so a count that
    /// skipped the automations would hand out an id a curve already had.
    ///
    /// Found 2026-09-09 while adding a track from the header: the new track was
    /// minted onto the track curve's id.
    #[test]
    fn a_fresh_id_is_past_the_curves_too() {
        let piece = piece();
        // Track 1, lane 2, region 3, the track curve 4, the box envelope 5.
        assert_eq!(fresh_id(&piece), 6);
    }

    /// **A rows report is the tracks, whole.** A name that is an id is that
    /// track; one that is not is a track a hand made; a track the report does
    /// not name is gone, and its boxes with it. All of it is one `SetTracks`,
    /// because the tracks are one list and a hand did one thing to it.
    #[test]
    fn the_rows_report_states_the_tracks_and_a_new_name_makes_one() {
        let piece = piece();
        let held = Strip {
            name: "1".into(),
            mute: false,
            solo: false,
            gain: 1.0,
        };
        // What is already true is no edit at all.
        assert!(read_rows(&piece, std::slice::from_ref(&held)).is_empty());

        // The mixer: one verb over a track, and the level lands in the config
        // table the client reads it out of.
        let muted = Strip {
            mute: true,
            gain: 0.5,
            ..held.clone()
        };
        let out = read_rows(&piece, &[muted]);
        let [MultitrackIntent::SetTracks { tracks }] = &out[..] else {
            panic!("one whole statement, whatever changed: {out:?}");
        };
        assert!(tracks[0].muted);
        assert_eq!(level_of(&tracks[0]), 0.5);

        // A name that is no track's id is a track a hand added — with a lane,
        // since a track that could hold nothing is not one — and the ids come
        // from the piece's own counter.
        let out = read_rows(
            &piece,
            &[
                held.clone(),
                Strip {
                    name: "new".into(),
                    ..held.clone()
                },
            ],
        );
        let [MultitrackIntent::SetTracks { tracks }] = &out[..] else {
            panic!("one statement: {out:?}");
        };
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[1].id, NodeId(6), "past everything the piece names");
        assert_eq!(tracks[1].lanes.len(), 1, "and it can hold a box");

        // A track the report leaves out is gone, and it takes its boxes.
        let out = read_rows(&piece, &[]);
        let [MultitrackIntent::SetTracks { tracks }] = &out[..] else {
            panic!("one statement: {out:?}");
        };
        assert!(tracks.is_empty());
    }

    /// **The label is not read.** A row's label is a made-up `track N` where
    /// the track has no name of its own, so believing the report would write
    /// that string into the document the first time anything else moved.
    #[test]
    fn a_rows_label_is_drawn_and_never_written_back() {
        let piece = piece();
        let out = read_rows(
            &piece,
            &[Strip {
                name: "1".into(),
                mute: false,
                solo: false,
                gain: 1.0,
            }],
        );
        assert!(out.is_empty(), "the picture's own label is not a change");
        assert_eq!(rows(&piece)[0].label, "track 1", "which is what it draws");
        assert!(piece.tracks[0].name.is_none(), "and not what it holds");
    }

    #[test]
    fn a_curve_the_piece_never_declared_is_not_made_by_dragging_it() {
        let piece = piece();
        let stray = vec![Curved {
            name: "hello".into(),
            points: vec![Point {
                at: 0.0,
                value: 1.0,
                data: Opaque::none(),
            }],
        }];
        assert!(read_points(&piece, &stray).is_empty());
    }

    /// **A number compares by value, not by spelling.** A page writes `0.0` as
    /// `0` and a script writes it as `0.0`; a curve reported back exactly as it
    /// was drawn is no edit in either.
    #[test]
    fn a_point_s_data_compares_by_what_it_says() {
        let mut piece = Multitrack::default();
        let mut track = Track::new(NodeId(1), NodeId(2));
        let mut curve = Automation::new(NodeId(4), Opaque::none());
        curve.points = vec![Point {
            at: 0.0,
            value: 0.0,
            data: Opaque(serde_json::json!({ "shape": 1, "curve": 0.0 })),
        }];
        track.automation.push(curve);
        piece.tracks.push(track);

        let spelled = vec![Curved {
            name: "4".into(),
            points: vec![Point {
                at: 0.0,
                value: 0.0,
                data: Opaque(serde_json::json!({ "shape": 1.0, "curve": 0 })),
            }],
        }];
        assert!(read_points(&piece, &spelled).is_empty());

        let moved = vec![Curved {
            name: "4".into(),
            points: vec![Point {
                at: 0.0,
                value: 0.0,
                data: Opaque(serde_json::json!({ "shape": 1, "curve": 0.5 })),
            }],
        }];
        assert_eq!(read_points(&piece, &moved).len(), 1);
    }
}
