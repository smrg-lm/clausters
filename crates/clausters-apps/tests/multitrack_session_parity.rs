//! The multitrack's session file this crate reads is the one the Python
//! client writes.
//!
//! The vector beside it is a session saved by `clausters.gui.editing`'s
//! `MultitrackSession` (`crates/clausters-document/tests/gen-multitrack-vector.py`
//! writes it, with the multitrack vector the document's own parity test
//! reads, and both are committed). Nothing in CI runs the Python client's call
//! sites, so without a crossing like this one the two halves could drift
//! until a session would not open.

const VECTOR: &str = include_str!("../../clausters-document/tests/multitrack_vector.json");

fn vector() -> Multitrack {
    serde_json::from_str(VECTOR).expect("the Python client's multitrack must parse here")
}

use clausters_apps::multitrack::session::MultitrackSession;
use clausters_document::multitrack::Multitrack;
use clausters_document::timebase::{Beat, Second};
use clausters_document::{Lifetime, Location, NodeId, SourceId};

// ---- the session: the multitrack, and where its samples are ----

const SESSION: &str = include_str!("multitrack_session_vector.json");

fn saved() -> MultitrackSession {
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

/// **Where a pass ends is the session's**, in the three forms a playback
/// takes it: absent or `null` for one that rolls on, `"contents"`, a number
/// of seconds for an end marker -- and a word that names none is refused.
#[test]
fn a_session_keeps_where_a_pass_ends() {
    use clausters_document::End;
    assert_eq!(saved().end, End::Contents, "as the client wrote it");
    let read = |end: &str| {
        serde_json::from_str::<MultitrackSession>(&format!(r#"{{"format": 5, "end": {end}}}"#))
            .map(|session| session.end)
    };
    assert_eq!(read("null").unwrap(), End::Open);
    assert_eq!(read("12.5").unwrap(), End::At(12.5));
    assert!(read(r#""later""#).is_err());
    let open: MultitrackSession = serde_json::from_str(r#"{"format": 5}"#).unwrap();
    assert_eq!(open.end, End::Open);
    assert!(
        serde_json::to_value(&open).unwrap().get("end").is_none(),
        "the default is not written"
    );
    // A marker the contents grow past goes with them; one inside them stays.
    assert_eq!(End::At(10.0).carried(8.0, 12.0), End::At(12.0));
    assert_eq!(End::At(10.0).carried(8.0, 9.0), End::At(10.0));
    assert_eq!(End::At(3.0).carried(8.0, 12.0), End::At(3.0));
    assert_eq!(End::Contents.carried(8.0, 12.0), End::Contents);
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
fn the_multitrack_inside_the_session_is_the_same_multitrack() {
    let session = saved();
    assert_eq!(session.multitrack, vector());
    // ...and the table covers what it plays.
    assert_eq!(session.dangling(), Vec::<SourceId>::new());
}

// ---- the presentation, which is parallel to the multitrack and never inside it ----

#[test]
fn the_session_carries_two_views_of_one_multitrack_and_they_disagree_on_purpose() {
    // Screen state crossing both clients: written by the
    // Python client, parsed here, and read back by the web client. What makes
    // it worth a vector is that a view is *not* the multitrack -- a reader that
    // dropped the field would open the same music and lose the window.
    let session = saved();
    assert_eq!(
        session.views.len(),
        2,
        "a multitrack in two windows has two views"
    );

    let arranger = &session.views[0];
    assert_eq!(arranger.name.as_deref(), Some("arranger"));
    assert_eq!(arranger.visible.unwrap().length(), Second(48.0));
    assert_eq!(arranger.quant, Beat(4.0));
    assert!(arranger.autofit, "the default, and left out of the file");
    assert_eq!(arranger.selected, vec![NodeId(20), NodeId(32)]);
    assert_eq!(arranger.focused, Some(NodeId(20)));
    assert_eq!(arranger.track(NodeId(10)).height, Some(96.0));
    assert!(arranger.track(NodeId(10)).take_lanes_shown, "comping open");
    assert_eq!(arranger.track(NodeId(30)).color.as_deref(), Some("#4488cc"));
    assert_eq!(arranger.take_lane(NodeId(12)).height, Some(32.0));
    assert_eq!(
        arranger.extra["fold"], "tracks",
        "a newer window's own state"
    );

    let editor = &session.views[1];
    assert_eq!(editor.visible.unwrap().start, Second(8.0));
    assert_eq!(
        editor.quant,
        Beat(0.25),
        "the same multitrack, a finer grid"
    );
    assert!(!editor.autofit, "an editor's window is the reader's");
    assert_eq!(editor.scroll, 140.0);
    assert_eq!(editor.selection.unwrap().length(), Second(4.0));
    assert_eq!(editor.detail, Some(NodeId(42)));
}

#[test]
fn a_view_says_nothing_about_what_plays() {
    // The whole argument for parallel rather than a field on the model: drop
    // every view and the multitrack is the same multitrack, byte for byte.
    let mut session = saved();
    let multitrack = serde_json::to_value(&session.multitrack).unwrap();
    session.views.clear();
    assert_eq!(
        serde_json::to_value(&session.multitrack).unwrap(),
        multitrack
    );
    assert_eq!(session.multitrack, vector());
}

#[test]
fn a_view_of_a_track_that_is_gone_goes_with_it() {
    // State goes when the thing goes. Pruning against a multitrack that no longer
    // holds the guitars drops their height, their colour and the selection that
    // named their region -- and leaves everything the multitrack still holds.
    let session = saved();
    let mut view = session.views[0].clone();
    let mut multitrack = session.multitrack.clone();
    multitrack.tracks.retain(|t| t.id != NodeId(30));

    assert!(view.prune(&multitrack));
    assert!(view.tracks.contains_key(&NodeId(10)), "the vocals stay");
    assert!(!view.tracks.contains_key(&NodeId(30)), "the guitars go");
    assert_eq!(
        view.selected,
        vec![NodeId(20)],
        "and so does what named them"
    );
    assert_eq!(view.focused, Some(NodeId(20)));
}
