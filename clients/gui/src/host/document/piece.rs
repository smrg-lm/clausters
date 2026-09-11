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
//! which one plays is the track's own choice ([`clausters_document::multitrack::Track::active`]). So a row on
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
//! samples, and what crosses between them is the piece's own **tempo map**
//! rather than a ratio: a position is `secs_at(beat) × rate`, and a *length* is
//! the difference of two of those, because how long four beats last depends on
//! where they start. A single ratio is right only for a piece that never
//! changes tempo, and getting it wrong is silent — the boxes are drawn and the
//! readers placed in the same wrong place, so the picture and the sound agree
//! about it.
//!
//! A region's window into its samples is in **seconds**, which meets the axis
//! through the rate and never through the tempo: a recording's length is a
//! wall-clock fact.

use clausters_core::tempomap::{TempoChange, TempoMap};
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::edit::MultitrackIntent;
use clausters_document::multitrack::picture;
use clausters_document::{Beat, NodeId, SourceId};
use serde_json::{Value, json};

use super::sources::Takes;
use super::tree::{ClipRow, LaneRow, Piece};

/// The lane height a row is drawn at, in logical pixels.
const LANE_H: f64 = 96.0;

/// The tempo a piece that never said one is read at, in beats per second —
/// one, so a beat is a second and a piece with no tempo behaves exactly as it
/// did before there was a map.
///
/// It is the **reader's** default and not the document's: a piece that said no
/// tempo did not say one, and writing 120 into the format would be the crate
/// deciding a musical question ([`clausters_document::multitrack::Multitrack::tempo`]).
pub const DEFAULT_TEMPO: f64 = 1.0;

/// How the picture is scaled.
#[derive(Debug, Clone)]
pub struct Look<'a> {
    /// Beats to seconds, as the piece itself states it — steps, ramps and all.
    ///
    /// Owned rather than borrowed, and rebuilt from the piece each time it is
    /// asked for: it is a handful of segments, and a copy kept beside the piece
    /// is a copy to keep in step with every edit that moves a tempo.
    pub tempo: TempoMap,
    /// Frames a second — where the musical axis and the wall clock both meet
    /// the timeline.
    pub rate: f64,
    /// The session's samples, once somebody resolved them to server buffers.
    pub takes: Option<&'a Takes>,
}

impl Default for Look<'_> {
    fn default() -> Self {
        Self {
            tempo: TempoMap::new(DEFAULT_TEMPO),
            rate: 48_000.0,
            takes: None,
        }
    }
}

impl Look<'_> {
    /// Where a beat falls on the timeline, in frames.
    pub fn frame_at(&self, beats: f64) -> f64 {
        self.tempo.secs_at(beats) * self.rate
    }

    /// How long a stretch of beats lasts there — **the difference of two
    /// positions**, because four beats are not one length: under a ritardando
    /// they are longer later than earlier.
    pub fn frames_over(&self, from: f64, len: f64) -> f64 {
        self.frame_at(from + len) - self.frame_at(from)
    }

    /// The beat a frame falls on: the inverse, and the way an edit comes back.
    pub fn beat_at(&self, frame: f64) -> f64 {
        self.tempo
            .beats_at(frame / self.rate.max(f64::MIN_POSITIVE))
    }
}

/// The map a piece states, with the reader's default where it states nothing.
///
/// One line, and it is a **binding** rather than a rule: the three decisions a
/// run of authored entries needs — a ramp reaching the next one, the default
/// before the first, an empty list being the default alone — are
/// [`TempoMap::from_changes`]'s, in the crate that models tempo.
pub fn tempo_map(piece: &Multitrack) -> TempoMap {
    let changes: Vec<TempoChange> = piece
        .tempo
        .iter()
        .map(|t| TempoChange {
            beats: t.at.0,
            // The document writes beats per **minute**, as a score does; every
            // tempo in the map is per second.
            tempo: t.bpm / 60.0,
            ramp: t.ramp,
        })
        .collect();
    TempoMap::from_changes(&changes, DEFAULT_TEMPO).unwrap_or_else(|_| TempoMap::new(DEFAULT_TEMPO))
}

