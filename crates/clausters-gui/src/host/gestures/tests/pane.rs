//! A pane of stacked bodies: which body a press reaches, its rulers and its scroll.

use super::*;

fn layered_pane_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"points","axes":{"x":{"unit":"samples"}},"children":[
            {"id":11,"type":"curve","duration":100,"points":[0,0.2,1,0,50,0.2,1,0,100,0.2,1,0]},
            {"id":12,"type":"curve","duration":100,"points":[0,0.8,1,0,50,0.8,1,0,100,0.8,1,0]}
        ]}] }"#,
    )
}

#[test]
fn pane_hides_a_body_from_drawing_and_editing_and_keeps_its_ruler() {
    let mut host = layered_pane_host();
    let ctx = GestureCtx::new(1, 600, 400);
    let pane = placed_rect(&host, &ctx, 10);
    let body = placed_rect(&host, &ctx, 11);
    assert!(body.h < pane.h, "one ruler strip is reserved by the pane");
    assert_eq!(body, placed_rect(&host, &ctx, 12));
    set_prop(&mut host, 10, "hidden", "points");
    let placed = host.layout_window(1, 600, 400).unwrap();
    assert!(!placed.iter().any(|p| p.widget.id == Some(11)));
    assert_eq!(hit(&host, &ctx, 300.0, 200.0).unwrap().id, 10);
    set_prop(&mut host, 10, "layer", "points:1");
    assert_eq!(hit(&host, &ctx, 300.0, 200.0).unwrap().id, 12);
    set_prop(&mut host, 10, "hidden", "");
    assert!(
        host.layout_window(1, 600, 400)
            .unwrap()
            .iter()
            .any(|p| p.widget.id == Some(11))
    );
}

#[test]
fn pane_can_select_contents_from_placement_and_reports_the_existing_layer_payload() {
    let mut host = layered_pane_host();
    set_prop(&mut host, 10, "layer", "placement");
    let ctx = GestureCtx::new(1, 600, 400);
    let rect = placed_rect(&host, &ctx, 12);
    let x = f64::from(rect.x + rect.w * 0.5);
    let y = f64::from(rect.y + rect.h * 0.2);
    let effects = Gestures::default().press(&mut host, &ctx, x, y);
    assert!(has_emit_tag(&effects, 10, "layer"));
    assert_eq!(
        host.window_def(1).unwrap().find(10).unwrap().layer,
        crate::host::layers::Layer::Content(1)
    );
}

#[test]
fn a_curve_over_a_signal_edits_on_the_panes_shared_gutter() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"points","children":[
            {"id":11,"type":"signal","view":"trace","navigable":1,"data":[0,1,0,-1]},
            {"id":12,"type":"curve","duration":100,"points":[0,0.2,1,0,50,0.2,1,0,100,0.2,1,0]}
        ]}]}"#,
    );
    let ctx = GestureCtx::new(1, 600, 400);
    let rect = placed_rect(&host, &ctx, 12);
    let hit = hit(&host, &ctx, 300.0, 200.0).unwrap();
    let (_, axis) = interact::time_of(&hit.chain).unwrap();
    assert!(axis.body.x > rect.x, "the signal reserves a value ruler");
    let x = f64::from(axis.body.x + axis.body.w * 0.5);
    let y = f64::from(rect.y + rect.h * 0.8);
    let mut gestures = Gestures::default();
    gestures.press(&mut host, &ctx, x, y);
    gestures.drag_to(&mut host, &ctx, x, y - 20.0);
    let effects = gestures.release(&mut host, &ctx, x, y - 20.0);
    let args = emitted_args(&effects, 12).expect("the active curve reports its points");
    let value = match &args[6] {
        OscType::Float(v) => f64::from(*v),
        OscType::Double(v) => *v,
        other => panic!("expected a point value, got {other:?}"),
    };
    assert!(
        value > 0.2,
        "the point moved; the segment beside it was not bent: {args:?}"
    );
}

