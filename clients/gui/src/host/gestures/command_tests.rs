//! **Commands**: a tool's verb, a menu entry's, and an element's own context
//! menu -- each performed as the key bound to it would be, on the view the
//! window's focus or its `main` names.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use clausters_editing::verbs::ActionTarget;

use super::super::interact;
use super::super::layout::Rect;
use super::super::menu::EntryKind;
use super::super::popup::Owner;
use super::super::widget::WidgetKind;
use super::super::widget::element::Key;
use super::super::{ClientId, GUI_DEF, Host};
use super::*;

fn host_from(json: &str) -> Host {
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    host
}

fn ctx() -> GestureCtx {
    GestureCtx::new(1, 800, 400)
}

fn rect_of(host: &Host, ctx: &GestureCtx, id: i32) -> Rect {
    host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)
        .unwrap()
        .iter()
        .find(|p| p.widget.id == Some(id))
        .expect("the widget is placed")
        .rect
}

fn mid(r: Rect) -> (f64, f64) {
    ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64)
}

fn click(
    g: &mut Gestures,
    host: &mut Host,
    ctx: &GestureCtx,
    at: (f64, f64),
) -> Vec<GestureEffect> {
    let mut out = g.press(host, ctx, at.0, at.1);
    out.extend(g.release(host, ctx, at.0, at.1));
    out
}

fn emitted(effects: &[GestureEffect]) -> Vec<(i32, Vec<OscType>)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } => Some((*widget_id, args.clone())),
            _ => None,
        })
        .collect()
}

fn selected_notes(host: &Host, id: i32) -> Vec<usize> {
    let WidgetKind::Custom(el) = &host.window_def(1).unwrap().find(id).unwrap().kind else {
        panic!("not an element")
    };
    el.as_any()
        .and_then(|a| a.downcast_ref::<crate::host::elements::notes::Notes>())
        .expect("a roll")
        .selected()
        .to_vec()
}

/// A roll the window names as its `main` view, a tool beside it, and a bar.
const ROLL: &str = r#"{"type":"window","margin":0,"flow":"col","main":90,
    "menu":[{"label":"Edit","menu":[{"label":"Select all","verb":"select_all"},
                                    {"label":"Save","verb":"save"}]}],
    "children":[
      {"id":5,"type":"button","flat":true,"verb":"select_all","label":"All"},
      {"id":90,"type":"notes","min":48.0,"max":72.0,"weight":1,
       "notes":[0.0,400.0,60.0,100,0, 6000.0,400.0,61.0,100,0]}]}"#;

/// **A tool performs its verb on the window's `main` view**, before a hand
/// has been in it, and says nothing of its own: no press, no click, and the
/// focus is not taken from what the tool acts on.
#[test]
fn a_tool_performs_its_verb_on_the_window_s_main_view() {
    let mut host = host_from(ROLL);
    let mut g = Gestures::default();
    let ctx = ctx();
    assert!(selected_notes(&host, 90).is_empty());
    let at = mid(rect_of(&host, &ctx, 5));
    let effects = click(&mut g, &mut host, &ctx, at);
    assert_eq!(selected_notes(&host, 90), vec![0, 1], "every note, held");
    assert!(
        emitted(&effects).iter().all(|(id, _)| *id != 5),
        "a tool reports nothing of its own: {effects:?}"
    );
    assert_ne!(host.focused(), Some((1, 5)), "a tool takes no focus");
}

/// ...and a press on it leaves the focus where it was: what it acts on is
/// what the focus is on.
#[test]
fn a_press_on_a_tool_leaves_the_focus_where_it_was() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"button","flat":true,"verb":"select_all","label":"All"},
            {"id":6,"type":"text","h":32}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 6));
    click(&mut g, &mut host, &ctx, at);
    let focused = host.focused();
    assert_eq!(focused, Some((1, 6)));
    let at = mid(rect_of(&host, &ctx, 5));
    click(&mut g, &mut host, &ctx, at);
    assert_eq!(host.focused(), focused);
}

