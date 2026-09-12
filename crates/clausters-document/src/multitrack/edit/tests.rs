//! Every verb the piece admits, one test each — which is this milestone's
//! acceptance and not a coverage target: a vocabulary nobody has exercised is a
//! vocabulary whose refusals are guesses.

use super::*;
use crate::multitrack::{Automation, Lane};
use crate::{Lifetime, SegmentSource, SourceId, SourceRef};

fn window(source: u64) -> crate::SegmentRef {
    crate::SegmentRef {
        source: SegmentSource::Samples(SourceRef {
            source: SourceId(source),
            lifetime: Lifetime::Session,
            generation: 0,
            range: None,
        }),
        start: 0.0,
        duration: 8.0,
    }
}

fn region(id: u64, at: f64, len: f64) -> Region {
    Region::new(NodeId(id), Beat(at), Beat(len), Content::window(window(1)))
}

/// Two tracks: the first comped from two lanes with a region on each, the
/// second empty with one automation curve.
fn piece() -> Multitrack {
    let mut first = Track::new(NodeId(10), NodeId(11)).named("vocals");
    first.lanes[0].place(region(100, 0.0, 4.0));
    let mut second = Lane::new(NodeId(12)).named("take 2");
    second.place(region(101, 8.0, 4.0));
    first.lanes.push(second);
    let mut other = Track::new(NodeId(20), NodeId(21)).named("guitar");
    other.automation.push(Automation::new(
        NodeId(22),
        Opaque(serde_json::json!({ "ctl": "level" })),
    ));
    let mut piece = Multitrack::new();
    piece.tracks = vec![first, other];
    piece
}

fn edit(piece: &mut Multitrack, intent: MultitrackIntent) -> Outcome<MultitrackIntent> {
    apply(piece, &intent, &Against::unstated(), &Rules::none())
}

// ---- the tracks, and which lane plays ----

#[test]
fn adding_removing_and_reordering_tracks_are_one_verb() {
    let mut piece = piece();
    let reordered = vec![piece.tracks[1].clone(), piece.tracks[0].clone()];
    let outcome = edit(
        &mut piece,
        MultitrackIntent::SetTracks { tracks: reordered },
    );
    assert!(outcome.applied);
    assert_eq!(piece.tracks[0].id, NodeId(20));
    assert_eq!(piece.version, crate::FIRST_VERSION + 1);
}

#[test]
fn comping_names_the_lane_and_not_its_index() {
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        MultitrackIntent::SetActiveLane {
            track: NodeId(10),
            lane: NodeId(12),
        },
    );
    assert!(outcome.applied);
    assert_eq!(
        piece.track(NodeId(10)).unwrap().active_lane().unwrap().id,
        NodeId(12)
    );

    // A lane of another track is not this track's choice to make.
    let refused = edit(
        &mut piece,
        MultitrackIntent::SetActiveLane {
            track: NodeId(10),
            lane: NodeId(21),
        },
    );
    assert!(!refused.applied);
    assert_eq!(
        refused.reason.as_deref(),
        Some("that lane is not on that track")
    );
    assert!(
        matches!(refused.effective, MultitrackIntent::SetActiveLane { lane, .. } if lane == NodeId(21)),
        "a refusal hands back what was asked when nothing else describes it"
    );
}

#[test]
fn a_lanes_contents_are_stated_whole_and_come_back_in_position_order() {
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        MultitrackIntent::SetLane {
            lane: NodeId(11),
            regions: vec![region(103, 16.0, 4.0), region(102, 4.0, 4.0)],
        },
    );
    assert!(outcome.applied);
    assert_eq!(outcome.reason.as_deref(), Some("put in position order"));
    let lane = piece.lane(NodeId(11)).unwrap().1;
    assert_eq!(
        lane.regions.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![NodeId(102), NodeId(103)],
        "and the region that was there is gone, because this is whole"
    );
}

// ---- a region's geometry ----

#[test]
fn a_region_moved_to_another_track_is_one_edit() {
    // The acceptance, and the reason the address is part of the value: this is
    // one intent, so it is one entry in a log and one undo -- and there is no
    // moment where the region is on neither lane.
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        MultitrackIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(20),
            lane: NodeId(21),
            position: Beat(16.0),
            layer: 2,
        },
    );
    assert!(outcome.applied);
    let (track, lane, moved) = piece.locate(NodeId(100)).unwrap();
    assert_eq!((track.id, lane.id), (NodeId(20), NodeId(21)));
    assert_eq!(moved.position, Beat(16.0));
    assert_eq!(moved.layer, 2);
    assert!(piece.lane(NodeId(11)).unwrap().1.regions.is_empty());
}

