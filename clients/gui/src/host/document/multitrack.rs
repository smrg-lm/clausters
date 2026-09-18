//! Drawing **the multitrack** as a multitrack, and reading a hand's answer back.
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
//! The multitrack measures **seconds** and the shared time axis measures
//! timeline samples, so what crosses between them is the rate alone, by the
//! core's one rule: a position lands on a whole sample and a length is the
//! difference of its ends. The tempo map the multitrack holds
//! places nothing; a ruler reads it. A region's window into its samples is in
//! seconds too, which meets the axis the same way.

use clausters_document::SourceId;
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::edit::MultitrackIntent;
use clausters_document::multitrack::picture;
use clausters_editing::multitrack as projection;

use super::sources::Takes;
use super::tree::{ClipRow, LaneRow, Picture};

/// How the picture is scaled.
#[derive(Debug, Clone)]
pub struct Look<'a> {
    /// Frames a second — where the multitrack's seconds meet the timeline.
    pub rate: f64,
    /// The session's samples, once somebody resolved them to server buffers.
    pub takes: Option<&'a Takes>,
    /// **What each source is**, when the multitrack came from a session: the table a
    /// join's spans are read out of, so a join is drawn from the takes it reads.
    pub sources:
        Option<&'a std::collections::BTreeMap<SourceId, clausters_document::session::Source>>,
}

impl Default for Look<'_> {
    fn default() -> Self {
        Self {
            rate: 48_000.0,
            takes: None,
            sources: None,
        }
    }
}

impl Look<'_> {
    /// Where a second falls on the timeline, in frames: the shared projection's
    /// rule, which lands it on a whole sample.
    pub fn frame_at(&self, secs: f64) -> f64 {
        self.projection().frame_at(secs)
    }

    /// This same look, as the shared projection asks for it.
    ///
    /// The two are the same two facts — a rate, and which server buffer a
    /// source was read into — and the only difference is that the projection
    /// asks the second as a question ([`projection::Buffers`]) rather than as a
    /// table, because its three callers hold it three ways.
    pub fn projection(&self) -> projection::Look<'_> {
        projection::Look {
            rate: self.rate,
            sources: self,
        }
    }

    /// The second a frame falls on: the inverse, and the way an edit comes
    /// back.
    pub fn secs_at(&self, frame: f64) -> f64 {
        self.projection().secs_at(frame)
    }
}

/// The multitrack as the `multitrack` widget takes it, and as an edit-back is
/// resolved against.
///
/// **The shape is the crate's and the time is this host's**
/// ([`clausters_document::multitrack::picture`]): what a row and a box *are* is
/// the format's business and is written once for every client; turning seconds
/// into frames on the shared axis is the rate's, which is what this adds.
pub fn shown(multitrack: &Multitrack, look: &Look<'_>) -> Picture {
    // **The props are the projection's**, and they are the same props the two
    // clients send: one list of rows and one of boxes, in one shape, so a multitrack
    // opened here and a multitrack opened from a script are the same picture rather
    // than two pictures that agree. What is left here is the *binding* -- which
    // node each row and box stands for -- which is this host's own, since it is
    // what an edit-back is resolved against.
    let projected = projection::props(multitrack, &look.projection());
    let rows = picture::rows(multitrack);
    let boxes = picture::boxes(multitrack);
    Picture {
        lanes: rows
            .iter()
            .map(|row| LaneRow {
                node: row.track,
                holder: row.lane,
                // A track has no offset: the multitrack's timeline is one, and a
                // region states where it is on it.
                base: 0.0,
            })
            .collect(),
        clips: boxes
            .iter()
            .map(|box_| ClipRow {
                node: box_.region,
                lane: box_.row,
            })
            .collect(),
        // **All of it.** Which keys a picture has is the projection's to say,
        // and a host that named a couple of them drew a multitrack with its curves
        // missing.
        props: projected,
    }
}