/// **A bar entry is its verb performed**, the way its key performs it: Select
/// all holds every note and reports no pick. A verb the host does not perform
/// is still the owner's, reported as the pick it always was.
#[test]
fn a_bar_entry_is_its_verb_performed() {
    let mut host = host_from(ROLL);
    let mut g = Gestures::default();
    let ctx = ctx();
    let band = host.menu_bar_rect(1, ctx.fb_w, ctx.fb_h).unwrap();
    let title = {
        let tree = host.window_def(1).unwrap();
        crate::host::menubar::titles(
            crate::host::menubar::entries(tree).unwrap(),
            band,
            host.metrics_for(1),
        )[0]
        .1
    };
    click(&mut g, &mut host, &ctx, mid(title));
    let rows = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0]
        .rows
        .clone();
    let effects = click(&mut g, &mut host, &ctx, mid(rows[0]));
    assert_eq!(selected_notes(&host, 90), vec![0, 1]);
    assert!(
        emitted(&effects)
            .iter()
            .all(|(_, args)| args.first() != Some(&OscType::String("menu".into()))),
        "performed, not reported: {effects:?}"
    );
    click(&mut g, &mut host, &ctx, mid(title));
    let rows = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0]
        .rows
        .clone();
    let effects = click(&mut g, &mut host, &ctx, mid(rows[1]));
    assert!(
        emitted(&effects).contains(&(
            1,
            vec![
                OscType::String("menu".into()),
                OscType::String("save".into())
            ]
        )),
        "the owner's verb, reported: {effects:?}"
    );
}

/// A multitrack whose one track has two curves, one of them shown.
const STACK: &str = r#"{"type":"window","margin":0,"flow":"col","children":[
    {"id":70,"type":"multitrack","link":9,"snap":0,"h":300,"sample_rate":48000,
     "tracks":["10", "bass", 120, 0, 0, 1.0, 1],
     "curves":["31","10","gain",0,1,40, "32","10","pan",0,1,40],
     "hidden":"32",
     "clips":["a","10",0,1000,0,"",-1, "b","10",4000,1000,0,"",-1]}]}"#;

/// **The context menu over a track lists its curves**, each a check that
/// shows or hides that one alone: the picture moves at once and the owner is
/// told in a `shown` report.
#[test]
fn a_track_s_context_menu_shows_its_curves_one_at_a_time() {
    let mut host = host_from(STACK);
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = ctx();
    let rect = rect_of(&host, &ctx, 70);
    let at = (f64::from(rect.x + rect.w * 0.5), f64::from(rect.y) + 30.0);
    g.context(&mut host, &ctx, at.0, at.1).expect("a menu");
    let stack = host.popup(1).unwrap();
    assert_eq!(stack.owner, Owner::Own(70));
    let labels: Vec<&str> = stack.levels[0]
        .entries
        .iter()
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "gain",
            "pan",
            "",
            "Add track",
            "",
            "Reset track heights",
            "Compact tracks"
        ]
    );
    assert!(stack.levels[0].entries[0].marked(), "gain is shown");
    assert!(!stack.levels[0].entries[1].marked(), "pan is not");
    let rows = host.popup_placed(1, ctx.fb_w, ctx.fb_h).unwrap()[0]
        .rows
        .clone();
    let effects = click(&mut g, &mut host, &ctx, mid(rows[1]));
    assert!(host.popup(1).is_none());
    assert!(
        emitted(&effects).contains(&(
            70,
            vec![
                OscType::String("shown".into()),
                OscType::String("32".into()),
                OscType::Int(1)
            ]
        )),
        "{effects:?}"
    );
    // ...and the menu opened again says so
    g.context(&mut host, &ctx, at.0, at.1).expect("a menu");
    assert!(host.popup(1).unwrap().levels[0].entries[1].marked());
}

/// **Over a clip, the clip's own curves** -- and a clip with none says so,
/// in a line nothing picks.
#[test]
fn a_clip_s_context_menu_names_its_own_curves() {
    let mut host = host_from(STACK);
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = ctx();
    let rect = rect_of(&host, &ctx, 70);
    let indent = interact::hit(
        &host,
        1,
        800,
        400,
        f64::from(rect.x) + 5.0,
        f64::from(rect.y) + 5.0,
        &|_, _| 1,
    )
    .map(|h| h.indent)
    .unwrap();
    // the clip stands at the start of the body
    let at = (f64::from(rect.x + indent) + 3.0, f64::from(rect.y) + 30.0);
    g.context(&mut host, &ctx, at.0, at.1).expect("a menu");
    let first = &host.popup(1).unwrap().levels[0].entries[0];
    assert_eq!(first.label, "No clip automation");
    assert!(!first.pickable());
}