/// **A stack too tall for its body scrolls under the wheel over its ruler**,
/// and Ctrl zooms the amplitude there as before: sixteen channels in a short
/// window keep their rows readable instead of thinning to lines.
#[test]
fn a_stack_too_tall_for_its_body_scrolls_and_ctrl_zooms() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":61,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(61, 1000);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.slot_channels.insert(61, 16);
    let scroll = |host: &Host| {
        host.window_def(1)
            .unwrap()
            .find(61)
            .unwrap()
            .kind
            .signal()
            .unwrap()
            .lane_scroll
    };
    let effects = g.wheel(&mut host, &ctx, 10.0, 150.0, -2.0);
    assert!(!has_emit_tag(&effects, 61, "view_y"), "a scroll is no zoom");
    assert!(scroll(&host) > 0.0, "the wheel down moved the stack");
    g.wheel(&mut host, &ctx, 10.0, 150.0, 100.0);
    assert_eq!(scroll(&host), 0.0, "and back up to its top, no further");
    ctx.ctrl = true;
    let zoom = g.wheel(&mut host, &ctx, 10.0, 150.0, 1.0);
    assert!(
        has_emit_tag(&zoom, 61, "view_y"),
        "Ctrl zooms the amplitude"
    );
}

/// **A take bounds the axis a curve is drawn over it on**: the audio editor's
/// pane holds the take's signal and its gain curve, and zooming out stops at
/// the take -- the curve neither lengthens the axis nor makes it an authoring
/// grid with empty time to scroll into. A pane of curves alone keeps that
/// headroom, which is what a surface written into needs.
#[test]
fn a_take_bounds_the_axis_a_curve_is_drawn_over() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"take","axes":{"x":{"unit":"samples"}},"children":[
            {"id":11,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2},
            {"id":12,"type":"curve","duration":4,"points":[0,1,1,0,4,1,1,0]}
        ]}] }"#,
    );
    host.set_timeline_total(11, 1000);
    let ctx = GestureCtx::new(1, 600, 400);
    let mut g = Gestures::default();
    let rect = placed_rect(&host, &ctx, 11);
    let (x, y) = (
        f64::from(rect.x + rect.w * 0.5),
        f64::from(rect.y + rect.h * 0.5),
    );
    for _ in 0..20 {
        g.wheel(&mut host, &ctx, x, y, -4.0);
    }
    let (nav, total) = host.timeline_nav(10).unwrap();
    assert_eq!(
        total, 1000,
        "the take's frames, not the curve's or a grid's"
    );
    assert!(
        nav.len <= 1000.0,
        "zoomed out no further than the take: {}",
        nav.len
    );

    let mut curves = layered_pane_host();
    let ctx = GestureCtx::new(1, 600, 400);
    let rect = placed_rect(&curves, &ctx, 11);
    let (x, y) = (
        f64::from(rect.x + rect.w * 0.5),
        f64::from(rect.y + rect.h * 0.5),
    );
    for _ in 0..20 {
        g.wheel(&mut curves, &ctx, x, y, -4.0);
    }
    let (nav, _) = curves.timeline_nav(10).unwrap();
    assert!(
        nav.len > 100.0,
        "curves alone keep their authoring headroom"
    );
}

/// **The vertical axis of a take in a pane is the take's**: a wheel over its
/// ruler zooms its amplitude and a drag there pans it, as on a bare signal.
#[test]
fn a_takes_ruler_in_a_pane_zooms_and_pans_its_amplitude() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"take","axes":{"x":{"unit":"samples"}},"children":[
            {"id":11,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}
        ]}] }"#,
    );
    host.set_timeline_total(11, 1000);
    let ctx = GestureCtx::new(1, 600, 400);
    let mut g = Gestures::default();
    let rect = placed_rect(&host, &ctx, 11);
    let y = f64::from(rect.y + rect.h * 0.4);
    let x = f64::from(rect.x) + 8.0;
    let zoom = g.wheel(&mut host, &ctx, x, y, 2.0);
    assert!(
        has_emit_tag(&zoom, 11, "view_y"),
        "the wheel zooms the take: {zoom:?}"
    );
    let (before, _) = host
        .window_def(1)
        .unwrap()
        .find(11)
        .unwrap()
        .kind
        .editor()
        .unwrap()
        .y_view();
    g.press(&mut host, &ctx, x, y);
    let mut pan = g.drag_to(&mut host, &ctx, x, y + 30.0);
    pan.extend(g.release(&mut host, &ctx, x, y + 30.0));
    let (after, _) = host
        .window_def(1)
        .unwrap()
        .find(11)
        .unwrap()
        .kind
        .editor()
        .unwrap()
        .y_view();
    assert!(
        after != before,
        "the drag pans it: {before} -> {after}, {pan:?}"
    );
}

