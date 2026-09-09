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
}

/// The key a client's fader is kept under in a track's opaque table.
///
/// The document holds no mixer: a track has `muted` and `soloed` because those
/// are facts about the piece, and a level is the client's own idea carried in
/// `config`. Naming the key here is what keeps every client's fader the same
/// fader.
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
            let (source, start, content) = window_of(region);
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
            (held.points != curve.points).then(|| MultitrackIntent::SetAutomation {
                automation: id,
                points: curve.points.clone(),
            })
        })
        .collect()
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
        if resized {
            out.push(MultitrackIntent::TrimRegion {
                region: region_id,
                position: box_.position,
                length: box_.length,
                // What it reads is unchanged here: a trim of the left edge
                // moves the window too, and a flat report says where the box is
                // rather than what it now reads.
                content: None,
            });
        }
        if crossed || (moved && !resized) {
            out.push(MultitrackIntent::PlaceRegion {
                region: region_id,
                track: box_.row,
                lane: row.lane,
                position: box_.position,
                layer: region.layer,
            });
        }
    }
    out.extend(lane_lists(piece, &rows, &seen, &fresh, next_id));
    out
}

/// An id past everything the piece already names — its tracks, its lanes and
/// its regions, which share one id space.
pub fn fresh_id(piece: &Multitrack) -> u64 {
    piece
        .tracks
        .iter()
        .flat_map(|track| {
            std::iter::once(track.id.0).chain(track.lanes.iter().flat_map(|lane| {
                std::iter::once(lane.id.0).chain(lane.regions.iter().map(|r| r.id.0))
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
        let mut regions: Vec<Region> = lane
            .regions
            .iter()
            .filter(|r| seen.contains(&r.id))
            .cloned()
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

/// The fader out of a track's own table; a track with none is at unity.
fn level_of(track: &Track) -> f64 {
    track
        .config
        .0
        .get(LEVEL)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0)
}

/// The samples a region is a window onto, where it opens and how much there is.
fn window_of(region: &Region) -> (Option<SourceId>, f64, f64) {
    let Content::Window { window, .. } = &region.content else {
        return (None, 0.0, 0.0);
    };
    (
        window.source.samples().map(|s| s.source),
        window.start,
        window.duration,
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
}