/// **A box dragged past the end lengthens the axis there and then**: the
/// multitrack reaches further, and the group's extent follows while the hand
/// is still down -- so the edge scroll carries on past the old end, and a zoom
/// out afterwards reaches the box. An owner that answers the drag with an
/// acknowledgement alone sends no set that would do it later.
#[test]
fn a_box_dragged_past_the_end_lengthens_the_axis() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":70,"type":"multitrack","link":9,"snap":0,"h":300,"sample_rate":48000,
             "tracks":["10", "one", 120, 0, 0, 1.0, 1],
             "clips":["a","10",0,48000,0,"",-1, "b","10",96000,48000,0,"",-1]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = ctx();
    let before = host.timelines().total_of(70);
    assert_eq!(before, 144_000);
    let rect = rect_of(&host, &ctx, 70);
    // the second box, at two thirds of the axis, pulled to the right edge
    let y = f64::from(rect.y) + 30.0;
    let indent = interact::hit(&host, 1, 800, 400, f64::from(rect.x) + 5.0, y, &|_, _| 1)
        .map(|h| h.indent)
        .unwrap();
    let body_x = |frac: f32| f64::from(rect.x + indent + (rect.w - indent) * frac);
    g.press(&mut host, &ctx, body_x(0.8), y);
    g.drag_to(&mut host, &ctx, body_x(0.9), y);
    g.drag_to(&mut host, &ctx, body_x(0.99), y);
    let during = host.timelines().total_of(70);
    assert!(during > before, "the axis follows the box: {during}");
    g.release(&mut host, &ctx, body_x(0.99), y);
    assert!(host.timelines().total_of(70) > before);
}

/// **A client's `/gui_verb` is the tool's verb with no tool**: the host checks
/// the window and hands the verb to the front, and the front performs it
/// through the same dispatch -- here, every note of the `main` roll held, as
/// the tool above holds them.
#[test]
fn a_verb_a_client_asks_for_is_the_one_a_tool_performs() {
    use super::super::{GUI_VERB, HostEffect};
    let mut host = host_from(ROLL);
    let from = ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )));
    let asked = |host: &mut Host, window: i32, verb: &str| {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_VERB.into(),
                args: vec![OscType::Int(window), OscType::String(verb.into())],
            }),
            from,
        )
    };
    // a window that is not there is no one to ask
    assert!(asked(&mut host, 99, "select_all").is_empty());
    let effects = asked(&mut host, 1, "select_all");
    let [HostEffect::Perform { window, verb }] = &effects[..] else {
        panic!("the front is asked to perform it: {effects:?}");
    };
    assert_eq!((*window, verb.as_str()), (1, "select_all"));

    let ctx = ctx();
    Gestures::default().command(&mut host, &ctx, verb);
    assert_eq!(selected_notes(&host, 90), vec![0, 1], "every note, held");

    // a verb the host does not perform is the window's owner's, told as the
    // window would tell it
    let effects = Gestures::default().command(&mut host, &ctx, "make_coffee");
    assert!(
        emitted(&effects).iter().any(|(_, args)| args
            .iter()
            .any(|a| matches!(a, OscType::String(s) if s == "make_coffee"))),
        "the owner is told: {effects:?}"
    );
}

/// Writes a window's `actions` prop as its owner would, over the wire.
fn publish_actions(host: &mut Host, actions: serde_json::Value) {
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: super::super::GUI_SET.into(),
            args: vec![
                OscType::Int(1),
                OscType::String("actions".into()),
                OscType::String(actions.to_string()),
            ],
        }),
        ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
}

/// An action reading of `kind` over the document, available.
fn reading(kind: &str) -> serde_json::Value {
    serde_json::json!({"kind": kind, "enabled": true,
        "target": {"kind": "document", "member": 0, "object": 1}})
}

/// **An action that cannot be taken is refused alike** from a tool, a
/// client's command and a key, its entry is disabled, and the same verb runs
/// from all three once the owner says it can.
#[test]
fn an_unavailable_action_is_refused_by_a_tool_a_command_and_a_key() {
    let mut host = host_from(ROLL);
    let mut refused = reading("command");
    refused["enabled"] = false.into();
    refused["reason"] = "the file has pending work".into();
    publish_actions(&mut host, serde_json::json!({ "select_all": refused }));
    let mut gestures = Gestures::default();
    let mut ctx = ctx();
    let at = mid(rect_of(&host, &ctx, 5));
    click(&mut gestures, &mut host, &ctx, at);
    gestures.command(&mut host, &ctx, "select_all");
    ctx.ctrl = true;
    gestures.press_key(&mut host, &ctx, Key::Char('a'), None);
    assert!(selected_notes(&host, 90).is_empty(), "nothing was selected");
    let mut entries = host.window_def(1).unwrap().menu.clone().unwrap();
    host.action_entries(1, &mut entries);
    assert_eq!(
        crate::host::menu::find_verb(&entries, "select_all"),
        Some((vec![0, 0], false))
    );
    publish_actions(
        &mut host,
        serde_json::json!({ "select_all": reading("command") }),
    );
    gestures.press_key(&mut host, &ctx, Key::Char('a'), None);
    assert_eq!(selected_notes(&host, 90), [0, 1]);
}