/// **Each quantity keeps its own vertical.** A take zoomed out in amplitude
/// and switched to a spectrogram is ruled in hertz over the whole of its
/// frequency axis, and a wheel there opens no further than Nyquist; switched
/// back, the amplitude window is where it was left.
#[test]
fn a_stack_moved_to_frequency_takes_up_the_frequency_window() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"take","axes":{"x":{"unit":"samples"}},"children":[
            {"id":11,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}
        ]}] }"#,
    );
    host.set_timeline_total(11, 1000);
    let ctx = GestureCtx::new(1, 600, 400);
    let mut g = Gestures::default();
    let rect = placed_rect(&host, &ctx, 11);
    let (x, y) = (f64::from(rect.x) + 8.0, f64::from(rect.y + rect.h * 0.5));
    let editor = |host: &Host| {
        host.window_def(1)
            .unwrap()
            .find(11)
            .unwrap()
            .kind
            .editor()
            .unwrap()
            .clone()
    };
    for _ in 0..6 {
        g.wheel(&mut host, &ctx, x, y, -2.0);
    }
    let amplitude = editor(&host).y_view();
    assert!(
        amplitude.1 > 1.0,
        "an amplitude opens past full scale: {amplitude:?}"
    );
    let mut effects = Vec::new();
    let mut set = |host: &mut Host, stack: serde_json::Value| {
        assert!(host.set_props(11, vec![("layers".into(), stack)], &mut effects));
    };
    set(&mut host, serde_json::json!(["spectrogram"]));
    let hz = editor(&host);
    assert_eq!(hz.ruler_y, crate::host::widget::RulerY::Hz);
    assert_eq!(hz.y_view(), (0.0, 1.0), "the frequency axis, whole");
    for _ in 0..6 {
        g.wheel(&mut host, &ctx, x, y, -2.0);
    }
    assert_eq!(editor(&host).y_view(), (0.0, 1.0), "nothing above Nyquist");
    set(&mut host, serde_json::json!(["peak", "rms"]));
    let back = editor(&host);
    assert_eq!(back.ruler_y, crate::host::widget::RulerY::Norm);
    assert_eq!(
        back.y_view(),
        amplitude,
        "the amplitude window, as it was left"
    );
}

/// **A pane's gutter is the body in hand's.** With the take in hand the take
/// rules it; with the curve in hand the curve does, and the take does not --
/// one gutter, one unit.
#[test]
fn a_panes_vertical_ruler_follows_the_body_in_hand() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"take","axes":{"x":{"unit":"samples"}},"children":[
            {"id":11,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2},
            {"id":12,"type":"curve","min":0,"max":2,"points":[0,1,1,0,1000,1,1,0]}
        ]}] }"#,
    );
    let rules = |host: &Host| {
        let pane = host.window_def(1).unwrap().find(10).unwrap();
        (
            crate::host::layers::child_rules_y(pane, 0),
            crate::host::layers::child_rules_y(pane, 1),
        )
    };
    assert_eq!(rules(&host), (true, false), "the take in hand rules");
    let mut effects = Vec::new();
    assert!(host.set_props(
        10,
        vec![("layer".into(), serde_json::json!("points"))],
        &mut effects
    ));
    assert_eq!(rules(&host), (false, true), "the envelope in hand rules");
    host.set_props(
        10,
        vec![("layer".into(), serde_json::json!("placement"))],
        &mut effects,
    );
    assert_eq!(
        rules(&host),
        (true, false),
        "nothing in hand: the take the pane is built around"
    );
}

#[test]
fn pane_routes_a_press_to_the_active_body_below_another_body() {
    let mut host = layered_pane_host();
    let ctx = GestureCtx::new(1, 600, 400);
    let rect = placed_rect(&host, &ctx, 11);
    let x = f64::from(rect.x + rect.w * 0.5);
    let y = f64::from(rect.y + rect.h * 0.8);
    let target = hit(&host, &ctx, x, y).unwrap();
    assert_eq!(
        target.id, 11,
        "the later curve must not intercept the active body's press"
    );
    assert_eq!(host.timeline_key(10), host.timeline_key(11));
    assert_eq!(host.timeline_key(10), host.timeline_key(12));
    let mut gestures = Gestures::default();
    gestures.press(&mut host, &ctx, x, y);
    gestures.drag_to(&mut host, &ctx, x, y - 20.0);
    let effects = gestures.release(&mut host, &ctx, x, y - 20.0);
    assert!(has_emit_tag(&effects, 11, "points"), "{effects:?}");
    assert!(!has_emit_tag(&effects, 12, "points"));
    set_prop(&mut host, 10, "layer", "points:1");
    assert_eq!(hit(&host, &ctx, x, y).unwrap().id, 12);
}