#[test]
fn a_placement_snaps_and_says_what_it_snapped_to() {
    let mut piece = piece();
    let outcome = apply(
        &mut piece,
        &MultitrackIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(10),
            lane: NodeId(11),
            position: Beat(4.3),
            layer: 0,
        },
        &Against::unstated(),
        &Rules::quantized(1.0),
    );
    let MultitrackIntent::PlaceRegion { position, .. } = outcome.effective else {
        panic!("the effective edit is the one that landed");
    };
    assert_eq!(position, Beat(4.0));
    assert_eq!(outcome.reason.as_deref(), Some("snapped to the grid"));
}

#[test]
fn a_trim_reports_the_effective_placement_after_snapping() {
    let mut piece = piece();
    let outcome = apply(
        &mut piece,
        &MultitrackIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(1.1),
            length: Beat(2.9),
            content: None,
        },
        &Against::unstated(),
        &Rules::quantized(1.0),
    );
    let MultitrackIntent::TrimRegion {
        position,
        length,
        content,
        ..
    } = outcome.effective
    else {
        panic!("a trim reports a trim");
    };
    assert_eq!((position, length), (Beat(1.0), Beat(3.0)));
    assert!(
        content.is_some(),
        "and it says what the region reads, so the inverse puts the window back"
    );
}

#[test]
fn a_trim_that_moved_the_window_carries_the_window_it_moved_to() {
    let mut piece = piece();
    let moved = Content::window(window(2));
    edit(
        &mut piece,
        MultitrackIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(2.0),
            length: Beat(2.0),
            content: Some(moved.clone()),
        },
    );
    assert_eq!(piece.locate(NodeId(100)).unwrap().2.content, moved);
}

#[test]
fn a_region_cannot_be_trimmed_to_nothing() {
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        MultitrackIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(0.0),
            length: Beat(0.0),
            content: None,
        },
    );
    assert!(!outcome.applied);
    assert_eq!(piece.locate(NodeId(100)).unwrap().2.length, Beat(4.0));
}

// ---- the two that change how many regions there are ----

#[test]
fn a_split_names_the_two_identities_and_applying_it_twice_changes_nothing() {
    let mut piece = piece();
    let split = MultitrackIntent::SplitRegion {
        region: NodeId(100),
        at: Beat(1.0),
        left: NodeId(110),
        right: NodeId(111),
        left_content: None,
        right_content: Some(Content::window(window(2))),
    };
    assert!(edit(&mut piece, split.clone()).applied);
    let lane = piece.lane(NodeId(11)).unwrap().1;
    assert_eq!(
        lane.regions
            .iter()
            .map(|r| (r.id, r.position, r.length))
            .collect::<Vec<_>>(),
        vec![
            (NodeId(110), Beat(0.0), Beat(1.0)),
            (NodeId(111), Beat(1.0), Beat(3.0)),
        ]
    );
    // The half after the cut reads what the caller said it reads, because the
    // crate will not turn a beat into a frame to work it out.
    assert_eq!(
        piece.locate(NodeId(111)).unwrap().2.content,
        Content::window(window(2))
    );

    let version = piece.version;
    let again = edit(&mut piece, split);
    assert!(
        !again.applied,
        "absolute, so a resend is not a second split"
    );
    assert_eq!(piece.version, version);
}

#[test]
fn a_cut_outside_the_region_is_refused_and_says_so() {
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        MultitrackIntent::SplitRegion {
            region: NodeId(100),
            at: Beat(9.0),
            left: NodeId(110),
            right: NodeId(111),
            left_content: None,
            right_content: None,
        },
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.reason.as_deref(),
        Some("the cut is not inside the region")
    );
}

