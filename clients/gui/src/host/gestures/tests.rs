//! The gesture machine's tests: one press -> drag -> release sequence per
//! gesture, driven over a hand-built widget tree with no window and no GPU.

use clausters_core::osc::{OscMessage, OscPacket};

use super::super::metrics::Metrics;
use super::super::widget::ScrollView;
use super::super::widget::element::Key;
use super::super::{ClientId, GUI_DEF, GUI_SET, Host, scroll};
use super::*;
#[cfg(feature = "patcher")]
use crate::host::graphics::patch;
use crate::host::graphics::piano;

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
    host
}

/// Where the layout put widget `id` in window `def_id` — the rectangle a test
/// aims its presses inside, rather than guessing at pixels the size table owns.
fn placed_rect(host: &Host, ctx: &GestureCtx, id: i32) -> Rect {
    let tree = host.window_def(ctx.def_id).unwrap();
    let m = host.metrics_for(ctx.def_id);
    let area = Rect::new(0.0, 0.0, ctx.fb_w as f32, ctx.fb_h as f32);
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

/// The arguments of the last event emitted for `id` — what a reader on the
/// other end of the wire would actually receive.
fn emitted_args(effects: &[GestureEffect], id: i32) -> Option<Vec<OscType>> {
    effects.iter().rev().find_map(|e| match e {
        GestureEffect::Emit {
            widget_id, args, ..
        } if *widget_id == id => Some(args.clone()),
        _ => None,
    })
}

// ---- the menu: a list that opens, and the press that picks from it ----

fn menu_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":7,"type":"menu","label":"View","w":200,"h":48,
             "options":["ruler: shown","ruler: hidden","ruler: locked"]}]}"#,
    )
}

fn menu_index(host: &Host, id: i32) -> usize {
    match host
        .window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .event_value()
    {
        Some(OscType::Int(n)) => n as usize,
        other => panic!("not a menu: {other:?}"),
    }
}

/// The open list of menu `id`, read off the widget — which is where it lives:
/// the machine keeps no note of who opened what.
fn menu_popup(host: &Host, id: i32) -> Option<crate::host::layout::Rect> {
    host.window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .overlay_rect()
}

#[test]
fn a_press_opens_the_menus_list_and_changes_nothing_yet() {
    let mut host = menu_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let effects = g.press(&mut host, &ctx, 40.0, 40.0);
    let popup = menu_popup(&host, 7).expect("the list is open");
    assert!(popup.h > 0.0 && popup.w > 0.0);
    assert_eq!(menu_index(&host, 7), 0, "opening picks nothing");
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { .. })),
        "and emits nothing"
    );
}

#[test]
fn a_press_on_a_row_picks_that_option_and_closes() {
    let mut host = menu_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 40.0, 40.0);
    // A click is a press **and** a release: the machine now refuses a press
    // arriving mid-drag, which is what a bare second press models.
    g.release(&mut host, &ctx, 40.0, 40.0);
    let popup = menu_popup(&host, 7).unwrap();
    // The middle row: the option a click on it means, wherever the list
    // was placed (it hangs below the field, or above it near an edge).
    let row_h = popup.h as f64 / 3.0;
    let effects = g.press(
        &mut host,
        &ctx,
        popup.x as f64 + 5.0,
        popup.y as f64 + row_h * 1.5,
    );
    assert!(menu_popup(&host, 7).is_none(), "the list closes");
    assert_eq!(menu_index(&host, 7), 1);
    assert!(
        effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 7, args, .. }
                if args.first() == Some(&OscType::Int(1))
        )),
        "the pick is the widget's value, as a cycling press was"
    );
}

#[test]
fn a_press_outside_the_list_only_closes_it() {
    let mut host = menu_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 40.0, 40.0);
    // A click is a press **and** a release: the machine now refuses a press
    // arriving mid-drag, which is what a bare second press models.
    g.release(&mut host, &ctx, 40.0, 40.0);
    let effects = g.press(&mut host, &ctx, 550.0, 380.0);
    assert!(menu_popup(&host, 7).is_none());
    assert_eq!(menu_index(&host, 7), 0, "nothing picked");
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { .. })),
        "an open list swallows the press that dismisses it"
    );
}

#[test]
fn a_list_with_no_room_below_opens_upwards() {
    // The same menu at the bottom of a short window: the list has to go
    // somewhere, and off the bottom edge is not somewhere.
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":6,"type":"label","text":"filler","weight":1},
            {"id":7,"type":"menu","w":200,"h":48,
             "options":["a","b","c","d","e","f"]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 200);
    g.press(&mut host, &ctx, 40.0, 180.0);
    let popup = menu_popup(&host, 7).unwrap();
    assert!(popup.y + popup.h <= 200.0, "the list fits in the window");
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
/// and `dac` (an inlet and an outlet), a cord tone.out → dac.in.
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

/// The patcher element behind widget 7 — its graph and its selection are view
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

/// The whole widget-binding chain from a real press: a toggle bound to a
/// `stack`'s `index` flips the page inside the host, the window is asked to
/// repaint, and **nothing leaves for the script** — the point of a binding.
#[test]
fn a_press_on_a_bound_toggle_flips_the_stack_it_drives() {
    let mut host = host_from(
        r#"{"type":"window","children":[
        {"id":10,"type":"toggle","label":"view","h":32,
         "bind":["widget",20,"index"]},
        {"id":20,"type":"layout","flow":"stack","index":0,"children":[
            {"id":21,"type":"label","text":"one"},
            {"id":22,"type":"label","text":"two"}]}]}"#,
    );
    let page = |host: &Host| match host.window_def(1).unwrap().find(20).unwrap().kind {
        WidgetKind::Stack { index, .. } => index,
        ref other => panic!("not a stack: {other:?}"),
    };
    assert_eq!(page(&host), 0);

    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // The toggle sits in the window's top strip (its declared height), and
    // the press lands on its box, at the left of the cell — the rest of that
    // strip is the layout's air, not the control.
    let mut effects = g.press(&mut host, &ctx, 20.0, 20.0);
    effects.extend(g.release(&mut host, &ctx, 20.0, 20.0));

    assert_eq!(page(&host), 1, "the toggle's value became the page");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Redraw(1))),
        "the apply asks the window to repaint"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { .. })),
        "a bound widget emits nothing to the script"
    );
}

#[cfg(feature = "patcher")]
#[test]
fn dragging_a_box_selects_it_moves_it_and_emits_the_move() {
    let mut host = patch_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let area = Rect::new(0.0, 0.0, 600.0, 400.0);
    let before = patch_of(&host);
    let b0 = patch::obj_rect(area, &before, 0, 1.0);
    // Grab the box body, clear of the outlet pin at the bottom-centre.
    let (px, py) = ((b0.x + 12.0) as f64, (b0.y + 8.0) as f64);
    g.press(&mut host, &ctx, px, py);
    assert_eq!(selection_of(&host), vec![0]);
    g.drag_to(&mut host, &ctx, px + 150.0, py + 80.0);
    let effects = g.release(&mut host, &ctx, px + 150.0, py + 80.0);
    assert!(has_emit_tag(&effects, 7, "move"), "the round trip leaves");
    let after = patch_of(&host);
    // The first drag makes the auto placement explicit, moved by the delta.
    let (x0, y0) = (b0.x - area.x, b0.y - area.y);
    assert_eq!(after.boxes[0].x, Some(x0 + 150.0));
    assert_eq!(after.boxes[0].y, Some(y0 + 80.0));
    // The untouched box keeps its auto placement.
    assert_eq!(after.boxes[1].x, None);
}

#[cfg(feature = "patcher")]
#[test]
fn a_plain_drag_marquees_and_shift_pans_leaving_the_selection() {
    let mut host = patch_host();
    let mut g = Gestures::default();
    let plain = GestureCtx::new(1, 600, 400);
    let mut shift = GestureCtx::new(1, 600, 400);
    shift.shift = true;
    let area = Rect::new(0.0, 0.0, 600.0, 400.0);
    let before = patch_of(&host);
    // The canvas is the **drawn panel**, so a sweep starts on its own bare
    // paper: at its bottom-left corner, clear of the boxes stacked up the
    // middle. The paper *outside* it belongs to the workspace, which is the
    // last case below.
    let panel = patch::content_rect(area, &before, 1.0);
    let (sweep_x, sweep_y) = (
        (panel.x + panel.w - 2.0) as f64,
        (panel.y + panel.h - 2.0) as f64,
    );
    let b1 = patch::obj_rect(area, &before, 1, 1.0);
    g.press(&mut host, &plain, sweep_x, sweep_y);
    g.drag_to(
        &mut host,
        &plain,
        (b1.x - 2.0) as f64,
        (panel.y + 2.0) as f64,
    );
    assert_eq!(
        selection_of(&host),
        vec![0, 1],
        "the marquee spans both boxes"
    );
    assert!(g.dragging(), "the element holds the sweep, and draws it");
    g.release(
        &mut host,
        &plain,
        (b1.x - 2.0) as f64,
        (panel.y + 2.0) as f64,
    );
    assert!(!g.dragging());
    // Shift+drag on the canvas pans (the heavy-view convention): it starts
    // no marquee and leaves the selection untouched.
    g.press(&mut host, &shift, sweep_x, sweep_y);
    g.drag_to(&mut host, &shift, sweep_x + 30.0, sweep_y - 30.0);
    assert_eq!(
        selection_of(&host),
        vec![0, 1],
        "Shift pans: the element never saw the press"
    );
    g.release(&mut host, &shift, sweep_x + 30.0, sweep_y - 30.0);
    assert_eq!(selection_of(&host), vec![0, 1], "Shift+drag does not clear");
    // **The paper beside the graph is the workspace's**, with no modifier at
    // all: the panel hugs the boxes and the widget's rect is whatever the
    // scroll view gave it, so a drag out here pans rather than sweeping a
    // marquee over nothing — which is what it used to do, in both fronts.
    let outside = ((panel.x + panel.w + 20.0) as f64, 200.0);
    g.press(&mut host, &plain, outside.0, outside.1);
    g.drag_to(&mut host, &plain, outside.0 - 40.0, 160.0);
    assert_eq!(
        selection_of(&host),
        vec![0, 1],
        "outside the panel the element never saw the press"
    );
    g.release(&mut host, &plain, outside.0 - 40.0, 160.0);
    // A plain click on the canvas's own bare paper (a zero-size marquee)
    // clears the set.
    g.press(&mut host, &plain, sweep_x, sweep_y);
    g.release(&mut host, &plain, sweep_x, sweep_y);
    assert!(selection_of(&host).is_empty());
}

#[cfg(feature = "patcher")]
#[test]
fn a_cord_drag_from_an_outlet_lands_on_an_inlet() {
    let mut host = patch_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let area = Rect::new(0.0, 0.0, 600.0, 400.0);
    let before = patch_of(&host);
    // Grab dac's outlet, drop on... first detach: grab tone's outlet.
    let (px, py) = patch::port_pin(area, &before, 0, patch::Side::Out, 0, 1.0);
    g.press(&mut host, &ctx, px as f64, py as f64);
    assert!(g.dragging(), "a press on a port starts the cord");
    // Released over dac's inlet: the cord lands, no move is emitted.
    let (ix, iy) = patch::port_pin(area, &before, 1, patch::Side::In, 0, 1.0);
    let effects = g.release(&mut host, &ctx, ix as f64, iy as f64);
    assert!(has_emit_tag(&effects, 7, "wire"));
    assert!(!has_emit_tag(&effects, 7, "move"));
}

#[test]
fn wheel_zooms_the_workspace_anchored_at_the_cursor() {
    let mut host = workspace("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let (cx, cy) = (300.0, 200.0);
    let before = view_of(&host, 20);
    let m = Metrics::default();
    let content_under_cursor =
        |v: ScrollView| (v.view_x + cx / v.zoom(&m), v.view_y + cy / v.zoom(&m));
    let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
    let after = view_of(&host, 20);
    assert!(after.zoom(&m) > before.zoom(&m), "wheel up zooms in");
    let (bx, by) = content_under_cursor(before);
    let (ax, ay) = content_under_cursor(after);
    assert!((bx - ax).abs() < 1e-6 && (by - ay).abs() < 1e-6);
    // View state: always an event (never a bound forward), plus a repaint.
    assert!(has_emit_tag(&effects, 20, "view"));
    assert!(effects.contains(&GestureEffect::Redraw(1)));
}

#[test]
fn dragging_the_empty_plane_pans_both_axes() {
    let mut host = workspace("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // A press on the container's empty area (away from the child) grabs it.
    g.press(&mut host, &ctx, 500.0, 350.0);
    assert!(g.dragging());
    let effects = g.drag_to(&mut host, &ctx, 450.0, 300.0);
    let v = view_of(&host, 20);
    // The content follows the cursor: dragging left/up moves the view right/down.
    assert_eq!((v.view_x, v.view_y), (50.0, 50.0));
    assert!(has_emit_tag(&effects, 20, "view"));
    g.release(&mut host, &ctx, 450.0, 300.0);
    assert!(!g.dragging());
}

#[test]
fn the_plane_pans_every_direction_from_its_origin() {
    // The regression this fixes: a plane sitting at the content's top-left
    // corner (its default) used to be clamped dead against down/right
    // drags — half the gestures did nothing and it read as broken. The
    // free plane overscrolls, so every direction moves it.
    let mut host = workspace("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    assert_eq!(
        (view_of(&host, 20).view_x, view_of(&host, 20).view_y),
        (0.0, 0.0)
    );
    g.press(&mut host, &ctx, 500.0, 350.0);
    let effects = g.drag_to(&mut host, &ctx, 560.0, 390.0);
    let v = view_of(&host, 20);
    assert_eq!((v.view_x, v.view_y), (-60.0, -40.0), "down/right moves it");
    assert!(has_emit_tag(&effects, 20, "view"));
    // And it stops at half a viewport out, so the content is never lost.
    g.drag_to(&mut host, &ctx, 5000.0, 5000.0);
    let v = view_of(&host, 20);
    assert_eq!((v.view_x, v.view_y), (-300.0, -200.0));
}

/// The same anchor, on a plane whose **content follows the zoom**: a graph
/// sizes its plane to itself-but-never-below-the-viewport, so the visible
/// content shrinks as the zoom grows. Clamping the new pan against the
/// content of the *old* zoom slid the plane out from under the cursor —
/// invisible on a plane with an explicit `content_w`, which is why the test
/// above did not catch it.
// A graph-sized plane is a patcher's: without the family the same node is the
// ordinary workspace, whose content extent is not the graph's.
#[cfg(feature = "patcher")]
#[test]
fn wheel_zoom_over_a_graph_sized_plane_holds_the_cursor_too() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":20,"type":"plane","margin":0,"children":[
              {"id":7,"type":"plane","boxes":[
                {"def":"tone","outlets":["out"]},
                {"def":"dac","inlets":["in"],"outlets":["out"]}],
               "cords":[0,0,1,0]}]}]}"#,
    );
    let m = Metrics::default();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let (cx, cy) = (420.0, 260.0);
    let area = crate::host::layout::Rect::new(0.0, 0.0, 600.0, 400.0);
    // Where the graph itself sits on screen — the thing an eye tracks, and
    // the thing the content extent moves when it follows the zoom.
    let graph = |host: &Host| {
        crate::host::layout::layout(area, host.window_def(1).unwrap(), &m)
            .into_iter()
            .find(|p| p.widget.id == Some(7))
            .map(|p| (p.rect.x + p.rect.w * 0.5, p.rect.y + p.rect.h * 0.5))
            .expect("the patch is placed")
    };
    let before = view_of(&host, 20);
    let (bx, by) = graph(&host);
    g.wheel(&mut host, &ctx, cx, cy, 1.0);
    let after = view_of(&host, 20);
    let factor = (after.zoom(&m) / before.zoom(&m)) as f32;
    assert!(factor > 1.0, "wheel up zooms in");
    let (ax, ay) = graph(&host);
    // A zoom about the cursor maps every pixel p to cursor + (p - cursor) * f.
    let (wx, wy) = (
        cx as f32 + (bx - cx as f32) * factor,
        cy as f32 + (by - cy as f32) * factor,
    );
    assert!(
        (ax - wx).abs() < 0.5 && (ay - wy).abs() < 0.5,
        "the graph slid under the cursor: expected {wx},{wy}, got {ax},{ay}"
    );
}

#[test]
fn a_vertical_scroll_view_is_the_workspace_constrained_by_configuration() {
    // `axis: "y"` with `zoom: 0` *is* a plain vertical scroll view: the
    // wheel scrolls, x never moves, the zoom stays put.
    let mut host = workspace(r#","axis":"y","zoom":0"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.wheel(&mut host, &ctx, 300.0, 200.0, -1.0);
    let v = view_of(&host, 20);
    assert_eq!(
        v.zoom(&Metrics::default()),
        1.0,
        "zoom disabled: the wheel does not scale"
    );
    assert_eq!(v.view_x, 0.0, "the x axis is not pannable");
    assert_eq!(v.view_y, scroll::WHEEL_PAN_PX, "the wheel scrolls down");
    // A drag on the plane likewise moves only y.
    g.press(&mut host, &ctx, 500.0, 350.0);
    g.drag_to(&mut host, &ctx, 400.0, 300.0);
    let v = view_of(&host, 20);
    assert_eq!(v.view_x, 0.0);
    assert_eq!(v.view_y, scroll::WHEEL_PAN_PX + 50.0);
}

#[test]
fn a_horizontal_strip_pans_only_x_and_clamps_to_the_content() {
    let mut host = workspace(r#","axis":"x","zoom":0"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // The wheel drives the single axis; far past the end it clamps to
    // content - visible = 2000 - 600.
    for _ in 0..100 {
        g.wheel(&mut host, &ctx, 300.0, 200.0, -1.0);
    }
    let v = view_of(&host, 20);
    assert_eq!(v.view_x, 1400.0);
    assert_eq!(v.view_y, 0.0);
}

#[test]
fn a_widget_inside_the_workspace_still_takes_the_press() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":20,"type":"plane","margin":0,"content_w":2000,"content_h":2000,
             "children":[{"id":21,"type":"toggle","value":0,
                          "x":0,"y":0,"w":100,"h":50}]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // Over the toggle's box: the widget wins, no pan drag starts.
    let effects = g.press(&mut host, &ctx, 8.0, 25.0);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { widget_id: 21, .. })),
        "the toggle consumed the press: {effects:?}"
    );
    assert_eq!(view_of(&host, 20).view_x, 0.0, "so no pan drag started");
    g.release(&mut host, &ctx, 50.0, 25.0);
    // Scrolled out of view, the same widget is no longer hit: the press
    // falls through to the plane.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: super::super::GUI_SET.into(),
            args: vec![
                OscType::Int(20),
                OscType::String("view_x".into()),
                OscType::Float(500.0),
            ],
        }),
        from(),
    );
    g.press(&mut host, &ctx, 50.0, 25.0);
    assert!(g.dragging(), "the scrolled-away widget is not hit");
}

