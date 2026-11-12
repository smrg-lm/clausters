//! A workspace and a plane under the hand: the marquee, the pan, the wheel and the patcher's cords.

use super::*;

/// The whole widget-binding chain from a real press: a toggle bound to a
/// `stack`'s `index` flips the page inside the host, the window is asked to
/// repaint, and **nothing leaves for the script** -- the point of a binding.
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
    // the press lands on its box, at the left of the cell -- the rest of that
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

/// The same anchor, on a plane whose **content follows the zoom**: a graph
/// sizes its plane to itself-but-never-below-the-viewport, so the visible
/// content shrinks as the zoom grows. Clamping the new pan against the
/// content of the *old* zoom slid the plane out from under the cursor --
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
    let area = clausters_gui_draw::rect::Rect::new(0.0, 0.0, 600.0, 400.0);
    // Where the graph itself sits on screen -- the thing an eye tracks, and
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
fn a_plane_whose_zoom_waits_for_ctrl_scrolls_on_the_wheel() {
    // `zoom: "ctrl"` is a page: turning the wheel turns the pages, and only
    // Ctrl with it changes the scale.
    let mut host = workspace(r#","zoom":"ctrl""#);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    g.wheel(&mut host, &ctx, 300.0, 200.0, -1.0);
    let v = view_of(&host, 20);
    assert_eq!(
        v.zoom(&Metrics::default()),
        1.0,
        "the wheel alone does not scale"
    );
    assert_eq!(v.view_y, scroll::WHEEL_PAN_PX, "it scrolls");
    ctx.ctrl = true;
    g.wheel(&mut host, &ctx, 300.0, 200.0, 1.0);
    assert!(
        view_of(&host, 20).zoom(&Metrics::default()) > 1.0,
        "Ctrl with the wheel zooms in"
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
    // The release reports nothing -- the value left on every step -- but the
    // window repaints: an element that drew itself held has to be drawn let go.
    let effects = g.release(&mut host, &ctx, 399.0, 25.0);
    assert_eq!(effects, vec![GestureEffect::Redraw(1)]);
    assert!(!g.dragging());
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
    assert!(g.dragging(), "the machine holds the sweep");
    // And the frame is told to draw it: one marquee, drawn where every swept
    // selection in the window is drawn, rather than by the element itself.
    assert!(
        matches!(g.grab(), crate::host::frame::Grab::Marquee(7, _)),
        "the rectangle is the machine's to draw: {:?}",
        g.grab(),
    );
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
    // marquee over nothing -- which is what it used to do, in both fronts.
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
fn the_plane_pans_every_direction_from_its_origin() {
    // The regression this fixes: a plane sitting at the content's top-left
    // corner (its default) used to be clamped dead against down/right
    // drags -- half the gestures did nothing and it read as broken. The
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
            addr: crate::host::GUI_SET.into(),
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
