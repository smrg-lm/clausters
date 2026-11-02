//! **A window's own undo order** (`history: true`): what a hand does to its
//! controls, walked by the host -- `crate::host::panel`.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::super::{ClientId, GUI_DEF, GUI_SET, Host};
use super::*;

fn from() -> ClientId {
    ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )))
}

fn def(host: &mut Host, id: i32, json: &str) {
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(id), OscType::String(json.into())],
        }),
        from(),
    );
}

/// A window of two sliders, the first bound to a node's control.
fn panel(history: bool) -> Host {
    let mut host = Host::new();
    def(
        &mut host,
        1,
        &format!(
            r#"{{"type":"window","history":{history},"children":[
                {{"id":10,"type":"slider","min":0.0,"max":10.0,"value":2.5,
                  "bind":["/node_set",1001,"freq"]}},
                {{"id":11,"type":"slider","min":0.0,"max":10.0,"value":5.0}}]}}"#
        ),
    );
    host
}

fn value(host: &Host, window: i32, id: i32) -> f32 {
    match host
        .window_def(window)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .event_value()
    {
        Some(OscType::Float(v)) => v,
        other => panic!("not a slider: {other:?}"),
    }
}

/// The middle of slider `id`'s strip.
fn at(host: &Host, ctx: &GestureCtx, id: i32) -> (f64, f64) {
    let r = host
        .layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)
        .unwrap()
        .iter()
        .find(|p| p.widget.id == Some(id))
        .unwrap()
        .rect;
    ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64)
}

/// Holds slider `id` from its middle to `x`, and lets it go there.
fn hold(g: &mut Gestures, host: &mut Host, ctx: &GestureCtx, id: i32, x: f64) {
    let (cx, cy) = at(host, ctx, id);
    g.press(host, ctx, cx, cy);
    g.drag_to(host, ctx, (cx + x) * 0.5, cy);
    g.drag_to(host, ctx, x, cy);
    g.release(host, ctx, x, cy);
}

fn emitted(effects: &[GestureEffect], widget: i32) -> Vec<Vec<OscType>> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } if *widget_id == widget => Some(args.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_hold_is_one_entry_and_a_walk_writes_what_it_stored() {
    let mut host = panel(true);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 10, 390.0);
    let moved = value(&host, 1, 10);
    assert!(moved > 9.0, "dragged to the right: {moved}");

    g.command(&mut host, &ctx, "undo");
    assert_eq!(
        value(&host, 1, 10),
        2.5,
        "one undo takes the whole hold back"
    );
    g.command(&mut host, &ctx, "redo");
    assert_eq!(value(&host, 1, 10), moved, "and one redo brings it again");
    g.command(&mut host, &ctx, "undo");
    let out = g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 10), 2.5, "nothing more to undo");
    assert!(
        emitted(&out, 1).is_empty(),
        "the window's own order took the verb; the owner is not told"
    );
}

#[test]
fn an_unbound_control_hears_its_walk_as_a_hand_would_send_it() {
    let mut host = panel(true);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 11, 10.0);
    let out = g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 11), 5.0);
    assert_eq!(emitted(&out, 11), vec![vec![OscType::Float(5.0)]]);
}

#[test]
fn a_window_without_history_reports_undo_to_its_owner() {
    let mut host = panel(false);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 11, 10.0);
    let moved = value(&host, 1, 11);
    let out = g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 11), moved, "the host holds no history");
    assert_eq!(emitted(&out, 1), vec![vec![OscType::String("undo".into())]]);
}

#[test]
fn two_instances_keep_two_orders() {
    let mut host = panel(true);
    def(
        &mut host,
        2,
        r#"{"type":"window","history":true,"children":[
            {"id":20,"type":"slider","min":0.0,"max":10.0,"value":5.0}]}"#,
    );
    let mut g = Gestures::default();
    let one = GestureCtx::new(1, 400, 200);
    let two = GestureCtx::new(2, 400, 200);
    hold(&mut g, &mut host, &one, 11, 10.0);
    hold(&mut g, &mut host, &two, 20, 390.0);
    let moved = value(&host, 2, 20);
    g.command(&mut host, &one, "undo");
    assert_eq!(value(&host, 1, 11), 5.0, "the first window walked its own");
    assert_eq!(value(&host, 2, 20), moved, "the second was not reached");
}

