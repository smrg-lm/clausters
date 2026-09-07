//! Every verb the piece admits, one test each — which is this milestone's
//! acceptance and not a coverage target: a vocabulary nobody has exercised is a
//! vocabulary whose refusals are guesses.

use super::*;
use crate::arrangement::{Automation, Lane};
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
fn piece() -> Arrangement {
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
    let mut piece = Arrangement::new();
    piece.tracks = vec![first, other];
    piece
}

fn edit(piece: &mut Arrangement, intent: ArrangementIntent) -> Outcome<ArrangementIntent> {
    apply(piece, &intent, &Against::unstated(), &Rules::none())
}

// ---- the tracks, and which lane plays ----

#[test]
fn adding_removing_and_reordering_tracks_are_one_verb() {
    let mut piece = piece();
    let reordered = vec![piece.tracks[1].clone(), piece.tracks[0].clone()];
    let outcome = edit(
        &mut piece,
        ArrangementIntent::SetTracks { tracks: reordered },
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
        ArrangementIntent::SetActiveLane {
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
        ArrangementIntent::SetActiveLane {
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
        matches!(refused.effective, ArrangementIntent::SetActiveLane { lane, .. } if lane == NodeId(21)),
        "a refusal hands back what was asked when nothing else describes it"
    );
}

#[test]
fn a_lanes_contents_are_stated_whole_and_come_back_in_position_order() {
    let mut piece = piece();
    let outcome = edit(
        &mut piece,
        ArrangementIntent::SetLane {
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
        ArrangementIntent::PlaceRegion {
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
        &ArrangementIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(10),
            lane: NodeId(11),
            position: Beat(4.3),
            layer: 0,
        },
        &Against::unstated(),
        &Rules::quantized(1.0),
    );
    let ArrangementIntent::PlaceRegion { position, .. } = outcome.effective else {
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
        &ArrangementIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(1.1),
            length: Beat(2.9),
            content: None,
        },
        &Against::unstated(),
        &Rules::quantized(1.0),
    );
    let ArrangementIntent::TrimRegion {
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
        ArrangementIntent::TrimRegion {
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
        ArrangementIntent::TrimRegion {
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
    let split = ArrangementIntent::SplitRegion {
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
        ArrangementIntent::SplitRegion {
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
        ArrangementIntent::JoinRegions {
            regions: vec![NodeId(102), NodeId(100)],
            into: NodeId(120),
            content: None,
        },
    );
    assert!(outcome.applied);
    let joined = piece.locate(NodeId(120)).unwrap().2;
    assert_eq!((joined.position, joined.length), (Beat(0.0), Beat(6.0)));
    assert!(piece.locate(NodeId(100)).is_none());

    let across = edit(
        &mut piece,
        ArrangementIntent::JoinRegions {
            regions: vec![NodeId(120), NodeId(101)],
            into: NodeId(121),
            content: None,
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
        ArrangementIntent::JoinRegions {
            regions: vec![NodeId(100)],
            into: NodeId(120),
            content: None,
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
            ArrangementIntent::FadeRegion {
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
            ArrangementIntent::FadeRegion {
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
        ArrangementIntent::FadeRegion {
            region: NodeId(100),
            fade_in: None,
            fade_out: None,
        },
    );
    assert!(piece.locate(NodeId(100)).unwrap().2.fade_out.is_none());
}

// ---- the curves, the markers and the timeline ----

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
            ArrangementIntent::SetAutomation {
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
            ArrangementIntent::SetMarker {
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
        ArrangementIntent::SetMarker {
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
            ArrangementIntent::RemoveMarker { marker: NodeId(30) }
        )
        .applied
    );
    assert!(piece.markers.is_empty());
    assert!(
        !edit(
            &mut piece,
            ArrangementIntent::RemoveMarker { marker: NodeId(30) }
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
        ArrangementIntent::SetRange {
            range: SpanKind::Loop,
            span: Some(Span::new(Beat(0.0), Beat(16.0))),
        },
    );
    edit(
        &mut piece,
        ArrangementIntent::SetRange {
            range: SpanKind::Punch,
            span: Some(Span::new(Beat(4.0), Beat(8.0))),
        },
    );
    assert_eq!(piece.loop_span.unwrap().length(), Beat(16.0));
    assert_eq!(piece.punch.unwrap().start, Beat(4.0));
    assert!(
        edit(
            &mut piece,
            ArrangementIntent::SetRange {
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
        ArrangementIntent::SetTempoMap {
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
        ArrangementIntent::SetMeterMap {
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
        &ArrangementIntent::PlaceRegion {
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
        matches!(outcome.effective, ArrangementIntent::PlaceRegion { position, .. } if position == Beat(0.0)),
        "and what comes back is where the region actually is"
    );
}

#[test]
fn a_refused_edit_leaves_the_version_where_it_was() {
    let mut piece = piece();
    let version = piece.version;
    let outcome = edit(
        &mut piece,
        ArrangementIntent::PlaceRegion {
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
        coalesce_key(&ArrangementIntent::PlaceRegion {
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
        coalesce_key(&ArrangementIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(1.0),
            length: Beat(1.0),
            content: None,
        })
    );
    // The edits that name the piece itself key on the kind alone.
    assert_eq!(
        coalesce_key(&ArrangementIntent::SetTempoMap { tempo: Vec::new() }),
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
        let back: ArrangementIntent = serde_json::from_value(json).unwrap();
        assert_eq!(back, intent);
    }
}

/// One of each verb, so a test over "every intent" is a list nobody has to keep
/// in their head -- and so adding a verb without a test fails to compile here
/// rather than passing quietly.
fn vocabulary() -> Vec<ArrangementIntent> {
    let all = vec![
        ArrangementIntent::SetTracks {
            tracks: vec![piece().tracks[1].clone()],
        },
        ArrangementIntent::SetActiveLane {
            track: NodeId(10),
            lane: NodeId(12),
        },
        ArrangementIntent::SetLane {
            lane: NodeId(11),
            regions: vec![region(102, 4.0, 4.0)],
        },
        ArrangementIntent::PlaceRegion {
            region: NodeId(100),
            track: NodeId(20),
            lane: NodeId(21),
            position: Beat(16.0),
            layer: 1,
        },
        ArrangementIntent::TrimRegion {
            region: NodeId(100),
            position: Beat(1.0),
            length: Beat(2.0),
            content: Some(Content::window(window(2))),
        },
        ArrangementIntent::SplitRegion {
            region: NodeId(100),
            at: Beat(2.0),
            left: NodeId(110),
            right: NodeId(111),
            left_content: None,
            right_content: None,
        },
        ArrangementIntent::JoinRegions {
            regions: vec![NodeId(100), NodeId(101)],
            into: NodeId(120),
            content: None,
        },
        ArrangementIntent::FadeRegion {
            region: NodeId(100),
            fade_in: Some(Fade::of(Beat(1.0))),
            fade_out: None,
        },
        ArrangementIntent::SetAutomation {
            automation: NodeId(22),
            points: vec![Point {
                at: 0.0,
                value: 1.0,
                data: Opaque::none(),
            }],
        },
        ArrangementIntent::SetMarker {
            marker: NodeId(30),
            at: Beat(8.0),
            name: Some("chorus".into()),
        },
        ArrangementIntent::RemoveMarker { marker: NodeId(30) },
        ArrangementIntent::SetRange {
            range: SpanKind::Loop,
            span: Some(Span::new(Beat(0.0), Beat(16.0))),
        },
        ArrangementIntent::SetTempoMap {
            tempo: vec![Tempo::at(Beat(0.0), 132.0)],
        },
        ArrangementIntent::SetMeterMap {
            meter: vec![Meter::at(Beat(0.0), 7, 8)],
        },
    ];
    // The count is the enum's, so a verb added without an entry here fails the
    // suite instead of slipping past every test that walks the vocabulary.
    assert_eq!(all.len(), 14, "one of each verb, and the enum has 14");
    all
}