#[test]
fn slider_press_and_drag_set_the_value_and_emit() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":10,"type":"slider","min":0.0,"max":10.0,"value":2.5}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 100);
    // The slider is natural-thick: a strip under the window's margin, not
    // the whole pane, so the press aims inside it.
    let effects = g.press(&mut host, &ctx, 200.0, 25.0);
    assert!(g.dragging());
    let after_press = slider_value(&host, 10);
    assert!(after_press > 2.5, "press near the middle raises 2.5");
    // Unbound: the new value leaves as a /gui_event carrying one float.
    assert!(effects.iter().any(|e| matches!(
        e,
        GestureEffect::Emit { widget_id: 10, args, .. } if args.len() == 1
    )));
    assert!(effects.contains(&GestureEffect::Redraw(1)));
    // Dragging to the far right pins the value at max.
    g.drag_to(&mut host, &ctx, 399.0, 25.0);
    assert_eq!(slider_value(&host, 10), 10.0);
    // The release reports nothing — the value left on every step — but the
    // window repaints: an element that drew itself held has to be drawn let go.
    let effects = g.release(&mut host, &ctx, 399.0, 25.0);
    assert_eq!(effects, vec![GestureEffect::Redraw(1)]);
    assert!(!g.dragging());
}

#[test]
fn button_press_emits_one_and_release_emits_zero() {
    let mut host = host_from(r#"{"type":"window","children":[{"id":20,"type":"button"}]}"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 100);
    // A button is one control line tall (its natural height).
    let effects = g.press(&mut host, &ctx, 100.0, 16.0);
    assert!(g.dragging(), "held until it is let go");
    assert!(effects.iter().any(|e| matches!(
        e,
        GestureEffect::Emit { widget_id: 20, args, .. } if args == &[OscType::Int(1)]
    )));
    let effects = g.release(&mut host, &ctx, 100.0, 16.0);
    assert!(effects.iter().any(|e| matches!(
        e,
        GestureEffect::Emit { widget_id: 20, args, .. } if args == &[OscType::Int(0)]
    )));
    assert!(!g.dragging());
}

/// **The interface half of a press**, which no binding touches: the hand's
/// three events go to the script beside the value, and a click is the release
/// that landed on the button.
#[test]
fn a_button_reports_what_the_hand_did_beside_what_it_is_worth() {
    let mut host = host_from(r#"{"type":"window","children":[{"id":21,"type":"button"}]}"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 100);
    assert_eq!(tags(&g.press(&mut host, &ctx, 100.0, 16.0), 21), ["press"]);
    assert_eq!(
        tags(&g.release(&mut host, &ctx, 100.0, 16.0), 21),
        ["release", "click"]
    );
}

/// The cancellation a command button has and a piano key does not: the hand
/// slid off before letting go, so the release happened and the click did not.
#[test]
fn a_press_the_hand_slid_off_is_no_click() {
    let mut host = host_from(r#"{"type":"window","children":[{"id":22,"type":"button"}]}"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 100);
    g.press(&mut host, &ctx, 100.0, 16.0);
    g.drag_to(&mut host, &ctx, 100.0, 90.0);
    let effects = g.release(&mut host, &ctx, 100.0, 90.0);
    assert_eq!(tags(&effects, 22), ["release"]);
    // The gate still closed. An abandoned press must not leave a note sounding:
    // the value is the server's half and knows nothing about the hand's regret.
    assert!(effects.iter().any(|e| matches!(
        e,
        GestureEffect::Emit { widget_id: 22, args, .. } if args == &[OscType::Int(0)]
    )));
}

/// **A binding swallows the value and never the command.** A bound button
/// drives the audio server with no script in the path — that is what
/// `/gui_bind` is for — and the script still hears the click, because a command
/// is not a value and has nowhere else to go.
#[test]
fn a_binding_swallows_the_value_and_never_the_interface_events() {
    let mut host = host_from(
        r#"{"type":"window","children":[
             {"id":23,"type":"button","bind":["server","/node_set",1000,"gate"]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 100);
    let effects = g.press(&mut host, &ctx, 100.0, 16.0);
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 23, args, .. } if args == &[OscType::Int(1)]
        )),
        "the value went to the server, not to the script"
    );
    assert_eq!(tags(&effects, 23), ["press"]);
    assert_eq!(
        tags(&g.release(&mut host, &ctx, 100.0, 16.0), 23),
        ["release", "click"]
    );
}

/// The tags a widget reported, in order — an interface event is one string and
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

#[test]
fn toggle_press_flips_the_state() {
    let mut host =
        host_from(r#"{"type":"window","children":[{"id":30,"type":"toggle","value":0}]}"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 100);
    let effects = g.press(&mut host, &ctx, 14.0, 16.0);
    assert_eq!(
        host.window_def(1)
            .unwrap()
            .find(30)
            .unwrap()
            .kind
            .event_value(),
        Some(OscType::Int(1)),
        "the press flipped it"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { widget_id: 30, .. }))
    );
    // A toggle is a click, and the click already happened: the press is still
    // held (every taken press is, until the button comes up), and moving the
    // cursor under it does nothing.
    g.drag_to(&mut host, &ctx, 150.0, 16.0);
    assert_eq!(
        host.window_def(1)
            .unwrap()
            .find(30)
            .unwrap()
            .kind
            .event_value(),
        Some(OscType::Int(1))
    );
}

/// A knob is turned by cursor positions like everything else: what it reads out
/// of them is the travel since the press, which is the element's own business.
/// Nothing is captured, so it is the same gesture on either front.
#[test]
fn a_knob_turns_on_cursor_motion_and_captures_nothing() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":40,"type":"knob","min":0.0,"max":1.0,"value":0.5}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 200);
    // A knob is as tall as its disc plus its read-out (its natural height), so
    // it is a strip at the top of the window, and the disc is centred across
    // it: the press aims at the dial, not merely at the cell.
    g.press(&mut host, &ctx, 100.0, 30.0);
    // Dragging up turns it, from wherever the cursor now is -- including well
    // off the disc, which is where a knob's travel normally ends up.
    let effects = g.drag_to(&mut host, &ctx, 22.0, 10.0);
    assert!(effects.contains(&GestureEffect::Redraw(1)));
    let turned = host
        .window_def(1)
        .unwrap()
        .find(40)
        .unwrap()
        .kind
        .event_value();
    assert!(
        matches!(turned, Some(OscType::Float(v)) if v > 0.5),
        "up is more: {turned:?}"
    );
    g.release(&mut host, &ctx, 22.0, 10.0);
}

/// **The air a layout leaves around a control is the window's, not the
/// control's.** A checkbox stretched across a row is a small box with a word
/// beside it and a great deal of nothing after that; pressing the nothing used
/// to flip the value. The filter is the machine's — the element only declares
/// its shape — so this is the same mechanism the knob and the slider go
/// through, checked once at the level where it is applied.
#[test]
fn a_toggle_does_not_flip_from_the_air_beside_it() {
    // The panel that showed it: a row of mixed controls, so the toggle's cell
    // is as tall as its tallest sibling and as wide as its share of the row —
    // air on both axes, which is the case a width-only bound reads as no fix.
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":29,"type":"layout","flow":"row","margin":0,"children":[
                {"id":30,"type":"toggle","label":"on","value":0},
                {"id":31,"type":"knob","min":0.0,"max":1.0,"value":0.5}]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 200);
    let value = |host: &Host| {
        host.window_def(1)
            .unwrap()
            .find(30)
            .unwrap()
            .kind
            .event_value()
    };
    let cell = placed_rect(&host, &ctx, 30);
    assert!(cell.h > 60.0, "the row made the cell tall: {}", cell.h);
    let mid = (cell.y + cell.h * 0.5) as f64;
    // Past the box and its one-word label, on the box's own row.
    let effects = g.press(&mut host, &ctx, (cell.x + cell.w) as f64 - 4.0, mid);
    assert_eq!(value(&host), Some(OscType::Int(0)), "nothing flipped");
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { widget_id: 30, .. })),
        "and nothing left for the script: {effects:?}"
    );
    g.release(&mut host, &ctx, (cell.x + cell.w) as f64 - 4.0, mid);
    // The column of air under the box, on the box's own x.
    let low = (cell.y + cell.h) as f64 - 4.0;
    g.press(&mut host, &ctx, (cell.x + 6.0) as f64, low);
    assert_eq!(value(&host), Some(OscType::Int(0)), "nor did the air below");
    g.release(&mut host, &ctx, (cell.x + 6.0) as f64, low);
    // The box itself does flip it.
    g.press(&mut host, &ctx, (cell.x + 6.0) as f64, mid);
    assert_eq!(value(&host), Some(OscType::Int(1)));
}

/// **A dial is a disc, so its corners are not it.** A knob's cell is a label
/// strip over the disc over a read-out, and a row spreads it wider still: press
/// the paper beside the dial and the value used to jump, because the hit-test
/// was the rectangle the layout gave the element rather than the circle the
/// renderer drew in it.
#[test]
fn a_knob_is_grabbed_by_its_disc_and_not_by_the_corners_of_its_cell() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":40,"type":"knob","min":0.0,"max":1.0,"value":0.5}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 200, 200);
    // The far left of the cell, on the same row as the dial's centre: inside
    // the rectangle, nowhere near the drawn disc.
    g.press(&mut host, &ctx, 4.0, 30.0);
    g.drag_to(&mut host, &ctx, 4.0, 10.0);
    assert_eq!(
        value_of(&host, 40),
        Some(OscType::Float(0.5)),
        "the corner of the cell does not turn the value"
    );
    g.release(&mut host, &ctx, 4.0, 10.0);
    // The dial itself still does.
    g.press(&mut host, &ctx, 100.0, 30.0);
    g.drag_to(&mut host, &ctx, 100.0, 10.0);
    assert!(
        matches!(value_of(&host, 40), Some(OscType::Float(v)) if v > 0.5),
        "the disc turns"
    );
}

/// One widget's current value, as `/gui_query` would read it.
fn value_of(host: &Host, id: i32) -> Option<OscType> {
    host.window_def(1)?.find(id)?.kind.event_value()
}

#[test]
fn waveform_press_and_drag_select_a_range() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(50, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    // Press inside the view body (right of the y-ruler strip), then drag.
    let effects = g.press(&mut host, &ctx, 400.0, 150.0);
    assert!(has_emit_tag(&effects, 50, "selection"));
    let effects = g.drag_to(&mut host, &ctx, 600.0, 150.0);
    assert!(has_emit_tag(&effects, 50, "selection"));
    // The selection landed in the widget's navigation group — where every
    // reader of it looks — with a positive length.
    let key = host.timeline_key(50).unwrap();
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
}

/// **A plain drag over a waveform is the time span it has always been**, and
/// the value band is a step a plan asks for.
///
/// The default is not a matter of taste: a drag on a waveform means *this
/// stretch of time* in every editor there has ever been, and a marquee that
/// also cut a band of amplitudes out of it would answer a question nobody
/// asked. What a band is good for is the script's business, so the script names
/// the step -- which is the D track's own rule about modes.
#[test]
fn a_marquee_restricts_in_value_only_where_the_plan_asks_for_it() {
    let plain = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#;
    let mut host = host_from(plain);
    host.set_timeline_total(50, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    // The default plan, swept with height: the span, and nothing else.
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    let args = emitted_args(&effects, 50).expect("the sweep reports");
    assert_eq!(args.len(), 3, "a plain drag is a time span: {args:?}");
    let editor = host.widget_kind(1, 50).unwrap().editor().unwrap();
    assert_eq!(editor.value_range(), None);

    // The same sweep where the plan asked for the rectangle.
    let boxed = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2,
             "gestures":{"drag":"select_box"}}]}"#;
    let mut host = host_from(boxed);
    host.set_timeline_total(50, 1000);
    let mut g = Gestures::default();
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    assert!(has_emit_tag(&effects, 50, "selection"));
    // The payload grew by the two numbers, and only when there are two.
    let args = emitted_args(&effects, 50).expect("the sweep reports");
    assert_eq!(args.len(), 5, "start, len and the range: {args:?}");
    let editor = host.widget_kind(1, 50).unwrap().editor().unwrap();
    let (min, max) = editor.value_range().expect("a rectangle restricts");
    // The default domain is full-scale amplitude, and the sweep ran from above
    // the centre line to below it: a band inside [-1, 1], the higher edge
    // coming from the *upper* pixel.
    assert!(min > -1.0 && max < 1.0 && max > min, "{min}..{max}");
    // A sweep along one height, under the same plan, clears it: a new selection
    // replaces the old one whole rather than keeping a finished gesture's band.
    g.release(&mut host, &ctx, 600.0, 200.0);
    g.press(&mut host, &ctx, 400.0, 150.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 150.0);
    let args = emitted_args(&effects, 50).expect("the second sweep reports");
    assert_eq!(args.len(), 3, "one axis is the two numbers it always was");
    let editor = host.widget_kind(1, 50).unwrap().editor().unwrap();
    assert_eq!(editor.value_range(), None, "the old range does not linger");
}

/// **`select_box` declines where the picture has one measured axis**, so a plan
/// may name both steps and get a rectangle where there is one to draw and the
/// plain span where there is not -- the mixed stack an editor actually has.
#[test]
fn the_box_step_falls_through_to_the_plain_sweep() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":51,"type":"signal","view":"spectrogram","data":[0.0,0.5,-0.5,1.0],
             "fft_size":4,"gestures":{"drag":"select_box select"}}]}"#,
    );
    host.set_timeline_total(51, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    let args = emitted_args(&effects, 51).expect("the sweep reports");
    assert_eq!(
        args.len(),
        3,
        "the box step declined, the span ran: {args:?}"
    );
    let key = host.timeline_key(51).unwrap();
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
}

/// A view whose vertical measures **frequency** has a second axis and it is not
/// a value: its selection is a band of bins, which is its own field in the
/// document's selection and a later milestone's gesture. So a marquee over a
/// spectrogram stays the one-axis sweep it was, rather than reporting hertz in
/// a range whose reader would take them for amplitudes.
#[test]
fn a_spectral_view_reports_no_value_range() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":51,"type":"signal","view":"spectrogram","data":[0.0,0.5,-0.5,1.0],
             "fft_size":4}]}"#,
    );
    host.set_timeline_total(51, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    let args = emitted_args(&effects, 51).expect("the sweep reports");
    assert_eq!(
        args.len(),
        3,
        "no value range on a frequency axis: {args:?}"
    );
}

/// **The three verbs split where the host's authority does.** A copy is a read
/// and the host may do it: the selected span leaves the element's own contents
/// and lands on the clipboard, typed and with the rate it was taken at. A cut
/// and a paste change data the host does not own, so they leave as intents.
#[test]
fn copy_reads_the_samples_and_cut_and_paste_leave_as_intents() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace",
             "data":[0.0,0.1,0.2,0.3,0.4,0.5,0.6,0.7],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(50, 8);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    // Nothing selected: there is nothing to copy, and nothing is claimed.
    assert!(
        g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
            .is_none()
    );
    // Select samples 2..=5, then copy: the block is the contents itself.
    host.select_timeline(50, 2.0, 5.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("a view under the cursor answers");
    assert!(
        emitted_args(&effects, 50).is_none(),
        "a copy changed nothing, so it reports nothing: {effects:?}"
    );
    assert!(clip.is_whole());
    assert_eq!(clip.doc().unwrap().kind(), "samples");
    assert_eq!(&clip.blobs()[0][..], &[0.2, 0.3, 0.4, 0.5]);

    // Cut: the host owns none of it, so what leaves is the request.
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Cut, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 50).expect("a cut reports");
    assert_eq!(args[0], OscType::String("cut".into()));
    assert_eq!(args.len(), 3, "the span it names: {args:?}");

    // Paste: the position, the kind, the document, and the payload beside it —
    // the clipboard travels *with* the intent, because it is the host's and the
    // owner may never have seen what is on it.
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Paste, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 50).expect("a paste reports");
    assert_eq!(args[0], OscType::String("paste".into()));
    assert_eq!(args[2], OscType::String("samples".into()));
    assert!(matches!(args[4], OscType::Blob(ref b) if b.len() == 16));
}

/// **A source the host cannot read declines, out loud.** A mapped pyramid is an
/// overview and a live view has no addressable past; putting silence on the
/// clipboard would be the one answer worse than saying no, and saying nothing
/// at all teaches "sometimes copy does not work".
#[test]
fn a_copy_the_host_cannot_honestly_make_is_refused() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":52,"type":"signal","view":"trace","bus":0,"rate":"audio"}]}"#,
    );
    host.set_timeline_total(52, 1000);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    host.select_timeline(52, 10.0, 40.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 52).expect("the refusal is reported");
    assert_eq!(args[0], OscType::String("refused".into()));
    assert_eq!(args[1], OscType::String("copy".into()));
    assert!(clip.is_empty(), "and nothing was put on the clipboard");
}

