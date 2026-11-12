//! The gesture machine's tests: one press -> drag -> release sequence per
//! gesture, driven over a hand-built widget tree with no window and no GPU.

use clausters_core::osc::{OscMessage, OscPacket};

use super::super::interact;
use super::super::widget::ScrollView;
use super::super::widget::element::Key;
use super::super::{ClientId, GUI_DEF, GUI_SET, Host, scroll};
use super::*;
#[cfg(feature = "patcher")]
use crate::host::graphics::patch;
use crate::host::graphics::piano;
use clausters_gui_draw::metrics::Metrics;

mod bounds;
mod clicks;
mod controls;
mod element;
mod keyboard;
mod markers;
mod multitrack;
mod pane;
mod piano_keys;
mod plane;
mod roll;
mod samples;
#[cfg(feature = "notation")]
mod score;
mod spectrum;
mod status_bar;
mod table;
mod take;

fn from() -> ClientId {
    ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )))
}

fn host_from(json: &str) -> Host {
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        from(),
    );
    no_status_bar(&mut host, 1);
    host
}

/// Turns window `def_id`'s status bar off (`crate::host::status`).
///
/// The bar is chrome along the bottom edge and the tree is laid out above it,
/// so a test that computes its own pixels from the framebuffer would be aiming
/// at a rectangle the widget no longer occupies. These tests are about what a
/// gesture does, not about where the chrome is, so their windows carry none --
/// and the bar's own behaviour is tested where it belongs, on a window that
/// has one.
fn no_status_bar(host: &mut Host, def_id: i32) {
    if let Some(WidgetKind::Window { status, .. }) =
        host.window_def_mut(def_id).map(|t| &mut t.kind)
    {
        *status = false;
    }
}

/// Where the layout put widget `id` in window `def_id` -- the rectangle a test
/// aims its presses inside, rather than guessing at pixels the size table owns.
fn placed_rect(host: &Host, ctx: &GestureCtx, id: i32) -> Rect {
    let tree = host.window_def(ctx.def_id).unwrap();
    let m = host.metrics_for(ctx.def_id);
    // The content area, not the framebuffer: the status bar has the bottom of
    // the window and a press aimed into it would miss the widget entirely.
    let area = host.content_area(ctx.def_id, ctx.fb_w, ctx.fb_h);
    crate::host::layout::layout(area, tree, m)
        .into_iter()
        .find(|p| p.widget.id == Some(id))
        .expect("the widget is placed")
        .rect
}

/// A live `/gui_set` of one string-valued prop, as a script would send it.
fn set_prop(host: &mut Host, id: i32, key: &str, value: &str) {
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(id),
                OscType::String(key.into()),
                OscType::String(value.into()),
            ],
        }),
        from(),
    );
}

fn has_emit_tag(effects: &[GestureEffect], id: i32, tag: &str) -> bool {
    effects.iter().any(|e| match e {
        GestureEffect::Emit {
            widget_id, args, ..
        } => *widget_id == id && args.first() == Some(&OscType::String(tag.into())),
        _ => false,
    })
}

/// The arguments of the last event emitted for `id` -- what a reader on the
/// other end of the wire would actually receive.
fn emitted_args(effects: &[GestureEffect], id: i32) -> Option<Vec<OscType>> {
    effects.iter().rev().find_map(|e| match e {
        GestureEffect::Emit {
            widget_id, args, ..
        } if *widget_id == id => Some(args.clone()),
        _ => None,
    })
}

fn slider_value(host: &Host, id: i32) -> f32 {
    match host
        .window_def(1)
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

/// A `scroll` workspace's live view state.
fn view_of(host: &Host, id: i32) -> ScrollView {
    match &host.window_def(1).unwrap().find(id).unwrap().kind {
        WidgetKind::Scroll { view, .. } => *view,
        other => panic!("not a scroll: {other:?}"),
    }
}

/// A window holding one 2D workspace with a 2000x2000 content area.
fn workspace(extra: &str) -> Host {
    host_from(&format!(
        r#"{{"type":"window","margin":0,"children":[
            {{"id":20,"type":"plane","margin":0,
              "content_w":2000,"content_h":2000{extra},
              "children":[{{"id":21,"type":"label","text":"a",
                            "x":100,"y":100,"w":80,"h":40}}]}}]}}"#
    ))
}

/// A window holding one full-area directed patch: `tone` (an outlet)
/// and `dac` (an inlet and an outlet), a cord tone.out -> dac.in.
#[cfg(feature = "patcher")]
fn patch_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":7,"type":"plane",
             "boxes":[{"def":"tone","outlets":["out"]},
                      {"def":"dac","inlets":["in"],"outlets":["out"]}],
             "cords":[0,0,1,0]}]}"#,
    )
}

/// The patcher element behind widget 7 -- its graph and its selection are view
/// state, reached through the element's own `as_any` door.
#[cfg(feature = "patcher")]
fn patcher(host: &Host) -> &crate::host::elements::patch::Patch {
    let WidgetKind::Custom(el) = &host.window_def(1).unwrap().find(7).unwrap().kind else {
        panic!("not an element")
    };
    el.as_any()
        .and_then(|a| a.downcast_ref::<crate::host::elements::patch::Patch>())
        .expect("a patcher")
}

#[cfg(feature = "patcher")]
fn patch_of(host: &Host) -> crate::host::graphics::patch::PatchDraw {
    patcher(host).draw_state().clone()
}

#[cfg(feature = "patcher")]
fn selection_of(host: &Host) -> Vec<usize> {
    patcher(host).selected().to_vec()
}

/// The tags a widget reported, in order -- an interface event is one string and
/// nothing else, which is what tells it from a value here.
fn tags(effects: &[GestureEffect], widget: i32) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } if *widget_id == widget => match args.as_slice() {
                [OscType::String(tag)] => Some(tag.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn spectrum_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"signal","view":"spectrum","bus":0,"navigable":1,"w":800,"h":300}]}"#,
    )
}

fn x_window(host: &Host, id: i32) -> (f64, f64) {
    host.window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .editor()
        .unwrap()
        .x_view()
}

/// Points a spectrum's frequency window the way a script does, over the wire.
fn set_x_window(host: &mut Host, id: i32, start: f32, len: f32) {
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(id),
                OscType::String("view_start".into()),
                OscType::Float(start),
                OscType::String("view_len".into()),
                OscType::Float(len),
            ],
        }),
        from(),
    );
}

/// The multi-note selection of a roll -- view state no query reports, reached
/// through the element's own `as_any` door, which is what it is for.
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

fn key(g: &Gestures, host: &mut Host, ctx: &GestureCtx, k: Key) -> Option<Vec<GestureEffect>> {
    g.key(host, ctx, k)
}

/// Every `/gui_event` a gesture asked for on `id`, in order -- a list of
/// messages, since a release may report several.
fn emitted(effects: &[GestureEffect], id: i32) -> Vec<Vec<OscType>> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } if *widget_id == id => Some(args.clone()),
            _ => None,
        })
        .collect()
}

/// The markers of widget `id` in window 1, and the name being typed there.
fn markers_of(host: &Host, id: i32) -> (Vec<String>, Option<String>) {
    let editor = host
        .window_def(1)
        .and_then(|t| t.find(id))
        .and_then(|w| w.kind.editor())
        .expect("a view with a ruler");
    (
        editor.markers.iter().map(|m| m.label.clone()).collect(),
        editor.naming.as_ref().map(|n| n.text.clone()),
    )
}
