//! A roll under the hand: the sweep, the cursor a click puts and where a paste lands.

use super::*;

/// **Alt sweeps a time range over a roll, anywhere, and takes no notes**; an
/// Alt click that never moves is the roll's toggle of the note under it, not a
/// place for the cursor.
#[test]
fn alt_sweeps_a_range_over_a_roll_and_an_alt_click_toggles_a_note() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,
             "notes":[0.0,400.0,60.0,100,0, 6000.0,400.0,61.0,100,0]}]}"#,
    );
    host.set_timeline_total(90, 10000);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 400);
    ctx.alt = true;
    let grid = {
        let h = interact::hit(&host, 1, 800, 400, 400.0, 100.0, &|_, _| 1).unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    let key = host.timeline_key(90).unwrap();
    // A sweep that starts on the first note: a span, and the note untouched.
    let y60 = {
        let (lo, hi) = (48.0, 72.0);
        crate::host::graphics::pianoroll::pitch_to_y(60.0, lo, hi, grid) as f64
    };
    let x0 = grid.x as f64 + 2.0;
    let x1 = grid.x as f64 + grid.w as f64 * 0.3;
    let effects = g.press(&mut host, &ctx, x0, y60);
    assert!(has_emit_tag(&effects, 90, "selection"));
    g.drag_to(&mut host, &ctx, x1, y60);
    g.release(&mut host, &ctx, x1, y60);
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0, "a span");
    assert!(selected_notes(&host, 90).is_empty(), "and no note taken");
    // An Alt click on the note: toggled into the roll's selection.
    let effects = g.press(&mut host, &ctx, x0, y60);
    let released = g.release(&mut host, &ctx, x0, y60);
    assert_eq!(
        selected_notes(&host, 90),
        vec![0],
        "{effects:?} {released:?}"
    );
    assert!(
        !has_emit_tag(&released, 90, "locate"),
        "the click was the note's, not a place"
    );
}

/// **Ctrl+V pastes at the cursor**, which is the one the click placed -- not at
/// a step position of the roll's own, which only step entry ever moved.
#[test]
fn the_rolls_paste_lands_on_the_windows_cursor() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,"snap":0,
             "notes":[0.0,400.0,60.0,100,0]}]}"#,
    );
    host.set_timeline_total(90, 10000);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 400);
    let grid = {
        let h = interact::hit(&host, 1, 800, 400, 400.0, 100.0, &|_, _| 1).unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    host.clipboard.set_text(r#"[0.0,400.0,60.0,100,0]"#);
    // Put the cursor at a fifth of the axis with a click...
    let x = grid.x as f64 + grid.w as f64 * 0.2;
    let y = grid.y as f64 + grid.h as f64 * 0.5;
    g.press(&mut host, &ctx, x, y);
    g.release(&mut host, &ctx, x, y);
    let cursor = host
        .timelines()
        .state(host.timeline_key(90).unwrap())
        .unwrap()
        .playhead;
    // ...and paste over the roll.
    ctx.ctrl = true;
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('v'), Some((x, y)))
        .expect("the roll took the paste");
    let args = emitted_args(&effects, 90).expect("the paste reports the notes");
    let starts: Vec<f64> = args[1..]
        .chunks(5)
        .filter_map(|c| match c.first() {
            Some(OscType::Double(v)) => Some(*v),
            _ => None,
        })
        .collect();
    assert!(
        starts.iter().any(|s| (*s - cursor).abs() < 40.0),
        "the pasted note starts at the cursor ({cursor}), not at 0: {starts:?}"
    );
}