#[test]
fn a_join_spans_from_the_first_to_the_last_and_refuses_across_lanes() {
    let mut piece = piece();
    piece
        .lane_mut(NodeId(11))
        .unwrap()
        .place(region(102, 4.0, 2.0));
    let outcome = edit(
        &mut piece,
        MultitrackIntent::JoinRegions {
            regions: vec![NodeId(102), NodeId(100)],
            into: NodeId(120),
            content: None,
            source: None,
        },
    );
    assert!(outcome.applied);
    let joined = piece.locate(NodeId(120)).unwrap().2;
    assert_eq!((joined.position, joined.length), (Beat(0.0), Beat(6.0)));
    assert!(piece.locate(NodeId(100)).is_none());

    let across = edit(
        &mut piece,
        MultitrackIntent::JoinRegions {
            regions: vec![NodeId(120), NodeId(101)],
            into: NodeId(121),
            content: None,
            source: None,
        },
    );
    assert!(!across.applied);
    assert_eq!(
        across.reason.as_deref(),
        Some("those regions are not on one lane")
    );
}

#[test]
fn a_join_of_one_region_is_not_a_join() {
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        MultitrackIntent::JoinRegions {
            regions: vec![NodeId(100)],
            into: NodeId(120),
            content: None,
            source: None,
        },
    );
    assert!(!outcome.applied);
    assert_eq!(
        outcome.reason.as_deref(),
        Some("a join needs two regions or more")
    );
}

// ---- fades, and the crossfade that is two of them ----

#[test]
fn a_crossfade_is_two_fades_over_an_overlap_and_not_a_third_object() {
    let mut piece = piece();
    piece
        .lane_mut(NodeId(11))
        .unwrap()
        .place(region(102, 3.0, 4.0));
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::FadeRegion {
                region: NodeId(100),
                fade_in: None,
                fade_out: Some(Fade::of(Beat(1.0))),
            }
        )
        .applied
    );
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::FadeRegion {
                region: NodeId(102),
                fade_in: Some(Fade::of(Beat(1.0))),
                fade_out: None,
            }
        )
        .applied
    );
    let first = piece.locate(NodeId(100)).unwrap().2;
    let second = piece.locate(NodeId(102)).unwrap().2;
    assert!(first.overlaps(second));
    assert_eq!(first.fade_out.as_ref().unwrap().length, Beat(1.0));
    assert_eq!(second.fade_in.as_ref().unwrap().length, Beat(1.0));

    // Stated whole, so clearing one is stating it as absent.
    edit(
        &mut piece,
        MultitrackIntent::FadeRegion {
            region: NodeId(100),
            fade_in: None,
            fade_out: None,
        },
    );
    assert!(piece.locate(NodeId(100)).unwrap().2.fade_out.is_none());
}

// ---- the curves, the markers and the timeline ----

#[test]
fn a_regions_own_curve_is_the_same_verb_in_the_other_place() {
    // A track's curve runs the length of the track and is drawn in a lane
    // beside it; a region's runs the length of the region and is drawn inside
    // it. Both exist, neither stands in for the other, and **the verb is one**
    // — which is the whole reason a region carries the same `Automation` a
    // track does rather than a second type.
    let mut piece = piece();
    let curve = Automation::new(NodeId(30), Opaque(serde_json::json!({ "ctl": "gain" })));
    piece.tracks[0].lanes[0].regions[0].automation.push(curve);

    let points = vec![Point {
        at: 0.0,
        value: 0.5,
        data: Opaque::none(),
    }];
    let before = current(
        &piece,
        &MultitrackIntent::SetAutomation {
            automation: NodeId(30),
            points: points.clone(),
        },
    )
    .expect("a region's curve is found wherever a track's is");
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::SetAutomation {
                automation: NodeId(30),
                points: points.clone(),
            }
        )
        .applied
    );
    assert_eq!(
        piece.tracks[0].lanes[0].regions[0].automation[0].points,
        points
    );
    // And it inverts: the curve as it was, read before the edit landed.
    edit(&mut piece, before);
    assert!(
        piece.tracks[0].lanes[0].regions[0].automation[0]
            .points
            .is_empty()
    );
}

#[test]
fn a_curve_a_region_carries_is_not_the_tracks_and_is_addressed_apart() {
    let mut piece = piece();
    piece.tracks[0].lanes[0].regions[0]
        .automation
        .push(Automation::new(
            NodeId(30),
            Opaque(serde_json::json!({ "ctl": "gain" })),
        ));
    assert_eq!(
        piece.automations().count(),
        2,
        "the track's and the region's"
    );
    assert!(piece.automation(NodeId(22)).is_some(), "the track's");
    assert!(piece.automation(NodeId(30)).is_some(), "the region's");
    assert!(piece.track(NodeId(10)).unwrap().automation.is_empty());
}

