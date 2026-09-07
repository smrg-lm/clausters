//! The reconcile, driven the way a client drives it: two `/gui_def`s over one
//! window, with the host having moved things in between.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use crate::host::widget::WidgetKind;
use crate::host::{ClientId, GUI_DEF, Host};

fn from() -> ClientId {
    ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )))
}

fn define(host: &mut Host, json: &str) {
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        from(),
    );
}

/// A window of two lanes, the second one with a clip on it.
fn window(second_clip: &str) -> String {
    format!(
        r#"{{"type":"window","title":"w","children":[
             {{"id":10,"type":"field","label":"one","children":[
               {{"id":11,"type":"field","offset":0.0,"dur":4.0}}]}},
             {{"id":20,"type":"field","label":"two","children":[{second_clip}]}}]}}"#
    )
}

/// Where a lane's window on the time axis is, as the host holds it.
fn view(host: &Host, id: i32) -> (f64, f64) {
    let tree = host.window_def(1).expect("the window");
    let lane = tree.find(id).expect("the lane");
    let editor = lane.kind.editor().expect("a lane has editor chrome");
    (editor.x_start, editor.x_len)
}

fn selection(host: &Host, id: i32) -> (f64, f64) {
    let tree = host.window_def(1).unwrap();
    let editor = tree.find(id).unwrap().kind.editor().unwrap();
    (editor.sel_start, editor.sel_len)
}

/// Moves the window and the selection of a lane the way a hand does — through
/// the host's own state, reporting nothing, which is the whole reason a def
/// cannot be trusted to carry them back.
fn scroll_and_select(host: &mut Host, id: i32) {
    let tree = host.window_def_mut(1).unwrap();
    let editor = tree.find_mut(id).unwrap().kind.editor_mut().unwrap();
    editor.x_start = 1000.0;
    editor.x_len = 4000.0;
    editor.sel_start = 1500.0;
    editor.sel_len = 500.0;
}

#[test]
fn a_lane_that_survives_keeps_its_zoom_when_another_lane_changes() {
    // O23's acceptance, and the complaint this branch opened with: a clip
    // appearing in one lane used to take the zoom of every other lane with it.
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 10);

    // The second lane gains a clip. Everything else is redrawn as it was.
    define(
        &mut host,
        &window(r#"{"id":21,"type":"field","offset":8.0,"dur":4.0}"#),
    );

    assert_eq!(
        view(&host, 10),
        (1000.0, 4000.0),
        "the reader's window stands"
    );
    assert_eq!(selection(&host, 10), (1500.0, 500.0));
    let tree = host.window_def(1).unwrap();
    assert!(tree.find(21).is_some(), "and the new clip is there");
}

#[test]
fn a_def_that_states_a_window_moves_it() {
    // The other half of the rule, and the one that keeps a script able to drive
    // a view at all: nothing said is nothing written, so a key the def states is
    // the def's.
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 10);

    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":10,"type":"field","label":"one","view_start":0.0,"view_len":9000.0},
             {"id":20,"type":"field","label":"two"}]}"#,
    );
    assert_eq!(
        view(&host, 10),
        (0.0, 9000.0),
        "the def said, so the def wins"
    );
    assert_eq!(
        selection(&host, 10),
        (1500.0, 500.0),
        "and what it did not say is still the host's"
    );
}

#[test]
fn a_widget_that_is_new_takes_the_def_and_nothing_else() {
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 10);

    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":10,"type":"field"},
             {"id":30,"type":"field","view_start":7.0,"view_len":70.0}]}"#,
    );
    assert_eq!(view(&host, 30), (7.0, 70.0));
    assert_eq!(view(&host, 10), (1000.0, 4000.0));
}

#[test]
fn an_id_that_now_names_a_different_kind_carries_nothing() {
    // "Keeps too much" is the failure this design was warned about: a knob that
    // inherited a lane's zoom because it landed on the same id.
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 10);

    // The id now names a waveform, which has a window on a time axis of its
    // own -- so this is the case where carrying would actually land somewhere.
    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":10,"type":"waveform","data":[0.0,0.5,1.0]}]}"#,
    );
    let tree = host.window_def(1).unwrap();
    let now = tree.find(10).expect("the widget");
    assert!(
        !matches!(now.kind, WidgetKind::Track { .. }),
        "the id names something else now"
    );
    if let Some(editor) = now.kind.editor() {
        assert_eq!(
            (editor.x_start, editor.x_len),
            (0.0, 0.0),
            "and it did not inherit a lane's window"
        );
    }
}

#[test]
fn a_lane_that_moved_to_another_parent_keeps_its_own_state() {
    // Identity by id, wherever it moved to — which is the reason a reconcile
    // matches by id and not by path: re-parenting a clip is the multitrack's
    // most common gesture, and a path goes stale exactly there.
    let mut host = Host::new();
    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":5,"type":"layout","children":[{"id":10,"type":"field"}]},
             {"id":6,"type":"layout"}]}"#,
    );
    scroll_and_select(&mut host, 10);

    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":5,"type":"layout"},
             {"id":6,"type":"layout","children":[{"id":10,"type":"field"}]}]}"#,
    );
    assert_eq!(view(&host, 10), (1000.0, 4000.0));
}

#[test]
fn a_selection_a_marquee_left_survives_and_nothing_on_the_wire_sets_it() {
    // The per-widget mark has no wire key at all, so there is no "the def said
    // so" case for it: it is the host's, always.
    let mut host = Host::new();
    define(
        &mut host,
        &window(r#"{"id":21,"type":"field","offset":8.0,"dur":4.0}"#),
    );
    host.window_def_mut(1)
        .unwrap()
        .find_mut(21)
        .unwrap()
        .selected = true;

    define(
        &mut host,
        &window(r#"{"id":21,"type":"field","offset":9.0,"dur":4.0}"#),
    );
    assert!(host.window_def(1).unwrap().find(21).unwrap().selected);
}

#[test]
fn a_subtree_spliced_in_place_reconciles_against_what_was_there() {
    // The other entrance: a def of a widget *inside* an open window. It splices
    // rather than rebuilding the window, and it has to keep the same things.
    let mut host = Host::new();
    define(
        &mut host,
        &window(r#"{"id":21,"type":"field","offset":8.0,"dur":4.0}"#),
    );
    scroll_and_select(&mut host, 20);

    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(20),
                OscType::String(
                    r#"{"type":"field","label":"two","children":[
                         {"id":21,"type":"field","offset":8.0,"dur":4.0},
                         {"id":22,"type":"field","offset":16.0,"dur":4.0}]}"#
                        .into(),
                ),
            ],
        }),
        from(),
    );
    assert_eq!(view(&host, 20), (1000.0, 4000.0));
    assert!(host.window_def(1).unwrap().find(22).is_some());
}
