use super::*;
use crate::NodeId;
use crate::multitrack::{Content, Lane, Region, Track};
use crate::timebase::Second;

fn multitrack() -> Multitrack {
    let mut vocals = Track::new(NodeId(10), NodeId(11)).named("vocals");
    vocals.lanes.push(Lane::new(NodeId(12)));
    vocals.lanes[0].place(Region::new(
        NodeId(100),
        Second(0.0),
        Second(4.0),
        Content::Composite {
            node: Box::new(crate::Node::new(
                NodeId(1),
                crate::Body::Aggregate {
                    grouping: crate::Grouping::Concrete,
                    members: Vec::new(),
                    config: crate::Opaque::none(),
                },
            )),
        },
    ));
    let mut multitrack = Multitrack::new();
    multitrack.tracks = vec![vocals, Track::new(NodeId(20), NodeId(21))];
    multitrack
}

#[test]
fn a_view_that_says_nothing_writes_an_empty_object() {
    // Nothing derived, nothing defaulted, nothing invented -- the arrangement's
    // rule, and a view of a multitrack nobody has touched costs a file two braces.
    assert_eq!(serde_json::to_string(&View::new()).unwrap(), "{}");
}

#[test]
fn the_multitrack_round_trips_the_same_whether_or_not_a_view_of_it_exists() {
    // The whole argument for parallel rather than a field: a reader that wants
    // the multitrack reads the multitrack, and nothing in the view can reach it.
    let multitrack = multitrack();
    let written = serde_json::to_value(&multitrack).unwrap();

    let mut view = View::new().named("arranger");
    view.visible = Some(Span::new(Second(0.0), Second(32.0)));
    view.track_mut(NodeId(10)).height = Some(96.0);
    view.selected = vec![NodeId(100)];

    let session = crate::Session::new(crate::Document::empty())
        .with_multitrack(multitrack.clone())
        .with_view(view);
    let back: crate::Session =
        serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
    assert_eq!(back.multitrack, multitrack);
    assert_eq!(serde_json::to_value(&back.multitrack).unwrap(), written);
    assert_eq!(back.views.len(), 1);
    assert_eq!(back.views[0].name.as_deref(), Some("arranger"));
    assert_eq!(back.views[0].track(NodeId(10)).height, Some(96.0));
}

#[test]
fn two_windows_over_one_multitrack_are_two_views_and_disagree_on_purpose() {
    let arranger = {
        let mut v = View::new().named("arranger");
        v.visible = Some(Span::new(Second(0.0), Second(64.0)));
        v.quant = Beat(4.0);
        v
    };
    let editor = {
        let mut v = View::new().named("editor");
        v.visible = Some(Span::new(Second(8.0), Second(12.0)));
        v.quant = Beat(0.25);
        v.detail = Some(NodeId(100));
        v
    };
    let session = crate::Session::new(crate::Document::empty())
        .with_multitrack(multitrack())
        .with_view(arranger)
        .with_view(editor);
    let back: crate::Session =
        serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
    assert_eq!(back.views.len(), 2);
    assert_eq!(back.views[0].quant, Beat(4.0));
    assert_eq!(back.views[1].quant, Beat(0.25));
    assert_eq!(back.views[1].detail, Some(NodeId(100)));
}

#[test]
fn a_track_nobody_touched_reads_as_the_default_and_costs_nothing_to_store() {
    let mut view = View::new();
    assert_eq!(view.track(NodeId(10)), TrackView::default());
    assert!(view.tracks.is_empty(), "asking is not touching");
    view.track_mut(NodeId(10)).lanes_shown = true;
    assert_eq!(view.tracks.len(), 1);
}

#[test]
fn state_goes_when_the_thing_goes() {
    // The rule the client's screen-state tables were fixed to obey, in this
    // structure's terms. Keeping it is worse than losing it: a height kept for
    // a track that is not the same track is a defect that looks like a feature.
    let mut view = View::new();
    view.track_mut(NodeId(10)).height = Some(96.0);
    view.track_mut(NodeId(999)).height = Some(48.0);
    view.lane_mut(NodeId(12)).height = Some(24.0);
    view.lane_mut(NodeId(888)).height = Some(24.0);
    view.selected = vec![NodeId(100), NodeId(777)];
    view.focused = Some(NodeId(777));
    view.detail = Some(NodeId(100));

    assert!(view.prune(&multitrack()));
    assert_eq!(
        view.tracks.keys().copied().collect::<Vec<_>>(),
        vec![NodeId(10)]
    );
    assert_eq!(
        view.lanes.keys().copied().collect::<Vec<_>>(),
        vec![NodeId(12)]
    );
    assert_eq!(
        view.selected,
        vec![NodeId(100)],
        "the region that is still there"
    );
    assert_eq!(view.focused, None, "and nothing points at what is gone");
    assert_eq!(view.detail, Some(NodeId(100)));
    assert!(
        !view.prune(&multitrack()),
        "and pruning twice finds nothing to do"
    );
}

#[test]
fn a_field_a_newer_writer_added_survives_the_round_trip() {
    // The same door every struct in this format has: what cannot be interpreted
    // is carried. A newer window's own state must not be lost by an older one
    // opening the session and saving it again.
    let json = r#"{"name":"arranger","fold":"tracks",
                   "tracks":{"10":{"height":96.0,"waveform":"rectified"}}}"#;
    let view: View = serde_json::from_str(json).unwrap();
    assert_eq!(view.extra["fold"], "tracks");
    assert_eq!(view.tracks[&NodeId(10)].extra["waveform"], "rectified");
    let back = serde_json::to_value(&view).unwrap();
    assert_eq!(back["fold"], "tracks");
    assert_eq!(back["tracks"]["10"]["waveform"], "rectified");
}

#[test]
fn a_session_written_without_views_reads_back_without_them() {
    let session = crate::Session::new(crate::Document::empty()).with_multitrack(multitrack());
    let written = serde_json::to_value(&session).unwrap();
    assert!(
        written.get("views").is_none(),
        "nothing said is nothing written"
    );
    let back: crate::Session = serde_json::from_value(written).unwrap();
    assert!(back.views.is_empty());
}

#[test]
fn the_routing_table_names_screen_state_and_nothing_a_domain_reads() {
    for tag in super::NOT_AN_EDIT {
        assert!(super::is_screen_state(tag), "{tag}");
    }
    for tag in ["clips", "lanes", "notes", "points", "samples", "level"] {
        assert!(!super::is_screen_state(tag), "{tag} is an edit");
    }
}

#[test]
fn the_routing_table_says_each_tag_once() {
    let mut seen = super::NOT_AN_EDIT.to_vec();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(seen.len(), before);
}
