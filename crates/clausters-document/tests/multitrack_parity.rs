//! The multitrack this crate defines is the one the Python client writes.
//!
//! The crate's own suite proves the shape round-trips against itself, which
//! says nothing about whether anyone else agrees with it. This does: the vector
//! beside it is a multitrack built with `clausters.document`, as that
//! module writes it (`gen-multitrack-vector.py` writes the file, and it is
//! committed). Nothing in CI runs the Python client's call sites, so without a
//! crossing like this one the Python and Rust halves could drift until a user found
//! out with a session that would not open.
//!
//! When the format changes on purpose: re-run the generator and commit whatever
//! moved. When it changes by accident, this fails first.

use clausters_document::NodeId;
use clausters_document::multitrack::*;
use clausters_document::timebase::{Beat, Second};

const VECTOR: &str = include_str!("multitrack_vector.json");

fn vector() -> Multitrack {
    serde_json::from_str(VECTOR).expect("the Python client's multitrack must parse here")
}

#[test]
fn the_clients_arrangement_parses_and_survives_a_round_trip() {
    let multitrack = vector();

    // Lossless rather than byte-identical: key order in JSON carries no
    // information, and the client writes what it holds in its own order.
    let out: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&multitrack).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
    assert_eq!(out, original);
}

#[test]
fn the_two_sides_agree_about_what_the_multitrack_is() {
    // Named one at a time, because a whole-value comparison that fails says
    // only that something moved.
    let multitrack = vector();
    assert_eq!(multitrack.tracks.len(), 3);
    assert_eq!(multitrack.end(), Second(48.0));
    assert_eq!(multitrack.tempo_at(Beat(40.0)).unwrap().tempo, 2.0);
    assert!(multitrack.tempo_at(Beat(40.0)).unwrap().ramp);
    assert_eq!(multitrack.meter_at(Beat(40.0)).unwrap().beats, 7);
    assert_eq!(multitrack.markers.len(), 2);
    assert_eq!(multitrack.loop_span.unwrap().length(), Second(32.0));
    assert_eq!(multitrack.punch.unwrap().start, Second(8.0));
}

#[test]
fn a_comped_track_keeps_every_take_and_plays_the_one_it_names() {
    let multitrack = vector();
    let vocals = multitrack.track(NodeId(10)).expect("the vocal track");
    assert_eq!(
        vocals.take_lanes.len(),
        3,
        "the takes nobody chose are kept"
    );
    assert_eq!(vocals.active, 1);
    assert_eq!(
        vocals.active_take_lane().unwrap().name.as_deref(),
        Some("take 2")
    );
    // Three windows, three sources, three identities.
    let sources: Vec<u64> = vocals
        .take_lanes
        .iter()
        .flat_map(|lane| lane.regions.iter())
        .map(|r| r.content.source().unwrap().0)
        .collect();
    assert_eq!(sources, vec![100, 101, 102]);
}

#[test]
fn an_overlap_keeps_its_crossfade_its_layer_and_its_playrate() {
    let multitrack = vector();
    let lane = &multitrack.track(NodeId(30)).unwrap().take_lanes[0];
    assert!(lane.regions[0].overlaps(&lane.regions[1]));
    assert_eq!(
        lane.regions[0].fade_out.as_ref().unwrap().length,
        Second(4.0)
    );
    let second = &lane.regions[1];
    assert_eq!(second.layer, 1, "which one is on top");
    assert!(second.muted);
    assert_eq!(
        second.fade_in.as_ref().unwrap().shape,
        clausters_core::envshape::SHAPE_EXPONENTIAL
    );
    let Content::Window { playrate, args, .. } = &second.content else {
        panic!("a window");
    };
    assert_eq!(*playrate, 1.5);
    assert_eq!(args.0["seed"], 7);
}

#[test]
fn a_region_keeps_a_field_a_newer_writer_added() {
    let multitrack = vector();
    let region = &multitrack.track(NodeId(40)).unwrap().take_lanes[0].regions[0];
    assert_eq!(region.content.source().map(|s| s.0), Some(200));
    assert_eq!(region.extra["warp"]["mode"], "beats");
}

#[test]
fn an_automation_curve_keeps_the_shapes_neither_side_reads() {
    let multitrack = vector();
    let curve = &multitrack.track(NodeId(30)).unwrap().automation[0];
    assert!(curve.visible && curve.enabled);
    assert_eq!(curve.target.0["ctl"], "level");
    assert_eq!(curve.points[1].data.0["shape"], "exp");
}

#[test]
fn a_region_carries_curves_of_its_own_and_they_are_not_its_tracks() {
    // The two places a curve belongs: a track's runs the length of the track
    // and is drawn in a row beside it, a region's runs the length of the
    // region and is drawn inside it. One type, so one reader -- which is what
    // this asserts, since the Python client wrote both through one class.
    let multitrack = vector();
    let region = &multitrack.track(NodeId(30)).unwrap().take_lanes[0].regions[0];
    let own = &region.automation[0];
    assert_eq!(own.id, NodeId(35));
    assert_eq!(own.target.0["ctl"], "gain");
    assert_eq!(own.points.len(), 2);
    assert_eq!(
        multitrack.automations().count(),
        2,
        "the track's and the region's, and one walk finds both"
    );
    assert_eq!(
        multitrack.automation(NodeId(35)).map(|a| a.id),
        Some(NodeId(35))
    );
}

#[test]
fn a_field_the_client_added_and_this_build_has_no_name_for_survives() {
    let multitrack = vector();
    assert_eq!(multitrack.extra["groove"]["name"], "mpc60");
    let region = &multitrack.track(NodeId(40)).unwrap().take_lanes[0].regions[0];
    assert_eq!(region.extra["warp"]["mode"], "beats");
}
