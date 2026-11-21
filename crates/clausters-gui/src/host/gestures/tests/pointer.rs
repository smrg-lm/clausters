//! The pointer's shape: what a press would do, said before it and held by it,
//! and the tip of the place under the pointer.

use super::*;
use crate::host::pointer::Pointer;

/// The body of the time axis widget `id` is drawn against, in window 1.
fn axis_body(host: &Host, ctx: &GestureCtx, id: i32) -> Rect {
    let r = placed_rect(host, ctx, id);
    let (x, y) = ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64);
    let h = interact::hit(host, 1, ctx.fb_w, ctx.fb_h, x, y, &|_, _| 1).unwrap();
    interact::time_of(&h.chain).unwrap().1.body
}

/// **Text a press puts a caret in shows the I-beam**, a button the arrow,
/// and off the window the pointer is the arrow again.
#[test]
fn a_text_field_shows_the_i_beam_and_a_button_the_arrow() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"layout":"col","children":[
            {"id":5,"type":"text","value":"abc"},
            {"id":6,"type":"button","label":"Go"}]}"#,
    );
    let ctx = GestureCtx::new(1, 400, 200);
    let mut g = Gestures::default();
    let field = placed_rect(&host, &ctx, 5);
    let button = placed_rect(&host, &ctx, 6);
    let mid = |r: Rect| ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64);
    let at = mid(field);
    g.motion(&mut host, &ctx, at.0, at.1);
    assert_eq!(g.pointer(), Pointer::Text);
    let at = mid(button);
    g.motion(&mut host, &ctx, at.0, at.1);
    assert_eq!(g.pointer(), Pointer::Default);
    let at = mid(field);
    g.motion(&mut host, &ctx, at.0, at.1);
    g.hover(&mut host, &ctx, None);
    assert_eq!(g.pointer(), Pointer::Default, "off the window");
}

/// **The code element's text is an I-beam and its line numbers an arrow**,
/// and a selection swept from the text keeps the I-beam over the numbers
/// until the release.
#[test]
fn a_selection_keeps_the_i_beam_until_its_release() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"code","h":200,"text":"one\ntwo\nthree"}]}"#,
    );
    let ctx = GestureCtx::new(1, 400, 240);
    let mut g = Gestures::default();
    let r = placed_rect(&host, &ctx, 5);
    let y = (r.y + 8.0) as f64;
    let (numbers, text) = ((r.x + 4.0) as f64, (r.x + r.w * 0.5) as f64);
    g.motion(&mut host, &ctx, numbers, y);
    assert_eq!(g.pointer(), Pointer::Default, "the line numbers");
    g.motion(&mut host, &ctx, text, y);
    assert_eq!(g.pointer(), Pointer::Text, "the text");
    g.press(&mut host, &ctx, text, y);
    g.drag_to(&mut host, &ctx, numbers, y);
    assert_eq!(g.pointer(), Pointer::Text, "held over the numbers");
    g.release(&mut host, &ctx, numbers, y);
    assert_eq!(g.pointer(), Pointer::Default, "let go there");
}

/// **A waveform a drag selects over shows the I-beam**: the container's
/// sweep answers where nothing of the element's is under the pointer.
#[test]
fn a_sweep_over_samples_shows_the_i_beam() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.5,-0.5,1.0,0.25,-0.25,0.75,0.0],"base_bucket":2,
             "gestures":{"drag":"select"}}]}"#,
    );
    host.set_timeline_total(50, 100_000);
    let ctx = GestureCtx::new(1, 800, 300);
    let mut g = Gestures::default();
    let body = axis_body(&host, &ctx, 50);
    let at = (
        (body.x + body.w * 0.5) as f64,
        (body.y + body.h * 0.5) as f64,
    );
    g.motion(&mut host, &ctx, at.0, at.1);
    assert_eq!(g.pointer(), Pointer::Text);
}

