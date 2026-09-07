//! The arrangement this crate defines is the one the Python client writes.
//!
//! The crate's own suite proves the shape round-trips against itself, which
//! says nothing about whether anyone else agrees with it. This does: the vector
//! beside it is an arrangement built with `clausters.arrangement`, as that
//! module writes it (`gen-arrangement-vector.py` writes the file, and it is
//! committed). Nothing in CI runs the Python client's call sites, so without a
//! crossing like this one the two halves of O21 could drift until a user found
//! out with a session that would not open.
//!
//! When the format changes on purpose: re-run the generator and commit whatever
//! moved. When it changes by accident, this fails first.

use clausters_document::arrangement::*;
use clausters_document::timebase::Beat;

const VECTOR: &str = include_str!("arrangement_vector.json");

fn vector() -> Arrangement {
    serde_json::from_str(VECTOR).expect("the Python client's arrangement must parse here")
}

#[test]
fn the_clients_arrangement_parses_and_survives_a_round_trip() {
    let piece = vector();

    // Lossless rather than byte-identical: key order in JSON carries no
    // information, and the client writes what it holds in its own order.
    let out: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&piece).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
    assert_eq!(out, original);
}

#[test]
fn the_two_sides_agree_about_what_the_piece_is() {
    // Named one at a time, because a whole-value comparison that fails says
    // only that something moved.
    let piece = vector();
    assert_eq!(piece.tracks.len(), 3);
    assert_eq!(piece.end(), Beat(48.0));
    assert_eq!(piece.tempo_at(Beat(40.0)).unwrap().bpm, 120.0);
    assert!(piece.tempo_at(Beat(40.0)).unwrap().ramp);
    assert_eq!(piece.meter_at(Beat(40.0)).unwrap().beats, 7);
    assert_eq!(piece.markers.len(), 2);
    assert_eq!(piece.loop_span.unwrap().length(), Beat(32.0));
    assert_eq!(piece.punch.unwrap().start, Beat(8.0));
}

#[test]
fn a_comped_track_keeps_every_take_and_plays_the_one_it_names() {
    let piece = vector();
    let vocals = piece.track(NodeId(10)).expect("the vocal track");
    assert_eq!(vocals.lanes.len(), 3, "the takes nobody chose are kept");
    assert_eq!(vocals.active, 1);
    assert_eq!(
        vocals.active_lane().unwrap().name.as_deref(),
        Some("take 2")
    );
    // Three windows, three sources, three identities.
    let sources: Vec<u64> = vocals
        .lanes
        .iter()
        .flat_map(|lane| lane.regions.iter())
        .map(|r| {
            r.content
                .as_window()
                .unwrap()
                .source
                .samples()
                .unwrap()
                .source
                .0
        })
        .collect();
    assert_eq!(sources, vec![100, 101, 102]);
}

#[test]
fn an_overlap_keeps_its_crossfade_its_layer_and_its_playrate() {
    let piece = vector();
    let lane = &piece.track(NodeId(30)).unwrap().lanes[0];
    assert!(lane.regions[0].overlaps(&lane.regions[1]));
    assert_eq!(lane.regions[0].fade_out.as_ref().unwrap().length, Beat(4.0));
    let second = &lane.regions[1];
    assert_eq!(second.layer, 1, "which one is on top");
    assert!(second.muted);
    assert_eq!(second.fade_in.as_ref().unwrap().shape.0["curve"], "exp");
    let Content::Window { playrate, args, .. } = &second.content else {
        panic!("a window");
    };
    assert_eq!(*playrate, 1.5);
    assert_eq!(args.0["seed"], 7);
}

#[test]
fn a_composite_region_arrives_as_the_general_tree() {
    let piece = vector();
    let region = &piece.track(NodeId(40)).unwrap().lanes[0].regions[0];
    let node = region.content.as_node().expect("the tree, placed");
    assert_eq!(node.id, NodeId(43));
    assert!(matches!(node.body, Body::Aggregate { .. }));
}

#[test]
fn an_automation_curve_keeps_the_shapes_neither_side_reads() {
    let piece = vector();
    let curve = &piece.track(NodeId(30)).unwrap().automation[0];
    assert!(curve.visible && curve.enabled);
    assert_eq!(curve.target.0["ctl"], "level");
    assert_eq!(curve.points[1].data.0["shape"], "exp");
}

#[test]
fn a_field_the_client_added_and_this_build_has_no_name_for_survives() {
    let piece = vector();
    assert_eq!(piece.extra["groove"]["name"], "mpc60");
    let region = &piece.track(NodeId(40)).unwrap().lanes[0].regions[0];
    assert_eq!(region.extra["warp"]["mode"], "beats");
}

use clausters_document::{Body, Lifetime, Location, NodeId, Session, SourceId};

// ---- the session: the piece, and where its samples are ----

const SESSION: &str = include_str!("arrangement_session_vector.json");