/// **A sweep to the first sample leaves the pointer off the view, and the copy
/// is still the selection's.** Dragging to the very start or end of the contents
/// parks the pointer in the window's margin — or outside the window, where there
/// is no pointer at all — and a copy addressed only to what is under it answered
/// nothing, silently, over a selection plainly on screen. The window's most
/// recent selection is the fallback addressee, and the last one made wins.
#[test]
fn a_block_operation_falls_back_to_the_window_s_last_selection() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":54,"type":"signal","view":"trace",
             "data":[0.0,0.1,0.2,0.3,0.4,0.5,0.6,0.7],"base_bucket":2},
            {"id":55,"type":"signal","view":"trace",
             "data":[1.0,0.9,0.8,0.7,0.6,0.5,0.4,0.3],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(54, 8);
    host.set_timeline_total(55, 8);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();

    // Both views hold one, the second made last: that is the one addressed.
    host.select_timeline(54, 0.0, 4.0);
    host.select_timeline(55, 2.0, 5.0);
    // Off the window entirely — what `CursorLeft` leaves behind.
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, -1.0, -1.0, &mut clip)
        .expect("the selection answers where the pointer does not");
    assert!(emitted_args(&effects, 55).is_none(), "{effects:?}");
    assert_eq!(&clip.blobs()[0][..], &[0.8, 0.7, 0.6, 0.5]);

    // The pointer still wins wherever it is over a view: it lands on the first.
    let mut clip = crate::host::clipboard::Clip::default();
    let rect = placed_rect(&host, &ctx, 54);
    let (cx, cy) = (
        (rect.x + rect.w / 2.0) as f64,
        (rect.y + rect.h / 2.0) as f64,
    );
    g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, cx, cy, &mut clip)
        .expect("the view under the pointer answers");
    assert_eq!(&clip.blobs()[0][..], &[0.0, 0.1, 0.2, 0.3, 0.4]);

    // A selection cleared gives the title up: nothing is addressed by accident.
    host.set_timeline_selection(55, None, Some(0.0));
    host.set_timeline_selection(54, None, Some(0.0));
    let mut clip = crate::host::clipboard::Clip::default();
    assert!(
        g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, -1.0, -1.0, &mut clip)
            .is_none()
    );
}

/// **A mapped take is readable, and a slot does not take that away.** The
/// navigable views are the ones the clipboard was written for, and they are also
/// the ones whose data a loader routes into a GPU slot — so for a while a copy
/// over the very source it was meant for refused, the element holding nothing
/// while the picture on screen was drawn from the samples. The element keeps the
/// pyramid the slot draws (`frame::keep_data`), and the copy reads it.
#[test]
fn a_take_routed_into_a_slot_is_still_the_elements_samples() {
    use crate::host::widget::element::Loaded;
    use crate::waveform::WaveformData;
    use std::sync::Arc;

    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":53,"type":"signal","view":"trace","navigable":1,
             "path":"take.f32","bulk":true,"base_bucket":2}]}"#,
    );
    // What a loader resolved: the mapped file's samples, summarized. It fills
    // the slot, and the element keeps the same pyramid.
    let peaks = Arc::new(WaveformData::from_interleaved(
        &[0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
        1,
        2,
    ));
    let widget = host
        .window_def_mut(1)
        .and_then(|t| t.find_mut(53))
        .expect("the view is in the tree");
    crate::host::frame::keep_data(widget, &Loaded::Peaks(peaks));
    host.set_timeline_total(53, 8);

    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    host.select_timeline(53, 2.0, 5.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("a view under the cursor answers");
    assert!(
        emitted_args(&effects, 53).is_none(),
        "the copy was made, so nothing was refused: {effects:?}"
    );
    assert_eq!(&clip.blobs()[0][..], &[0.2, 0.3, 0.4, 0.5]);
}

#[test]
fn wheel_zooms_the_time_axis_and_emits_the_view() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":60,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(60, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let before = host.timeline_nav(60).unwrap().0.len;
    let effects = g.wheel(&mut host, &ctx, 400.0, 150.0, 1.0);
    let after = host.timeline_nav(60).unwrap().0.len;
    assert!(after < before, "wheel-in shrinks the visible window");
    assert!(has_emit_tag(&effects, 60, "view"));
}

/// The lane count is two halves of one answer: the front knows how many
/// channels reached the card, the widget knows what it does with them. An
/// overlaid trace draws one lane out of four channels, a stacked one four —
/// and the machine, which divides by it, asks rather than matching on a kind.
#[test]
fn a_widget_says_how_it_stacks_what_the_front_uploaded() {
    let host = host_from(
        r#"{"type":"window","children":[
            {"id":60,"type":"signal","view":"trace","data":[0.0,1.0],"navigable":1},
            {"id":61,"type":"signal","view":"trace","data":[0.0,1.0],"navigable":1,
             "overlay":1},
            {"id":62,"type":"label","text":"nothing on the card"}]}"#,
    );
    let tree = host.window_def(1).unwrap();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.slot_channels.insert(60, 4);
    ctx.slot_channels.insert(61, 4);
    assert_eq!(ctx.lanes(60, &tree.find(60).unwrap().kind), 4);
    assert_eq!(ctx.lanes(61, &tree.find(61).unwrap().kind), 1);
    // A widget with no slot was given nothing, and nothing is one lane.
    assert_eq!(ctx.lanes(62, &tree.find(62).unwrap().kind), 1);
}

/// The amplitude axis zooms symmetrically: whatever lane the cursor is
/// over, the window keeps its centre — so every channel's zero line stays
/// at its lane's centre instead of sliding out of the lane. The regression
/// this fixes: the anchor used to be the cursor's height within its lane,
/// which is meaningless for the *other* lanes of one shared window — a
/// wheel near the top of channel 2 pushed every channel's wave to the
/// bottom of its lane, clipped.
#[test]
fn the_amplitude_axis_zooms_about_its_centre_whatever_lane_is_under_the_cursor() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":61,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(61, 1000);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    // Four channels: the body splits into four lanes.
    ctx.slot_channels.insert(61, 4);
    // Wheel over the y-ruler strip (left of the body), high inside the
    // *last* lane — the worst case for a cursor-derived anchor.
    let effects = g.wheel(&mut host, &ctx, 10.0, 212.0, 4.0);
    assert!(has_emit_tag(&effects, 61, "view_y"));
    let (start, len) = host
        .window_def(1)
        .unwrap()
        .find(61)
        .unwrap()
        .kind
        .editor()
        .unwrap()
        .y_view();
    assert!(len < 1.0, "wheel-in shrinks the amplitude window");
    // Zero (display 0.5) sits at the centre of the window, so it lands at
    // the centre of every lane.
    assert!(
        (start + len / 2.0 - 0.5).abs() < 1e-9,
        "the window stays centred on zero: got ({start}, {len})"
    );
}

// ---- the one navigable axis that is not time: a spectrum's frequency ----

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

/// The wheel over a navigable spectrum zooms its **frequency** axis, anchored
/// at the cursor — and it reports the element's own `"view_x"`, not a group's
/// `"view"`, because there is no group: the axis belongs to the element the way
/// every vertical axis already does.
#[test]
fn the_wheel_zooms_a_spectrums_frequency_axis_under_the_cursor() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    assert_eq!(x_window(&host, 80), (0.0, 1.0));
    // Near the right end of the body, so the anchor is unambiguous.
    let effects = g.wheel(&mut host, &ctx, 700.0, 150.0, 4.0);
    assert!(has_emit_tag(&effects, 80, "view_x"));
    let (start, len) = x_window(&host, 80);
    assert!(len < 1.0, "wheel-in shrinks the frequency window");
    assert!(
        start + len > 0.8,
        "the window keeps the frequency under the cursor: got ({start}, {len})"
    );
    // Nothing joined a time axis on the way: a spectrum is in no group.
    assert!(!host.window_def(1).unwrap().find(80).unwrap().is_timeline());
}

/// A drag anywhere on the axis pans it, absolutely from the press snapshot,
/// and `R` puts the whole axis back — the same key that resets the timelines,
/// since to a reader it is the same "show me all of it".
#[test]
fn a_drag_pans_the_frequency_axis_and_r_resets_it() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    g.wheel(&mut host, &ctx, 400.0, 150.0, 6.0); // zoom in, so there is slack
    let (start, len) = x_window(&host, 80);
    assert!(len < 1.0 && start > 0.0);
    g.press(&mut host, &ctx, 400.0, 150.0);
    assert!(g.dragging(), "the axis was grabbed");
    let effects = g.drag_to(&mut host, &ctx, 500.0, 150.0);
    assert!(has_emit_tag(&effects, 80, "view_x"));
    let (panned, panned_len) = x_window(&host, 80);
    assert!(
        panned < start && (panned_len - len).abs() < 1e-9,
        "dragging right walks the window down at a fixed zoom: ({panned}, {panned_len})"
    );
    g.release(&mut host, &ctx, 500.0, 150.0);
    // ...and the same window arrives from the other side: `/gui_set` of the x
    // axis' own keys reaches the element, rather than being swallowed by the
    // group model that owns those keys on a timeline member.
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(80),
                OscType::String("view_start".into()),
                OscType::Float(0.25),
                OscType::String("view_len".into()),
                OscType::Float(0.5),
            ],
        }),
        from(),
    );
    assert_eq!(x_window(&host, 80), (0.25, 0.5));
    let effects = g.reset_timelines(&mut host, &ctx);
    assert!(has_emit_tag(&effects, 80, "view_x"));
    assert_eq!(x_window(&host, 80), (0.0, 1.0), "R restores the whole axis");
}

/// The container's plan decides, and the element takes what it is handed:
/// on a lane, the clip under the cursor moves, empty lane space locates the
/// transport, and the header — beside the axis, on no position at all —
/// does neither.
#[test]
fn a_lanes_plan_grabs_the_clip_first_and_locates_where_there_is_none() {
    let mut host = lane_host();
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let body = {
        let h = interact::hit(&host, 1, 800, 200, 400.0, 100.0, &|_, _| 1).unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    let midy = (body.y + body.h / 2.0) as f64;

    // Over the first clip (the axis spans 0..10000 samples over the body):
    // the element wins, and the drag moves it.
    let on_clip = body.x as f64 + body.w as f64 * 0.02;
    g.press(&mut host, &ctx, on_clip, midy);
    assert!(g.dragging(), "the clip under the cursor was grabbed");
    g.release(&mut host, &ctx, on_clip, midy);

    // Empty lane space: the element declines and the lane's own plan
    // locates the transport there.
    let empty = body.x as f64 + body.w as f64 * 0.5;
    let effects = g.press(&mut host, &ctx, empty, midy);
    assert!(has_emit_tag(&effects, 70, "locate"));
    assert!(!g.dragging());

    // The header strip, left of the axis: no clip, no position, no locate.
    let effects = g.press(&mut host, &ctx, body.x as f64 - 10.0, midy);
    assert!(
        !has_emit_tag(&effects, 70, "locate"),
        "a press beside the axis names no time"
    );
}

/// Shift+drag pans, on every timeline view, because the *axis* claims it
/// before the widget drawn on it ever sees the press.
#[test]
fn shift_drag_pans_whatever_timeline_view_is_under_it() {
    for view in [
        r#"{"id":80,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}"#,
        r#"{"id":80,"type":"field","label":"lane","children":[
               {"id":81,"type":"field","offset":0.0,"dur":1000.0}]}"#,
        r#"{"id":80,"type":"notes","min":48.0,"max":72.0,"notes":[0.0,500.0,60.0,100,0]}"#,
        r#"{"id":80,"type":"field"}"#,
    ] {
        let mut host = host_from(&format!(
            r#"{{"type":"window","margin":0,"children":[{view}]}}"#
        ));
        host.set_timeline_total(80, 10000);
        host.zoom_timeline(80, 0.25, 0.5); // leave room to pan in both directions
        let start_before = host.timeline_nav(80).unwrap().0.start;
        let mut g = Gestures::default();
        let mut ctx = GestureCtx::new(1, 800, 200);
        ctx.shift = true;
        // Near the top edge, where a free-standing ruler (the shortest of
        // the four) also lands.
        g.press(&mut host, &ctx, 500.0, 8.0);
        assert!(g.dragging(), "shift+press on {view} started no drag");
        g.drag_to(&mut host, &ctx, 300.0, 8.0);
        let start_after = host.timeline_nav(80).unwrap().0.start;
        assert!(
            start_after > start_before,
            "dragging left pans the axis right on {view}"
        );
    }
}

/// A sweep on the roll's grid is one marquee: the time span drives the
/// shared selection every linked view follows, and the rectangle it covers
/// in time x pitch picks the notes.
#[test]
fn a_sweep_on_the_roll_selects_the_time_span_and_the_notes_inside_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,
             "notes":[0.0,400.0,60.0,100,0, 6000.0,400.0,61.0,100,0]}]}"#,
    );
    host.set_timeline_total(90, 10000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let grid = {
        let h = interact::hit(&host, 1, 800, 400, 400.0, 100.0, &|_, _| 1).unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    // Sweep the first tenth of the axis, over every pitch the window shows.
    let x0 = grid.x as f64 + 1.0;
    let x1 = grid.x as f64 + grid.w as f64 * 0.1;
    let effects = g.press(&mut host, &ctx, x0, (grid.y + grid.h) as f64 - 1.0);
    assert!(has_emit_tag(&effects, 90, "selection"));
    g.drag_to(&mut host, &ctx, x1, grid.y as f64 + 1.0);
    let key = host.timeline_key(90).unwrap();
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
    assert_eq!(
        selected_notes(&host, 90),
        vec![0],
        "only the note inside the swept rectangle"
    );
}

/// The multi-note selection of a roll — view state no query reports, reached
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

/// The table is the container's, and the wire can set it: a waveform told
/// to pan on a plain drag pans, with no element touched.
#[test]
fn the_gestures_prop_repoints_a_modifier_without_touching_the_element() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":95,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2,
             "gestures":{"drag":"pan","shift":"select"}}]}"#,
    );
    host.set_timeline_total(95, 10000);
    host.zoom_timeline(95, 0.25, 0.5);
    let start_before = host.timeline_nav(95).unwrap().0.start;
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    // Plain drag: pans, and never touches the selection.
    g.press(&mut host, &ctx, 500.0, 150.0);
    g.drag_to(&mut host, &ctx, 300.0, 150.0);
    assert!(host.timeline_nav(95).unwrap().0.start > start_before);
    let key = host.timeline_key(95).unwrap();
    assert_eq!(host.timelines().state(key).unwrap().sel_len, 0.0);
    g.release(&mut host, &ctx, 300.0, 150.0);
    // ...and Shift now does what a plain drag used to.
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.shift = true;
    g.press(&mut host, &ctx, 400.0, 150.0);
    g.drag_to(&mut host, &ctx, 600.0, 150.0);
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
}

/// ...and a live `/gui_set` moves it, on top of the kind's defaults: the
/// modifiers it does not name keep them.
#[test]
fn a_gui_set_of_the_table_keeps_the_modifiers_it_does_not_name() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":96,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(96, 10000);
    set_prop(&mut host, 96, "gestures", r#"{"drag":"locate"}"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let effects = g.press(&mut host, &ctx, 400.0, 150.0);
    assert!(has_emit_tag(&effects, 96, "locate"), "the set took effect");
    // Shift was not named, so it still pans.
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.shift = true;
    g.press(&mut host, &ctx, 400.0, 150.0);
    assert!(g.dragging(), "the default shift plan survived the set");
}

// --- multitrack lanes: the edge auto-scroll ---

/// The header band beside the axis is the lane's own element: its controls
/// take a press there, and the value leaves as an edit-back the way every
/// other host-owned edit does.
/// The pixels a multitrack has that are not a lane — the gap between two of
/// them, the slack under the last one, a margin — are the axis with nothing
/// drawn on them, so the gestures of the axis work there.
#[test]
fn the_windows_one_axis_answers_off_the_lanes() {
    // The example's shape: the lanes inside a vertical scroll view with
    // more room than content, so the plane is pinned and the slack under
    // the lane belongs to nobody.
    let mut host = host_from(
        r#"{"type":"window","margin":8,"flow":"col","children":[
            {"id":30,"type":"plane","axis":"y","zoom":0,"flow":"col","margin":0,
             "content_h":80,"weight":1,"children":[
              {"id":40,"type":"field","label":"lane","h":60,"link":7,"children":[
                {"id":41,"type":"field","offset":0,"dur":1000},
                {"id":42,"type":"field","offset":40000,"dur":1000}]}]}]}"#,
    );
    host.sync_track_totals();
    let key = super::super::timeline::group_key(40, Some(7));
    let mut fx = Vec::new();
    host.set_props(
        40,
        vec![
            ("view_start".into(), serde_json::json!(20_000.0)),
            ("view_len".into(), serde_json::json!(4_000.0)),
        ],
        &mut fx,
    );
    let nav = |h: &Host| h.timelines().nav(key).unwrap();
    let mut ctx = GestureCtx::new(1, 800, 400);
    let (below_x, below_y) = (400.0, 300.0); // under the lane, over nothing

    // The wheel zooms the axis, from off it.
    let mut g = Gestures::default();
    let before = nav(&host).len;
    g.wheel(&mut host, &ctx, below_x, below_y, -1.0);
    assert!(nav(&host).len > before, "the wheel zoomed the one axis out");

    // Ctrl+wheel is still the lanes' thickness.
    ctx.ctrl = true;
    let effects = g.wheel(&mut host, &ctx, below_x, below_y, 1.0);
    assert!(
        host.window_def(1).unwrap().find(40).unwrap().place.h > Some(60.0),
        "the lane grew from a press over empty space"
    );
    assert!(has_emit_tag(&effects, 40, "height"));
    ctx.ctrl = false;

    // And Shift+drag pans it.
    let start = nav(&host).start;
    ctx.shift = true;
    g.press(&mut host, &ctx, below_x, below_y);
    g.drag_to(&mut host, &ctx, below_x - 120.0, below_y);
    assert!(
        nav(&host).start > start,
        "the axis panned from off the lanes"
    );
}