/// **A click on a multitrack places the cursor.** The gesture the widget's own
/// map does not name and the machine owns: a press that never left the slop is
/// where the hand pointed, so it locates over bare stack and over a clip alike
/// -- and a sweep, which left a rectangle, does not.
///
/// It is the one verb of the whole vocabulary that a `multitrack` inherits
/// rather than implements, which is exactly why nothing tested it.
#[test]
fn a_click_on_a_multitrack_locates_and_a_sweep_does_not() {
    let mut host = host_from(
        r#"{"type":"window","status":0,"margin":0,"layout":"col","children":[
            {"id":70,"type":"multitrack","link":"a","ruler":"samples",
             "sample_rate":48000,
             "tracks":["one", "", 120, 0, 0, 1.0, 1],
             "clips":["a","one",0,1000,0,"",-1, "b","one",9000,1000,0,"",-1]}]}"#,
    );
    host.sync_track_totals();
    let ctx = GestureCtx::new(1, 800, 400);
    let rect = placed_rect(&host, &ctx, 70);
    let midy = (rect.y + rect.h * 0.5) as f64;
    // The **axis' own body**, not the widget's: a multitrack keeps a header
    // gutter beside it, and a press there names no time at all.
    let body = {
        let h = interact::hit(
            &host,
            1,
            800,
            400,
            (rect.x + rect.w * 0.5) as f64,
            midy,
            &|_, _| 1,
        )
        .unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    let empty = body.x as f64 + body.w as f64 * 0.5;

    let mut g = Gestures::default();
    let effects = g.press(&mut host, &ctx, empty, midy);
    assert!(
        !has_emit_tag(&effects, 70, "locate"),
        "a press does not know yet whether it is a click"
    );
    let effects = g.release(&mut host, &ctx, empty, midy);
    assert!(
        has_emit_tag(&effects, 70, "locate"),
        "the slack between boxes is a place, so the mark is reachable without \
         going up to the strip: {effects:?}"
    );

    // A box is not: the clips are content, and a click on one is that clip's.
    // Its row is the first `height` of the stack; `midy` above is the slack
    // under the last lane, which is why that one was a place.
    let on_lane = rect.y as f64 + 60.0;
    let on_clip = body.x as f64 + body.w as f64 * 0.02;
    g.press(&mut host, &ctx, on_clip, on_lane);
    let effects = g.release(&mut host, &ctx, on_clip, on_lane);
    assert!(
        !has_emit_tag(&effects, 70, "locate"),
        "a click on a clip is not a place: {effects:?}"
    );

    // The **ruler strip** is: the bottom `ruler_h` of the stack's rect.
    let on_ruler = (rect.y + rect.h) as f64 - 2.0;
    g.press(&mut host, &ctx, empty, on_ruler);
    let effects = g.release(&mut host, &ctx, empty, on_ruler);
    assert!(
        has_emit_tag(&effects, 70, "locate"),
        "a click on the ruler places the cursor: {effects:?}"
    );

    // A sweep left a rectangle, and a rectangle is not a place.
    g.press(&mut host, &ctx, empty, on_ruler);
    g.drag_to(&mut host, &ctx, empty + 90.0, on_ruler);
    let effects = g.release(&mut host, &ctx, empty + 90.0, on_ruler);
    assert!(
        !has_emit_tag(&effects, 70, "locate"),
        "a rectangle is not a cursor"
    );

    // Beside the axis -- the header gutter -- there is no position and no click.
    let effects = {
        g.press(&mut host, &ctx, body.x as f64 - 10.0, on_ruler);
        g.release(&mut host, &ctx, body.x as f64 - 10.0, on_ruler)
    };
    assert!(
        !has_emit_tag(&effects, 70, "locate"),
        "a press beside the axis names no time"
    );
}

/// ...and the time range is still there, by name: a roll planned `select`
/// sweeps the span every linked view draws, exactly as a lane does.
#[test]
fn a_roll_asked_for_the_range_by_name_sweeps_the_span() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,"gestures":{"drag":"select"},
             "notes":[0.0,400.0,60.0,100,0, 6000.0,400.0,61.0,100,0]}]}"#,
    );
    host.set_timeline_total(90, 10000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let grid = {
        let h = interact::hit(&host, 1, 800, 400, 400.0, 100.0, &|_, _| 1).unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    let x0 = grid.x as f64 + 1.0;
    let x1 = grid.x as f64 + grid.w as f64 * 0.1;
    let effects = g.press(&mut host, &ctx, x0, (grid.y + grid.h) as f64 - 1.0);
    assert!(has_emit_tag(&effects, 90, "selection"));
    g.drag_to(&mut host, &ctx, x1, grid.y as f64 + 1.0);
    let key = host.timeline_key(90).unwrap();
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
}

/// **A curve standing on its own asks for a frame per pointer move**, since
/// it draws a readout of what the pointer is over; a window of controls asks
/// for none.
#[test]
fn a_window_with_a_standalone_curve_redraws_on_hover() {
    let host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":70,"type":"curve","min":0.0,"max":1.0,
             "points":[0.0,0.0,1,0.0, 1.0,1.0,1,0.0]}]}"#,
    );
    assert!(host.window_def(1).unwrap().has_hover_readout());
    let quiet = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":71,"type":"button","label":"play"}]}"#,
    );
    assert!(!quiet.window_def(1).unwrap().has_hover_readout());
}