/// **A menu shows a switch as a check and a choice as one of its group**,
/// in the state the owner reads now, without writing it into the window's
/// menu.
#[test]
fn a_menu_marks_switches_and_choices_from_their_readings() {
    let mut host = host_from(
        r#"{"type":"window","menu":[{"label":"View","menu":[
            {"label":"Loop","verb":"loop"},
            {"label":"Waveform","verb":"view_wave"},
            {"label":"Both","verb":"view_both"}]}],"children":[]}"#,
    );
    let mut on = reading("toggle");
    on["checked"] = true.into();
    let choice = |checked: bool| {
        let mut c = reading("choice");
        c["group"] = "views".into();
        c["value"] = "view_both".into();
        c["checked"] = checked.into();
        c
    };
    publish_actions(
        &mut host,
        serde_json::json!({"loop": on, "view_wave": choice(false), "view_both": choice(true)}),
    );
    let mut entries = host.window_def(1).unwrap().menu.clone().unwrap();
    host.action_entries(1, &mut entries);
    let EntryKind::Submenu(view) = &entries[0].kind else {
        panic!("a list")
    };
    assert_eq!(view[0].kind, EntryKind::Check(true));
    assert_eq!(
        view[1].kind,
        EntryKind::Radio {
            group: "views".into(),
            on: false
        }
    );
    assert_eq!(
        view[2].kind,
        EntryKind::Radio {
            group: "views".into(),
            on: true
        }
    );
    let EntryKind::Submenu(sent) = &host.window_def(1).unwrap().menu.as_ref().unwrap()[0].kind
    else {
        panic!("a list")
    };
    assert_eq!(
        sent[0].kind,
        EntryKind::Action,
        "the owner's menu is left as sent"
    );
}

/// **A paste is offered only where the clipboard fits**: as many channels,
/// the same rate for a mix, and a server copy on the destination's
/// connection.
#[test]
fn a_paste_is_read_against_the_clipboard_and_its_destination() {
    use clausters_document::clipboard::{BufferLocation, Clipboard, Content};
    let mut host = host_from(r#"{"type":"window","children":[]}"#);
    let mut paste = reading("command");
    paste["paste"] = serde_json::json!({"kind": "samples", "channels": 2, "rate": 48000.0,
            "connection": 7, "generation": 1});
    publish_actions(&mut host, serde_json::json!({"paste": paste, "mix": paste}));
    let enabled = |host: &Host, name: &str| host.action_state(1, name, &host.clipboard).unwrap();
    assert!(!enabled(&host, "paste").enabled, "an empty clipboard");
    let copy = |channels, rate, connection| {
        let mut doc = Clipboard::text("");
        doc.content = Content::Buffer {
            channels,
            frames: 10,
            sample_rate: rate,
            buffer: 3,
            location: Some(BufferLocation {
                connection,
                generation: 1,
                publication: 1,
            }),
        };
        doc
    };
    host.clipboard.put(copy(2, 44100.0, 7), Vec::new());
    assert!(
        enabled(&host, "paste").enabled,
        "a paste keeps its own rate"
    );
    let mix = enabled(&host, "mix");
    assert!(!mix.enabled && mix.reason.unwrap().contains("rate"));
    host.clipboard.put(copy(1, 48000.0, 7), Vec::new());
    assert!(!enabled(&host, "paste").enabled, "another channel count");
    host.clipboard.put(copy(2, 48000.0, 8), Vec::new());
    assert!(!enabled(&host, "paste").enabled, "another connection");
    host.clipboard.set_text("words");
    assert!(!enabled(&host, "paste").enabled, "text is not samples");
}

/// **A focused field answers its own editing first**: an undo while typing
/// walks the field's history and never the document behind it, a run of
/// typing is one step, and a copy copies the field's text.
#[test]
fn a_focused_field_answers_its_editing_before_the_document() {
    let mut host =
        host_from(r#"{"type":"window","children":[{"type":"text","id":5,"value":"ab"}]}"#);
    publish_actions(&mut host, serde_json::json!({ "undo": reading("command") }));
    host.focus(1, 5);
    let mut gestures = Gestures::default();
    let ctx = ctx();
    let value = |host: &Host| match &host.window_def(1).unwrap().find(5).unwrap().kind {
        WidgetKind::Custom(el) => el.value(),
        _ => None,
    };
    let state = host.action_state(1, "undo", &host.clipboard).unwrap();
    assert_eq!(state.target, ActionTarget::Text { widget: 5 });
    assert!(!state.enabled, "nothing typed yet");
    assert!(emitted(&gestures.command(&mut host, &ctx, "undo")).is_empty());
    for c in ['c', 'd'] {
        gestures.press_key(&mut host, &ctx, Key::Char(c), None);
    }
    assert_eq!(value(&host), Some(OscType::String("cdab".into())));
    assert!(
        host.action_state(1, "undo", &host.clipboard)
            .unwrap()
            .enabled
    );
    let undone = gestures.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host), Some(OscType::String("ab".into())), "one step");
    assert!(
        emitted(&undone).iter().all(|(id, _)| *id == 5),
        "only the field reports: {undone:?}"
    );
    let mut redo = super::GestureCtx::new(1, 800, 400);
    redo.ctrl = true;
    redo.shift = true;
    gestures.press_key(&mut host, &redo, Key::Char('Z'), None);
    assert_eq!(value(&host), Some(OscType::String("cdab".into())));
    gestures.command(&mut host, &ctx, "select_all");
    gestures.command(&mut host, &ctx, "copy");
    assert_eq!(host.clipboard.text(), "cdab");
}

