//! The module's own suite, kept whole rather than split four ways: every test
//! here drives a real [`Host`] built from a `/gui_def` document, and the host
//! builders are what they share.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::super::layout::{self};
use super::super::widget::WidgetKind;
use super::super::{ClientId, GUI_DEF, Host};
use super::*;
use crate::host::graphics::pianoroll;
use crate::host::placement::snap;

fn from() -> ClientId {
    ClientId::Udp(std::net::SocketAddr::from((
        std::net::Ipv4Addr::LOCALHOST,
        9000,
    )))
}

/// The lane count of a single-channel front (the tests draw no GPU slots).
fn mono(_id: i32, _kind: &WidgetKind) -> usize {
    1
}

/// A window (id 1) holding a panel (id 2) with a knob (id 3), beside a
/// `scroll` workspace (id 4) whose child is a second scroll (id 5) with a
/// knob (id 6) in it — two planes, one nested in the other.
fn nested_host() -> Host {
    let json = r#"{"type":"window","flow":"row","children":[
        {"id":2,"type":"layout","children":[{"id":3,"type":"knob"}]},
        {"id":4,"type":"plane","children":[
            {"id":5,"type":"plane","children":[{"id":6,"type":"knob"}]}
        ]}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        from(),
    );
    host
}

/// The chain is the containment the layout already resolved: outermost
/// first, the window included, every container over the hit and none that
/// is not over it.
#[test]
fn the_hit_carries_the_containers_over_it() {
    let host = nested_host();
    let (fb_w, fb_h) = (800, 400);
    let at = |x: f64, y: f64| hit(&host, 1, fb_w, fb_h, x, y, &mono);
    // The knob in the panel: window → panel, both layout containers, and
    // no plane to pan.
    let h = at(100.0, 50.0).unwrap();
    assert_eq!(h.id, 3);
    assert_eq!(
        h.chain.iter().map(|f| f.id).collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    assert!(h.chain.iter().all(|f| f.coords == Coords::Layout));
    assert!(plane_of(&h.chain).is_none(), "a panel is not a plane");
    // The knob in the nested workspace: both planes are over it, and the
    // **innermost** is the one a wheel or a pan addresses.
    let h = at(600.0, 200.0).unwrap();
    assert_eq!(h.id, 6);
    assert_eq!(
        h.chain.iter().map(|f| f.id).collect::<Vec<_>>(),
        vec![Some(1), Some(4), Some(5)]
    );
    let (id, rect, _) = plane_of(&h.chain).unwrap();
    assert_eq!(id, 5);
    assert!(rect.contains(600.0, 200.0));
}

/// A press on a workspace's own empty area addresses that workspace: the
/// `scroll` is the hit, so the chain has to end with it rather than stop
/// at its parent.
#[test]
fn a_workspace_is_its_own_plane_when_the_press_lands_on_it() {
    // No status bar: the press below aims at a pixel of the framebuffer, and
    // the bar would have the bottom of it (`crate::host::status`).
    let json = r#"{"type":"window","status":0,"children":[
        {"id":4,"type":"plane","flow":"free","children":[
            {"id":6,"type":"knob","x":0.0,"y":0.0,"w":20.0,"h":20.0}
        ]}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        from(),
    );
    let h = hit(&host, 1, 800, 400, 700.0, 380.0, &mono).unwrap();
    assert_eq!(h.id, 4, "empty plane area hits the workspace itself");
    assert_eq!(plane_of(&h.chain).map(|(id, ..)| id), Some(4));
}

#[test]
fn snap_rounds_to_the_grid_or_to_whole_samples() {
    assert_eq!(snap(437.0, 100.0), 400.0);
    assert_eq!(snap(451.0, 100.0), 500.0);
    assert_eq!(snap(12.4, 0.0), 12.0); // no grid: whole samples
}

#[test]
fn sample_at_inverts_the_body_pixel_map() {
    // A 1000-sample window over a 500 px body starting at x = 100.
    assert_eq!(sample_at(0.0, 1000.0, 100.0, 500.0, 100.0), 0.0);
    assert_eq!(sample_at(0.0, 1000.0, 100.0, 500.0, 600.0), 1000.0);
    assert_eq!(sample_at(2000.0, 1000.0, 100.0, 500.0, 350.0), 2500.0);
    // A degenerate body never divides by zero.
    assert!(sample_at(0.0, 1000.0, 100.0, 0.0, 300.0).is_finite());
}

/// A window (id 1) with one `pianoroll` (id 5): a single note spanning the
/// whole roll at MIDI 60 over the pitch window [48, 72], velocity lane on.
fn pianoroll_host() -> Host {
    let json = r#"{"type":"window","children":[
        {"id":5,"type":"notes","min":48.0,"max":72.0,"snap":100.0,
         "notes":[0.0,1000.0,60.0,100,0]}
    ]}"#;
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(1), OscType::String(json.into())],
        }),
        from(),
    );
    host
}

/// **An element says where the shared axis lies inside it.** A roll's is its
/// note grid, not its rectangle minus the chrome: the velocity and event
/// strips are stacked *under* the grid and read the same time, and the keyboard
/// gutter is a vertical surface whatever `ruler_y` says. The hit-test has to
/// place the axis exactly where the drawing did, so it asks the leaf.
#[test]
fn the_axis_of_a_roll_is_the_grid_it_draws_its_notes_in() {
    let host = pianoroll_host();
    let (fb_w, fb_h) = (800u32, 400u32);
    let tree = host.window_def(1).unwrap();
    let rect = layout::layout(host.content_area(1, fb_w, fb_h), tree, host.metrics_for(1))
        .into_iter()
        .find(|p| p.widget.id == Some(5))
        .unwrap()
        .rect;
    let r = pianoroll::regions(
        rect,
        true,
        false,
        true,
        pianoroll::KEYBOARD_W,
        host.metrics_for(1),
    );
    let cy = pianoroll::pitch_to_y(60.0, 48.0, 72.0, r.grid) as f64;
    let cx = (r.grid.x + r.grid.w * 0.5) as f64;
    let h = hit(&host, 1, fb_w, fb_h, cx, cy, &mono).unwrap();
    let (id, axis) = time_of(&h.chain).unwrap();
    assert_eq!((id, axis.body), (5, r.grid), "the roll is its own axis");
    // ...and the band left of the grid is its vertical surface, which is what a
    // wheel over the keyboard navigates.
    let y = axis.y.expect("a roll always offers a pitch axis");
    assert_eq!(y.strip.w, r.grid.x - rect.x);
    // The strips under the grid are on the same axis: a press there reads a
    // time, which is why they are not part of the body.
    assert!(axis.spans(cx));
}