/// The **server buffer** a source was read into, or `-1` for a box over
/// nothing: a window onto notes, a composite, or samples nobody read in yet.
impl projection::Buffers for Look<'_> {
    fn bufnum(&self, source: SourceId) -> i64 {
        self.takes
            .and_then(|takes| takes.get(source))
            .map_or(-1, |take| i64::from(take.bufnum))
    }

    /// The reverse of the lookup that drew it: a picture names a server buffer
    /// and the document names a source, and the table that resolved one is the
    /// only thing that reads it back.
    fn source(&self, bufnum: i64) -> Option<SourceId> {
        self.takes?.source_of(i32::try_from(bufnum).ok()?)
    }

    /// The spans a join is made of, out of the session's own table — the
    /// same statement a client's editor answers from the joins it minted.
    fn parts(&self, source: SourceId) -> Option<Vec<clausters_document::session::Part>> {
        match &self.sources?.get(&source)?.location {
            clausters_document::session::Location::Segments { parts } => Some(parts.clone()),
            _ => None,
        }
    }

    fn frames(&self, source: SourceId) -> Option<u64> {
        self.takes?.get(source)?.frames
    }
}

/// **What a report of the multitrack means**, in the multitrack's own vocabulary.
///
/// One reading for every tag a multitrack widget reports under — the boxes, the
/// rows, the break-points — because a host reports every gesture the same way
/// and the difference between them is the *multitrack's*, not the host's. It is
/// [`clausters_editing::multitrack::intake`]'s, which is the same reading both
/// clients go through: this crate held its own copy of it until the projections
/// moved, and a standalone host that read a septuple its own way would be
/// exactly the divergence the shared crate exists to end.
pub fn read(
    multitrack: &Multitrack,
    tag: &str,
    args: &[clausters_core::osc::OscType],
    look: &Look<'_>,
) -> Vec<(MultitrackIntent, &'static str)> {
    let values: Vec<serde_json::Value> = args.iter().map(atom).collect();
    projection::read(multitrack, tag, &values, &look.projection())
        .into_iter()
        .map(|intent| {
            let label = projection::label(&intent);
            (intent, label)
        })
        .collect()
}

/// An OSC atom as the JSON a reading, or an editor's turn, takes.
///
/// The wire's framing is the host's and the reading is the crate's, so this is
/// where the one becomes the other — the same line each client draws for its own
/// transport.
pub(crate) fn atom(value: &clausters_core::osc::OscType) -> serde_json::Value {
    use clausters_core::osc::OscType;
    match value {
        OscType::String(s) => serde_json::Value::String(s.clone()),
        OscType::Float(v) => serde_json::json!(f64::from(*v)),
        OscType::Double(v) => serde_json::json!(*v),
        OscType::Int(v) => serde_json::json!(*v),
        OscType::Long(v) => serde_json::json!(*v),
        OscType::Bool(v) => serde_json::json!(*v),
        _ => serde_json::Value::Null,
    }
}

/// [`read`] over a report of the boxes — every placement gesture's payload.
pub fn read_clips(
    multitrack: &Multitrack,
    args: &[clausters_core::osc::OscType],
    look: &Look<'_>,
) -> Vec<(MultitrackIntent, &'static str)> {
    read(multitrack, "clips", args, look)
}