/// **Nothing behind a dialog is reached by a verb**: with a dialog up and its
/// field not focused, an undo, a key or a menu entry reaches neither the
/// document nor the field; focused inside it, the field answers alone.
#[test]
fn a_dialog_up_keeps_verbs_from_what_is_behind_it() {
    let mut host = host_from(
        r#"{"type":"window","main":90,"menu":[{"label":"Edit","menu":[
            {"label":"Undo","verb":"undo"},{"label":"Select all","verb":"select_all"}]}],
          "children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,"weight":1,
             "notes":[0.0,400.0,60.0,100,0]},
            {"id":3,"type":"layout","modal":true,"w":240,"h":120,"children":[
              {"type":"text","id":5,"value":"name"}]}]}"#,
    );
    publish_actions(&mut host, serde_json::json!({ "undo": reading("command") }));
    let mut gestures = Gestures::default();
    let mut ctx = ctx();
    assert!(host.dialog(1).is_some());
    assert!(emitted(&gestures.command(&mut host, &ctx, "undo")).is_empty());
    gestures.command(&mut host, &ctx, "select_all");
    assert!(
        selected_notes(&host, 90).is_empty(),
        "the roll is behind it"
    );
    ctx.ctrl = true;
    assert!(
        emitted(
            &gestures
                .press_key(&mut host, &ctx, Key::Char('z'), None)
                .unwrap_or_default()
        )
        .is_empty()
    );
    let mut entries = host.window_def(1).unwrap().menu.clone().unwrap();
    host.action_entries(1, &mut entries);
    let EntryKind::Submenu(edit) = &entries[0].kind else {
        panic!("a list")
    };
    assert!(edit.iter().all(|e| !e.enabled), "the bar is behind it too");
    host.focus(1, 5);
    ctx.ctrl = false;
    gestures.command(&mut host, &ctx, "select_all");
    gestures.command(&mut host, &ctx, "copy");
    assert_eq!(host.clipboard.text(), "name", "the field inside answers");
}

/// ...and where the host cannot tell which view a reading addresses (no
/// `main`, two takes), a key still reaches the owner rather than being
/// refused on a reading that may predate the sweep.
#[test]
fn a_copy_the_host_cannot_place_is_left_to_the_owner() {
    let take = |id: i32| {
        serde_json::json!({"id": id, "type": "signal", "view": "trace",
            "data": vec![0.1; 1000], "base_bucket": 2, "copies": "owner"})
    };
    let mut host = host_from(
        &serde_json::json!({"type": "window", "flow": "col",
            "children": [take(50), take(51)]})
        .to_string(),
    );
    host.set_timeline_total(50, 1000);
    host.set_timeline_total(51, 1000);
    let mut unselected = reading("command");
    unselected["enabled"] = false.into();
    unselected["unselected"] = true.into();
    publish_actions(&mut host, serde_json::json!({ "copy": unselected }));
    assert_eq!(host.window_selected(1), None, "no view to read");
    let at = mid(rect_of(&host, &ctx(), 50));
    let mut g = Gestures::default();
    let mut ctx = ctx();
    g.press(&mut host, &ctx, at.0 - 100.0, at.1);
    g.drag_to(&mut host, &ctx, at.0 + 100.0, at.1);
    g.release(&mut host, &ctx, at.0 + 100.0, at.1);
    ctx.ctrl = true;
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('c'), Some(at))
        .unwrap_or_default();
    assert!(
        emitted(&effects)
            .iter()
            .any(|(id, args)| *id == 50 && args.first() == Some(&OscType::String("copy".into()))),
        "{effects:?}"
    );
}

