//! Where an axis stops: the floor of a zoom and the ends of a pan.

use super::*;

/// **Panning to the end of a frequency axis stops there.** The zoom floor is
/// measured forward from the window's left edge, and a pan hands over an edge
/// that is off the axis -- that is what dragging past it means, the write
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
        crate::host::graphics::signal::spectrum::axis_geometry(
            48_000.0,
            crate::spectrogram::FreqScale::Log,
            440.0,
        )
        .1,
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
/// window has to open -- four bins at 20 Hz are a quarter of the axis, and no
/// zoom can be finer than the analysis under it -- but what the reader *asked*
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
/// the analysis resolves where it sits -- what the frame draws and `"view_x"`
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
