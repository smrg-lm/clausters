//! The reconcile, driven the way a client drives it: two `/gui_def`s over one
//! window, with the host having moved things in between.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

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

/// A window of two multitracks -- the view whose axis a hand moves, and the one
/// a reconcile has to keep. `boxes` is the second one's `clips` prop.
fn window(boxes: &str) -> String {
    format!(
        r#"{{"type":"window","title":"w","children":[
             {{"id":10,"type":"multitrack","label":"one",
               "tracks":["a","",96.0,0,0,1.0,1],"clips":["c","a",0.0,4.0,0.0,"",-1]}},
             {{"id":20,"type":"multitrack","label":"two",
               "tracks":["b","",96.0,0,0,1.0,1]{boxes}}}]}}"#
    )
}

/// Where a view's window on the time axis is, as the host holds it.
fn view(host: &Host, id: i32) -> (f64, f64) {
    let tree = host.window_def(1).expect("the window");
    let w = tree.find(id).expect("the widget");
    let editor = w.kind.editor().expect("a timeline view has editor chrome");
    (editor.x_start, editor.x_len)
}

fn selection(host: &Host, id: i32) -> (f64, f64) {
    let tree = host.window_def(1).unwrap();
    let editor = tree.find(id).unwrap().kind.editor().unwrap();
    (editor.sel_start, editor.sel_len)
}

/// Moves the window and the selection of a view the way a hand does -- through
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
fn a_view_that_survives_keeps_its_zoom_when_another_one_changes() {
    // The reconcile's acceptance, and the complaint it opened with: a clip
    // appearing in one view used to take the zoom of every other one with it.
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 10);

    // The second view gains a box. Everything else is redrawn as it was.
    define(
        &mut host,
        &window(r#","clips":["d","b",8.0,4.0,0.0,"",-1]"#),
    );

    assert_eq!(
        view(&host, 10),
        (1000.0, 4000.0),
        "the reader's window stands"
    );
    assert_eq!(selection(&host, 10), (1500.0, 500.0));
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
             {"id":10,"type":"multitrack","view_start":0.0,"view_len":9000.0},
             {"id":20,"type":"multitrack"}]}"#,
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
             {"id":10,"type":"multitrack"},
             {"id":30,"type":"multitrack","view_start":7.0,"view_len":70.0}]}"#,
    );
    assert_eq!(view(&host, 30), (7.0, 70.0));
    assert_eq!(view(&host, 10), (1000.0, 4000.0));
}

#[test]
fn an_id_that_now_names_a_different_kind_carries_nothing() {
    // "Keeps too much" is the failure this design was warned about: a knob that
    // inherited a view's zoom because it landed on the same id.
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 10);

    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":10,"type":"knob","min":0.0,"max":1.0}]}"#,
    );
    let tree = host.window_def(1).unwrap();
    let now = tree.find(10).expect("the widget");
    assert!(
        now.kind.editor().is_none(),
        "the id names something with no time axis now"
    );
}

#[test]
fn a_view_that_moved_to_another_parent_keeps_its_own_state() {
    // Identity by id, wherever it moved to -- which is the reason a reconcile
    // matches by id and not by path: a path goes stale the moment a window is
    // rearranged, and a widget's screen state must not go with it.
    let mut host = Host::new();
    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":5,"type":"layout","children":[{"id":10,"type":"multitrack"}]},
             {"id":6,"type":"layout"}]}"#,
    );
    scroll_and_select(&mut host, 10);

    define(
        &mut host,
        r#"{"type":"window","title":"w","children":[
             {"id":5,"type":"layout"},
             {"id":6,"type":"layout","children":[{"id":10,"type":"multitrack"}]}]}"#,
    );
    assert_eq!(view(&host, 10), (1000.0, 4000.0));
}

#[test]
fn a_subtree_spliced_in_place_reconciles_against_what_was_there() {
    // The other entrance: a def of a widget *inside* an open window. It splices
    // rather than rebuilding the window, and it has to keep the same things.
    let mut host = Host::new();
    define(&mut host, &window(""));
    scroll_and_select(&mut host, 20);

    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(20),
                OscType::String(
                    r#"{"type":"multitrack","label":"two",
                         "tracks":["b","",96.0,0,0,1.0,1],
                         "clips":["d","b",8.0,4.0,0.0,"",-1]}"#
                        .into(),
                ),
            ],
        }),
        from(),
    );
    assert_eq!(view(&host, 20), (1000.0, 4000.0));
}

// ---- the bulk: `data: keep`, which is the largest payload on the wire ----

#[test]
fn a_standalone_signal_keeps_its_own_bulk() {
    // The word is not the clip's: a waveform is where the payload is largest,
    // and it reaches the same door through its own props rather than through a
    // container's.
    let hold = |data: &str| {
        format!(
            r#"{{"type":"window","title":"w","children":[
                 {{"id":30,"type":"signal","view":"trace","data":{data},"channels":1}}]}}"#
        )
    };
    let mut host = Host::new();
    define(&mut host, &hold("[0.5,-0.5,0.25,-0.25]"));
    define(&mut host, &hold("\"keep\""));

    let tree = host.window_def(1).unwrap();
    let el = tree.find(30).unwrap().kind.signal().expect("the waveform");
    match &el.source {
        crate::host::elements::signal::Source::Data(d) => {
            assert_eq!(d.samples.to_vec(), vec![0.5, -0.5, 0.25, -0.25]);
        }
        _ => panic!("a stored source"),
    }
}