/// The piece as the `multitrack` widget takes it, and as an edit-back is
/// resolved against.
///
/// **The shape is the crate's and the time is this host's**
/// ([`clausters_document::multitrack::picture`]): what a row and a box *are* is
/// the format's business and is written once for every client; turning beats
/// into frames on the shared axis is the tempo map's, which is what this adds.
pub fn shown(piece: &Multitrack, look: &Look<'_>) -> Piece {
    let rows = picture::rows(piece);
    let boxes = picture::boxes(piece);
    let mut lanes = Vec::with_capacity(rows.len());
    let mut lanes_prop = Vec::with_capacity(rows.len() * 6);
    for row in &rows {
        lanes.push(LaneRow {
            node: row.track,
            holder: row.lane,
            // A track has no offset: the piece's timeline is one, and a region
            // states where it is on it.
            base: 0.0,
        });
        lanes_prop.extend([
            json!(row.track.0.to_string()),
            json!(row.label.clone()),
            json!(LANE_H),
            json!(row.mute),
            json!(row.solo),
            json!(row.gain),
        ]);
    }
    let mut clips = Vec::with_capacity(boxes.len());
    let mut clips_prop = Vec::with_capacity(boxes.len() * 7);
    for box_ in &boxes {
        clips.push(ClipRow {
            node: box_.region,
            lane: box_.row,
        });
        clips_prop.extend([
            json!(box_.region.0.to_string()),
            json!(box_.row.0.to_string()),
            json!(look.frame_at(box_.position.0)),
            json!(look.frames_over(box_.position.0, box_.length.0)),
            json!(box_.start * look.rate),
            json!(box_.label.clone()),
            json!(bufnum_of(box_.source, look)),
        ]);
    }
    Piece {
        lanes,
        clips,
        lanes_prop: Value::Array(lanes_prop),
        clips_prop: Value::Array(clips_prop),
    }
}

/// The **server buffer** a source was read into, or `-1` for a box over
/// nothing: a window onto notes, a composite, or samples nobody read in yet.
fn bufnum_of(source: Option<SourceId>, look: &Look<'_>) -> i32 {
    source
        .and_then(|source| look.takes?.get(source))
        .map_or(-1, |take| take.bufnum)
}

/// **The piece's clips, as they now stand** — the one payload every placement
/// gesture leaves.
///
/// The flat wire form crossed into the crate's own
/// [`Placed`](picture::Placed), which is where beats meet frames and a buffer
/// number meets a source. What the list *means* is
/// [`picture::read`]'s and is written once.
pub fn read_clips(
    piece: &Multitrack,
    args: &[clausters_core::osc::OscType],
    look: &Look<'_>,
) -> Vec<(MultitrackIntent, &'static str)> {
    let rows = picture::rows(piece);
    let mut placed = Vec::new();
    for clip in args.as_chunks::<7>().0 {
        let (Some(name), Some(lane)) = (string_at(clip, 0), string_at(clip, 1)) else {
            continue;
        };
        // The row's name **is** an id, always: the drawing writes it and a hand
        // never renames a row.
        let Some(row) = lane.parse::<u64>().ok().map(NodeId) else {
            continue;
        };
        if !rows.iter().any(|r| r.track == row) {
            continue;
        }
        let at = float_at(clip, 2);
        let position = Beat(look.beat_at(at));
        let length = Beat(look.beat_at(at + float_at(clip, 3)) - position.0);
        placed.push(picture::Placed {
            name: name.to_string(),
            row,
            position,
            length,
            start: float_at(clip, 4) / look.rate.max(f64::MIN_POSITIVE),
            // How much a **new** box shows: the stretch it occupies, crossed to
            // the wall clock the only way a length may be.
            content: look.tempo.span_secs(position.0, position.0 + length.0),
            source: source_of(int_at(clip, 6), look),
        });
    }
    picture::read(piece, &placed, picture::fresh_id(piece))
        .into_iter()
        .map(|intent| {
            let label = match &intent {
                MultitrackIntent::TrimRegion { .. } => "trim a clip",
                MultitrackIntent::PlaceRegion { .. } => "move a clip",
                _ => "edit the clips",
            };
            (intent, label)
        })
        .collect()
}

