//! The gesture table a view carries: what a modifier is repointed to, and the one shape a press and a wheel read.

use super::*;

/// The arguments of the first `/gui_event` a verb emitted for `id`.
fn first_emit(effects: &[GestureEffect], id: i32) -> Option<Vec<OscType>> {
    effects.iter().find_map(|e| match e {
        GestureEffect::Emit {
            widget_id, args, ..
        } if *widget_id == id => Some(args.clone()),
        _ => None,
    })
}

/// **Every pointer question reads the shape the element draws**, not just the
/// press: an element drawn rounder or smaller than its cell answers the wheel
/// only where it is drawn, and the air around it belongs to whatever the
/// container puts there. A wheel is worse to get wrong than a press, since it
/// is not aimed at all -- it is where the hand happened to leave the pointer.
#[test]
fn the_wheel_reads_the_same_shape_the_press_does() {
    /// A leaf drawn as the disc inscribed in its cell, answering both the press
    /// and the wheel -- the two questions that must agree on where it is.
    #[derive(Debug, Clone)]
    struct Dial;
    impl crate::host::widget::Element for Dial {
        fn set(&mut self, _key: &str, _v: &serde_json::Value) -> bool {
            false
        }
        fn draw(
            &self,
            _d: &mut clausters_gui_draw::draw::Draw,
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
    // The corner of the same cell is outside the disc -- and outside the hit
    // slop around it, which is a few pixels and not a hundred.
    let corner = (cell.x as f64 + 1.0, cell.y as f64 + 1.0);
    let effects = g.wheel(&mut host, &ctx, corner.0, corner.1, 1.0);
    assert!(
        first_emit(&effects, 90).is_none(),
        "the corner is the window's: {effects:?}"
    );
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

/// **Navigation leaves no line on the status bar**: a locate is reported to
/// the owner, and the cursor the picture moves is all a reader needs -- the
/// line it used to leave said only the raw sample count.
#[test]
fn a_locate_is_reported_and_not_said() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":96,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(96, 10000);
    set_prop(&mut host, 96, "gestures", r#"{"drag":"locate"}"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let effects = g.press(&mut host, &ctx, 400.0, 150.0);
    assert!(has_emit_tag(&effects, 96, "locate"));
    let said: Vec<String> = host
        .statuses()
        .get(&1)
        .map(|s| s.lines().map(|l| l.text.clone()).collect())
        .unwrap_or_default();
    assert!(said.is_empty(), "{said:?}");
}