// ---- a server buffer's summary: the host's own download ----

/// A window of one signal drawing server buffer `buffer`, viewed as `view`.
fn buffer_view(buffer: i32, view: &str) -> String {
    format!(
        r#"{{"type":"window","title":"w","children":[
             {{"id":40,"type":"signal","view":"{view}","buffer":{buffer},"channels":1}}]}}"#
    )
}

/// Hands element 40 a resolved summary, as a download finishing does.
fn resolve(host: &mut Host) {
    let data = std::sync::Arc::new(crate::waveform::WaveformData::from_interleaved(
        &[0.5, -0.5, 0.25, -0.25],
        1,
        2,
    ));
    let tree = host.window_def_mut(1).unwrap();
    let took = tree
        .find_mut(40)
        .unwrap()
        .take_bulk(|| crate::host::widget::element::Loaded::Peaks(data.clone()));
    assert!(took, "the element takes the summary home");
}

fn summary(host: &Host) -> bool {
    let tree = host.window_def(1).unwrap();
    match &tree.find(40).unwrap().kind.signal().unwrap().source {
        crate::host::elements::signal::Source::Data(d) => d.body.is_some(),
        _ => panic!("a stored source"),
    }
}

#[test]
fn a_redefine_over_the_same_buffer_keeps_what_the_host_loaded() {
    // A view switch redefines the window. A page downloads a buffer's
    // summary, so forgetting it there meant every switch asked the server for
    // every drawn take again and drew a frame for each reply.
    let mut host = Host::new();
    define(&mut host, &buffer_view(7, "trace"));
    resolve(&mut host);
    define(&mut host, &buffer_view(7, "spectrogram"));
    assert!(summary(&host), "the same buffer is the same samples");
    let want = host
        .window_def_mut(1)
        .unwrap()
        .find_mut(40)
        .unwrap()
        .kind
        .needs()
        .bulk;
    assert!(want.is_none(), "and nothing is asked for again: {want:?}");

    // Another buffer is another picture.
    define(&mut host, &buffer_view(8, "spectrogram"));
    assert!(!summary(&host), "a different buffer starts empty");
}

/// Whether the take of widget 40 is owed a read -- what a front's bulk walk
/// asks the server for next.
fn rereads(host: &mut Host) -> bool {
    host.window_def_mut(1)
        .and_then(|t| t.find_mut(40))
        .and_then(|w| w.kind.as_samples_mut())
        .is_some_and(|s| s.wants_reload().is_some())
}

#[test]
fn a_take_told_to_read_again_keeps_its_picture_and_asks() {
    // `reload` is the owner saying the samples changed under the same
    // buffer -- an undo over it. The picture stays drawn until the new one
    // has come, and the read is owed.
    let mut host = Host::new();
    define(&mut host, &buffer_view(7, "trace"));
    resolve(&mut host);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_SET.into(),
            args: vec![
                OscType::Int(40),
                OscType::String("reload".into()),
                OscType::Int(1),
            ],
        }),
        from(),
    );
    define(&mut host, &buffer_view(7, "trace"));
    assert!(summary(&host), "the picture before the edit stays drawn");
    assert!(rereads(&mut host), "re-read, it asks again");
}

#[test]
fn a_redefine_that_says_reload_keeps_its_picture_and_asks() {
    // The owner can say it in the def itself: the same buffer, read again.
    let mut host = Host::new();
    define(&mut host, &buffer_view(7, "trace"));
    resolve(&mut host);
    define(
        &mut host,
        &buffer_view(7, "trace").replace(r#""channels":1"#, r#""channels":1,"reload":true"#),
    );
    assert!(
        summary(&host),
        "the picture stays until the new one has come"
    );
    assert!(rereads(&mut host), "the def said the samples changed");
}

#[test]
fn a_widget_inside_a_renumbered_container_still_carries() {
    // A client names the id-less nodes it sends, afresh on every def: the
    // layout around a take comes back under a new id each time, and the take
    // inside it under its own. The walk goes through the stranger.
    let wrapped = |layout: i32, view: &str| {
        format!(
            r#"{{"type":"window","title":"w","children":[
                 {{"id":{layout},"type":"layout","children":[
                   {{"id":40,"type":"signal","view":"{view}","buffer":7,"channels":1}}]}}]}}"#
        )
    };
    let mut host = Host::new();
    define(&mut host, &wrapped(500, "trace"));
    resolve(&mut host);
    define(&mut host, &wrapped(501, "spectrogram"));
    assert!(
        summary(&host),
        "the take kept its summary through a new container"
    );
}

#[test]
fn turning_a_resolved_buffer_into_another_picture_keeps_it() {
    // A view switch also sets the take's layers. A server buffer is one
    // resource in every form, so turning the texture on rebuilds the picture
    // from what is held instead of forgetting it and downloading it again.
    let mut host = Host::new();
    define(&mut host, &buffer_view(7, "trace"));
    resolve(&mut host);
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_SET.into(),
            args: vec![
                OscType::Int(40),
                OscType::String("measure".into()),
                OscType::String("spectrogram".into()),
            ],
        }),
        from(),
    );
    assert!(summary(&host), "the summary stands");
}
