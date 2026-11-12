//! The one navigable axis that is not time: a spectrum's frequency.

use super::*;

/// A sweep on the roll's grid is a **marquee**: the rectangle it covers in
/// time x pitch picks the notes, and it writes **no time span** -- a selection
/// of notes is a selection of the rectangles the notes are, as a patcher's is
/// of its boxes. The time range over the same grid is the other selection and
/// is asked for by name (`{"drag": "select"}`), the rule a lane already states.
#[test]
fn a_sweep_on_the_roll_selects_the_notes_and_writes_no_span() {
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
    assert!(
        !has_emit_tag(&effects, 90, "selection"),
        "a marquee reports no span"
    );
    g.drag_to(&mut host, &ctx, x1, grid.y as f64 + 1.0);
    let key = host.timeline_key(90).unwrap();
    assert_eq!(
        host.timelines().state(key).unwrap().sel_len,
        0.0,
        "the grid's sweep leaves no band behind it"
    );
    assert_eq!(
        selected_notes(&host, 90),
        vec![0],
        "only the note inside the swept rectangle"
    );
}

/// The wheel over a navigable spectrum zooms its **frequency** axis, anchored
/// at the cursor -- and it reports the element's own `"view_x"`, not a group's
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
/// and `R` puts the whole axis back -- the same key that resets the timelines,
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
