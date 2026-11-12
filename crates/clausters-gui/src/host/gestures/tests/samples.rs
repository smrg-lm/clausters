//! Samples under the pencil: one grabbed, a stroke drawn, and the refusals.

use super::*;

/// The step **declines where a sample is not a thing on screen** -- the trace
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

/// **A stroke belongs to the channel it started on.** Dragging up out of a
/// lane must clamp to *that* channel's maximum, not read the value the lane
/// above would show at the same height -- which is what a stereo stroke did
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
/// past the window -- that is what a drag grab is for -- and a pencil that
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

/// **A fault is said out loud and consumed; nothing to act on is declined.**
///
/// The two are one line apart in the arms and they are not the same thing. A
/// step that finds nothing to act on where the picture is good -- the samples
/// are not drawn one by one -- is not this gesture's press, so the plan tries
/// its next step, which is what `"sample select"` is composed of and what
/// `docs/gui-protocol.md` specifies for `sample`. A step that finds the picture
/// *wrong* -- a view holding no samples at all -- has hit a fault, and falling
/// through there is how the pencil silently became a selection tool after an
/// undo (found 2026-09-07, `editors/edit_samples`).
#[test]
fn a_view_with_no_samples_refuses_instead_of_sweeping() {
    // `bulk` with a path nothing resolves: the element names a resource, so it
    // holds no inline samples, and the zoom is well inside where dots are
    // drawn. Nothing is on screen to draw *on*, and that is a fault.
    let def = r#"{"type":"window","status":0,"children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "path":"nothing.f32","bulk":true,"channels":1,
             "gestures":{"drag":"draw","ctrl":"sample select"}}]}"#;
    let mut host = host_from(def);
    host.set_timeline_total(50, 20);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);

    let effects = g.press(&mut host, &ctx, 400.0, 150.0);
    let args = emitted_args(&effects, 50).expect("the pencil says so");
    assert_eq!(args[0], OscType::String("refused".into()));
    assert_eq!(args[1], OscType::String("draw".into()));
    assert!(!g.dragging(), "and no stroke was opened");

    // The press was **consumed**: the sweep behind it never ran, which is the
    // whole of the fix. A drag after it reports no selection.
    let effects = g.drag_to(&mut host, &ctx, 600.0, 150.0);
    assert!(
        !has_emit_tag(&effects, 50, "selection"),
        "a refused stroke does not become a selection: {effects:?}"
    );
}

/// **The position cursor is placed where a click lands on nothing.** It says
/// where the *reader* is -- where a playback starts and where a paste lands -- so
/// it is never a side effect of pointing *at* something: a click on a note is
/// that note's and moves no line, while the grid nothing is drawn on, and the
/// ruler, are the ordinary ways to say "here".
#[test]
fn a_click_on_the_rolls_grid_puts_the_cursor_where_it_pointed() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,
             "notes":[0.0,400.0,60.0,100,0]}]}"#,
    );
    host.set_timeline_total(90, 10000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let grid = {
        let h = interact::hit(&host, 1, 800, 400, 400.0, 100.0, &|_, _| 1).unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    let key = host.timeline_key(90).unwrap();
    // The empty grid, at a fifth of the axis.
    let x = grid.x as f64 + grid.w as f64 * 0.2;
    let y = grid.y as f64 + grid.h as f64 * 0.5;
    g.press(&mut host, &ctx, x, y);
    let effects = g.release(&mut host, &ctx, x, y);
    assert!(
        has_emit_tag(&effects, 90, "locate"),
        "the empty grid is a place: {effects:?}"
    );
    let cursor = host.timelines().state(key).unwrap().cursor;
    assert!(
        (cursor - 2000.0).abs() < 40.0,
        "at the time the pointer named: {cursor}"
    );
    // And it is the **other** line: the playhead is never placed.
    assert!(
        host.timelines().state(key).unwrap().playhead < 0.0,
        "the playhead is not the cursor"
    );
    // The roll's **own ruler strip** -- the bottom `ruler_h` of its rect, drawn
    // by the view rather than by a widget -- places it too.
    let rect = placed_rect(&host, &ctx, 90);
    let on_ruler = (rect.y + rect.h) as f64 - 2.0;
    let far = grid.x as f64 + grid.w as f64 * 0.5;
    g.press(&mut host, &ctx, far, on_ruler);
    let effects = g.release(&mut host, &ctx, far, on_ruler);
    assert!(
        has_emit_tag(&effects, 90, "locate"),
        "a click on the ruler is a place"
    );
    assert!(
        (host.timelines().state(key).unwrap().cursor - 5000.0).abs() < 40.0,
        "and it moved there"
    );
}

/// The draw mode. A stroke writes every sample it passes -- including the
/// ones *between* two motion events, which is what makes it a stroke and not a
/// comb -- and leaves as one intent carrying both runs.
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

/// **The pencil's threshold is the drawing's**, not one of its own.
///
/// The defect (found 2026-09-07, on `editors/edit_samples`): the gate refused
/// below one pixel per sample, and the trace draws its sample dots at three
/// radii of spacing -- twelve pixels a sample at the default density. Between
/// the two numbers a stroke was allowed over a picture with no dots in it, so
/// a drag across the body wrote hundreds of samples the hand could not aim at
/// and could not see. The example's own instructions had the rule: *zoom in
/// until the samples are discs, then draw*.
#[test]
fn the_pencil_waits_for_the_dots_the_trace_draws() {
    use crate::host::graphics::signal::trace;

    // Enough samples behind the view that the zoomed-in case has one under the
    // pointer: the gate is what this tests, not what happens past the data.
    let data = ["0.0"; 400].join(",");
    let def = format!(
        r#"{{"type":"window","status":0,"children":[
            {{"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[{data}],"base_bucket":2,
             "gestures":{{"drag":"draw"}}}}]}}"#
    );
    let def = def.as_str();
    let ctx = GestureCtx::new(1, 800, 300);

    // The rule itself, in the unit the gate reads: a pixel a sample carries no
    // dots, and the old gate allowed a stroke at anything up to it.
    let radius = Metrics::default().point_radius;
    assert!(!trace::samples_are_drawn(1.0, radius), "the old ceiling");
    assert!(!trace::samples_are_drawn(0.25, radius), "four px a sample");
    assert!(trace::samples_are_drawn(
        trace::drawable_per_px(radius),
        radius
    ));

    // 400 samples across a body under 800 px wide: about half a sample a pixel,
    // so the old gate let it through, and the trace draws it as a bare line.
    let mut host = host_from(def);
    host.set_timeline_total(50, 400);
    let mut g = Gestures::default();
    let effects = g.press(&mut host, &ctx, 100.0, 100.0);
    let args = emitted_args(&effects, 50).expect("it says why");
    assert_eq!(args[0], OscType::String("refused".into()));
    assert_eq!(args[1], OscType::String("draw".into()));
    assert!(
        !g.dragging(),
        "and no stroke was opened over a picture with no dots in it"
    );
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_none(),
        "nothing was written"
    );

    // Zoomed until the dots fit -- twenty samples across the same body, tens of
    // pixels apart -- the same press draws.
    let mut host = host_from(def);
    host.set_timeline_total(50, 20);
    let mut g = Gestures::default();
    let effects = g.press(&mut host, &ctx, 100.0, 100.0);
    assert!(
        emitted_args(&effects, 50).is_none(),
        "no refusal where the samples are drawn one by one: {:?}",
        emitted_args(&effects, 50)
    );
    assert!(g.dragging(), "the stroke is open");
}