#[test]
fn an_automation_lane_is_edited_with_the_curves_own_verb() {
    let mut piece = piece();
    let points = vec![
        Point {
            at: 0.0,
            value: 0.0,
            data: Opaque::none(),
        },
        Point {
            at: 4.0,
            value: 1.0,
            data: Opaque(serde_json::json!({ "shape": "exp" })),
        },
    ];
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::SetAutomation {
                automation: NodeId(22),
                points: points.clone(),
            }
        )
        .applied
    );
    let curve = &piece.track(NodeId(20)).unwrap().automation[0];
    assert_eq!(curve.points, points);
    assert_eq!(
        curve.points[1].data.0["shape"], "exp",
        "carried, not interpreted -- an undo that straightened it would lose it"
    );
}

#[test]
fn a_marker_is_placed_moved_renamed_and_removed() {
    let mut piece = piece();
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::SetMarker {
                marker: NodeId(30),
                at: Beat(8.0),
                name: Some("chorus".into()),
            }
        )
        .applied
    );
    assert_eq!(piece.markers.len(), 1);
    edit(
        &mut piece,
        MultitrackIntent::SetMarker {
            marker: NodeId(30),
            at: Beat(12.0),
            name: Some("verse".into()),
        },
    );
    assert_eq!(piece.markers[0].at, Beat(12.0));
    assert_eq!(piece.markers[0].name.as_deref(), Some("verse"));
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::RemoveMarker { marker: NodeId(30) }
        )
        .applied
    );
    assert!(piece.markers.is_empty());
    assert!(
        !edit(
            &mut piece,
            MultitrackIntent::RemoveMarker { marker: NodeId(30) }
        )
        .applied,
        "removing what is not there is not an edit"
    );
}

#[test]
fn the_loop_and_the_punch_are_two_spans_and_unset_is_a_value() {
    let mut piece = piece();
    edit(
        &mut piece,
        MultitrackIntent::SetRange {
            range: SpanKind::Loop,
            span: Some(Span::new(Beat(0.0), Beat(16.0))),
        },
    );
    edit(
        &mut piece,
        MultitrackIntent::SetRange {
            range: SpanKind::Punch,
            span: Some(Span::new(Beat(4.0), Beat(8.0))),
        },
    );
    assert_eq!(piece.loop_span.unwrap().length(), Beat(16.0));
    assert_eq!(piece.punch.unwrap().start, Beat(4.0));
    assert!(
        edit(
            &mut piece,
            MultitrackIntent::SetRange {
                range: SpanKind::Loop,
                span: None,
            }
        )
        .applied
    );
    assert!(piece.loop_span.is_none());
}

#[test]
fn the_two_maps_are_the_pieces_and_arrive_in_position_order() {
    let mut piece = piece();
    edit(
        &mut piece,
        MultitrackIntent::SetTempoMap {
            tempo: vec![Tempo::at(Beat(16.0), 140.0), Tempo::at(Beat(0.0), 120.0)],
        },
    );
    assert_eq!(
        piece.tempo.iter().map(|t| t.at).collect::<Vec<_>>(),
        vec![Beat(0.0), Beat(16.0)]
    );
    assert_eq!(piece.tempo_at(Beat(20.0)).unwrap().bpm, 140.0);
    edit(
        &mut piece,
        MultitrackIntent::SetMeterMap {
            meter: vec![Meter::at(Beat(0.0), 7, 8)],
        },
    );
    assert_eq!(piece.meter_at(Beat(3.0)).unwrap().beats, 7);
}

// ---- the rules the vocabulary keeps, over every verb ----

#[test]
fn every_verb_states_a_value_and_a_resend_is_not_an_edit() {
    // Idempotence, over the whole vocabulary rather than one verb: applying
    // each edit twice leaves the piece where the first one put it, and the
    // second application moves no version.
    let mut piece = piece();
    for intent in vocabulary() {
        let first = edit(&mut piece, intent.clone());
        if !first.applied {
            continue;
        }
        let version = piece.version;
        let held = piece.clone();
        let again = edit(&mut piece, intent.clone());
        assert!(!again.applied, "{intent:?} applied twice");
        assert_eq!(piece.version, version, "{intent:?} moved the version twice");
        assert_eq!(piece, held, "{intent:?} changed the piece twice");
    }
}