/// **A press on a folded strip's name asks its owner to unfold it**, and a
/// press on an open row's ruler is still the axis grabbed.
#[test]
fn a_press_on_a_folded_strip_asks_to_unfold_it() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":61,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],
             "base_bucket":2,"folded":[0]}]}"#,
    );
    host.set_timeline_total(61, 1000);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.slot_channels.insert(61, 4);
    let mut unfolded = None;
    let mut grabbed = false;
    for y in (0..300).step_by(2) {
        let mut effects = g.press(&mut host, &ctx, 10.0, y as f64);
        effects.extend(g.release(&mut host, &ctx, 10.0, y as f64));
        match emitted_args(&effects, 61) {
            Some(args) if args.first() == Some(&OscType::String("unfold".into())) => {
                unfolded.get_or_insert(args);
            }
            _ if unfolded.is_some() => grabbed = true,
            _ => {}
        }
    }
    assert_eq!(
        unfolded,
        Some(vec![OscType::String("unfold".into()), OscType::Int(0)]),
        "the first strip is channel 0's"
    );
    assert!(grabbed, "below it the rows are open and nothing is asked");
}

/// **A verb from the menu bar reaches the take inside a pane.** The menu
/// carries no pointer over the take, so the window's one take is the
/// addressee -- and a pane, its curve and its take are three widgets on the
/// axis holding one take, not three takes.
#[test]
fn select_all_from_the_menu_sweeps_the_take_in_a_pane() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
        {"id":10,"type":"pane","layer":"take","axes":{"x":{"unit":"samples"}},"children":[
            {"id":11,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2},
            {"id":12,"type":"curve","min":0,"max":1,"points":[0,1,1,0,3,1,1,0]}
        ]}] }"#,
    );
    host.set_timeline_total(11, 4);
    let ctx = GestureCtx::new(1, 600, 400);
    let mut g = Gestures::default();
    let out = g.command(&mut host, &ctx, "select_all");
    let state = host
        .timeline_key(11)
        .and_then(|key| host.timelines().state(key))
        .unwrap();
    assert_eq!(state.selection(), Some((0.0, 4.0)), "{out:?}");
}

/// **The work over a window is called off from it**: the work-in-progress
/// panel's button, and Escape while the panel is up, report `"call_off"`
/// from the window to its owner, whose work it is; with no work running,
/// Escape is what it always was.
#[test]
fn the_work_over_a_window_is_called_off_by_its_button_and_escape() {
    let mut host = host_from(
        r#"{"type":"window","progress":0.4,"progress_label":"Reverse","children":[
            {"id":10,"type":"knob"}]}"#,
    );
    let ctx = GestureCtx::new(1, 800, 400);
    let mut g = Gestures::default();
    let button = host
        .progress_cancel_rect(1, ctx.fb_w, ctx.fb_h)
        .expect("a button while work runs");
    let (x, y) = (
        f64::from(button.x + button.w * 0.5),
        f64::from(button.y + button.h * 0.5),
    );
    let pressed = g.press(&mut host, &ctx, x, y);
    assert!(
        has_emit_tag(&pressed, 1, crate::host::gestures::CALL_OFF),
        "{pressed:?}"
    );
    g.release(&mut host, &ctx, x, y);
    let escaped = key(&g, &mut host, &ctx, Key::Escape).unwrap_or_default();
    assert!(
        has_emit_tag(&escaped, 1, crate::host::gestures::CALL_OFF),
        "{escaped:?}"
    );

    // the work ended: no button, and Escape calls nothing off
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(1),
                OscType::String("progress".into()),
                OscType::Float(-1.0),
            ],
        }),
        from(),
    );
    assert!(host.progress_cancel_rect(1, ctx.fb_w, ctx.fb_h).is_none());
    let escaped = key(&g, &mut host, &ctx, Key::Escape).unwrap_or_default();
    assert!(
        !has_emit_tag(&escaped, 1, crate::host::gestures::CALL_OFF),
        "{escaped:?}"
    );
}