/// ...but the fall-through is for pixels with **nothing on them**. An element
/// that draws a picture of its own and simply has no wheel is not empty: the
/// reader pointed at it, and the wheel there must leave the window's one axis
/// alone. Shift+drag is a different gesture with a documented reach, so it
/// still pans from over that element.
#[test]
fn the_wheel_does_not_fall_through_an_element_that_draws() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":40,"type":"field","label":"lane","h":100,"link":7,"children":[
               {"id":41,"type":"field","offset":0,"dur":1000}]},
            {"id":50,"type":"signal","view":"phase","bus":0,"h":200}]}"#,
    );
    host.sync_track_totals();
    let key = super::super::timeline::group_key(40, Some(7));
    let nav = |h: &Host| h.timelines().nav(key).unwrap();
    let mut ctx = GestureCtx::new(1, 800, 400);
    let mut g = Gestures::default();

    // Deep inside the goniometer, which has no wheel of its own.
    let (over_x, over_y) = (400.0, 250.0);
    assert_eq!(
        host.window_def(1)
            .unwrap()
            .find(50)
            .map(|w| w.kind.is_bare_surface()),
        Some(false),
        "the phasescope draws a picture of its own"
    );
    let before = nav(&host).len;
    g.wheel(&mut host, &ctx, over_x, over_y, -1.0);
    assert_eq!(
        nav(&host).len,
        before,
        "the wheel over an element zoomed the axis behind it"
    );

    // Shift+drag from the same pixel still pans: a different gesture, and its
    // reach over any element is the intended one.
    let start = nav(&host).start;
    ctx.shift = true;
    g.press(&mut host, &ctx, over_x, over_y);
    g.drag_to(&mut host, &ctx, over_x - 120.0, over_y);
    assert!(
        nav(&host).start > start,
        "Shift+drag lost its reach over an element"
    );
}

/// Shift+drag is the **lane's** gesture wherever it starts, clip or no
/// clip. A clip is a container of its own local axis, so it must not answer
/// for a pan; before it declined, a busy arrangement could only be panned
/// in the gaps between its clips.
#[test]
fn shift_drag_over_a_clip_pans_the_lane_it_is_on() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":40,"type":"field","label":"lane","link":7,"children":[
               {"id":41,"type":"field","offset":0,"dur":1000},
               {"id":42,"type":"field","offset":40000,"dur":1000}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 200);
    ctx.shift = true;
    let key = super::super::timeline::group_key(40, Some(7));
    // Zoomed in, so the axis has room to move, and pressing on the clip at
    // the far end of the composition.
    let mut fx = Vec::new();
    host.set_props(
        40,
        vec![
            ("view_start".into(), serde_json::json!(39_000.0)),
            ("view_len".into(), serde_json::json!(2_000.0)),
        ],
        &mut fx,
    );
    let before = host.timelines().nav(key).unwrap().start;
    g.press(&mut host, &ctx, 700.0, 100.0);
    g.drag_to(&mut host, &ctx, 500.0, 100.0);
    let after = host.timelines().nav(key).unwrap().start;
    assert!(
        after > before,
        "the lane's window moved: {before} -> {after}"
    );
    // And the clip stayed where it was: Shift never grabbed it.
    assert!(matches!(
        host.window_def(1).unwrap().find(42).unwrap().kind,
        WidgetKind::Clip { offset, .. } if (offset - 40_000.0).abs() < 0.5
    ));
}

/// The stack a multitrack lives in cannot make its lanes thicker — a
/// plane's zoom is uniform and would stretch time with it — so the lane's
/// own `h` is the knob, and Ctrl+wheel is the gesture that turns it.
#[test]
fn ctrl_wheel_over_a_lane_resizes_it_and_edits_back() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":70,"type":"field","label":"lane","h":120,
             "children":[{"id":71,"type":"field","offset":0.0,"dur":1000.0}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 400);
    ctx.ctrl = true;
    let effects = g.wheel(&mut host, &ctx, 400.0, 60.0, 1.0);
    let h = match &host.window_def(1).unwrap().find(70).unwrap().place.h {
        Some(h) => *h,
        None => panic!("the lane took a height"),
    };
    assert!(h > 120.0, "wheel up makes the lane thicker: {h}");
    assert!(has_emit_tag(&effects, 70, "height"), "and says so");

    // The plain wheel is still the time axis: the lane keeps its thickness.
    ctx.ctrl = false;
    g.wheel(&mut host, &ctx, 400.0, 60.0, 1.0);
    assert_eq!(
        host.window_def(1).unwrap().find(70).unwrap().place.h,
        Some(h)
    );
}

#[test]
fn a_press_on_the_lane_header_works_its_controls_and_edits_back() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":70,"type":"field","label":"lane","mute":false,"level":0.0,
             "children":[{"id":71,"type":"field","offset":0.0,"dur":1000.0}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let (rect, body_x) = {
        let h = interact::hit(&host, 1, 800, 200, 400.0, 100.0, &|_, _| 1).unwrap();
        let lane = h.chain.iter().find(|f| f.id == Some(70)).unwrap();
        (lane.rect, interact::time_of(&h.chain).unwrap().1.body.x)
    };
    let m = *host.metrics_for(1);
    let band = crate::host::timeline::gutter_band(rect, body_x - rect.x);
    let header = match &host.window_def(1).unwrap().find(70).unwrap().kind {
        WidgetKind::Track { header, .. } => header.clone(),
        other => panic!("not a lane: {other:?}"),
    };
    let parts = crate::host::graphics::track::header_parts(band, &header, &m);
    let mid = |r: Rect| ((r.x + r.w / 2.0) as f64, (r.y + r.h / 2.0) as f64);

    // The mute toggles and says so.
    let (x, y) = mid(parts.mute.expect("the lane offers a mute"));
    let effects = g.press(&mut host, &ctx, x, y);
    assert!(has_emit_tag(&effects, 70, "mute"));
    assert_eq!(lane_header_of(&host).mute, Some(true));

    // The fader takes its value from where it was pressed, and keeps
    // taking it while the drag runs.
    let fader = parts.fader.expect("the lane offers a fader");
    let (x, y) = mid(fader);
    let effects = g.press(&mut host, &ctx, x, y);
    assert!(has_emit_tag(&effects, 70, "level"));
    assert!((lane_header_of(&host).level.unwrap() - 0.5).abs() < 0.05);
    g.drag_to(&mut host, &ctx, (fader.x + fader.w) as f64, y);
    assert!((lane_header_of(&host).level.unwrap() - 1.0).abs() < 0.01);
    g.release(&mut host, &ctx, (fader.x + fader.w) as f64, y);

    // The name row names no control: the press falls through to the lane,
    // which names no position beside its axis either.
    let (x, y) = mid(parts.label);
    let effects = g.press(&mut host, &ctx, x, y);
    assert!(!has_emit_tag(&effects, 70, "locate"));
}

fn lane_header_of(host: &Host) -> crate::host::graphics::track::Header {
    match &host.window_def(1).unwrap().find(70).unwrap().kind {
        WidgetKind::Track { header, .. } => header.clone(),
        other => panic!("not a lane: {other:?}"),
    }
}

/// One lane, one short clip, on a long axis — so zooming in leaves most of
/// the timeline off screen, which is the case the edge scroll exists for.
fn lane_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":70,"type":"field","label":"lane","children":[
                {"id":71,"type":"field","offset":0.0,"dur":1000.0},
                {"id":72,"type":"field","offset":9000.0,"dur":1000.0}
            ]}]}"#,
    )
}

fn clip_offset(host: &Host, id: i32) -> f64 {
    match &host.window_def(1).unwrap().find(id).unwrap().kind {
        WidgetKind::Clip { offset, .. } => *offset,
        other => panic!("not a clip: {other:?}"),
    }
}

/// An **automation clip**: the curve is an element filling the clip's curve
/// body, and the container routes the press into it. Two things have to hold
/// at once, and the second is the one the port nearly lost — a body claims its
/// break-points and *declines* everywhere else, so the clip it shares a
/// rectangle with still moves.
/// **The interaction rule between a clip's two levels**, on the case that used
/// to have none: a clip carrying an automation. Pressing the curve's own
/// contents — a break-point, or the line between two of them — selects that
/// layer and edits it; pressing anywhere the curve draws nothing is the clip's,
/// which moves and takes the active layer back. Nothing here is a precedence
/// between claimants: each press asks what is drawn under it.
#[test]
fn a_press_selects_the_layer_it_lands_on_and_the_background_is_the_clips() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"automation","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "points":[0.0,0.0,1,0.0, 500.0,1.0,1,0.0, 1000.0,0.0,1,0.0],
                 "points_min":0.0,"points_max":1.0}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);

    // The clip spans the whole axis, so its peak (t=500, value 1) sits at the
    // middle of the lane body, at its top edge.
    let lane = host
        .layout_window(1, 800, 200)
        .unwrap()
        .iter()
        .find(|p| p.widget.id == Some(81))
        .expect("the clip is placed")
        .rect;
    let peak = ((lane.x + lane.w * 0.5) as f64, lane.y as f64 + 1.0);

    // On the **line** between two points — the rising half sits at value 0.5
    // over the middle of the lane's height at a quarter of its width: the curve
    // takes the press and bends, which is the gesture the lit segment offers
    // and which inside a clip used to be lit and then not delivered.
    let on_line = (
        (lane.x + lane.w * 0.25) as f64,
        (lane.y + lane.h * 0.5) as f64,
    );
    let selected = g.press(&mut host, &ctx, on_line.0, on_line.1);
    assert!(g.dragging(), "the segment took the press");
    // Selecting is announced, once, in the same word a `/gui_set layer` takes:
    // a script that follows the hand hears where it went.
    assert!(
        selected.iter().any(|e| matches!(e,
            GestureEffect::Emit { widget_id: 81, args, .. }
                if args.first() == Some(&OscType::String("layer".into()))
                    && args.get(1) == Some(&OscType::String("points".into())))),
        "{selected:?}"
    );
    g.drag_to(&mut host, &ctx, on_line.0, on_line.1 - 20.0);
    let effects = g.release(&mut host, &ctx, on_line.0, on_line.1 - 20.0);
    assert!(has_emit_tag(&effects, 81, "points"), "{effects:?}");
    assert_eq!(clip_offset(&host, 81), 0.0, "the clip did not move");
    assert_eq!(
        active_layer(&host, 81),
        "points",
        "the curve is what is held"
    );

    // On a point: the same layer, and the drag reports the whole list in the
    // envelope's own units, tagged `points`.
    g.press(&mut host, &ctx, peak.0, peak.1);
    assert!(g.dragging());
    g.drag_to(&mut host, &ctx, peak.0, (lane.y + lane.h) as f64 - 1.0);
    let effects = g.release(&mut host, &ctx, peak.0, (lane.y + lane.h) as f64 - 1.0);
    assert!(has_emit_tag(&effects, 81, "points"), "{effects:?}");
    assert_eq!(clip_offset(&host, 81), 0.0, "the clip itself did not move");

    // Off the curve altogether — the clip's own background, where the
    // automation draws nothing: the clip moves, and the placement is the layer
    // in hand again, so its grips are back. (The peak has just been dragged to
    // the floor, so the curve's second half runs along the bottom edge and the
    // middle of the lane is empty.)
    let away = (
        (lane.x + lane.w * 0.75) as f64,
        (lane.y + lane.h * 0.5) as f64,
    );
    g.press(&mut host, &ctx, away.0, away.1);
    g.drag_to(&mut host, &ctx, away.0 + 40.0, away.1);
    g.release(&mut host, &ctx, away.0 + 40.0, away.1);
    assert!(
        clip_offset(&host, 81) > 0.0,
        "the clip moved under the curve"
    );
    assert_eq!(active_layer(&host, 81), "placement");
}
/// **A press repeated mid-drag is not a new gesture**, which is the
/// single-pointer rule the touch slot states for fingers and nothing stated for
/// the pointer.
///
/// A browser's event stream repeats it: winit turns any `pointermove` carrying
/// a button (`PointerEvent.button != -1`, which some browsers report on a plain
/// move) into a synthesized `MouseInput` whose state is *pressed* while that
/// button is down, so a drag arrives as a fresh press per frame. Taking those
/// re-runs every press-time decision, and a bend anchored at the press is
/// re-anchored to wherever the pointer is now -- the incremental drift the
/// absolute form was written to end, coming back through the door beside it.
/// The desktop's stream cannot do it, which is why it read as a platform
/// difference; the fix is here, so both fronts hand the machine one press.
#[test]
fn a_press_repeated_mid_drag_does_not_re_anchor_the_gesture() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"automation","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "points":[0.0,0.0,1,0.0, 500.0,1.0,1,0.0, 1000.0,0.0,1,0.0],
                 "points_min":0.0,"points_max":1.0}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let lane = host
        .layout_window(1, 800, 200)
        .unwrap()
        .iter()
        .find(|p| p.widget.id == Some(81))
        .expect("the clip is placed")
        .rect;
    let on_line = (
        (lane.x + lane.w * 0.25) as f64,
        (lane.y + lane.h * 0.5) as f64,
    );
    // What the gesture amounts to: the break-point list the release reports.
    let bend = |effects: &[GestureEffect]| {
        effects
            .iter()
            .find_map(|e| match e {
                GestureEffect::Emit {
                    widget_id: 81,
                    args,
                    ..
                } if args.first() == Some(&OscType::String("points".into())) => Some(args.clone()),
                _ => None,
            })
            .expect("the release reports the curve")
    };

    // One press, then a drag upward, with a repeated press on every step —
    // exactly the stream a browser produces.
    g.press(&mut host, &ctx, on_line.0, on_line.1);
    assert!(g.dragging(), "the segment took the press");
    for dy in [10.0, 20.0, 30.0] {
        g.drag_to(&mut host, &ctx, on_line.0, on_line.1 - dy);
        g.press(&mut host, &ctx, on_line.0, on_line.1 - dy);
    }
    let with_repeats = bend(&g.release(&mut host, &ctx, on_line.0, on_line.1 - 30.0));

    // The same drag, pressed once, is the truth to match.
    let mut clean = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"automation","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "points":[0.0,0.0,1,0.0, 500.0,1.0,1,0.0, 1000.0,0.0,1,0.0],
                 "points_min":0.0,"points_max":1.0}]}]}"#,
    );
    clean.sync_track_totals();
    let mut g2 = Gestures::default();
    g2.press(&mut clean, &ctx, on_line.0, on_line.1);
    for dy in [10.0, 20.0, 30.0] {
        g2.drag_to(&mut clean, &ctx, on_line.0, on_line.1 - dy);
    }
    let once = bend(&g2.release(&mut clean, &ctx, on_line.0, on_line.1 - 30.0));
    assert_eq!(
        with_repeats, once,
        "the repeated presses re-anchored the bend"
    );
}

/// The wire name of the layer a container is being edited on — what a
/// `/gui_query` would report and what the `"layer"` payload announces.
fn active_layer(host: &Host, id: i32) -> String {
    let w = host.window_def(1).unwrap().find(id).unwrap();
    crate::host::layers::active(w).name(w)
}

/// **An edge drag is a trim**: the clip shows less of its contents and the
/// contents stands still. Pulling the start edge right advances the placement,
/// the duration *and* the window over the source by the same amount, and the
/// edit-back says all three — an owner told only where the clip sits would
/// re-cut the wrong part of the take.
#[test]
fn trimming_a_clips_head_moves_its_window_over_the_samples() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"takes","children":[
                {"id":81,"type":"field","offset":0.0,"dur":8.0,
                 "data":[0.0,0.2,0.4,0.6,0.8,1.0,0.5,0.0]}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let clip = placed_rect(&host, &ctx, 81);
    let grip_w = host.metrics_for(1).grip_w;
    let midy = (clip.y + clip.h * 0.5) as f64;
    let on_start_grip = (clip.x + grip_w * 0.5) as f64;
    // A quarter of the way in: two of the eight frames.
    let quarter = (clip.x + clip.w * 0.25) as f64;

    g.press(&mut host, &ctx, on_start_grip, midy);
    g.drag_to(&mut host, &ctx, quarter, midy);
    let effects = g.release(&mut host, &ctx, quarter, midy);
    let args = effects
        .iter()
        .find_map(|e| match e {
            GestureEffect::Emit {
                widget_id: 81,
                args,
                ..
            } if args.first() == Some(&OscType::String("clip".into())) => Some(args.clone()),
            _ => None,
        })
        .expect("the trim reported");
    let value = |i: usize| match args[i] {
        OscType::Float(f) => f,
        _ => panic!("not a number: {:?}", args[i]),
    };
    assert!(value(1) > 0.0, "the clip begins later: {}", value(1));
    assert!(value(2) < 8.0, "and is shorter: {}", value(2));
    assert!(
        (value(3) - value(1)).abs() < 0.01,
        "the window moved with the edge: start {} for offset {}",
        value(3),
        value(1)
    );

    // ...and the end edge stops where the contents does, because this clip
    // does not loop: there is nothing past the eighth frame to show or play.
    let on_end_grip = (clip.x + clip.w - grip_w * 0.5) as f64;
    g.press(&mut host, &ctx, on_end_grip, midy);
    g.drag_to(&mut host, &ctx, (clip.x + clip.w) as f64 + 400.0, midy);
    g.release(&mut host, &ctx, (clip.x + clip.w) as f64 + 400.0, midy);
    let (start, dur) = clip_window(&host, 81);
    assert!(
        (start + dur - 8.0).abs() < 0.01,
        "the window ends at the take's end: {start} + {dur}"
    );
}

