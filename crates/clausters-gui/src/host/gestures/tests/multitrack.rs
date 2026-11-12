//! A multitrack under a click: the locate, and the time range it lets go of.

use super::*;

/// **A click lets go of a time range, wherever it lands on the stack**: on the
/// slack, on the ruler and on a box alike, and only Alt sweeps one.
#[test]
fn a_click_on_a_multitrack_lets_go_of_its_time_range() {
    let mut host = host_from(
        r#"{"type":"window","status":0,"margin":0,"layout":"col","children":[
            {"id":70,"type":"multitrack","link":"a","ruler":"samples",
             "sample_rate":48000,
             "tracks":["one", "", 120, 0, 0, 1.0, 1],
             "clips":["a","one",0,1000,0,"",-1, "b","one",9000,1000,0,"",-1]}]}"#,
    );
    host.sync_track_totals();
    let mut ctx = GestureCtx::new(1, 800, 400);
    let rect = placed_rect(&host, &ctx, 70);
    let midy = (rect.y + rect.h * 0.5) as f64;
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
    let on_lane = rect.y as f64 + 60.0;
    let on_clip = body.x as f64 + body.w as f64 * 0.02;
    let on_ruler = (rect.y + rect.h) as f64 - 2.0;
    let key = host.timeline_key(70).unwrap();
    let mut g = Gestures::default();
    for (x, y) in [(empty, midy), (empty, on_ruler), (on_clip, on_lane)] {
        ctx.alt = true;
        g.press(&mut host, &ctx, empty, midy);
        g.drag_to(&mut host, &ctx, empty + 90.0, midy);
        g.release(&mut host, &ctx, empty + 90.0, midy);
        ctx.alt = false;
        assert!(host.timelines().state(key).unwrap().sel_len > 0.0, "swept");
        g.press(&mut host, &ctx, x, y);
        let effects = g.release(&mut host, &ctx, x, y);
        assert_eq!(
            host.timelines().state(key).unwrap().sel_len,
            0.0,
            "a click at {x},{y} lets it go: {effects:?}"
        );
        assert!(has_emit_tag(&effects, 70, "selection"), "and says so");
    }
}

/// **A double click is two presses close in time and in place** -- and the rule
/// is the machine's, counted from the clock the front hands it, because winit
/// reports no click count and a browser reports one of its own. Two fronts
/// answering differently is exactly the divergence the shared machine is for.
#[test]
fn two_presses_close_in_time_and_place_are_one_double_click() {
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);

    ctx.now_ms = 1_000.0;
    assert_eq!(g.count_press(&ctx, 100.0, 50.0), 1);
    ctx.now_ms = 1_200.0;
    assert_eq!(
        g.count_press(&ctx, 102.0, 51.0),
        2,
        "close enough, both ways"
    );
    ctx.now_ms = 1_300.0;
    assert_eq!(
        g.count_press(&ctx, 102.0, 51.0),
        3,
        "a run goes on counting"
    );

    // Too slow.
    ctx.now_ms = 5_000.0;
    assert_eq!(g.count_press(&ctx, 102.0, 51.0), 1);
    // ...and too far: a hand that pressed somewhere else meant two things,
    // however fast it was.
    ctx.now_ms = 5_100.0;
    assert_eq!(g.count_press(&ctx, 400.0, 51.0), 1);
}