/// The **source** a buffer number came from, which is the reverse of the lookup
/// that drew it: a picture names a server buffer and the document names a
/// source, and the table that resolved one is the only thing that reads it back.
fn source_of(bufnum: i64, look: &Look<'_>) -> Option<SourceId> {
    look.takes?.source_of(i32::try_from(bufnum).ok()?)
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
        let Some(id) = name.parse::<u64>().ok().map(NodeId) else {
            continue;
        };
        let Some(track) = tracks.iter_mut().find(|t| t.id == id) else {
            continue;
        };
        let (muted, soloed) = (truthy_at(lane, 3), truthy_at(lane, 4));
        let level = f64::from(float_at(lane, 5) as f32);
        let held = picture::level_of(track);
        if track.muted == muted && track.soloed == soloed && held == level {
            continue;
        }
        track.muted = muted;
        track.soloed = soloed;
        track.level = level;
        // What an older piece carried in the opaque table is the field's now,
        // and leaving both would be two answers to one question.
        if let Some(table) = track.config.0.as_object_mut() {
            table.remove(picture::LEVEL);
        }
        changed = true;
    }
    if !changed {
        return Vec::new();
    }
    vec![(MultitrackIntent::SetTracks { tracks }, "mix a track")]
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

/// An OSC integer, however it was written.
fn int_at(args: &[clausters_core::osc::OscType], n: usize) -> i64 {
    match args.get(n) {
        Some(clausters_core::osc::OscType::Int(v)) => i64::from(*v),
        Some(clausters_core::osc::OscType::Long(v)) => *v,
        Some(clausters_core::osc::OscType::Float(v)) => *v as i64,
        Some(clausters_core::osc::OscType::Double(v)) => *v as i64,
        _ => -1,
    }
}