#[test]
fn every_verb_inverts_to_what_the_piece_said_before_it() {
    // The whole of what makes undo cheap here, checked verb by verb rather
    // than trusted: read the inverse first, apply, apply the inverse, and the
    // piece is exactly what it was -- the version excepted, which moves
    // forward for an undo like it does for anything else.
    for intent in vocabulary() {
        let mut piece = piece();
        let Some(inverse) = current(&piece, &intent) else {
            panic!("{intent:?} cannot be described, so it cannot be logged");
        };
        let before = piece.clone();
        if !edit(&mut piece, intent.clone()).applied {
            continue;
        }
        assert!(
            edit(&mut piece, inverse).applied,
            "{intent:?} did not invert"
        );
        assert_eq!(
            (piece.tracks, piece.markers, piece.tempo, piece.meter),
            (before.tracks, before.markers, before.tempo, before.meter),
            "{intent:?}"
        );
    }
}

#[test]
fn an_edit_against_a_superseded_piece_comes_back_stale() {
    let mut piece = piece();
    let stale = Against::at(piece.version + 5);
    let outcome = apply(
        &mut piece,
        &MultitrackIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(10),
            lane: NodeId(11),
            position: Beat(8.0),
            layer: 0,
        },
        &stale,
        &Rules::none(),
    );
    assert!(outcome.stale);
    assert!(!outcome.applied);
    assert_eq!(piece.locate(NodeId(100)).unwrap().2.position, Beat(0.0));
    assert!(
        matches!(outcome.effective, MultitrackIntent::PlaceRegion { position, .. } if position == Beat(0.0)),
        "and what comes back is where the region actually is"
    );
}

#[test]
fn a_refused_edit_leaves_the_version_where_it_was() {
    let mut piece = piece();
    let version = piece.version;
    let outcome = edit(
        &mut piece,
        MultitrackIntent::PlaceRegion {
            region: NodeId(999),
            track: NodeId(10),
            lane: NodeId(11),
            position: Beat(0.0),
            layer: 0,
        },
    );
    assert!(!outcome.applied);
    assert_eq!(outcome.reason.as_deref(), Some("no such region"));
    assert_eq!(piece.version, version);
}

#[test]
fn a_run_of_drags_of_one_region_is_one_thing_the_person_did() {
    let drag = |position: f64| {
        coalesce_key(&MultitrackIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(10),
            lane: NodeId(11),
            position: Beat(position),
            layer: 0,
        })
    };
    assert_eq!(drag(1.0), drag(2.0));
    assert_ne!(
        drag(1.0),
        coalesce_key(&MultitrackIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(1.0),
            length: Beat(1.0),
            content: None,
        })
    );
    // The edits that name the piece itself key on the kind alone.
    assert_eq!(
        coalesce_key(&MultitrackIntent::SetTempoMap { tempo: Vec::new() }),
        "settempomap"
    );
}

#[test]
fn the_wire_shape_is_one_tag_and_survives_a_round_trip() {
    for intent in vocabulary() {
        let json = serde_json::to_value(&intent).unwrap();
        assert!(
            json.get("intent").is_some(),
            "{intent:?} is not tagged like every other edit"
        );
        let back: MultitrackIntent = serde_json::from_value(json).unwrap();
        assert_eq!(back, intent);
    }
}

