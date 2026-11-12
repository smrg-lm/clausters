//! The controls: a slider, a button, a toggle and a knob under a press and a drag.

use super::*;

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

/// **A binding swallows the value and never the command.** A bound button
/// drives the audio server with no script in the path -- that is what
/// `/gui_bind` is for -- and the script still hears the click, because a command
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
/// to flip the value. The filter is the machine's -- the element only declares
/// its shape -- so this is the same mechanism the knob and the slider go
/// through, checked once at the level where it is applied.
#[test]
fn a_toggle_does_not_flip_from_the_air_beside_it() {
    // The panel that showed it: a row of mixed controls, so the toggle's cell
    // is as tall as its tallest sibling and as wide as its share of the row --
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

/// One widget's current value, as `/gui_query` would read it.
fn value_of(host: &Host, id: i32) -> Option<OscType> {
    host.window_def(1)?.find(id)?.kind.event_value()
}