fn saved() -> Session {
    serde_json::from_str(SESSION).expect("the Python client's session must parse here")
}

#[test]
fn the_clients_session_parses_and_survives_a_round_trip() {
    let session = saved();
    let out: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(SESSION).unwrap();
    assert_eq!(out, original);
}

#[test]
fn a_source_table_written_there_reads_as_sources_here() {
    let session = saved();
    assert_eq!(session.sources.len(), 6);
    let take = session
        .source(SourceId(100))
        .expect("a file the table locates");
    assert!(matches!(&take.location, Location::File { path } if path == "takes/100.wav"));
    assert_eq!(take.lifetime, Lifetime::Session);
    assert_eq!(take.channels, Some(2));
    assert_eq!(take.sample_rate, Some(48_000.0));
    // Carried and never interpreted: what produced these samples.
    assert_eq!(
        session
            .source(SourceId(200))
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .0["def"],
        "sines"
    );
}

#[test]
fn a_save_that_cannot_promise_everything_says_which_part() {
    // The three states a table has to be able to hold, each read back as
    // itself: a file that is there, samples nobody wrote down, and a working
    // copy whose destructive edit is still open.
    let session = saved();
    assert_eq!(session.volatile(), vec![SourceId(201)]);
    assert_eq!(session.open_edits(), vec![SourceId(300)]);
    assert_eq!(
        session.source(SourceId(300)).unwrap().lifetime,
        Lifetime::Temporary
    );
    assert!(session.is_readable());
}

#[test]
fn the_piece_inside_the_session_is_the_same_piece() {
    let session = saved();
    assert_eq!(session.arrangement, vector());
    // ...and the table covers what it plays.
    assert_eq!(session.dangling(), Vec::<SourceId>::new());
}

// ---- the presentation, which is parallel to the piece and never inside it ----

#[test]
fn the_session_carries_two_views_of_one_piece_and_they_disagree_on_purpose() {
    // The prerequisite `O23` asked for, crossing: screen state written by the
    // Python client, parsed here, and read back by the web client. What makes
    // it worth a vector is that a view is *not* the piece -- a reader that
    // dropped the field would open the same music and lose the window.
    let session = saved();
    assert_eq!(
        session.views.len(),
        2,
        "a piece in two windows has two views"
    );

    let arranger = &session.views[0];
    assert_eq!(arranger.name.as_deref(), Some("arranger"));
    assert_eq!(arranger.visible.unwrap().length(), Beat(48.0));
    assert_eq!(arranger.quant, Beat(4.0));
    assert!(arranger.autofit, "the default, and left out of the file");
    assert_eq!(arranger.selected, vec![NodeId(20), NodeId(32)]);
    assert_eq!(arranger.focused, Some(NodeId(20)));
    assert_eq!(arranger.track(NodeId(10)).height, Some(96.0));
    assert!(arranger.track(NodeId(10)).lanes_shown, "comping open");
    assert_eq!(arranger.track(NodeId(30)).color.as_deref(), Some("#4488cc"));
    assert_eq!(arranger.lane(NodeId(12)).height, Some(32.0));
    assert_eq!(
        arranger.extra["fold"], "tracks",
        "a newer window's own state"
    );

    let editor = &session.views[1];
    assert_eq!(editor.visible.unwrap().start, Beat(8.0));
    assert_eq!(editor.quant, Beat(0.25), "the same piece, a finer grid");
    assert!(!editor.autofit, "an editor's window is the reader's");
    assert_eq!(editor.scroll, 140.0);
    assert_eq!(editor.selection.unwrap().length(), Beat(4.0));
    assert_eq!(editor.detail, Some(NodeId(42)));
}

#[test]
fn a_view_says_nothing_about_what_plays() {
    // The whole argument for parallel rather than a field on the model: drop
    // every view and the piece is the same piece, byte for byte.
    let mut session = saved();
    let piece = serde_json::to_value(&session.arrangement).unwrap();
    session.views.clear();
    assert_eq!(serde_json::to_value(&session.arrangement).unwrap(), piece);
    assert_eq!(session.arrangement, vector());
}

#[test]
fn a_view_of_a_track_that_is_gone_goes_with_it() {
    // State goes when the thing goes. Pruning against a piece that no longer
    // holds the guitars drops their height, their colour and the selection that
    // named their region -- and leaves everything the piece still holds.
    let session = saved();
    let mut view = session.views[0].clone();
    let mut piece = session.arrangement.clone();
    piece.tracks.retain(|t| t.id != NodeId(30));

    assert!(view.prune(&piece));
    assert!(view.tracks.contains_key(&NodeId(10)), "the vocals stay");
    assert!(!view.tracks.contains_key(&NodeId(30)), "the guitars go");
    assert_eq!(
        view.selected,
        vec![NodeId(20)],
        "and so does what named them"
    );
    assert_eq!(view.focused, Some(NodeId(20)));
}