fn truthy_at(args: &[clausters_core::osc::OscType], n: usize) -> bool {
    super::truthy_at(args, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::document::sources::Takes;
    use clausters_core::osc::OscType;
    use clausters_document::multitrack::{Content, Region, Track};
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
            looping: false,
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

    /// A hundred frames a beat and forty-eight thousand a second: the two
    /// scales stay apart in the tests, so a length converted through the wrong
    /// one is obvious rather than plausible.
    fn look() -> Look<'static> {
        Look {
            tempo: TempoMap::new(480.0), // 480 beats a second: 100 frames each
            rate: 48_000.0,
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

    /// **A piece with a ritardando is not placed by one ratio.** Four beats are
    /// not one length: under a tempo that changes they last longer later than
    /// earlier, so a position is the map's second times the rate and a length
    /// is the difference of two of those.
    ///
    /// The defect this pins is silent — the boxes and the readers are both
    /// derived here, so a single ratio draws and sounds the same wrong place
    /// and the two agree about it.
    #[test]
    fn a_tempo_change_moves_the_boxes_and_a_ratio_would_not() {
        use clausters_document::multitrack::Tempo;

        let mut piece = piece();
        // Sixty a minute — a beat a second — and half that from beat 4 on.
        piece.set_tempo(Tempo {
            at: Beat(0.0),
            bpm: 60.0,
            ramp: false,
            extra: Default::default(),
        });
        piece.set_tempo(Tempo {
            at: Beat(4.0),
            bpm: 30.0,
            ramp: false,
            extra: Default::default(),
        });
        let look = Look {
            tempo: tempo_map(&piece),
            rate: 48_000.0,
            takes: None,
        };
        // The region at beat 4 lasting 2 beats: it starts one second per beat
        // in, and lasts *two* seconds a beat.
        assert_eq!(look.frame_at(4.0), 4.0 * 48_000.0);
        assert_eq!(
            look.frames_over(4.0, 2.0),
            4.0 * 48_000.0,
            "two beats at half the tempo are four seconds"
        );
        // ...and the same two beats before the change are half that, which is
        // the whole of what one ratio cannot say.
        assert_eq!(look.frames_over(0.0, 2.0), 2.0 * 48_000.0);

        // And it comes back the way it went: a payload in frames reads as the
        // beats it was drawn from.
        assert!((look.beat_at(4.0 * 48_000.0) - 4.0).abs() < 1e-9);
        assert!((look.beat_at(8.0 * 48_000.0) - 6.0).abs() < 1e-9);
    }

    /// A piece that never said a tempo reads at the reader's default, which is
    /// a beat a second — so nothing about a piece without tempo changed when
    /// the map arrived.
    #[test]
    fn a_piece_with_no_tempo_is_a_beat_a_second() {
        let look = Look {
            tempo: tempo_map(&piece()),
            rate: 48_000.0,
            takes: None,
        };
        assert_eq!(look.frame_at(3.0), 3.0 * 48_000.0);
        assert_eq!(look.frames_over(3.0, 2.0), 2.0 * 48_000.0);
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

    /// **A box the piece has no region for becomes one.** A split's tail, a
    /// paste, anything a hand made: the payload says which lane it landed on,
    /// which buffer it is a window onto and where in that buffer it opens,
    /// which is everything a region needs — so it is *built* rather than
    /// inferred, and the same rule serves whatever gesture produced it.
    ///
    /// The defect this closes: a split names its halves `"12 2"`, which is no
    /// node id, so the reader dropped the tail and kept the `TrimRegion` that
    /// shortened the original — a cut that silently truncated a region and lost
    /// the rest of it.
    #[test]
    fn a_box_the_piece_does_not_know_becomes_a_region_on_its_lane() {
        let piece = piece();
        let takes = {
            let mut takes = Takes::default();
            takes.insert(
                SourceId(1),
                crate::host::document::sources::Take {
                    bufnum: 7,
                    channels: Some(1),
                    frames: Some(96_000),
                },
            );
            takes
        };
        let look = Look {
            tempo: TempoMap::new(1.0), // a beat a second
            rate: 48_000.0,
            takes: Some(&takes),
        };
        // The piece as a split of region 12 leaves it: the original shortened,
        // and a tail beside it under a name that is not an id.
        let mut args = clips(&[("12", "10", 0.0, 48_000.0)]);
        args.extend([
            OscType::String("12 2".into()),
            OscType::String("10".into()),
            OscType::Float(48_000.0),
            OscType::Float(48_000.0),
            OscType::Float(24_000.0), // half a second into the source
            OscType::String(String::new()),
            OscType::Int(7),
        ]);
        // The rest of the piece, in this look's own units (a beat a second).
        args.extend(clips(&[
            ("13", "10", 4.0 * 48_000.0, 2.0 * 48_000.0),
            ("22", "20", 0.0, 2.0 * 48_000.0),
        ]));
        let edits = read_clips(&piece, &args, &look);

        let lane = edits.iter().find_map(|(i, _)| match i {
            MultitrackIntent::SetLane { lane, regions } => Some((*lane, regions)),
            _ => None,
        });
        let Some((lane, regions)) = lane else {
            panic!("the lane, whole: {edits:?}")
        };
        assert_eq!(lane, NodeId(11));
        assert_eq!(regions.len(), 3, "the two that stayed and the new one");
        let made = regions.last().expect("the new one");
        assert!(
            piece.regions().all(|r| r.id != made.id),
            "it took an id the piece did not already use"
        );
        assert_eq!(made.position, Beat(1.0), "where the payload put it");
        assert_eq!(made.length, Beat(1.0));
        match &made.content {
            Content::Window { window, .. } => {
                assert_eq!(
                    window.source.samples().map(|s| s.source),
                    Some(SourceId(1)),
                    "the source its buffer number resolves to"
                );
                assert_eq!(window.start, 0.5, "half a second in, as the box said");
                assert_eq!(window.duration, 1.0);
            }
            other => panic!("a window onto the samples it named: {other:?}"),
        }
    }

    /// ...and a box naming a buffer this session never read is **not** made
    /// into a region: the document would name a source nobody can resolve, and
    /// a piece that cannot be reopened is worse than a box that did not stick.
    #[test]
    fn a_box_over_a_buffer_nobody_loaded_is_not_invented() {
        let piece = piece();
        let mut args = clips(&[("12", "10", 0.0, 200.0)]);
        args.extend([
            OscType::String("nowhere".into()),
            OscType::String("10".into()),
            OscType::Float(400.0),
            OscType::Float(200.0),
            OscType::Float(0.0),
            OscType::String(String::new()),
            OscType::Int(-1),
        ]);
        args.extend(clips(&[
            ("13", "10", 400.0, 200.0),
            ("22", "20", 0.0, 200.0),
        ]));
        let edits = read_clips(&piece, &args, &look());
        assert!(
            !edits
                .iter()
                .any(|(i, _)| matches!(i, MultitrackIntent::SetLane { .. })),
            "nothing added, and nothing removed either: {edits:?}"
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
        assert_eq!(tracks[1].level, 0.5, "and the fader is a field of its own");
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
