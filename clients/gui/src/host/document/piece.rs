//! Drawing **the piece** as a multitrack, and reading a hand's answer back.
//!
//! The twin of [`super::tree`] over the other description. A session written
//! today carries [`Multitrack`] — tracks, lanes, regions, and the timeline they
//! sit on — and leaves the general tree empty, so a host that read only the
//! tree opened a real session as an empty window. This is the leg the crate's
//! plan calls the destination and the tree the thing being walked off.
//!
//! # One lane per track, and it shows the track's active lane
//!
//! A track holds several lanes because that is what comping is made of, and
//! which one plays is the track's own choice ([`Track::active`]). So a row on
//! screen is a **track**, showing the lane it plays; the others are the takes
//! behind it, and showing them is an expansion the view has no state for yet.
//! A region dragged onto another row therefore names that row's track *and*
//! its active lane, which is exactly what [`MultitrackIntent::PlaceRegion`]
//! asks for.
//!
//! # A name on the wire is an id
//!
//! The same rule the tree's drawing follows: a lane is named by its track's id
//! and a clip by its region's, so an edit-back is read with no map on the side.
//!
//! # What the picture is measured in
//!
//! The piece measures **beats** and the shared time axis measures timeline
//! samples, so every number crossing this seam is multiplied or divided by one
//! ratio, once — and a region's window into its samples is in **seconds**,
//! which meets the axis through the rate and never through the tempo.

use clausters_document::multitrack::edit::MultitrackIntent;
use clausters_document::multitrack::{Content, Lane, Multitrack, Region, Track};
use clausters_document::{Beat, NodeId};
use serde_json::{Value, json};

use super::sources::Takes;
use super::tree::{ClipRow, LaneRow, Piece, node_named};

/// The lane height a row is drawn at, in logical pixels.
const LANE_H: f64 = 96.0;

/// The three keys a strip reads out of a track's `config` and writes back into
/// it. `mute` and `solo` are fields of [`Track`] itself; only the fader is
/// carried in the client's own table, because the document holds no mixer.
const LEVEL: &str = "level";

/// How the picture is scaled.
#[derive(Debug, Clone, Copy)]
pub struct Look<'a> {
    /// Samples per beat — what a region's position and length are drawn with.
    pub units_per_beat: f64,
    /// Samples per second — what a **window** into samples is measured in, so
    /// no tempo scales a recording.
    pub units_per_second: f64,
    /// The session's samples, once somebody resolved them to server buffers.
    pub takes: Option<&'a Takes>,
}

impl Default for Look<'_> {
    fn default() -> Self {
        Self {
            units_per_beat: 48_000.0,
            units_per_second: 48_000.0,
            takes: None,
        }
    }
}

/// The piece as the `multitrack` widget takes it, and as an edit-back is
/// resolved against.
pub fn shown(piece: &Multitrack, look: &Look<'_>) -> Piece {
    let mut lanes = Vec::with_capacity(piece.tracks.len());
    let mut clips = Vec::new();
    let mut lanes_prop = Vec::with_capacity(piece.tracks.len() * 6);
    let mut clips_prop = Vec::new();
    for track in &piece.tracks {
        let Some(lane) = active_lane(track) else {
            continue;
        };
        lanes.push(LaneRow {
            node: track.id,
            // A row's clips are the lane's, so what a region is added to and
            // removed from is that lane rather than the track.
            holder: lane.id,
            // A track has no offset: the piece's timeline is one, and a region
            // states where it is on it.
            base: 0.0,
        });
        lanes_prop.extend([
            json!(track.id.0.to_string()),
            json!(
                track
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("track {}", track.id.0))
            ),
            json!(LANE_H),
            json!(track.muted),
            json!(track.soloed),
            json!(level_of(track)),
        ]);
        for region in &lane.regions {
            clips.push(ClipRow {
                node: region.id,
                lane: track.id,
            });
            clips_prop.extend([
                json!(region.id.0.to_string()),
                json!(track.id.0.to_string()),
                json!(region.position.0 * look.units_per_beat),
                json!(region.length.0 * look.units_per_beat),
                json!(start_of(region) * look.units_per_second),
                json!(label_of(region)),
                json!(buffer_of(region, look)),
            ]);
        }
    }
    Piece {
        lanes,
        clips,
        lanes_prop: Value::Array(lanes_prop),
        clips_prop: Value::Array(clips_prop),
    }
}