/// [`read`] over a report of the rows — the mixer's payload.
///
/// Mute, solo and the fader are one [`MultitrackIntent::SetTracks`] because the
/// multitrack's only verb over a track is the whole list, which is what makes adding,
/// removing and reordering one verb and costs the inverse a copy of the tracks.
pub fn read_lanes(
    multitrack: &Multitrack,
    args: &[clausters_core::osc::OscType],
    look: &Look<'_>,
) -> Vec<(MultitrackIntent, &'static str)> {
    read(multitrack, "lanes", args, look)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::document::sources::Takes;
    use clausters_core::osc::OscType;
    use clausters_document::multitrack::{Content, Region, Track};
    use clausters_document::{
        Against, Beat, NodeId, Opaque, Rules, Second, SegmentRef, SegmentSource, SourceId,
    };
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
        let mut region = Region::new(NodeId(id), Second(position), Second(length), window(1, 0.0));
        region.name = Some(format!("r{id}"));
        region
    }

    /// Two tracks, one lane each: the first holds two regions, the second one.
    fn multitrack() -> Multitrack {
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

    /// A hundred frames a second, so a position reads as its seconds times a
    /// hundred.
    fn look() -> Look<'static> {
        Look {
            rate: 100.0,
            takes: None,
            sources: None,
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
    fn a_multitrack_draws_a_row_per_track_naming_ids() {
        let shown = shown(&multitrack(), &look());
        assert_eq!(
            shown.lanes.iter().map(|l| l.node).collect::<Vec<_>>(),
            vec![NodeId(10), NodeId(20)]
        );
        assert_eq!(
            shown.lanes.iter().map(|l| l.holder).collect::<Vec<_>>(),
            vec![NodeId(11), NodeId(21)],
            "a row's clips are its active lane's, which is what a region joins"
        );
        let lanes = shown.props["lanes"].as_array().expect("flat");
        assert_eq!(lanes[0], "10", "named by the track's id");
        assert_eq!(lanes[1], "t10", "and labelled by its name");
        let clips = shown.props["clips"].as_array().expect("flat");
        assert_eq!(clips[0], "12");
        assert_eq!(clips[1], "10", "on the row of the track that holds it");
        assert_eq!(clips[2], 0.0, "seconds, in timeline units");
        assert_eq!(clips[3], 200.0, "two seconds at a hundred units each");
    }

    /// **A tempo moves no box.** The multitrack is in seconds, so a position
    /// and a length are the rate's alone, and a tempo change the multitrack
    /// holds is a ruler's to read.
    #[test]
    fn a_tempo_change_moves_no_box() {
        use clausters_document::multitrack::Tempo;

        let mut multitrack = multitrack();
        multitrack.set_tempo(Tempo::at(Beat(0.0), 1.0));
        multitrack.set_tempo(Tempo::at(Beat(4.0), 0.5));
        let look = Look {
            rate: 48_000.0,
            takes: None,
            sources: None,
        };
        assert_eq!(look.frame_at(4.0), 4.0 * 48_000.0);
        assert_eq!(look.frame_at(2.0), 2.0 * 48_000.0);
        assert!((look.secs_at(8.0 * 48_000.0) - 8.0).abs() < 1e-9);
        assert_eq!(
            shown(&multitrack, &look).props,
            shown(&self::multitrack(), &look).props
        );
    }

    /// **A move is a `PlaceRegion` and never changes what the region reads**;
    /// a resize is the trim, which is the other verb. The payload says neither
    /// — it says where the boxes are — so this is where the two are told apart.
    #[test]
    fn a_move_a_cross_and_a_trim_are_each_their_own_verb() {
        let multitrack = multitrack();
        // Nothing moved: an idempotent report of what already holds.
        let same = read_clips(
            &multitrack,
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
        // box** -- it is the multitrack as it now stands -- so these list all three.
        let moved = read_clips(
            &multitrack,
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
                        && *position == Second(3.0)
            ),
            "{moved:?}"
        );

        // ...and dragged onto the other row: the same verb with a different
        // track, which is why a cross is not a second mechanism.
        let crossed = read_clips(
            &multitrack,
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
            &multitrack,
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
                    if *region == NodeId(12) && *position == Second(1.0) && *length == Second(1.0)
            ),
            "{trimmed:?}"
        );
        assert_eq!(trimmed.len(), 1, "and not a move beside it: {trimmed:?}");
    }

    /// **A box the payload leaves out was deleted**, and that is the lane's own
    /// whole-list verb — the multitrack has no "remove one region", for the same
    /// reason it has no "add one".
    #[test]
    fn a_clip_the_payload_does_not_name_is_removed_from_its_lane() {
        let multitrack = multitrack();
        let edits = read_clips(
            &multitrack,
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

    /// **A box the multitrack has no region for becomes one.** A split's tail, a
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
    fn a_box_the_multitrack_does_not_know_becomes_a_region_on_its_lane() {
        let multitrack = multitrack();
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
            rate: 48_000.0,
            takes: Some(&takes),
            sources: None,
        };
        // The multitrack as a split of region 12 leaves it: the original shortened,
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
        // The rest of the multitrack, in this look's own units (a beat a second).
        args.extend(clips(&[
            ("13", "10", 4.0 * 48_000.0, 2.0 * 48_000.0),
            ("22", "20", 0.0, 2.0 * 48_000.0),
        ]));
        let edits = read_clips(&multitrack, &args, &look);

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
            multitrack.regions().all(|r| r.id != made.id),
            "it took an id the multitrack did not already use"
        );
        assert_eq!(made.position, Second(1.0), "where the payload put it");
        assert_eq!(made.length, Second(1.0));
        match &made.content {
            Content::Window { window, .. } => {
                assert_eq!(
                    window.source.samples().map(|s| s.source),
                    Some(SourceId(1)),
                    "the source its buffer number resolves to"
                );
                assert_eq!(window.start, 0.5, "half a second in, as the box said");
                // **How much of its source the window reaches**, not what the
                // box shows: the take holds two seconds, and a region is a view
                // onto all of it. The box's own second is its length above.
                assert_eq!(window.duration, 2.0, "the whole take the table knows");
            }
            other => panic!("a window onto the samples it named: {other:?}"),
        }
    }

    /// ...and a box naming a buffer this session never read is **not** made
    /// into a region: the document would name a source nobody can resolve, and
    /// a multitrack that cannot be reopened is worse than a box that did not stick.
    #[test]
    fn a_box_over_a_buffer_nobody_loaded_is_not_invented() {
        let multitrack = multitrack();
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
        let edits = read_clips(&multitrack, &args, &look());
        assert!(
            !edits
                .iter()
                .any(|(i, _)| matches!(i, MultitrackIntent::SetLane { .. })),
            "nothing added, and nothing removed either: {edits:?}"
        );
    }

    /// The strip: mute, solo and the fader travel in the multitrack's one verb over
    /// a track, and a strip saying what the track already says is not an edit.
    #[test]
    fn only_the_strip_that_moved_rewrites_the_tracks() {
        let multitrack = multitrack();
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
                    // **What a track with no automation is drawn with.** The
                    // toggle asks for one where there is none, so a fixture
                    // saying "shown" would be asking this multitrack for two curves
                    // it never had.
                    OscType::Int(0),
                ]);
            }
            args
        };
        assert!(
            read_lanes(
                &multitrack,
                &lanes(&[("10", false, 1.0), ("20", false, 1.0)]),
                &look()
            )
            .is_empty(),
            "the strips as they were drawn are not an edit"
        );
        let edits = read_lanes(
            &multitrack,
            &lanes(&[("10", false, 1.0), ("20", true, 0.5)]),
            &look(),
        );
        let [(MultitrackIntent::SetTracks { tracks }, _)] = edits.as_slice() else {
            panic!("the tracks, whole: {edits:?}");
        };
        assert!(!tracks[0].muted, "the one nobody touched is untouched");
        assert!(tracks[1].muted);
        assert_eq!(tracks[1].level, 0.5, "and the fader is a field of its own");
    }

    /// End to end through the crate's own door: the edits this reads apply, and
    /// the multitrack says what the hand said.
    #[test]
    fn the_edits_it_reads_apply_to_the_multitrack() {
        let mut multitrack = multitrack();
        let edits = read_clips(
            &multitrack.clone(),
            &clips(&[
                ("12", "20", 100.0, 200.0),
                ("13", "10", 400.0, 200.0),
                ("22", "20", 0.0, 200.0),
            ]),
            &look(),
        );
        for (intent, _) in edits {
            let outcome = clausters_document::multitrack::edit::apply(
                &mut multitrack,
                &intent,
                &Against::default(),
                &Rules::none(),
            );
            assert!(outcome.applied, "{intent:?}");
        }
        assert!(
            multitrack.tracks[1]
                .lanes
                .iter()
                .any(|l| l.regions.iter().any(|r| r.id == NodeId(12))),
            "the region is on the second track now"
        );
        assert!(
            multitrack.tracks[0]
                .lanes
                .iter()
                .all(|l| l.regions.iter().all(|r| r.id != NodeId(12))),
            "and not on the first one as well"
        );
    }
}