/// One of each verb, so a test over "every intent" is a list nobody has to keep
/// in their head -- and so adding a verb without a test fails to compile here
/// rather than passing quietly.
fn vocabulary() -> Vec<MultitrackIntent> {
    let all = vec![
        MultitrackIntent::SetTracks {
            tracks: vec![piece().tracks[1].clone()],
        },
        MultitrackIntent::SetActiveLane {
            track: NodeId(10),
            lane: NodeId(12),
        },
        MultitrackIntent::SetLane {
            lane: NodeId(11),
            regions: vec![region(102, 4.0, 4.0)],
        },
        MultitrackIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(20),
            lane: NodeId(21),
            position: Beat(16.0),
            layer: 1,
        },
        MultitrackIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(1.0),
            length: Beat(2.0),
            content: Some(Content::window(window(2))),
        },
        MultitrackIntent::SplitRegion {
            region: NodeId(100),
            at: Beat(2.0),
            left: NodeId(110),
            right: NodeId(111),
            left_content: None,
            right_content: None,
        },
        MultitrackIntent::JoinRegions {
            regions: vec![NodeId(100), NodeId(101)],
            into: NodeId(120),
            content: None,
            source: None,
        },
        MultitrackIntent::FadeRegion {
            region: NodeId(100),
            fade_in: Some(Fade::of(Beat(1.0))),
            fade_out: None,
        },
        MultitrackIntent::SetAutomation {
            automation: NodeId(22),
            points: vec![Point {
                at: 0.0,
                value: 1.0,
                data: Opaque::none(),
            }],
        },
        MultitrackIntent::SetMarker {
            marker: NodeId(30),
            at: Beat(8.0),
            name: Some("chorus".into()),
        },
        MultitrackIntent::RemoveMarker { marker: NodeId(30) },
        MultitrackIntent::SetRange {
            range: SpanKind::Loop,
            span: Some(Span::new(Beat(0.0), Beat(16.0))),
        },
        MultitrackIntent::SetTempoMap {
            tempo: vec![Tempo::at(Beat(0.0), 132.0)],
        },
        MultitrackIntent::SetMeterMap {
            meter: vec![Meter::at(Beat(0.0), 7, 8)],
        },
    ];
    // The count is the enum's, so a verb added without an entry here fails the
    // suite instead of slipping past every test that walks the vocabulary.
    assert_eq!(all.len(), 14, "one of each verb, and the enum has 14");
    all
}

// ---- through a history: one pile, two vocabularies ----

mod through_a_history {
    use super::*;
    use crate::history::{Editable, History};
    use crate::points::{POINTS, Points, PointsIntent, payload as points_payload};

    /// Undo, spelled the way a caller has to spell it: the pile hands back the
    /// inverses with the structure each belongs to, and the caller applies them
    /// through that domain's own door.
    fn undo(history: &mut History, piece: &mut Multitrack) {
        let undone = history.undo().expect("something to undo");
        for (_, load) in undone.legs {
            Piece::new(piece).apply(&load);
        }
    }

    #[test]
    fn a_region_moved_between_tracks_undoes_in_one_step() {
        // O22's acceptance. One intent, so one entry -- and the undo puts the
        // region back on the lane it came from, not merely at the beat it came
        // from.
        let mut piece = piece();
        let mut history = History::new();
        let multitrack = history.register(MULTITRACK);

        history.apply(
            multitrack,
            &mut Piece::new(&mut piece),
            &payload(&MultitrackIntent::PlaceRegion {
                region: NodeId(100),
                track: NodeId(20),
                lane: NodeId(21),
                position: Beat(16.0),
                layer: 1,
            }),
            "move the region",
        );
        assert_eq!(history.len(), 1, "one gesture, one entry");
        assert_eq!(
            piece.locate(NodeId(100)).map(|(t, l, _)| (t.id, l.id)),
            Some((NodeId(20), NodeId(21)))
        );

        undo(&mut history, &mut piece);
        let (track, lane, back) = piece.locate(NodeId(100)).expect("back where it was");
        assert_eq!((track.id, lane.id), (NodeId(10), NodeId(11)));
        assert_eq!((back.position, back.layer), (Beat(0.0), 0));
        assert!(!history.can_undo());
    }

    #[test]
    fn a_split_undoes_as_the_lane_that_was_there() {
        let mut piece = piece();
        let before = piece.lane(NodeId(11)).unwrap().1.clone();
        let mut history = History::new();
        let multitrack = history.register(MULTITRACK);
        history.apply(
            multitrack,
            &mut Piece::new(&mut piece),
            &payload(&MultitrackIntent::SplitRegion {
                region: NodeId(100),
                at: Beat(1.0),
                left: NodeId(110),
                right: NodeId(111),
                left_content: None,
                right_content: None,
            }),
            "split",
        );
        assert_eq!(piece.lane(NodeId(11)).unwrap().1.regions.len(), 2);
        undo(&mut history, &mut piece);
        assert_eq!(
            piece.lane(NodeId(11)).unwrap().1.regions,
            before.regions,
            "the region that was made out of two is the one that comes back"
        );
    }

