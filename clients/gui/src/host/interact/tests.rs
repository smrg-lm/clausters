//! The module's own suite, kept whole rather than split four ways: every test
//! here drives a real [`Host`] built from a `/gui_def` document, and the host
//! builders are what they share.

use clausters_core::osc::{OscMessage, OscPacket, OscType};

use super::super::layout::{self, Rect};
use super::super::widget::WidgetKind;
use super::super::{ClientId, GUI_DEF, Host};
use super::coords::clip_part;
use super::coords::snap;
use super::*;
use crate::host::graphics::pianoroll;
use crate::host::graphics::track;
use crate::viewport::View;

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

/// A window (id 1) with one track (id 5) holding two abutting clips: A
/// (id 10) over [0, 400), B (id 11) over [400, 400), grid 100.
fn track_host() -> Host {
    let json = r#"{"type":"window","children":[
        {"id":5,"type":"field","snap":100.0,"children":[
            {"id":10,"type":"field","offset":0.0,"dur":400.0},
            {"id":11,"type":"field","offset":400.0,"dur":400.0}
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

/// The lane body and shared nav of the one track, computed the same way the
/// renderer and hit-test do — so the test hits real pixels.
fn geometry(host: &Host, fb_w: u32, fb_h: u32) -> (Rect, View) {
    let tree = host.window_def(1).unwrap();
    let nav = track::window_nav(tree);
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let track_rect = layout::layout(area, tree, host.metrics_for(1))
        .into_iter()
        .find(|p| matches!(p.widget.kind, WidgetKind::Track { .. }))
        .unwrap()
        .rect;
    (
        track::lane_body(
            track_rect,
            false,
            host.metrics_for(1).header_w,
            host.metrics_for(1),
        ),
        nav,
    )
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
    let json = r#"{"type":"window","children":[
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

#[test]
fn clip_drag_placement_moves_and_resizes_from_the_snapshot() {
    // Body: the offset follows the delta, snapped; the duration is kept.
    let (off, dur) = clip_drag_placement(ClipPart::Body, 730.0, 500.0, 400.0, 300.0, 100.0);
    assert_eq!((off, dur), (600.0, 300.0));
    // End: resizing never crosses the start (duration floors at 0).
    let (off, dur) = clip_drag_placement(ClipPart::End, 0.0, 690.0, 400.0, 300.0, 100.0);
    assert_eq!(off, 400.0);
    assert!(dur >= 0.0);
    // Start: the onset stays within [0, end], the end fixed.
    let (off, dur) = clip_drag_placement(ClipPart::Start, 0.0, 900.0, 400.0, 300.0, 100.0);
    assert_eq!((off, dur), (0.0, 700.0));
    let (off, dur) = clip_drag_placement(ClipPart::Start, 950.0, 400.0, 400.0, 300.0, 100.0);
    assert_eq!((off, dur), (700.0, 0.0));
}

#[test]
fn clip_part_splits_body_from_edges() {
    let m = crate::host::metrics::Metrics::default();
    let both = (true, true);
    let wide = Rect::new(100.0, 0.0, 200.0, 40.0);
    // A wide clip with both ends on screen: a grip at each end, body between.
    assert_eq!(clip_part(wide, both, &m, 102.0), ClipPart::Start);
    assert_eq!(clip_part(wide, both, &m, 297.0), ClipPart::End);
    assert_eq!(clip_part(wide, both, &m, 200.0), ClipPart::Body);
    // Too narrow to hold two grips: all body.
    assert_eq!(
        clip_part(Rect::new(100.0, 0.0, 8.0, 40.0), both, &m, 101.0),
        ClipPart::Body
    );
    // An end off screen has no grip: the rectangle's edge there is the
    // window's, not the clip's, so a press on it grabs the body and pans or
    // moves like any other pixel of the material.
    assert_eq!(clip_part(wide, (false, true), &m, 102.0), ClipPart::Body);
    assert_eq!(clip_part(wide, (true, false), &m, 297.0), ClipPart::Body);
}

#[test]
fn tmp_probe_zoomed_clip_ends() {
    let mut host = track_host();
    // Zoom the shared axis into [100, 500) of an 800-sample composition: clip A
    // (0..400) has its end on screen and its start off; clip B (400..800) the
    // other way round.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: "/gui_set".into(),
            args: vec![
                OscType::Int(5),
                OscType::String("view_start".into()),
                OscType::Float(100.0),
                OscType::String("view_len".into()),
                OscType::Float(400.0),
            ],
        }),
        from(),
    );
    let (fb_w, fb_h) = (1000, 200);
    let (body, nav) = geometry(&host, fb_w, fb_h);
    println!("nav = {nav:?}, body = {body:?}");
    let midy = (body.y + body.h / 2.0) as f64;
    for (id, offset, dur) in [(10, 0.0, 400.0), (11, 400.0, 400.0)] {
        let Some((x0, x1)) = track::clip_x_range(body, &nav, offset, dur) else {
            println!("clip {id}: not visible");
            continue;
        };
        let rect = track::clip_rect(body, x0, x1);
        let local = track::clip_local_view(body, &nav, offset, dur, rect);
        let ends = track::clip_ends_on_screen(&local, dur);
        println!("clip {id}: rect {x0}..{x1}, local {local:?}, ends {ends:?}");
        for x in [x0 + 2.0, x1 - 2.0] {
            let h = hit(&host, 1, fb_w, fb_h, x as f64, midy, &mono).unwrap();
            let lane = time_of(&h.chain).unwrap();
            let lo = local_time_of(&h.chain).unwrap();
            let hh = clip_hit(&host, 1, lane, lo, x as f64).unwrap();
            println!("   press at {x}: id {} part {:?}", hh.id, hh.part);
        }
    }
}

#[test]
fn the_hit_lands_on_the_placed_clip_and_names_the_part_under_the_cursor() {
    let host = track_host();
    let (fb_w, fb_h) = (1000, 200);
    let (body, nav) = geometry(&host, fb_w, fb_h);
    // A press on a lane: the hit is the **clip** the layout placed there,
    // and the chain still carries the lane's axis over it — the geometry
    // computed above, which is what the renderer draws through.
    let at = |x: f64, y: f64| {
        let h = hit(&host, 1, fb_w, fb_h, x, y, &mono).unwrap();
        let lane = time_of(&h.chain).unwrap();
        assert_eq!((lane.0, lane.1.body, lane.1.nav), (5, body, nav));
        (h, lane)
    };
    let (ax0, ax1) = track::clip_x_range(body, &nav, 0.0, 400.0).unwrap();
    let midy = (body.y + body.h / 2.0) as f64;
    let part_at = |x: f64| {
        let (h, lane) = at(x, midy);
        // The clip's own axis rides the chain beside the lane's.
        let local = local_time_of(&h.chain).unwrap();
        assert_eq!((local.0, local.1.body), (h.id, h.rect));
        let hit = clip_hit(&host, 1, lane, local, x).unwrap();
        (hit.id, hit.part)
    };

    // The body of clip A -> a move on id 10; its edges -> a resize.
    assert_eq!(part_at(((ax0 + ax1) / 2.0) as f64), (10, ClipPart::Body));
    assert_eq!(part_at((ax0 + 2.0) as f64), (10, ClipPart::Start));
    assert_eq!(part_at((ax1 - 2.0) as f64), (10, ClipPart::End));
    // Deeper into the lane -> clip B, and the hit itself says so.
    let (bx0, bx1) = track::clip_x_range(body, &nav, 400.0, 400.0).unwrap();
    let (h, _) = at(((bx0 + bx1) / 2.0) as f64, midy);
    assert_eq!(h.id, 11);
    // The clip's rectangle is the layout's, not a re-derivation.
    assert_eq!(h.rect, track::clip_rect(body, bx0, bx1));
    // Over the header band: no clip is placed there, so the hit is the
    // lane itself and the press falls through to its plan.
    let (h, _) = at((body.x - 10.0) as f64, midy);
    assert_eq!(h.id, 5);
}

#[test]
fn clip_set_and_event_args_move_and_report() {
    let mut host = track_host();
    clip_set(&mut host, 1, 10, Some(150.0), Some(250.0));
    // A negative offset clamps to 0.
    clip_set(&mut host, 1, 11, Some(-5.0), None);
    let tree = host.window_def(1).unwrap();
    let args = clip_event_args(tree, 10).unwrap();
    assert_eq!(args[0], OscType::String("clip".into()));
    assert_eq!(args[1], OscType::Float(150.0));
    assert_eq!(args[2], OscType::Float(250.0));
    assert_eq!(clip_event_args(tree, 11).unwrap()[1], OscType::Float(0.0));
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
    let rect = layout::layout(
        Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32),
        tree,
        host.metrics_for(1),
    )
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