/// **Every pointer question reads the shape the element draws**, not just the
/// press: an element drawn rounder or smaller than its cell answers the wheel
/// only where it is drawn, and the air around it belongs to whatever the
/// container puts there. A wheel is worse to get wrong than a press, since it
/// is not aimed at all — it is where the hand happened to leave the pointer.
#[test]
fn the_wheel_reads_the_same_shape_the_press_does() {
    /// A leaf drawn as the disc inscribed in its cell, answering both the press
    /// and the wheel — the two questions that must agree on where it is.
    #[derive(Debug, Clone)]
    struct Dial;
    impl crate::host::widget::Element for Dial {
        fn set(&mut self, _key: &str, _v: &serde_json::Value) -> bool {
            false
        }
        fn draw(
            &self,
            _d: &mut crate::host::paint::Draw,
            _ctx: &crate::host::widget::element::Ctx,
        ) {
        }
        fn hit_area(
            &self,
            input: &crate::host::widget::element::Input,
        ) -> crate::host::widget::element::HitArea {
            let r = input.rect;
            crate::host::widget::element::HitArea::Disc {
                cx: r.x + r.w * 0.5,
                cy: r.y + r.h * 0.5,
                r: r.w.min(r.h) * 0.5,
            }
        }
        fn wheel(
            &mut self,
            _at: (f64, f64),
            _delta: (f64, f64),
            _input: &crate::host::widget::element::Input,
        ) -> Option<crate::host::widget::element::Events> {
            Some(crate::host::widget::element::Events::value(OscType::Int(1)))
        }
        fn clone_box(&self) -> Box<dyn crate::host::widget::Element> {
            Box::new(self.clone())
        }
    }
    crate::host::widget::element::register("test_dial", |_props, _blobs| Ok(Box::new(Dial)));

    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"test_dial","w":200,"h":200}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 400, 400);
    let cell = placed_rect(&host, &ctx, 90);
    let centre = (
        (cell.x + cell.w * 0.5) as f64,
        (cell.y + cell.h * 0.5) as f64,
    );
    let effects = g.wheel(&mut host, &ctx, centre.0, centre.1, 1.0);
    assert!(
        first_emit(&effects, 90).is_some(),
        "the dial answered where it is drawn: {effects:?}"
    );
    // The corner of the same cell is outside the disc — and outside the hit
    // slop around it, which is a few pixels and not a hundred.
    let corner = (cell.x as f64 + 1.0, cell.y as f64 + 1.0);
    let effects = g.wheel(&mut host, &ctx, corner.0, corner.1, 1.0);
    assert!(
        first_emit(&effects, 90).is_none(),
        "the corner is the window's: {effects:?}"
    );
}

/// **Split and join are the placement layer's edit verbs**, and both leave as
/// intents: the host holds no composition, so it says where the cut falls and
/// which clips are to be read as one, and the owner answers with the tree that
/// then stands.
#[test]
fn a_clip_is_cut_at_the_cursor_and_joined_with_what_touches_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"takes","children":[
                {"id":81,"type":"field","offset":0.0,"dur":400.0,"data":[0.0,1.0]},
                {"id":82,"type":"field","offset":400.0,"dur":400.0,"data":[0.0,1.0]},
                {"id":83,"type":"field","offset":900.0,"dur":100.0,"data":[0.0,1.0]}]}]}"#,
    );
    host.sync_track_totals();
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let clip = placed_rect(&host, &ctx, 81);
    let midy = (clip.y + clip.h * 0.5) as f64;
    let quarter = (clip.x + clip.w * 0.25) as f64;

    // No time cursor placed: the pointer is where the cut falls, stated in the
    // clip's own time.
    let effects = g
        .clip_verb(&mut host, &ctx, ClipEdit::Split, quarter, midy)
        .expect("the pointer is over a clip");
    let args = first_emit(&effects, 81).expect("the split reported");
    assert_eq!(args[0], OscType::String("split".into()));
    match args[1] {
        OscType::Float(t) => assert!(
            (t - 100.0).abs() < 4.0,
            "a quarter of the way into a 400-long clip: {t}"
        ),
        ref other => panic!("not a time: {other:?}"),
    }

    // Join: the clip under the pointer and what touches it — the third clip
    // starts a hundred samples later, so it is a different run and stays out.
    let effects = g
        .clip_verb(&mut host, &ctx, ClipEdit::Join, quarter, midy)
        .expect("the pointer is over a clip");
    let args = first_emit(&effects, 81).expect("the join reported");
    assert_eq!(
        args,
        vec![
            OscType::String("join".into()),
            OscType::Int(81),
            OscType::Int(82)
        ]
    );

    // A clip nothing touches has nothing to be joined with, so the key falls
    // through to whatever else the window does with it.
    let alone = placed_rect(&host, &ctx, 83);
    let on_alone = ((alone.x + alone.w * 0.5) as f64, midy);
    assert!(
        g.clip_verb(&mut host, &ctx, ClipEdit::Join, on_alone.0, on_alone.1)
            .is_none()
    );
}

/// The arguments of the first `/gui_event` a verb emitted for `id`.
fn first_emit(effects: &[GestureEffect], id: i32) -> Option<Vec<OscType>> {
    effects.iter().find_map(|e| match e {
        GestureEffect::Emit {
            widget_id, args, ..
        } if *widget_id == id => Some(args.clone()),
        _ => None,
    })
}

/// A clip's window over its contents: where it starts reading, and how long it
/// reads for.
fn clip_window(host: &Host, id: i32) -> (f64, f64) {
    let w = host.window_def(1).unwrap().find(id).unwrap();
    match &w.kind {
        WidgetKind::Clip { dur, .. } => (w.window.unwrap_or_default().start, *dur),
        other => panic!("not a clip: {other:?}"),
    }
}

/// **A locked body is not a layer a hand can take, so the clip is what moves.**
///
/// The defect this closes, in one press: `editable=false` used to be answered
/// by *consuming* the press with a refusal, so a clip whose contents were
/// locked could not be moved either — and where contents sits is the
/// composition's while what it holds is the contents's, which is precisely the
/// distinction that was missing. A layer that cannot be edited is never
/// selected by pointing at it, so the press lands where it always should have.
#[test]
fn a_locked_layer_hands_the_press_to_the_clip_it_is_drawn_in() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"pattern","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "min":48.0,"max":72.0,"editable":false,
                 "notes":[0.0,1000.0,60.0,100,0]}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let clip = placed_rect(&host, &ctx, 81);
    // Squarely on the note, which fills the clip: the one press that used to be
    // eaten by the refusal.
    let on_note = (
        (clip.x + clip.w * 0.5) as f64,
        (clip.y + clip.h * 0.5) as f64,
    );
    g.press(&mut host, &ctx, on_note.0, on_note.1);
    let effects = g.drag_to(&mut host, &ctx, on_note.0 + 60.0, on_note.1);
    g.release(&mut host, &ctx, on_note.0 + 60.0, on_note.1);
    assert!(
        !has_emit_tag(&effects, 81, "notes"),
        "the locked notes did not move: {effects:?}"
    );
    assert!(clip_offset(&host, 81) > 0.0, "the clip did");
    assert_eq!(active_layer(&host, 81), "placement");
}

/// **Selecting a layer is an operation, not a gesture**: `/gui_set layer` puts
/// a hand on the automation without a pointer anywhere near it, and a query
/// answers with the same word. The press rule is one caller of the same door.
#[test]
fn a_script_selects_a_layer_and_the_query_answers_with_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "data":[0.0,1.0,0.0],
                 "points":[0.0,0.0,1,0.0, 1000.0,1.0,1,0.0],
                 "points_min":0.0,"points_max":1.0}]}]}"#,
    );
    host.sync_track_totals();
    assert_eq!(active_layer(&host, 81), "placement", "the default");
    set_layer(&mut host, 81, "points");
    assert_eq!(active_layer(&host, 81), "points");
    // A name this clip has no layer for changes nothing, the way any unusable
    // value does.
    set_layer(&mut host, 81, "nonsense");
    assert_eq!(active_layer(&host, 81), "points");
    // ...and back to the placement, which is what a clip's own layer is called
    // on the way in as well.
    set_layer(&mut host, 81, "clip");
    assert_eq!(active_layer(&host, 81), "placement");
}

/// `/gui_set layer`, as the wire sends it.
fn set_layer(host: &mut Host, id: i32, name: &str) {
    let mut effects = Vec::new();
    host.set_props(
        id,
        vec![("layer".into(), serde_json::json!(name))],
        &mut effects,
    );
}

/// The `(start, dur)` of a roll's first note, read off the `notes` edit-back —
/// the payload a driver acts on, so asserting on it is asserting on what the
/// script would be told.
fn first_note(effects: &[GestureEffect]) -> (f32, f32) {
    effects
        .iter()
        .find_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("notes".into())) =>
            {
                match args[1..] {
                    [OscType::Float(start), OscType::Float(dur), ..] => Some((start, dur)),
                    _ => panic!("malformed notes payload: {args:?}"),
                }
            }
            _ => None,
        })
        .expect("a notes edit-back")
}

/// **A note dragged inside a clip stops at the clip's far edge.** The bug this
/// fixes: the note kept going, out of the rectangle that draws it — still in
/// the list, still sounding, and visible only by resizing the clip by hand.
///
/// The clip's length is not stretched to take it, because a clip's length is
/// what its *own* edge says: content that lengthened its container would make
/// one gesture (nudge a note) do two things (and move the piece's end with it).
#[test]
fn a_note_dragged_in_a_clip_stops_at_the_clips_edge() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"lead","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "min":48.0,"max":72.0,"notes":[0.0,200.0,60.0,100,0]}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let clip = placed_rect(&host, &ctx, 81);
    // The note is the first fifth of the clip; grab its middle, clear of the
    // edges (which resize it) and of the pitch window's rim.
    let midy = (clip.y + clip.h * 0.5) as f64;
    let on_note = (clip.x + clip.w * 0.1) as f64;

    g.press(&mut host, &ctx, on_note, midy);
    assert!(g.dragging(), "the press grabbed the note");
    // Drag far past the clip's right edge: the note travels, and its **tail**
    // parks on the edge — the whole note stays inside, not just its onset.
    g.drag_to(&mut host, &ctx, (clip.x + clip.w) as f64 + 400.0, midy);
    // The edit leaves whole on the release — one gesture, one edit.
    let effects = g.release(&mut host, &ctx, (clip.x + clip.w) as f64 + 400.0, midy);
    let (start, dur) = first_note(&effects);
    assert_eq!(dur, 200.0, "a move keeps the duration");
    assert_eq!(start, 800.0, "the tail stopped at the clip's dur (1000)");
    assert_eq!(
        clip_offset(&host, 81),
        0.0,
        "and the clip neither moved nor grew"
    );
}

fn clip_dur(host: &Host, id: i32) -> f64 {
    match &host.window_def(1).unwrap().find(id).unwrap().kind {
        WidgetKind::Clip { dur, .. } => *dur,
        other => panic!("not a clip: {other:?}"),
    }
}

/// **A press on a lit grip resizes the clip, whatever the body has there.**
/// The bug this fixes: the clip's bodies were offered every press first, so
/// the dozen pixels of the grip went to whatever the body found under them — a
/// note at the end of a roll clip moved instead of the clip's edge, from a
/// cursor sitting on the arrow that promised the opposite. An affordance that
/// is drawn has to be the one that acts.
#[test]
fn a_press_on_a_clips_grip_resizes_it_rather_than_the_note_under_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"lead","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0,
                 "min":48.0,"max":72.0,"notes":[900.0,100.0,60.0,100,0]}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    let clip = placed_rect(&host, &ctx, 81);
    let m = host.metrics_for(1);
    // The last note fills the clip's last tenth, so it is drawn *under* the end
    // grip: the press has to choose, and the grip is what is lit there.
    let midy = (clip.y + clip.h * 0.5) as f64;
    let on_grip = (clip.x + clip.w - m.grip_w * 0.5) as f64;

    g.press(&mut host, &ctx, on_grip, midy);
    assert!(g.dragging(), "the press was taken");
    let dragged = g.drag_to(&mut host, &ctx, on_grip + 60.0, midy);
    let effects = g.release(&mut host, &ctx, on_grip + 60.0, midy);
    assert!(
        !has_emit_tag(&dragged, 81, "clip"),
        "the resize is one edit, delivered at the end"
    );
    assert!(
        !has_emit_tag(&effects, 81, "notes"),
        "the note under the grip was not touched"
    );
    assert!(has_emit_tag(&effects, 81, "clip"), "the clip resized");
    assert!(
        clip_dur(&host, 81) > 1000.0,
        "the edge moved out: {}",
        clip_dur(&host, 81)
    );
    assert_eq!(clip_offset(&host, 81), 0.0, "and the other end stayed put");

    // Clear of the grip, the same note is the body's again — which is the half
    // of the rule that keeps the roll editable at all.
    let on_note = (clip.x + clip.w * 0.93) as f64;
    g.press(&mut host, &ctx, on_note, midy);
    g.drag_to(&mut host, &ctx, on_note - 40.0, midy);
    let effects = g.release(&mut host, &ctx, on_note - 40.0, midy);
    assert!(has_emit_tag(&effects, 81, "notes"), "the note moved");
}

/// **A clip cannot be dragged out of existence, and the way back is the zoom.**
/// The bug this fixes: an edge dragged past the other one left `dur` at zero,
/// and a clip of no duration draws no rectangle — nothing to press, no way back
/// at any zoom. It stops at **one sample** now (the grid says where an edge
/// lands, not how short a clip may be) and is drawn as a **line** rather than a
/// block: the line tracks the zoom the whole way down, so it never lies about
/// the length, and zooming in is what brings it back to a width the hand takes.
#[test]
fn a_clip_shrinks_to_one_sample_and_the_zoom_brings_it_back() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":80,"type":"field","label":"lane","children":[
                {"id":81,"type":"field","offset":0.0,"dur":1000.0},
                {"id":82,"type":"field","offset":9000.0,"dur":1000.0}]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    // A second clip far down the lane holds the axis open, so shrinking the
    // first one does not refit the window around what is left of it -- which
    // is the case the line exists for.
    let clip = placed_rect(&host, &ctx, 81);
    let m = *host.metrics_for(1);
    let midy = (clip.y + clip.h * 0.5) as f64;

    // Grab the end and drag it far past the start — past the lane, even.
    let on_grip = (clip.x + clip.w - m.grip_w * 0.5) as f64;
    g.press(&mut host, &ctx, on_grip, midy);
    g.drag_to(&mut host, &ctx, clip.x as f64 - 300.0, midy);
    g.release(&mut host, &ctx, clip.x as f64 - 300.0, midy);
    assert_eq!(
        (clip_offset(&host, 81), clip_dur(&host, 81)),
        (0.0, 1.0),
        "one sample left, and the end it was not holding stayed put"
    );

    // The rectangle is read through the lane's **group** axis, which is the one
    // the press maps through. One sample of a thousand is far under a pixel
    // here: it comes back as a line, and a line is all it claims to be.
    let placed = |host: &Host| {
        let h = interact::hit(host, 1, 800, 200, 400.0, midy, &|_, _| 1).unwrap();
        let t = interact::time_of(&h.chain).unwrap().1;
        crate::host::graphics::track::clip_x_range(
            t.body,
            &t.nav,
            clip_offset(host, 81),
            clip_dur(host, 81),
            m.divider_w,
        )
        .expect("the shrunk clip is still drawn")
    };
    let (x0, x1) = placed(&host);
    assert!(
        (x1 - x0 - m.divider_w).abs() < 0.01,
        "a hairline, not a block that freezes the apparent length: {x0}..{x1}"
    );

    // **Zoom in and it comes back**, which is the whole point of the line: it
    // marks where to zoom, and the sample widens with the window until it is
    // wide enough to take.
    for _ in 0..8 {
        host.zoom_timeline(80, 0.5, 0.0);
    }
    let (x0, x1) = placed(&host);
    assert!(
        x1 - x0 > m.grip_w,
        "the sample is a rectangle again at this zoom: {x0}..{x1}"
    );

    // And the same gesture grows it back — the state is reversible, which is
    // what "it disappeared" meant.
    let back = (x1 - 1.0) as f64;
    g.press(&mut host, &ctx, back, midy);
    assert!(g.dragging(), "the clip offers its edge again");
    g.drag_to(&mut host, &ctx, back + 200.0, midy);
    g.release(&mut host, &ctx, back + 200.0, midy);
    assert!(
        clip_dur(&host, 81) > 1.0,
        "grown back: {}",
        clip_dur(&host, 81)
    );
}