/// The lane a track plays, which is the one a row draws.
fn active_lane(track: &Track) -> Option<&Lane> {
    track.active_lane().or_else(|| track.lanes.first())
}

/// The fader, out of the track's own table. A track with none is at unity: the
/// widget's prop is a number and not an absence.
fn level_of(track: &Track) -> f64 {
    track
        .config
        .0
        .get(LEVEL)
        .and_then(Value::as_f64)
        .unwrap_or(1.0)
}

/// What a box is called on screen.
fn label_of(region: &Region) -> String {
    region
        .name
        .clone()
        .unwrap_or_else(|| format!("region {}", region.id.0))
}

/// The source frame the box's own zero reads, in **seconds** — a window states
/// it, and anything else starts at the beginning of what it holds.
fn start_of(region: &Region) -> f64 {
    match &region.content {
        Content::Window { window, .. } => window.start,
        _ => 0.0,
    }
}

/// The **server buffer** a box draws, or `-1` for a box over nothing: a window
/// onto notes, a composite, or samples nobody has read in yet.
fn buffer_of(region: &Region, look: &Look<'_>) -> i32 {
    let Content::Window { window, .. } = &region.content else {
        return -1;
    };
    window
        .source
        .samples()
        .and_then(|source| look.takes?.get(source.source))
        .map_or(-1, |take| take.bufnum)
}

/// **The piece's clips, as they now stand** — the one payload every placement
/// gesture leaves, read against the piece.
///
/// A region that stayed where it was is nothing; one that moved, changed row or
/// was trimmed is one verb each, and the vocabulary already tells the two
/// apart: [`MultitrackIntent::PlaceRegion`] never changes what a region reads,
/// so a move cannot silently retime the material, and
/// [`MultitrackIntent::TrimRegion`] is the one that does.
///
/// A region the payload **does not name** was deleted, and that is a
/// [`MultitrackIntent::SetLane`] over what its lane now holds — the lane's own
/// whole-list verb, which is also what a paste inverts to.
pub fn read_clips(
    piece: &Multitrack,
    args: &[clausters_core::osc::OscType],
    look: &Look<'_>,
) -> Vec<(MultitrackIntent, &'static str)> {
    let per_beat = look.units_per_beat.max(f64::MIN_POSITIVE);
    let shown = shown(piece, look);
    let mut out = Vec::new();
    let mut seen: Vec<NodeId> = Vec::new();
    for clip in args.as_chunks::<7>().0 {
        let (Some(name), Some(lane)) = (string_at(clip, 0), string_at(clip, 1)) else {
            continue;
        };
        let (Some(region_id), Some(track_id)) = (node_named(name), node_named(lane)) else {
            continue;
        };
        // A box naming a row the piece has none of is **kept where it is**: the
        // widget hands back what it could not place so it can be re-homed, and
        // acting on it would be moving a region into a track that is not there.
        let Some(row) = shown.lane(track_id) else {
            continue;
        };
        let Some((track, region)) = find_region(piece, region_id) else {
            continue;
        };
        seen.push(region_id);
        let position = Beat(float_at(clip, 2) / per_beat);
        let length = Beat(float_at(clip, 3) / per_beat);
        let crossed = track != track_id;
        let moved = (position - region.position).0.abs() > f64::EPSILON;
        let resized = (length - region.length).0.abs() > f64::EPSILON;
        // **A trim is the verb that changes what shows**, and it states the
        // position too, so a left-hand trim is one edit rather than a move and
        // a resize racing each other.
        if resized {
            out.push((
                MultitrackIntent::TrimRegion {
                    region: region_id,
                    position,
                    length,
                    // What it reads is unchanged here: the widget reports a
                    // `start` and the crate takes a whole `Content`, so a left
                    // trim moves the box and not yet its window
                    // (`clients/gui/PLAN.md`, "Found by use").
                    content: None,
                },
                "trim a clip",
            ));
        }
        if crossed || (moved && !resized) {
            out.push((
                MultitrackIntent::PlaceRegion {
                    region: region_id,
                    track: track_id,
                    lane: row.holder,
                    position,
                    layer: region.layer,
                },
                if crossed {
                    "move a clip to another track"
                } else {
                    "move a clip"
                },
            ));
        }
    }
    out.extend(removals(piece, &shown, &seen));
    out
}