/// **A note is taken by its body and pulled by its end**, the body closes
/// the hand while it is carried, and the pencil places precisely on empty
/// grid.
#[test]
fn a_rolls_note_says_which_part_a_press_takes() {
    let roll = |tool: &str| {
        let mut host = host_from(&format!(
            r#"{{"type":"window","margin":0,"children":[
                {{"id":90,"type":"notes","min":48.0,"max":72.0,"tool":"{tool}",
                 "notes":[0.0,1000.0,60.0,100,0]}}]}}"#
        ));
        host.set_timeline_total(90, 10000);
        host
    };
    let mut host = roll("select");
    let ctx = GestureCtx::new(1, 800, 400);
    let mut g = Gestures::default();
    let grid = axis_body(&host, &ctx, 90);
    let y60 = crate::host::graphics::pianoroll::pitch_to_y(60.0, 48.0, 72.0, grid) as f64;
    let end = (grid.x + grid.w * 0.1) as f64 - 1.0;
    let body = (grid.x + grid.w * 0.05) as f64;
    let empty = (grid.x + grid.w * 0.6) as f64;
    g.motion(&mut host, &ctx, body, y60);
    assert_eq!(g.pointer(), Pointer::Grab, "the body");
    g.motion(&mut host, &ctx, end, y60);
    assert_eq!(g.pointer(), Pointer::EwResize, "the end");
    g.motion(&mut host, &ctx, empty, y60);
    assert_eq!(g.pointer(), Pointer::Default, "empty grid sweeps");
    g.press(&mut host, &ctx, body, y60);
    assert_eq!(g.pointer(), Pointer::Grabbing, "carried");
    g.drag_to(&mut host, &ctx, body + 30.0, y60);
    assert_eq!(g.pointer(), Pointer::Grabbing);
    g.release(&mut host, &ctx, body + 30.0, y60);
    assert_eq!(g.pointer(), Pointer::Grab, "let go on it");

    let mut host = roll("draw");
    let mut g = Gestures::default();
    g.motion(&mut host, &ctx, empty, y60);
    assert_eq!(g.pointer(), Pointer::Crosshair, "the pencil");
    let mut host = roll("erase");
    let mut g = Gestures::default();
    g.motion(&mut host, &ctx, body, y60);
    assert_eq!(g.pointer(), Pointer::Erase, "the eraser");
    let mut host = roll("split");
    let mut g = Gestures::default();
    g.motion(&mut host, &ctx, body, y60);
    assert_eq!(g.pointer(), Pointer::Cut, "the scissors");
}

/// **The roll names the note under the pointer** once the pointer rests on
/// it, and the next note's tip shows at once.
#[test]
fn a_rolls_tip_names_the_note_under_the_pointer() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":90,"type":"notes","min":48.0,"max":72.0,
             "notes":[0.0,1000.0,60.0,100,0, 5000.0,1000.0,62.0,90,0]}]}"#,
    );
    host.set_timeline_total(90, 10000);
    let mut ctx = GestureCtx::new(1, 800, 400);
    let mut g = Gestures::default();
    let grid = axis_body(&host, &ctx, 90);
    let y = |p: f32| crate::host::graphics::pianoroll::pitch_to_y(p, 48.0, 72.0, grid) as f64;
    let tip = |host: &Host| {
        host.popups(1)
            .and_then(|p| p.tip.as_ref())
            .map(|t| t.text.clone())
    };
    ctx.now_ms = 1000.0;
    g.motion(&mut host, &ctx, (grid.x + grid.w * 0.05) as f64, y(60.0));
    assert_eq!(tip(&host), None, "not rested yet");
    ctx.now_ms = 1700.0;
    g.elapsed(&mut host, &ctx);
    assert_eq!(tip(&host).as_deref(), Some("C4, velocity 100"));
    ctx.now_ms = 1800.0;
    g.motion(&mut host, &ctx, (grid.x + grid.w * 0.55) as f64, y(62.0));
    assert_eq!(tip(&host).as_deref(), Some("D4, velocity 90"), "at once");
    g.motion(&mut host, &ctx, (grid.x + grid.w * 0.8) as f64, y(62.0));
    assert_eq!(tip(&host), None, "off the notes");
}

/// **A box is taken by its body and pulled by its edges** in a multitrack,
/// and a track's bottom edge sizes its row.
#[test]
fn a_multitrack_box_says_which_part_a_press_takes() {
    let mut host = host_from(
        r#"{"type":"window","status":0,"margin":0,"layout":"col","children":[
            {"id":70,"type":"multitrack","link":"a","ruler":"samples",
             "sample_rate":48000,
             "tracks":["one", "", 120, 0, 0, 1.0, 1],
             "clips":["a","one",0,1000,0,"",-1, "b","one",9000,1000,0,"",-1]}]}"#,
    );
    host.sync_track_totals();
    let ctx = GestureCtx::new(1, 800, 400);
    let mut g = Gestures::default();
    let rect = placed_rect(&host, &ctx, 70);
    let body = axis_body(&host, &ctx, 70);
    let y = rect.y as f64 + 60.0;
    let on_box = body.x as f64 + body.w as f64 * 0.05;
    let on_edge = body.x as f64 + body.w as f64 * 0.1 - 1.0;
    let slack = body.x as f64 + body.w as f64 * 0.5;
    g.motion(&mut host, &ctx, on_box, y);
    assert_eq!(g.pointer(), Pointer::Grab, "the body");
    g.motion(&mut host, &ctx, on_edge, y);
    assert_eq!(g.pointer(), Pointer::EwResize, "the end");
    g.motion(&mut host, &ctx, slack, y);
    assert_eq!(g.pointer(), Pointer::Default, "the slack sweeps");
    g.press(&mut host, &ctx, on_box, y);
    assert_eq!(g.pointer(), Pointer::Grabbing, "carried");
    g.release(&mut host, &ctx, on_box, y);
}