/// **The loop switch is said when it moves**: an owner re-sends `looping` with
/// every correction of a view, and a line for a switch nobody turned reads as
/// if one had been.
#[test]
fn a_loop_switch_sent_again_says_nothing() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.1,0.2]}]}"#,
    );
    let set = |host: &mut Host, on: i32| {
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: super::super::GUI_SET.into(),
                args: vec![
                    OscType::Int(50),
                    OscType::String("looping".into()),
                    OscType::Int(on),
                ],
            }),
            ClientId::Udp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                9000,
            ))),
        );
    };
    let said = |host: &Host| {
        host.statuses()
            .get(&1)
            .map(|s| s.lines().map(|l| l.text.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    set(&mut host, 0);
    assert!(said(&host).is_empty(), "off was already off");
    set(&mut host, 1);
    assert_eq!(said(&host).last().map(String::as_str), Some("loop on"));
}

/// A dialog in a stack's page is up only while that page is shown: page 0 of
/// an application's dialogs is empty.
#[test]
fn a_dialog_in_a_hidden_page_is_not_up() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":7,"type":"layout","flow":"stack","index":0,"h":0,"children":[
              {"type":"layout"},
              {"type":"layout","children":[
                {"id":3,"type":"layout","modal":true,"children":[]}]}]}]}"#,
    );
    assert!(host.dialog(1).is_none());
    let mut effects = Vec::new();
    host.set_props(
        7,
        vec![("index".into(), serde_json::json!(1))],
        &mut effects,
    );
    assert_eq!(host.dialog(1).and_then(|d| d.id), Some(3));
}

/// **A window that describes its actions offers no others of the host's
/// table**: a verb it gives no reading is not performed from its key and is
/// left off its key sheet, while a window with no readings offers them all.
#[test]
fn a_window_with_readings_offers_only_the_table_s_verbs_it_reads() {
    let mut host = host_from(ROLL);
    let sheet = |host: &Host| -> Vec<String> {
        host.keys
            .sheet_offered(&host.window_keys(1), &|verb| host.offers(1, verb))
            .into_iter()
            .flat_map(|section| section.rows.into_iter().map(|(words, _)| words))
            .collect()
    };
    assert!(
        sheet(&host).contains(&"Split".to_string()),
        "no readings: all"
    );
    publish_actions(&mut host, serde_json::json!({ "undo": reading("command") }));
    assert!(host.offers(1, "undo") && !host.offers(1, "split"));
    assert!(
        host.offers(1, "export"),
        "a verb outside the table is the owner's"
    );
    let words = sheet(&host);
    assert!(words.contains(&"Undo".to_string()) && !words.contains(&"Split".to_string()));
    let mut g = Gestures::default();
    let ctx = ctx();
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('e'), None)
        .unwrap_or_default();
    assert!(
        emitted(&effects).is_empty(),
        "split is not this window's: {effects:?}"
    );
}

/// **The selection an action reads is whatever the view holds**: a roll's
/// notes as much as a take's span, so a copy over notes just picked is
/// offered before the owner hears of the pick.
#[test]
fn an_element_s_own_selection_is_read_for_its_actions() {
    let mut host = host_from(ROLL);
    let mut unselected = reading("command");
    unselected["enabled"] = false.into();
    unselected["unselected"] = true.into();
    publish_actions(
        &mut host,
        serde_json::json!({"copy": unselected, "select_all": reading("command")}),
    );
    assert_eq!(host.window_selected(1), Some(false));
    assert!(
        !host
            .action_state(1, "copy", &host.clipboard)
            .unwrap()
            .enabled
    );
    Gestures::default().command(&mut host, &ctx(), "select_all");
    assert_eq!(
        host.window_selected(1),
        Some(true),
        "the roll holds its notes"
    );
    assert!(
        host.action_state(1, "copy", &host.clipboard)
            .unwrap()
            .enabled
    );
}