/// A clip dragged against the lane's edge pulls the view along, so it can
/// travel further than the visible window. The regression this fixes: the
/// drag mapped the cursor through the *press-time* window and nothing
/// scrolled, so a clip could never move more than one window's worth —
/// zoomed in, that was a sliver, and holding at the edge did nothing at all.
#[test]
fn a_clip_dragged_to_the_edge_pulls_the_view_along() {
    let mut host = lane_host();
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);

    // Zoom in hard, anchored at the left: the window shows a fraction of
    // the timeline, and the first clip sits at its left end.
    for _ in 0..6 {
        host.zoom_timeline(70, 0.6, 0.0);
    }
    let (before, _) = host.timeline_nav(70).unwrap();
    assert!(before.len < 2000.0, "zoomed in: {}", before.len);

    // Grab the clip and drag it hard against the right edge.
    // (past the lane's 96 px header strip, so the press lands on the clip)
    g.press(&mut host, &ctx, 300.0, 100.0);
    assert!(g.dragging(), "the press grabbed the clip");
    g.drag_to(&mut host, &ctx, 790.0, 100.0);
    let parked = clip_offset(&host, 71);
    assert!(parked > 0.0, "the drag moved it: {parked}");
    assert!(g.edge_scrolling(790.0), "and it is pinned at the edge");
    let (at_edge, _) = host.timeline_nav(70).unwrap();
    assert_eq!(at_edge.start, before.start, "the drag alone does not pan");

    // Now hold there: every tick pans the view and carries the clip.
    let mut effects = Vec::new();
    for _ in 0..10 {
        effects = g.tick(&mut host, &ctx, 790.0, 0.0, 1.0 / 30.0);
    }
    let (after, _) = host.timeline_nav(70).unwrap();
    assert!(
        after.start > at_edge.start,
        "the window followed the drag: {} -> {}",
        at_edge.start,
        after.start
    );
    assert!(
        clip_offset(&host, 71) > parked,
        "and the clip travelled with it: {parked} -> {}",
        clip_offset(&host, 71)
    );
    // The **view** move is reported as it goes: a pan is the container's own
    // state and a script following the axis needs it now, not at the end.
    // The view move is the *lane's* — the group member, not the clip.
    assert!(has_emit_tag(&effects, 70, "view"));
    // The clip's *edit* is not: one gesture is one edit, so it leaves whole on
    // the release, wherever the scrolling took it.
    assert!(!has_emit_tag(&effects, 71, "clip"), "not once per tick");
    let done = g.release(&mut host, &ctx, 790.0, 0.0);
    assert!(has_emit_tag(&done, 71, "clip"), "and once at the end");

    // A cursor clear of the margins scrolls nothing.
    let (held, _) = host.timeline_nav(70).unwrap();
    let idle = g.tick(&mut host, &ctx, 400.0, 0.0, 1.0 / 30.0);
    assert_eq!(host.timeline_nav(70).unwrap().0.start, held.start);
    assert!(idle.is_empty());

    // And the scroll stops with the drag.
    g.release(&mut host, &ctx, 790.0, 100.0);
    let (dropped, _) = host.timeline_nav(70).unwrap();
    g.tick(&mut host, &ctx, 790.0, 0.0, 1.0 / 30.0);
    assert_eq!(host.timeline_nav(70).unwrap().0.start, dropped.start);
}

/// Dragging keeps the zoom and scrolls, **from the untouched view too**.
/// The regression this fixes: a window showing exactly the whole timeline
/// was refitted to the new total on every drag step, so extending the
/// content zoomed the axis out from under the cursor instead of scrolling —
/// and the edge scroll's pan was overwritten as fast as it was applied. It
/// only appeared to work once the zoom had been changed at least once,
/// which is what took the window off the exact-full case.
#[test]
fn dragging_from_the_full_view_scrolls_instead_of_zooming_out() {
    let mut host = lane_host();
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    // Untouched: the window shows the whole timeline, exactly.
    let (before, total) = host.timeline_nav(70).unwrap();
    assert_eq!(before.len, total as f64, "showing it all, never zoomed");

    // Drag the far clip past the end and hold at the edge. It spans
    // 9000..10000 of a 10000-sample axis drawn over the body (96..800), so
    // it occupies roughly the last 70 px.
    g.press(&mut host, &ctx, 760.0, 100.0);
    assert!(g.dragging(), "the press grabbed the far clip");
    g.drag_to(&mut host, &ctx, 790.0, 100.0);
    for _ in 0..20 {
        g.tick(&mut host, &ctx, 790.0, 0.0, 1.0 / 30.0);
    }
    let (after, grown) = host.timeline_nav(70).unwrap();
    assert!(grown > total, "the content grew with the clip");
    assert!(
        (after.len - before.len).abs() < 1.0,
        "the zoom held: {} -> {}",
        before.len,
        after.len
    );
    assert!(
        after.start > before.start,
        "and the axis scrolled: {} -> {}",
        before.start,
        after.start
    );
    g.release(&mut host, &ctx, 790.0, 100.0);
}

/// The left edge scrolls the other way, and never past the axis origin.
#[test]
fn the_left_edge_scrolls_back_and_stops_at_the_origin() {
    let mut host = lane_host();
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);
    for _ in 0..6 {
        host.zoom_timeline(70, 0.6, 0.0);
    }
    // Start from a window well into the timeline, holding the far clip.
    host.pan_timeline(70, 9000.0);
    let (before, _) = host.timeline_nav(70).unwrap();
    assert!(before.start > 0.0);
    g.press(&mut host, &ctx, 400.0, 100.0);
    g.drag_to(&mut host, &ctx, 10.0, 100.0);
    for _ in 0..10 {
        g.tick(&mut host, &ctx, 10.0, 0.0, 1.0 / 30.0);
    }
    let (after, _) = host.timeline_nav(70).unwrap();
    assert!(after.start < before.start, "the window walked back");
    // Keep holding: it parks at the origin instead of running negative.
    for _ in 0..2000 {
        g.tick(&mut host, &ctx, 10.0, 0.0, 1.0 / 30.0);
    }
    assert_eq!(host.timeline_nav(70).unwrap().0.start, 0.0);
    assert!(
        clip_offset(&host, 72) >= 0.0,
        "and the clip never goes negative"
    );
}

// --- score ---

/// A one-score window, the page fitted 1:1 into the child rect: a window of
/// 1012x412 gives the child (6,6,1000,400), matching the 1000x400 viewBox.
#[cfg(feature = "notation")]
fn score_host() -> Host {
    // Editable, so the drag tests exercise the transpose gesture; the
    // read-only default is covered by its own test below.
    host_from(
        r#"{"type":"window","children":[
            {"id":80,"type":"score","vb":[1000,400],"editable":true,
             "glyphs":{"E0A4":"M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
             "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"},
                      {"k":"glyph","cp":"E0A4","xf":[500,200,0.72,-0.72],"id":"n1"}]}]}"#,
    )
}

/// What the page says is selected **now** — read the way a `/gui_query` reads
/// it, since a ported leaf answers for itself rather than showing its variant.
#[cfg(feature = "notation")]
fn score_selected(host: &Host) -> Option<String> {
    let info = host.window_def(1).unwrap().find(80).unwrap().kind.info();
    match info.iter().find(|(k, _)| k == "selected") {
        Some((_, serde_json::Value::String(s))) if !s.is_empty() => Some(s.clone()),
        Some(_) => None,
        None => panic!("not a score: {info:?}"),
    }
}

#[cfg(feature = "notation")]
fn element_emits(effects: &[GestureEffect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("element".into())) =>
            {
                match &args[1..] {
                    [OscType::String(s)] => Some(s.clone()),
                    _ => panic!("malformed element payload: {args:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn a_press_on_the_score_selects_the_element_and_emits_its_id() {
    let mut host = score_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    // the notehead sits at page (500, 200) -> child rect origin (6, 6)
    let effects = g.press(&mut host, &ctx, 556.0, 196.0);
    assert_eq!(element_emits(&effects), vec!["n1".to_string()]);
    assert_eq!(score_selected(&host).as_deref(), Some("n1"));
    // Clicking the same element again selects nothing new, so the script hears
    // nothing. Each click is a press **and** a release: the press alone is held,
    // because it may become a drag, and the machine refuses a second press while
    // it is.
    g.release(&mut host, &ctx, 556.0, 196.0);
    let again = g.press(&mut host, &ctx, 556.0, 196.0);
    assert!(
        element_emits(&again).is_empty(),
        "a re-press re-reported the selection: {again:?}"
    );
    // blank paper clears it, reported as an empty id
    g.release(&mut host, &ctx, 556.0, 196.0);
    let cleared = g.press(&mut host, &ctx, 106.0, 386.0);
    assert_eq!(element_emits(&cleared), vec![String::new()]);
    assert_eq!(score_selected(&host), None);
}

#[cfg(feature = "notation")]
fn transpose_emits(effects: &[GestureEffect]) -> Vec<(String, i32)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("transpose".into())) =>
            {
                match &args[1..] {
                    [OscType::String(s), OscType::Int(n)] => Some((s.clone(), *n)),
                    _ => panic!("malformed transpose payload: {args:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn dragging_a_note_up_the_staff_transposes_it_in_diatonic_steps() {
    let mut host = score_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    // grab the notehead at page (500, 200); the page is fitted 1:1, so a
    // diatonic step is the default 90 page units = 90 px
    g.press(&mut host, &ctx, 556.0, 196.0);
    // Two steps up. The displacement is drawn while the drag lasts and reports
    // nothing on the way — so what the machine owes it is the frame that draws
    // it, and the edit only travels on release.
    let moving = g.drag_to(&mut host, &ctx, 556.0, 16.0);
    assert!(transpose_emits(&moving).is_empty(), "nothing until release");
    assert!(moving.iter().any(|e| matches!(e, GestureEffect::Redraw(1))));
    // the release asks the client for the edit, in steps
    let effects = g.release(&mut host, &ctx, 556.0, 16.0);
    assert_eq!(transpose_emits(&effects), vec![("n1".to_string(), 2)]);
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn a_press_that_does_not_move_the_note_stays_a_selection() {
    let mut host = score_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    g.press(&mut host, &ctx, 556.0, 196.0);
    // wandering back and forth within one step is not an edit
    g.drag_to(&mut host, &ctx, 556.0, 240.0);
    let effects = g.release(&mut host, &ctx, 556.0, 240.0);
    assert!(transpose_emits(&effects).is_empty(), "no step, no edit");
    assert_eq!(score_selected(&host).as_deref(), Some("n1"));
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn a_read_only_score_selects_but_a_drag_does_not_transpose() {
    // The same host without `editable`: a press still selects and reports
    // the element (inspecting a page is not editing it), but dragging it a
    // full step neither previews nor emits a transpose.
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":80,"type":"score","vb":[1000,400],
             "glyphs":{"E0A4":"M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
             "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"},
                      {"k":"glyph","cp":"E0A4","xf":[500,200,0.72,-0.72],"id":"n1"}]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    let picked = g.press(&mut host, &ctx, 556.0, 196.0);
    assert_eq!(element_emits(&picked), vec!["n1".to_string()]);
    assert_eq!(score_selected(&host).as_deref(), Some("n1"));
    // two full steps up: nothing is displaced, and nothing is asked for
    let moving = g.drag_to(&mut host, &ctx, 556.0, 16.0);
    assert!(
        transpose_emits(&moving).is_empty(),
        "a read-only page edited: {moving:?}"
    );
    let effects = g.release(&mut host, &ctx, 556.0, 16.0);
    assert!(transpose_emits(&effects).is_empty(), "read-only: no edit");
}

// --- piano ---

/// A one-piano window (no overview, no label: the keys fill the widget
/// rect), plus the layout the gestures see. The window is 712x132 so the
/// child rect is (6,6,700,120): one octave C4..C5 = 8 white keys.
fn piano_host(extra: &str) -> (Host, piano::Layout) {
    let json = format!(
        r#"{{"type":"window","children":[
            {{"id":70,"type":"keys","min":60,"max":72,"overview":0{extra}}}]}}"#
    );
    let host = host_from(&json);
    let l = piano::layout(
        Rect::new(6.0, 6.0, 700.0, 120.0),
        60,
        72,
        false,
        false,
        &Metrics::default(),
    );
    (host, l)
}

fn note_emits(effects: &[GestureEffect]) -> Vec<(i32, i32, i32, i32)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("note".into())) =>
            {
                match args[1..] {
                    [
                        OscType::Int(p),
                        OscType::Int(v),
                        OscType::Int(s),
                        OscType::Int(c),
                    ] => Some((p, v, s, c)),
                    _ => panic!("malformed note payload: {args:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

/// The keyboard element behind widget 70 — the concrete leaf, through the
/// trait's downcast door, which is what an element's own state is asserted on.
fn keys_of(host: &Host) -> &crate::host::elements::keys::Keys {
    host.window_def(1)
        .unwrap()
        .find(70)
        .unwrap()
        .kind
        .as_element()
        .and_then(|el| el.as_any())
        .and_then(|any| any.downcast_ref())
        .expect("widget 70 is a keyboard")
}

fn piano_pressed(host: &Host) -> Vec<i32> {
    keys_of(host).pressed.clone()
}

#[test]
fn piano_press_glissando_and_release_emit_midi_shaped_notes() {
    let (mut host, l) = piano_host(r#","channel":2"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    // Press the front of C4: note-on, high velocity, channel carried.
    let c = piano::key_rect(&l, 60).unwrap();
    let effects = g.press(
        &mut host,
        &ctx,
        (c.x + c.w * 0.5) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 1);
    let (p, v, s, ch) = notes[0];
    assert_eq!((p, s, ch), (60, 1, 2));
    assert!(v > 120, "front-of-key press is loud, got {v}");
    assert_eq!(piano_pressed(&host), vec![60]);
    // Glissando onto D4: off 60, on 62 (the new key's own velocity).
    let d = piano::key_rect(&l, 62).unwrap();
    let effects = g.drag_to(
        &mut host,
        &ctx,
        (d.x + d.w * 0.5) as f64,
        (d.y + d.h * 0.5) as f64,
    );
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].0, notes[0].2), (60, 0));
    assert_eq!((notes[1].0, notes[1].2), (62, 1));
    assert_eq!(piano_pressed(&host), vec![62]);
    // Release: note-off of the held key.
    let effects = g.release(&mut host, &ctx, d.x as f64, d.y as f64);
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].0, notes[0].2), (62, 0));
    assert!(piano_pressed(&host).is_empty());
}

#[test]
fn piano_glissando_across_two_keys_releases_each_left_key() {
    // A glissando spanning more than one crossing: every key left behind
    // must get its note-off, and the final release offs the last key only.
    let (mut host, l) = piano_host("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    g.press(
        &mut host,
        &ctx,
        (c.x + c.w * 0.5) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    let d = piano::key_rect(&l, 62).unwrap();
    g.drag_to(
        &mut host,
        &ctx,
        (d.x + d.w * 0.5) as f64,
        (d.y + d.h * 0.5) as f64,
    );
    let e = piano::key_rect(&l, 64).unwrap();
    let effects = g.drag_to(
        &mut host,
        &ctx,
        (e.x + e.w * 0.5) as f64,
        (e.y + e.h * 0.5) as f64,
    );
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 2, "second crossing: one off, one on");
    assert_eq!((notes[0].0, notes[0].2), (62, 0), "the key left is 62");
    assert_eq!((notes[1].0, notes[1].2), (64, 1));
    assert_eq!(piano_pressed(&host), vec![64]);
    let effects = g.release(&mut host, &ctx, e.x as f64, e.y as f64);
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].0, notes[0].2), (64, 0));
    assert!(piano_pressed(&host).is_empty());
}

#[test]
fn piano_fixed_velocity_and_grayed_keys() {
    // A fixed velocity overrides the press-height map.
    let (mut host, l) = piano_host(r#","velocity":90"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    let effects = g.press(
        &mut host,
        &ctx,
        (c.x + 2.0) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    assert_eq!(note_emits(&effects)[0].1, 90);
    g.release(&mut host, &ctx, c.x as f64, c.y as f64);
    // A press outside the active range is inert: no event, no held key.
    let (mut host, _) = piano_host(r#","active_min":64,"active_max":72"#);
    let mut g = Gestures::default();
    let effects = g.press(
        &mut host,
        &ctx,
        (c.x + 2.0) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    assert!(note_emits(&effects).is_empty());
    assert!(piano_pressed(&host).is_empty());
    // The press is still **taken** — the keyboard is what the reader pointed
    // at, and letting it through would pan whatever is behind it — so it is
    // held like any other, and lets go with nothing to report.
    let effects = g.release(&mut host, &ctx, c.x as f64, c.y as f64);
    assert!(note_emits(&effects).is_empty());
    assert!(!g.dragging());
}

#[test]
fn piano_wheel_pans_the_range_and_pan_zero_freezes_it() {
    let (mut host, l) = piano_host("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    let (cx, cy) = ((c.x + c.w * 0.5) as f64, (c.y + c.h - 1.0) as f64);
    let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
    assert!(has_emit_tag(&effects, 70, "range"));
    let k = keys_of(&host);
    assert_eq!((k.min, k.max), (62, 74));
    // `pan: 0` silences every range gesture.
    let (mut host, _) = piano_host(r#","pan":0"#);
    let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
    assert!(effects.is_empty());
    let k = keys_of(&host);
    assert_eq!((k.min, k.max), (60, 72));
}

#[test]
fn piano_overview_drag_pans_and_wheel_zooms() {
    // With the overview on, the strip sits at the top of the widget rect.
    let host_json = r#"{"type":"window","children":[
        {"id":70,"type":"keys","min":60,"max":72}]}"#;
    let mut host = host_from(host_json);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let l = piano::layout(
        Rect::new(6.0, 6.0, 700.0, 120.0),
        60,
        72,
        true,
        false,
        &Metrics::default(),
    );
    let strip = l.overview.unwrap();
    let sy = (strip.y + strip.h * 0.5) as f64;
    // Drag along the strip: the window pans with the cursor.
    let x0 = piano::overview_key_x(strip, 66) as f64;
    let x1 = piano::overview_key_x(strip, 78) as f64;
    let effects = g.press(&mut host, &ctx, x0, sy);
    assert!(note_emits(&effects).is_empty(), "the strip plays no note");
    let effects = g.drag_to(&mut host, &ctx, x1, sy);
    assert!(has_emit_tag(&effects, 70, "range"));
    let k = keys_of(&host);
    assert_eq!(k.max - k.min, 12, "pan keeps the span");
    assert!(k.min > 60, "the window moved right");
    g.release(&mut host, &ctx, x1, sy);
    // Wheel over the strip zooms out (steps < 0 widens the span).
    let effects = g.wheel(&mut host, &ctx, x1, sy, -2.0);
    assert!(has_emit_tag(&effects, 70, "range"));
    let k = keys_of(&host);
    assert!(k.max - k.min > 12);
}

#[test]
fn piano_voice_mode_tracks_one_node_per_held_pitch() {
    let (mut host, l) = piano_host(r#","voice":"pv","voice_args":["pan",0.5]"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    let (cx, cy) = ((c.x + 2.0) as f64, (c.y + c.h - 1.0) as f64);
    g.press(&mut host, &ctx, cx, cy);
    let voices = host.voices_of(70).to_vec();
    assert_eq!(voices.len(), 1);
    assert_eq!(voices[0].0, 60);
    // Glissando: the old voice is released, a new node sounds the new key.
    let d = piano::key_rect(&l, 62).unwrap();
    g.drag_to(
        &mut host,
        &ctx,
        (d.x + d.w * 0.5) as f64,
        (d.y + 1.0) as f64,
    );
    let after = host.voices_of(70).to_vec();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].0, 62);
    assert_ne!(after[0].1, voices[0].1, "a fresh node id per voice");
    // Release clears the bookkeeping.
    g.release(&mut host, &ctx, cx, cy);
    assert!(host.voices_of(70).is_empty());
    // A freed widget releases whatever is still held.
    g.press(&mut host, &ctx, cx, cy);
    assert!(!host.voices_of(70).is_empty());
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: super::super::GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        }),
        from(),
    );
    assert!(host.voices_of(70).is_empty());
}