/// The regions the payload left out, as what each of their lanes now holds.
fn removals(
    piece: &Multitrack,
    shown: &Piece,
    seen: &[NodeId],
) -> Vec<(MultitrackIntent, &'static str)> {
    let mut out = Vec::new();
    for row in &shown.lanes {
        let Some(lane) = piece
            .tracks
            .iter()
            .find(|t| t.id == row.node)
            .and_then(active_lane)
        else {
            continue;
        };
        if lane.regions.iter().all(|r| seen.contains(&r.id)) {
            continue;
        }
        let regions: Vec<Region> = lane
            .regions
            .iter()
            .filter(|r| seen.contains(&r.id))
            .cloned()
            .collect();
        out.push((
            MultitrackIntent::SetLane {
                lane: lane.id,
                regions,
            },
            "remove a clip",
        ));
    }
    out
}

/// **The piece's strips, as they now stand** — the mixer's payload.
///
/// Mute and solo are the track's own fields and the fader is a key in its
/// table, so all three travel in one [`MultitrackIntent::SetTracks`]: the
/// piece's only verb over a track is the whole list, which is what makes
/// adding, removing and reordering one verb and costs the inverse a copy of the
/// tracks.
///
/// A strip saying what the track already says is not an edit, which is what
/// keeps one fader drag from rewriting every track.
pub fn read_lanes(
    piece: &Multitrack,
    args: &[clausters_core::osc::OscType],
) -> Vec<(MultitrackIntent, &'static str)> {
    let mut tracks = piece.tracks.clone();
    let mut changed = false;
    for lane in args.as_chunks::<6>().0 {
        let Some(name) = string_at(lane, 0) else {
            continue;
        };
        let Some(id) = node_named(name) else { continue };
        let Some(track) = tracks.iter_mut().find(|t| t.id == id) else {
            continue;
        };
        let (muted, soloed) = (truthy_at(lane, 3), truthy_at(lane, 4));
        let level = f64::from(float_at(lane, 5) as f32);
        if track.muted == muted && track.soloed == soloed && level_of(track) == level {
            continue;
        }
        track.muted = muted;
        track.soloed = soloed;
        // The fader is the client's key in an opaque table, so it is written
        // over what is there rather than replacing it: a track's config is its
        // instrument and its routing too.
        let mut config = track.config.0.as_object().cloned().unwrap_or_default();
        config.insert(LEVEL.into(), json!(level));
        track.config = clausters_document::Opaque(Value::Object(config));
        changed = true;
    }
    if !changed {
        return Vec::new();
    }
    vec![(MultitrackIntent::SetTracks { tracks }, "mix a track")]
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

fn string_at(args: &[clausters_core::osc::OscType], n: usize) -> Option<&str> {
    match args.get(n) {
        Some(clausters_core::osc::OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn float_at(args: &[clausters_core::osc::OscType], n: usize) -> f64 {
    match args.get(n) {
        Some(clausters_core::osc::OscType::Float(v)) => f64::from(*v),
        Some(clausters_core::osc::OscType::Double(v)) => *v,
        Some(clausters_core::osc::OscType::Int(v)) => f64::from(*v),
        Some(clausters_core::osc::OscType::Long(v)) => *v as f64,
        _ => 0.0,
    }
}

fn truthy_at(args: &[clausters_core::osc::OscType], n: usize) -> bool {
    super::truthy_at(args, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::osc::OscType;
    use clausters_document::multitrack::{Content, Track};
    use clausters_document::{Against, Opaque, Rules, SegmentRef, SegmentSource, SourceId};
    use clausters_document::{Lifetime, SourceRef};

    fn window(source: u64, start: f64) -> Content {
        Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start,
                duration: 2.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
        }
    }

    fn region(id: u64, position: f64, length: f64) -> Region {
        let mut region = Region::new(NodeId(id), Beat(position), Beat(length), window(1, 0.0));
        region.name = Some(format!("r{id}"));
        region
    }

    /// Two tracks, one lane each: the first holds two regions, the second one.
    fn piece() -> Multitrack {
        let track = |id: u64, lane: u64, regions: Vec<Region>| {
            let mut track = Track::new(NodeId(id), NodeId(lane));
            track.lanes[0].regions = regions;
            track.name = Some(format!("t{id}"));
            track
        };
        Multitrack {
            tracks: vec![
                track(10, 11, vec![region(12, 0.0, 2.0), region(13, 4.0, 2.0)]),
                track(20, 21, vec![region(22, 0.0, 2.0)]),
            ],
            ..Multitrack::default()
        }
    }

    fn look() -> Look<'static> {
        Look {
            units_per_beat: 100.0,
            units_per_second: 48_000.0,
            takes: None,
        }
    }

    /// The `"clips"` payload the widget leaves, from `(name, lane, at, dur)`.
    fn clips(entries: &[(&str, &str, f32, f32)]) -> Vec<OscType> {
        let mut args = Vec::new();
        for (name, lane, at, dur) in entries {
            args.extend([
                OscType::String((*name).into()),
                OscType::String((*lane).into()),
                OscType::Float(*at),
                OscType::Float(*dur),
                OscType::Float(0.0),
                OscType::String(String::new()),
                OscType::Int(-1),
            ]);
        }
        args
    }

    /// **A row is a track, showing the lane it plays**, and every name on the
    /// wire is an id — which is what lets an edit-back be read with no map.
    #[test]
    fn a_piece_draws_a_row_per_track_naming_ids() {
        let shown = shown(&piece(), &look());
        assert_eq!(
            shown.lanes.iter().map(|l| l.node).collect::<Vec<_>>(),
            vec![NodeId(10), NodeId(20)]
        );
        assert_eq!(
            shown.lanes.iter().map(|l| l.holder).collect::<Vec<_>>(),
            vec![NodeId(11), NodeId(21)],
            "a row's clips are its active lane's, which is what a region joins"
        );
        let lanes = shown.lanes_prop.as_array().expect("flat");
        assert_eq!(lanes[0], "10", "named by the track's id");
        assert_eq!(lanes[1], "t10", "and labelled by its name");
        let clips = shown.clips_prop.as_array().expect("flat");
        assert_eq!(clips[0], "12");
        assert_eq!(clips[1], "10", "on the row of the track that holds it");
        assert_eq!(clips[2], 0.0, "beats, in timeline units");
        assert_eq!(clips[3], 200.0, "two beats at a hundred units each");
    }

    /// **A move is a `PlaceRegion` and never changes what the region reads**;
    /// a resize is the trim, which is the other verb. The payload says neither
    /// — it says where the boxes are — so this is where the two are told apart.
    #[test]
    fn a_move_a_cross_and_a_trim_are_each_their_own_verb() {
        let piece = piece();
        // Nothing moved: an idempotent report of what already holds.
        let same = read_clips(
            &piece,
            &clips(&[
                ("12", "10", 0.0, 200.0),
                ("13", "10", 400.0, 200.0),
                ("22", "20", 0.0, 200.0),
            ]),
            &look(),
        );
        assert!(
            same.is_empty(),
            "a report of what holds is not an edit: {same:?}"
        );

        // One box dragged along its own row. **The payload always names every
        // box** -- it is the piece as it now stands -- so these list all three.
        let moved = read_clips(
            &piece,
            &clips(&[
                ("12", "10", 300.0, 200.0),
                ("13", "10", 400.0, 200.0),
                ("22", "20", 0.0, 200.0),
            ]),
            &look(),
        );
        assert!(
            matches!(
                moved.first().map(|(i, _)| i),
                Some(MultitrackIntent::PlaceRegion { region, track, lane, position, .. })
                    if *region == NodeId(12) && *track == NodeId(10) && *lane == NodeId(11)
                        && *position == Beat(3.0)
            ),
            "{moved:?}"
        );

        // ...and dragged onto the other row: the same verb with a different
        // track, which is why a cross is not a second mechanism.
        let crossed = read_clips(
            &piece,
            &clips(&[
                ("12", "20", 100.0, 200.0),
                ("13", "10", 400.0, 200.0),
                ("22", "20", 0.0, 200.0),
            ]),
            &look(),
        );
        assert!(
            matches!(
                crossed.first().map(|(i, _)| i),
                Some(MultitrackIntent::PlaceRegion { track, lane, .. })
                    if *track == NodeId(20) && *lane == NodeId(21)
            ),
            "{crossed:?}"
        );

        // A width that changed is the trim, and it states the position too --
        // so a left-hand trim is one edit and not a move racing a resize.
        let trimmed = read_clips(
            &piece,
            &clips(&[
                ("12", "10", 100.0, 100.0),
                ("13", "10", 400.0, 200.0),
                ("22", "20", 0.0, 200.0),
            ]),
            &look(),
        );
        assert!(
            matches!(
                trimmed.first().map(|(i, _)| i),
                Some(MultitrackIntent::TrimRegion { region, position, length, .. })
                    if *region == NodeId(12) && *position == Beat(1.0) && *length == Beat(1.0)
            ),
            "{trimmed:?}"
        );
        assert_eq!(trimmed.len(), 1, "and not a move beside it: {trimmed:?}");
    }

    /// **A box the payload leaves out was deleted**, and that is the lane's own
    /// whole-list verb — the piece has no "remove one region", for the same
    /// reason it has no "add one".
    #[test]
    fn a_clip_the_payload_does_not_name_is_removed_from_its_lane() {
        let piece = piece();
        let edits = read_clips(
            &piece,
            &clips(&[("12", "10", 0.0, 200.0), ("22", "20", 0.0, 200.0)]),
            &look(),
        );
        let [(MultitrackIntent::SetLane { lane, regions }, _)] = edits.as_slice() else {
            panic!("one lane rewritten: {edits:?}");
        };
        assert_eq!(*lane, NodeId(11));
        assert_eq!(
            regions.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![NodeId(12)],
            "what the lane now holds, and not what left it"
        );
    }

    /// The strip: mute, solo and the fader travel in the piece's one verb over
    /// a track, and a strip saying what the track already says is not an edit.
    #[test]
    fn only_the_strip_that_moved_rewrites_the_tracks() {
        let piece = piece();
        let lanes = |entries: &[(&str, bool, f32)]| {
            let mut args = Vec::new();
            for (name, muted, level) in entries {
                args.extend([
                    OscType::String((*name).into()),
                    OscType::String(String::new()),
                    OscType::Float(96.0),
                    OscType::Int(i32::from(*muted)),
                    OscType::Int(0),
                    OscType::Float(*level),
                ]);
            }
            args
        };
        assert!(
            read_lanes(&piece, &lanes(&[("10", false, 1.0), ("20", false, 1.0)])).is_empty(),
            "the strips as they were drawn are not an edit"
        );
        let edits = read_lanes(&piece, &lanes(&[("10", false, 1.0), ("20", true, 0.5)]));
        let [(MultitrackIntent::SetTracks { tracks }, _)] = edits.as_slice() else {
            panic!("the tracks, whole: {edits:?}");
        };
        assert!(!tracks[0].muted, "the one nobody touched is untouched");
        assert!(tracks[1].muted);
        assert_eq!(level_of(&tracks[1]), 0.5, "and the fader is in its table");
    }

    /// End to end through the crate's own door: the edits this reads apply, and
    /// the piece says what the hand said.
    #[test]
    fn the_edits_it_reads_apply_to_the_piece() {
        let mut piece = piece();
        let edits = read_clips(
            &piece.clone(),
            &clips(&[
                ("12", "20", 100.0, 200.0),
                ("13", "10", 400.0, 200.0),
                ("22", "20", 0.0, 200.0),
            ]),
            &look(),
        );
        for (intent, _) in edits {
            let outcome = clausters_document::multitrack::edit::apply(
                &mut piece,
                &intent,
                &Against::default(),
                &Rules::none(),
            );
            assert!(outcome.applied, "{intent:?}");
        }
        assert!(
            piece.tracks[1]
                .lanes
                .iter()
                .any(|l| l.regions.iter().any(|r| r.id == NodeId(12))),
            "the region is on the second track now"
        );
        assert!(
            piece.tracks[0]
                .lanes
                .iter()
                .all(|l| l.regions.iter().all(|r| r.id != NodeId(12))),
            "and not on the first one as well"
        );
    }
}