#[test]
fn a_set_from_outside_is_no_entry_and_the_walk_writes_absolute() {
    let mut host = panel(true);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 10, 390.0);
    // The press focused the slider, and a read-back leaves the control the
    // keyboard is in alone: the keyboard goes elsewhere first.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(10),
                OscType::String("focus".into()),
                OscType::Int(0),
            ],
        }),
        from(),
    );
    // Another writer moves the parameter: a read-back answer.
    host.read_back(
        &OscMessage {
            addr: "/node_set".into(),
            args: vec![
                OscType::Int(1001),
                OscType::String("freq".into()),
                OscType::Float(7.0),
            ],
        },
        &[],
    );
    assert_eq!(value(&host, 1, 10), 7.0);
    g.command(&mut host, &ctx, "undo");
    assert_eq!(
        value(&host, 1, 10),
        2.5,
        "the stored value, whatever it holds"
    );
    // ...and the read-back made no entry of its own: nothing is left.
    g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 10), 2.5);
    // A script's set is no entry either.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(11),
                OscType::String("value".into()),
                OscType::Float(1.0),
            ],
        }),
        from(),
    );
    g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 11), 1.0);
}

#[test]
fn two_widgets_bound_to_one_control_are_one_target() {
    let mut host = Host::new();
    def(
        &mut host,
        1,
        r#"{"type":"window","history":true,"children":[
            {"id":10,"type":"slider","min":0.0,"max":10.0,"value":2.5,
             "bind":["/node_set",1001,"freq"]},
            {"id":12,"type":"knob","min":0.0,"max":10.0,"value":2.5,
             "bind":["/node_set",1001,"freq"]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 10, 390.0);
    g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 10), 2.5);
    assert_eq!(value(&host, 1, 12), 2.5, "the other writer shows it too");
}

#[test]
fn a_node_that_ended_takes_its_entries_out_of_the_walk() {
    let mut host = panel(true);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 11, 10.0);
    hold(&mut g, &mut host, &ctx, 10, 390.0);
    let moved = value(&host, 1, 10);
    host.read_back(
        &OscMessage {
            addr: "/node_end".into(),
            args: vec![OscType::Int(1001)],
        },
        &[],
    );
    // The bound control's entry is passed over: the undo reaches the other.
    g.command(&mut host, &ctx, "undo");
    assert_eq!(
        value(&host, 1, 10),
        moved,
        "nothing is written to a gone node"
    );
    assert_eq!(value(&host, 1, 11), 5.0);
}

#[test]
fn a_redefined_window_keeps_its_order() {
    let mut host = panel(true);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 11, 10.0);
    // The same window defined again: its controls survive, and so does the
    // order over them.
    def(
        &mut host,
        1,
        r#"{"type":"window","history":true,"children":[
            {"id":10,"type":"slider","min":0.0,"max":10.0,"value":2.5,
             "bind":["/node_set",1001,"freq"]},
            {"id":11,"type":"slider","min":0.0,"max":10.0,"value":0.5}]}"#,
    );
    g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 11), 5.0);
}

#[test]
fn a_button_is_never_an_entry() {
    let mut host = Host::new();
    def(
        &mut host,
        1,
        r#"{"type":"window","history":true,"children":[
            {"id":30,"type":"button"},
            {"id":11,"type":"slider","min":0.0,"max":10.0,"value":5.0}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    hold(&mut g, &mut host, &ctx, 11, 10.0);
    let (bx, by) = at(&host, &ctx, 30);
    g.press(&mut host, &ctx, bx, by);
    g.release(&mut host, &ctx, bx, by);
    g.command(&mut host, &ctx, "undo");
    assert_eq!(value(&host, 1, 11), 5.0, "the undo went past the button");
}