// --- the keyboard: focus, the ring and the focused element ------------

/// A window with one editable `text` field (id 5) filling it. It is
/// **natural-sized**, so it is a control-high strip at the top of the window
/// rather than the whole pane — every press below aims inside that strip.
fn text_host() -> Host {
    host_from(r#"{"type":"window","margin":0,"children":[{"id":5,"type":"text"}]}"#)
}

/// Two fields side by side, which is the smallest tab ring there is.
fn two_field_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"flow":"row","children":[
            {"id":5,"type":"text"},{"id":6,"type":"text"}]}"#,
    )
}

fn text_value(host: &Host, id: i32) -> String {
    match host
        .window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .event_value()
    {
        Some(OscType::String(s)) => s,
        other => panic!("not a text field: {other:?}"),
    }
}

/// The string of the single `Emit` in `effects`, if any.
fn emitted_string(effects: &[GestureEffect]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        GestureEffect::Emit { args, .. } => match args.first() {
            Some(OscType::String(s)) if s != "focus" => Some(s.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// The `(widget, gained)` pairs the `"focus"` events in `effects` report.
fn focus_events(effects: &[GestureEffect]) -> Vec<(i32, bool)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } => match args.as_slice() {
                [OscType::String(tag), OscType::Int(on)] if tag == "focus" => {
                    Some((*widget_id, *on == 1))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn key(g: &Gestures, host: &mut Host, ctx: &GestureCtx, k: Key) -> Option<Vec<GestureEffect>> {
    g.key(host, ctx, k, &mut crate::host::clipboard::Clip::default())
}

#[test]
fn a_press_focuses_the_field_and_typing_emits_on_every_keystroke() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // A press focuses the field and says so — but does not edit it, so there
    // is no value event: a click is not an edit.
    let e = g.press(&mut host, &ctx, 30.0, 15.0);
    assert_eq!(host.focused(), Some((1, 5)));
    assert_eq!(focus_events(&e), vec![(5, true)]);
    assert!(emitted_string(&e).is_none());
    // Each character is delivered as the field's whole string, ungated.
    for (ch, expect) in [('h', "h"), ('i', "hi")] {
        let e =
            key(&g, &mut host, &ctx, Key::Char(ch)).expect("the focused field consumes the key");
        assert_eq!(emitted_string(&e).as_deref(), Some(expect));
        assert_eq!(text_value(&host, 5), expect);
    }
    // Backspace edits and re-emits.
    let e = key(&g, &mut host, &ctx, Key::Backspace).unwrap();
    assert_eq!(emitted_string(&e).as_deref(), Some("h"));
}

/// **A drag that reports nothing still repaints.** Extending a text selection
/// changes the picture and not the value, so there is nothing to deliver — and
/// a window with no other frame source would have shown the old selection until
/// something else moved.
#[test]
fn dragging_a_selection_repaints_even_though_it_reports_nothing() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let mut clip = crate::host::clipboard::Clip::default();
    g.press(&mut host, &ctx, 10.0, 15.0);
    for ch in "hello".chars() {
        g.key(&mut host, &ctx, Key::Char(ch), &mut clip);
    }
    let out = g.drag_to(&mut host, &ctx, 200.0, 15.0);
    assert!(
        emitted_string(&out).is_none(),
        "a selection is not a value: {out:?}"
    );
    assert!(
        out.iter().any(|e| matches!(e, GestureEffect::Redraw(1))),
        "and it still asks for the frame that draws it: {out:?}"
    );
}

#[test]
fn keys_are_ignored_when_nothing_is_focused() {
    let mut host = text_host();
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // Nothing focused: the machine declines the key (the front runs its
    // global shortcuts instead).
    assert!(key(&g, &mut host, &ctx, Key::Char('x')).is_none());
    assert_eq!(text_value(&host, 5), "");
}

#[test]
fn a_press_elsewhere_moves_the_focus_and_reports_both_ends() {
    let mut host = two_field_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 30.0, 30.0);
    // A click is a press and a release; a bare second press is not one.
    g.release(&mut host, &ctx, 30.0, 30.0);
    assert_eq!(host.focused(), Some((1, 5)));
    let e = g.press(&mut host, &ctx, 330.0, 30.0);
    assert_eq!(host.focused(), Some((1, 6)));
    assert_eq!(
        focus_events(&e),
        vec![(5, false), (6, true)],
        "the one that lost it is reported too, so a script can drop its caret"
    );
}

/// A press on something that reads no keyboard drops the focus — which is how
/// a caret disappears when you click away from a field.
#[test]
fn a_press_on_a_widget_that_takes_no_focus_clears_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"text"},{"id":7,"type":"button"}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 30.0, 10.0);
    // A click is a press and a release; a bare second press is not one.
    g.release(&mut host, &ctx, 30.0, 10.0);
    assert_eq!(host.focused(), Some((1, 5)));
    let e = g.press(&mut host, &ctx, 30.0, 60.0);
    assert_eq!(host.focused(), None);
    assert_eq!(focus_events(&e), vec![(5, false)]);
}

/// Tab walks the ring in **layout order**, and Shift+Tab back along it.
#[test]
fn tab_walks_the_ring_in_layout_order() {
    let mut host = two_field_host();
    let g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    key(&g, &mut host, &ctx, Key::Tab);
    assert_eq!(
        host.focused(),
        Some((1, 5)),
        "the ring is entered at its first stop"
    );
    key(&g, &mut host, &ctx, Key::Tab);
    assert_eq!(host.focused(), Some((1, 6)));
    ctx.shift = true;
    key(&g, &mut host, &ctx, Key::Tab);
    assert_eq!(host.focused(), Some((1, 5)));
}

/// **Past the last stop the focus leaves the tree**, and the front is told: in a
/// page that is what blurs the canvas, so a mounted GuiDef is an entrance and an
/// exit rather than a keyboard trap.
#[test]
fn tab_past_the_last_stop_hands_the_keyboard_back() {
    let mut host = two_field_host();
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    for _ in 0..2 {
        key(&g, &mut host, &ctx, Key::Tab);
    }
    assert_eq!(host.focused(), Some((1, 6)), "the last stop");
    let e = key(&g, &mut host, &ctx, Key::Tab).expect("Tab is always the ring's");
    assert_eq!(host.focused(), None);
    assert_eq!(focus_events(&e), vec![(6, false)]);
    assert!(
        e.iter().any(|f| matches!(f, GestureEffect::FocusOut(1))),
        "the front is told the focus left: {e:?}"
    );
}

/// A window with nothing focusable hands Tab straight back, rather than
/// swallowing it — the same exit, reached without ever entering.
#[test]
fn tab_in_a_window_with_no_ring_leaves_at_once() {
    let mut host = host_from(r#"{"type":"window","children":[{"id":9,"type":"button"}]}"#);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let e = key(&g, &mut host, &ctx, Key::Tab).unwrap();
    assert_eq!(host.focused(), None);
    assert!(e.iter().any(|f| matches!(f, GestureEffect::FocusOut(1))));
}

/// A script may point the keyboard itself — and is refused when it points at
/// something that reads none, rather than being left waiting for keystrokes
/// that cannot arrive.
#[test]
fn a_script_can_set_the_focus_and_a_widget_that_reads_no_keys_refuses_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"text"},{"id":7,"type":"button"}]}"#,
    );
    let mut effects = Vec::new();
    assert!(host.set_props(
        5,
        vec![("focus".into(), serde_json::json!(1))],
        &mut effects
    ));
    assert_eq!(host.focused(), Some((1, 5)));
    // ...and the key is not a prop: a query answers what the widget is, and
    // `focus` is not part of that.
    assert!(
        !host
            .registry()
            .get(5)
            .is_some_and(|w| w.props.contains_key("focus")),
        "focus was written into the document"
    );
    host.set_props(
        7,
        vec![("focus".into(), serde_json::json!(1))],
        &mut effects,
    );
    assert_eq!(host.focused(), Some((1, 5)), "the button refused it");
    // `focus 0` gives it up.
    host.set_props(
        5,
        vec![("focus".into(), serde_json::json!(0))],
        &mut effects,
    );
    assert_eq!(host.focused(), None);
}

#[test]
fn enter_inserts_a_newline_only_in_a_multiline_field() {
    let ctx = GestureCtx::new(1, 600, 400);
    // Single-line: Enter is inert (no send-on-Enter).
    let mut host = text_host();
    let mut g = Gestures::default();
    g.press(&mut host, &ctx, 30.0, 15.0);
    let e = key(&g, &mut host, &ctx, Key::Enter).unwrap();
    assert!(emitted_string(&e).is_none());
    assert_eq!(text_value(&host, 5), "");
    // Multiline: Enter inserts a newline and emits.
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[{"id":5,"type":"text","multiline":true}]}"#,
    );
    let mut g = Gestures::default();
    g.press(&mut host, &ctx, 30.0, 30.0);
    // A click is a press and a release; a bare second press is not one.
    g.release(&mut host, &ctx, 30.0, 30.0);
    key(&g, &mut host, &ctx, Key::Char('a'));
    let e = key(&g, &mut host, &ctx, Key::Enter).unwrap();
    assert_eq!(emitted_string(&e).as_deref(), Some("a\n"));
}

#[test]
fn cut_and_paste_move_text_through_the_clipboard() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    let mut clip = crate::host::clipboard::Clip::default();
    g.press(&mut host, &ctx, 30.0, 15.0);
    for ch in "abc".chars() {
        g.key(&mut host, &ctx, Key::Char(ch), &mut clip);
    }
    // Select all, then cut to the clipboard.
    ctx.ctrl = true;
    g.key(&mut host, &ctx, Key::Char('a'), &mut clip);
    g.key(&mut host, &ctx, Key::Char('x'), &mut clip);
    assert_eq!(clip.text(), "abc");
    assert_eq!(text_value(&host, 5), "");
    // Paste it back twice.
    g.key(&mut host, &ctx, Key::Char('v'), &mut clip);
    g.key(&mut host, &ctx, Key::Char('v'), &mut clip);
    assert_eq!(text_value(&host, 5), "abcabc");
}

/// **A frequency axis does not zoom past its own analysis.** Below a few FFT
/// bins across the whole body the curve is the interpolation between two
/// neighbours -- a flat line that no longer answers to the signal -- so the
/// zoom stops at a floor derived from `fft_size` and the sample rate rather
/// than at a constant fraction of the display axis.
#[test]
fn the_frequency_zoom_stops_at_the_analysis_resolution() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.sample_rate = 48_000.0;
    for _ in 0..40 {
        g.wheel(&mut host, &ctx, 400.0, 150.0, 2.0);
    }
    let (start, len) = x_window(&host, 80);
    // The window in bins, through the geometry the curve is drawn with.
    let (nyquist, f_lo) = crate::host::graphics::signal::spectrum::axis_geometry(48_000.0);
    let bins = |d: f64| {
        crate::host::ruler::display_to_hz(d, nyquist, crate::spectrogram::FreqScale::Log, f_lo)
            * 2048.0
            / 48_000.0
    };
    let span = bins(start + len) - bins(start);
    assert!(
        (3.5..=4.5).contains(&span),
        "the window bottomed out at {span:.2} bins, not at the analysis floor"
    );
    // Far short of what the display axis alone would have allowed.
    assert!(
        len > crate::viewport::MIN_SPAN * 5.0,
        "the floor is still the display constant ({len})"
    );
}

/// A gesture that moves nothing says nothing: once the axis is against its
/// floor, further wheel steps report no view. The bug this fixes filled the
/// script's stream with an unchanged window, one event per wheel notch.
#[test]
fn a_wheel_against_the_floor_reports_nothing() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.sample_rate = 48_000.0;
    let mut last = Vec::new();
    for _ in 0..40 {
        last = g.wheel(&mut host, &ctx, 400.0, 150.0, 2.0);
    }
    assert!(
        !has_emit_tag(&last, 80, "view_x"),
        "a wheel step that moved nothing still reported a view"
    );
    assert!(last.is_empty(), "and it asked for no repaint either");
    // Zooming back out still reports, so the gate is on movement and not on
    // the direction.
    let out = g.wheel(&mut host, &ctx, 400.0, 150.0, -2.0);
    assert!(has_emit_tag(&out, 80, "view_x"));
}

/// The same rule on the shared time axis: fully zoomed out, the wheel has
/// nowhere to go and reports nothing.
#[test]
fn a_wheel_against_the_time_axis_bound_reports_nothing() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":60,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(60, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    // Already showing the whole buffer: zooming out cannot move it.
    let out = g.wheel(&mut host, &ctx, 400.0, 150.0, -4.0);
    assert!(
        !has_emit_tag(&out, 60, "view"),
        "an unmovable axis reported"
    );
    // ...and zooming in still does.
    let out = g.wheel(&mut host, &ctx, 400.0, 150.0, 4.0);
    assert!(has_emit_tag(&out, 60, "view"));
}

/// The window a spectrum is **showing**, which is its request opened up to what
/// the analysis resolves where it sits — what the frame draws and `"view_x"`
/// reports, as against the `x_window` that was asked for.
fn shown_x_window(host: &Host, id: i32) -> (f64, f64) {
    host.window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .signal()
        .expect("a signal element")
        .freq_window(48_000.0)
}

/// **Panning to the end of a frequency axis stops there.** The zoom floor is
/// measured forward from the window's left edge, and a pan hands over an edge
/// that is off the axis — that is what dragging past it means, the write
/// clamping it a step later. Charging that overshoot to the floor widened the
/// window by however far the drag had gone, and the next step of the drag read
/// the wider window and went further: the picture rushed out to the whole axis
/// under a gesture that asked to move sideways.
#[test]
fn a_pan_past_the_axis_end_stops_at_its_floor() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.sample_rate = 48_000.0;
    // A window narrow enough that reaching the bottom of the axis takes a few
    // drags, so the runaway had room to compound.
    set_x_window(&mut host, 80, 0.30, 0.05);
    let mut seen = Vec::new();
    for _ in 0..8 {
        g.press(&mut host, &ctx, 100.0, 150.0);
        g.drag_to(&mut host, &ctx, 790.0, 150.0);
        g.release(&mut host, &ctx, 790.0, 150.0);
        seen.push(shown_x_window(&host, 80));
    }
    // Against the bottom of the axis, and no wider than four bins are there:
    // the floor at 20 Hz, which is where the pan was heading all along.
    let floor = crate::host::graphics::signal::spectrum::min_display_span(
        2048,
        48_000.0,
        crate::spectrogram::FreqScale::Log,
        crate::host::graphics::signal::spectrum::axis_geometry(48_000.0).1,
        0.0,
    );
    let (start, len) = *seen.last().unwrap();
    assert_eq!(start, 0.0, "the pan ran to the bottom of the axis");
    assert!(
        (len - floor).abs() < 1e-9,
        "the window settled at {len} instead of the floor {floor}: {seen:?}"
    );
}

/// **A pan does not spend the zoom it travels through.** Down a log axis the
/// window has to open — four bins at 20 Hz are a quarter of the axis, and no
/// zoom can be finer than the analysis under it — but what the reader *asked*
/// for is kept, so the way back up returns the window they set rather than the
/// one the bottom of the axis imposed.
#[test]
fn a_pan_down_the_axis_and_back_returns_the_zoom() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.sample_rate = 48_000.0;
    // Zoomed into a window that fits where it is and cannot where it is going.
    for _ in 0..10 {
        g.wheel(&mut host, &ctx, 700.0, 150.0, 3.0);
    }
    let zoomed = shown_x_window(&host, 80);
    assert!(zoomed.1 < 0.02, "zoomed in up the axis: {zoomed:?}");
    // A drag walks the window by its own width, so the trip takes as many of
    // them as the axis is wide -- and it speeds up on the way down as the
    // floor opens the window under it.
    let pan = |g: &mut Gestures, host: &mut Host, from: f64, to: f64| {
        g.press(host, &ctx, from, 150.0);
        g.drag_to(host, &ctx, to, 150.0);
        g.release(host, &ctx, to, 150.0);
    };
    for _ in 0..80 {
        pan(&mut g, &mut host, 100.0, 790.0);
        if shown_x_window(&host, 80).0 == 0.0 {
            break;
        }
    }
    let bottom = shown_x_window(&host, 80);
    assert_eq!(bottom.0, 0.0, "the pan reached the bottom: {bottom:?}");
    assert!(bottom.1 > 0.2, "and the axis opened there: {bottom:?}");
    // ...and back up, to somewhere the window the reader set fits again.
    let mut back = bottom;
    for _ in 0..80 {
        pan(&mut g, &mut host, 790.0, 100.0);
        back = shown_x_window(&host, 80);
        if (back.1 - zoomed.1).abs() < 1e-9 {
            break;
        }
    }
    assert!(
        (back.1 - zoomed.1).abs() < 1e-9,
        "the trip down spent the zoom: left at {}, came back at {}",
        zoomed.1,
        back.1
    );
}