    #[test]
    fn a_refused_edit_leaves_no_entry() {
        let mut piece = piece();
        let mut history = History::new();
        let multitrack = history.register(MULTITRACK);
        history.apply(
            multitrack,
            &mut Piece::new(&mut piece),
            &payload(&MultitrackIntent::RemoveMarker {
                marker: NodeId(999),
            }),
            "remove",
        );
        assert!(history.is_empty(), "a refusal is not an edit");
    }

    #[test]
    fn a_history_holding_a_piece_and_a_curve_undoes_them_in_one_order() {
        // The reason the piece is a domain rather than a second `apply`: an
        // application showing a multitrack and a curve has one history, and the
        // interleaved order is the pile's. Nothing routes by anything but the
        // structure each leg names.
        let mut piece = piece();
        let mut curve = Points::new(Vec::new());
        let mut history = History::new();
        let multitrack = history.register(MULTITRACK);
        let points = history.register(POINTS);

        history.apply(
            multitrack,
            &mut Piece::new(&mut piece),
            &payload(&MultitrackIntent::SetMarker {
                marker: NodeId(30),
                at: Beat(8.0),
                name: Some("chorus".into()),
            }),
            "add a marker",
        );
        history.apply(
            points,
            &mut curve,
            &points_payload(&PointsIntent::SetPoints {
                points: vec![Point {
                    at: 0.0,
                    value: 1.0,
                    data: Opaque::none(),
                }],
            }),
            "draw",
        );
        history.apply(
            multitrack,
            &mut Piece::new(&mut piece),
            &payload(&MultitrackIntent::SetRange {
                range: SpanKind::Loop,
                span: Some(Span::new(Beat(0.0), Beat(16.0))),
            }),
            "set the loop",
        );
        assert_eq!(history.len(), 3, "one pile over both");

        for expected in [multitrack, points, multitrack] {
            for (structure, load) in history.undo().expect("something to undo").legs {
                assert_eq!(structure, expected);
                if structure == multitrack {
                    Piece::new(&mut piece).apply(&load);
                } else {
                    curve.apply(&load);
                }
            }
        }
        assert!(piece.markers.is_empty());
        assert!(piece.loop_span.is_none());
        assert!(curve.0.is_empty());
        assert!(!history.can_undo());
    }
}

// ---- the door a client reaches: the piece as JSON state ----

#[test]
fn the_piece_is_edited_across_the_seam_as_state_and_an_inverse() {
    // What both clients already have a binding for (`domain_edit`), now
    // answering for the piece: hand over the state and the edit, take back the
    // new state and what would put it back. No new surface in either language,
    // which is what keeps the two from growing different doors to one
    // vocabulary.
    let state = Opaque(serde_json::to_value(piece()).unwrap());
    let load = payload(&MultitrackIntent::PlaceRegion {
        region: NodeId(100),
        track: NodeId(20),
        lane: NodeId(21),
        position: Beat(16.0),
        layer: 0,
    });
    let edited = crate::domain::edit(MULTITRACK, &state, &load).expect("the piece is served");
    assert!(edited.applied);

    let moved: Multitrack = serde_json::from_value(edited.state.0.clone()).unwrap();
    assert_eq!(
        moved.locate(NodeId(100)).map(|(t, _, _)| t.id),
        Some(NodeId(20))
    );
    assert_eq!(moved.version, crate::FIRST_VERSION + 1);

    let back = crate::domain::edit(MULTITRACK, &edited.state, &edited.current.unwrap())
        .expect("and the inverse goes back through the same door");
    let restored: Multitrack = serde_json::from_value(back.state.0).unwrap();
    assert_eq!(
        restored.locate(NodeId(100)).map(|(t, l, _)| (t.id, l.id)),
        Some((NodeId(10), NodeId(11)))
    );
}

#[test]
fn the_crate_names_the_pieces_vocabulary_where_a_caller_asks_for_it() {
    assert!(crate::domain::known(MULTITRACK));
    let load = payload(&MultitrackIntent::TrimRegion {
        region: NodeId(100),
        position: Beat(0.0),
        length: Beat(2.0),
        content: None,
    });
    assert_eq!(
        crate::domain::coalesce_key(MULTITRACK, &load).as_deref(),
        Some("trimregion:100"),
        "asked once here rather than spelled again per language"
    );
    assert_eq!(
        crate::domain::coalesce_key(MULTITRACK, &Opaque(serde_json::json!({"intent": "nope"}))),
        None,
        "and a payload written in another vocabulary says so"
    );
}