/// **A script's window is floored by the same analysis a gesture is**, and it
/// is floored where every other view key is: at the read, not in `apply`, so
/// one `/gui_set` carrying both keys does not depend on their order. Before
/// this, a `view_len` finer than the bins was drawn as asked and the first
/// touch of the pointer snapped it to the floor under the reader's hand.
#[test]
fn a_scripted_window_finer_than_the_analysis_is_opened_too() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.sample_rate = 48_000.0;
    set_x_window(&mut host, 80, 0.2, 0.001);
    let shown = shown_x_window(&host, 80);
    assert!(
        shown.1 > 0.1,
        "the axis showed a window finer than its bins: {shown:?}"
    );
    // And the pointer arriving changes nothing: it reads the same window.
    g.press(&mut host, &ctx, 400.0, 150.0);
    let effects = g.drag_to(&mut host, &ctx, 400.0, 150.0);
    g.release(&mut host, &ctx, 400.0, 150.0);
    assert!(
        !has_emit_tag(&effects, 80, "view_x"),
        "grabbing the curve moved the window on its own"
    );
    assert_eq!(shown_x_window(&host, 80), shown);
}

// ---- a registered element: the press it claims, and the one it declines ----

/// Two of the smallest possible elements: one that takes every press and
/// reports how many it has taken, one that never takes any.
/// A registered element that exercises the **whole** gesture sequence: it takes
/// a press, follows the cursor while it is held, and reports the total on
/// release. What a real leaf makes of those positions is its own business; what
/// the machine promises is that all three arrive.
#[derive(Debug, Clone)]
struct TestPad {
    taken: i32,
    claims: bool,
    /// Where the cursor last was.
    moved: (f64, f64),
    /// How many `drag` steps arrived.
    steps: i32,
}

impl crate::Element for TestPad {
    fn set(&mut self, _key: &str, _v: &serde_json::Value) -> bool {
        false
    }

    fn draw(&self, _d: &mut crate::host::paint::Draw, _ctx: &crate::host::widget::element::Ctx) {}

    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(self.taken))
    }

    fn press(
        &mut self,
        _at: (f64, f64),
        _input: &crate::host::widget::element::Input,
    ) -> crate::Claim {
        if !self.claims {
            return crate::Claim::Decline;
        }
        self.taken += 1;
        crate::Claim::value(OscType::Int(self.taken))
    }

    fn drag(
        &mut self,
        at: (f64, f64),
        _input: &crate::host::widget::element::Input,
    ) -> crate::host::widget::element::Events {
        self.moved = at;
        self.steps += 1;
        crate::host::widget::element::Events::none()
    }

    fn release(
        &mut self,
        _at: (f64, f64),
        _inside: bool,
        _input: &crate::host::widget::element::Input,
    ) -> crate::host::widget::element::Events {
        crate::host::widget::element::Events::message(vec![
            OscType::String("pad".into()),
            OscType::Int(self.steps),
            OscType::Float(self.moved.1 as f32),
        ])
    }

    fn clone_box(&self) -> Box<dyn crate::Element> {
        Box::new(self.clone())
    }
}

fn pad(
    props: &serde_json::Map<String, serde_json::Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn crate::Element>, String> {
    Ok(Box::new(TestPad {
        taken: 0,
        claims: props
            .get("claims")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        moved: (0.0, 0.0),
        steps: 0,
    }))
}

/// Every `/gui_event` a gesture asked for on `id`, in order — a list of
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

fn pad_taken(host: &Host, id: i32) -> i32 {
    match host
        .window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .event_value()
    {
        Some(OscType::Int(n)) => n,
        other => panic!("not a pad: {other:?}"),
    }
}

/// The default gesture table hands a leaf the press, so a registered element
/// gets one with no `gestures` prop and nothing else to configure — and what
/// it claims leaves as the widget's value, on the same `/gui_event` path a
/// built-in control's does.
#[test]
fn a_registered_element_takes_the_press_and_its_value_leaves() {
    crate::register("test_pad", pad);
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"test_pad"}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let effects = g.press(&mut host, &ctx, 300.0, 200.0);
    assert_eq!(pad_taken(&host, 5), 1, "the element saw the press");
    assert!(
        effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 5, args, .. }
                if args.first() == Some(&OscType::Int(1))
        )),
        "and its claim left as a value: {effects:?}"
    );
    crate::unregister("test_pad");
}

/// The press is **held**: a claim opens a drag that carries no geometry, so
/// every motion and the release land on the element that took it. What it
/// reports on release travels as an edit-back payload (a tagged list), not as a
/// value — the element says which by how many arguments it sends.
#[test]
fn a_claim_holds_the_press_through_the_drag_and_the_release() {
    crate::register("test_pad", pad);
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"test_pad"}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 300.0, 200.0);
    assert!(g.dragging(), "a taken press is held");
    g.drag_to(&mut host, &ctx, 300.0, 260.0);
    g.drag_to(&mut host, &ctx, 300.0, 290.0);
    let effects = g.release(&mut host, &ctx, 300.0, 290.0);
    assert!(!g.dragging(), "the release ends it");
    assert_eq!(
        emitted(&effects, 5),
        vec![vec![
            OscType::String("pad".into()),
            OscType::Int(2),
            OscType::Float(290.0)
        ]],
        "two drag steps, at the last cursor: {effects:?}"
    );
    crate::unregister("test_pad");
}

/// Declining is the other half of the contract: the press goes back to the
/// chain, which here is the plane under it — so it pans, exactly as a press on
/// a lane's empty space or a patcher's bare canvas does.
#[test]
fn a_declined_press_falls_through_to_the_plane() {
    crate::register("test_pad", pad);
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":4,"type":"plane","children":[
                {"id":5,"type":"test_pad","claims":false,"x":0,"y":0,"w":400,"h":300}]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 100.0, 100.0);
    assert_eq!(pad_taken(&host, 5), 0, "it declined");
    g.drag_to(&mut host, &ctx, 140.0, 100.0);
    g.release(&mut host, &ctx, 140.0, 100.0);
    let panned = match &host.window_def(1).unwrap().find(4).unwrap().kind {
        WidgetKind::Scroll { view, .. } => view.view_x,
        other => panic!("not a plane: {other:?}"),
    };
    assert!(panned != 0.0, "the plane never got the press back");
    crate::unregister("test_pad");
}

/// Undo and redo leave the window as a report, not as an action: the host holds
/// no history, so the whole of its part is naming the window and the verb.
///
/// Addressed to the **window** rather than to a widget, which is the correction
/// the milestone needed — a gesture-plan step consumes a press somewhere, and
/// undo is addressed to no place at all.
#[test]
fn undo_and_redo_leave_the_window_as_a_report() {
    let mut host = lane_host();
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 200);

    let effects = g.history(&mut host, &ctx, false);
    assert!(has_emit_tag(&effects, 1, "undo"), "named by the window id");
    assert!(g.history(&mut host, &ctx, true).iter().any(|e| matches!(
        e,
        GestureEffect::Emit { widget_id: 1, args, .. }
            if args.first() == Some(&OscType::String("redo".into()))
    )));

    // Stamped like any other edit-back, so the owner's acknowledgement can name
    // it and the two do not need a second rule between them.
    let stamps: Vec<i32> = g
        .history(&mut host, &ctx, false)
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(stamps.len(), 1);
    assert!(stamps[0] > 0, "a real stamp, not the reserved zero");
}

/// D1: a sample is a grabbable point. The whole route in one test — press the
/// disc, drag it, and one intent leaves on release carrying both the value it
/// reached and the one it came from, so the owner can apply it and invert it
/// without remembering anything.
#[test]
fn a_dragged_sample_leaves_as_one_absolute_intent() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.5,-0.5,1.0,0.25,-0.25,0.75,0.0],"base_bucket":2,
             "gestures":{"drag":"sample"}}]}"#;
    let mut host = host_from(def);
    // Eight samples across the body: far enough apart that the trace marks
    // each one, which is the same question the grab asks.
    host.set_timeline_total(50, 8);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);

    g.press(&mut host, &ctx, 400.0, 100.0);
    // The grab shows at once: the hand sees the value it is holding before
    // anyone has applied anything.
    let held = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("the press takes hold of a sample")
        .clone();
    let effects = g.drag_to(&mut host, &ctx, 400.0, 220.0);
    let moved = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("and keeps hold of it");
    assert_eq!(
        moved.start, held.start,
        "the drag moves the value, not which"
    );
    assert!(moved.values[0] < held.values[0], "dragging down lowers it");
    assert!(
        !has_emit_tag(&effects, 50, "sample"),
        "one gesture is one intent: nothing leaves along the way"
    );

    let effects = g.release(&mut host, &ctx, 400.0, 220.0);
    let args = emitted_args(&effects, 50).expect("the release reports");
    assert_eq!(
        args.len(),
        5,
        "tag, channel, frame, value, previous: {args:?}"
    );
    assert!(has_emit_tag(&effects, 50, "sample"));
    // The pending stays until the owner answers: dropping it here would snap
    // the picture back for the length of the round trip, which reads as a
    // refusal.
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_some(),
        "the edit is still in flight"
    );

    // And the acknowledgement is what lets go of it — O3's *drop every pending
    // at or below the stamp*, with the drawing finally following the outbox.
    let seq = match &effects[..] {
        [.., crate::host::gestures::GestureEffect::Emit { seq, .. }] => *seq,
        _ => effects
            .iter()
            .find_map(|e| match e {
                crate::host::gestures::GestureEffect::Emit { seq, .. } => Some(*seq),
                _ => None,
            })
            .expect("the intent went out stamped"),
    };
    host.handle_packet(
        clausters_core::osc::OscPacket::Message(clausters_core::osc::OscMessage {
            addr: crate::host::GUI_ACK.into(),
            args: vec![
                clausters_core::osc::OscType::Int(seq),
                clausters_core::osc::OscType::Int(1),
            ],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_none(),
        "the owner answered, so the hand lets go"
    );
}

/// The step **declines where a sample is not a thing on screen** — the trace
/// draws no discs there, so there is nothing to grab, and a plan that names it
/// falls through instead of offering a grab the picture does not show.
#[test]
fn grabbing_a_sample_declines_when_they_are_not_drawn() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.5,-0.5,1.0,0.25,-0.25,0.75,0.0],"base_bucket":2,
             "gestures":{"drag":"sample select"}}]}"#;
    let mut host = host_from(def);
    // A hundred thousand samples over the same body: a pixel is many samples,
    // so no disc is drawn and none can be taken.
    host.set_timeline_total(50, 100_000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    g.press(&mut host, &ctx, 400.0, 100.0);
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_none(),
        "nothing was taken hold of"
    );
    // And the plan's next step got the press instead, which is what declining
    // rather than consuming is for.
    let effects = g.drag_to(&mut host, &ctx, 600.0, 100.0);
    assert!(
        has_emit_tag(&effects, 50, "selection"),
        "the sweep behind it still works"
    );
}

/// D2: the draw mode. A stroke writes every sample it passes — including the
/// ones *between* two motion events, which is what makes it a stroke and not a
/// comb — and leaves as one intent carrying both runs.
#[test]
fn a_stroke_writes_every_sample_it_passes_and_leaves_as_one_intent() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0],"base_bucket":2,
             "gestures":{"drag":"draw"}}]}"#;
    let mut host = host_from(def);
    host.set_timeline_total(50, 8);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);

    // Two events far apart: the samples between them are the test.
    g.press(&mut host, &ctx, 100.0, 100.0);
    g.drag_to(&mut host, &ctx, 700.0, 200.0);
    let held = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("the stroke is held")
        .clone();
    assert!(
        held.values.len() >= 5,
        "the run covers what the pointer passed, not the two ends: {:?}",
        held.values.len()
    );
    assert_eq!(
        held.values.len(),
        held.previous.len(),
        "each written sample knows what it was"
    );
    // A ramp, because the pointer went down as it went right.
    assert!(
        held.values.first() > held.values.last(),
        "the values between the two events were filled in: {:?}",
        held.values
    );

    let effects = g.release(&mut host, &ctx, 700.0, 200.0);
    let args = emitted_args(&effects, 50).expect("the stroke reports");
    assert_eq!(
        args.len(),
        5,
        "tag, channel, start, values, previous: {args:?}"
    );
    assert!(has_emit_tag(&effects, 50, "draw"));
}

/// **A stroke belongs to the channel it started on.** Dragging up out of a
/// lane must clamp to *that* channel's maximum, not read the value the lane
/// above would show at the same height — which is what a stereo stroke did
/// until the read was made lane-explicit: the pencil jumped back to mid-scale
/// the moment it left its own lane.
#[test]
fn a_stroke_leaving_its_lane_clamps_to_its_own_channels_end() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,"channels":2,
             "data":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0],"base_bucket":2,
             "gestures":{"drag":"draw"}}]}"#;
    let mut host = host_from(def);
    host.set_timeline_total(50, 4);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.slot_channels.insert(50, 2);

    // Press low in the **lower** lane (channel 1), then drag straight up past
    // the top of the window.
    g.press(&mut host, &ctx, 100.0, 230.0);
    let held = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("the stroke is held")
        .clone();
    assert_eq!(held.channel, 1, "it started on the lower lane");

    // Into the **middle** of the lane above, which is where the defect showed:
    // at that height the upper lane reads mid-scale, and a stroke that took it
    // wrote a value the hand never aimed at.
    g.drag_to(&mut host, &ctx, 300.0, 70.0);
    let held = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("still held")
        .clone();
    assert_eq!(held.channel, 1, "and it stays on it");
    assert!(
        *held.values.last().expect("a run") > 0.9,
        "clamped to the channel's own top, not read as mid-scale in the lane \
         above: {:?}",
        held.values
    );
}

/// **A stroke stops at the edge of the picture.** The pointer keeps reporting
/// past the window — that is what a drag grab is for — and a pencil that
/// followed it would go on rewriting samples nobody can see, discovered only by
/// scrolling there afterwards.
#[test]
fn a_stroke_dragged_off_the_view_writes_nothing_past_the_last_visible_sample() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0],"base_bucket":2,
             "gestures":{"drag":"draw"}}]}"#;
    let mut host = host_from(def);
    host.set_timeline_total(50, 8);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);

    g.press(&mut host, &ctx, 100.0, 100.0);
    // Far outside the window, the way a hand that keeps going leaves it.
    g.drag_to(&mut host, &ctx, 4000.0, 150.0);
    let held = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("the stroke is held")
        .clone();
    assert!(
        held.start + held.values.len() <= 8,
        "the run ends inside the contents: {} + {}",
        held.start,
        held.values.len()
    );
}

/// **Refused where a pixel is more than one sample, and said out loud.** A
/// stroke there would write values the reader cannot see, and a silent decline
/// would teach that the pencil sometimes does not work.
#[test]
fn drawing_is_refused_out_loud_when_a_pixel_is_more_than_a_sample() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.5,-0.5,1.0,0.25,-0.25,0.75,0.0],"base_bucket":2,
             "gestures":{"drag":"draw select"}}]}"#;
    let mut host = host_from(def);
    host.set_timeline_total(50, 100_000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let effects = g.press(&mut host, &ctx, 400.0, 100.0);
    assert!(
        has_emit_tag(&effects, 50, "refused"),
        "the refusal is visible"
    );
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_none(),
        "and nothing was taken hold of"
    );
    // It **consumes** the press rather than falling through: a plan naming a
    // sweep behind it must not turn a refused stroke into a selection.
    let effects = g.drag_to(&mut host, &ctx, 600.0, 100.0);
    assert!(
        !has_emit_tag(&effects, 50, "selection"),
        "a refused stroke is not a sweep"
    );
}

/// **A window of lanes and clips can be reset**, which is the case a multitrack
/// actually is: its clips carry notes, curves or nothing at all, and there is no
/// signal element anywhere in it.
///
/// Found by hand on the session mode: the wheel zoomed the axis and `r` did
/// nothing, because the reset asked for views that *navigate a signal* and a
/// lane is not one. The wheel and the key have to agree about what the axis is.
#[test]
fn a_multitrack_with_no_signal_view_still_resets() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"field","label":"lane","children":[
                {"id":51,"type":"field","offset":0.0,"dur":1000.0,"label":"a"},
                {"id":52,"type":"field","offset":4000.0,"dur":1000.0,"label":"b"}]},
            {"id":53,"type":"field","h":20.0,"ruler":"beats"}]}"#;
    let mut host = host_from(def);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);

    // Zoom out over the lane, then reset: the key has to answer for what the
    // wheel just did.
    g.wheel(&mut host, &ctx, 400.0, 60.0, -3.0);
    let zoomed = host.timeline_nav(50).expect("the lane is on the axis").0;
    let effects = g.reset_timelines(&mut host, &ctx);
    let reset = host.timeline_nav(50).expect("still on it").0;
    assert!(
        !effects.is_empty(),
        "the reset reported something rather than running in silence"
    );
    assert_ne!(
        (zoomed.start, zoomed.len),
        (reset.start, reset.len),
        "and the window moved: {zoomed:?} -> {reset:?}"
    );
}
