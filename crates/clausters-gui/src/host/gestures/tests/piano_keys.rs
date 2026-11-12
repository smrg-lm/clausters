//! The piano: presses, glissandi, its range under the wheel and its voices.

use super::*;

#[test]
fn piano_fixed_velocity_and_grayed_keys() {
    // A fixed velocity overrides the press-height map.
    let (mut host, l) = piano_host(r#","velocity":90"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    let effects = g.press(
        &mut host,
        &ctx,
        (c.x + 2.0) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    assert_eq!(note_emits(&effects)[0].1, 90);
    g.release(&mut host, &ctx, c.x as f64, c.y as f64);
    // A press outside the active range is inert: no event, no held key.
    let (mut host, _) = piano_host(r#","active_min":64,"active_max":72"#);
    let mut g = Gestures::default();
    let effects = g.press(
        &mut host,
        &ctx,
        (c.x + 2.0) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    assert!(note_emits(&effects).is_empty());
    assert!(piano_pressed(&host).is_empty());
    // The press is still **taken** -- the keyboard is what the reader pointed
    // at, and letting it through would pan whatever is behind it -- so it is
    // held like any other, and lets go with nothing to report.
    let effects = g.release(&mut host, &ctx, c.x as f64, c.y as f64);
    assert!(note_emits(&effects).is_empty());
    assert!(!g.dragging());
}

fn note_emits(effects: &[GestureEffect]) -> Vec<(i32, i32, i32, i32)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("note".into())) =>
            {
                match args[1..] {
                    [
                        OscType::Int(p),
                        OscType::Int(v),
                        OscType::Int(s),
                        OscType::Int(c),
                    ] => Some((p, v, s, c)),
                    _ => panic!("malformed note payload: {args:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

/// A one-piano window (no overview, no label: the keys fill the widget
/// rect), plus the layout the gestures see. The window is 712x132 so the
/// child rect is (6,6,700,120): one octave C4..C5 = 8 white keys.
fn piano_host(extra: &str) -> (Host, piano::Layout) {
    let json = format!(
        r#"{{"type":"window","children":[
            {{"id":70,"type":"keys","min":60,"max":72,"overview":0{extra}}}]}}"#
    );
    let host = host_from(&json);
    let l = piano::layout(
        Rect::new(6.0, 6.0, 700.0, 120.0),
        60,
        72,
        false,
        false,
        &Metrics::default(),
    );
    (host, l)
}

/// The keyboard element behind widget 70 -- the concrete leaf, through the
/// trait's downcast door, which is what an element's own state is asserted on.
fn keys_of(host: &Host) -> &crate::host::elements::keys::Keys {
    host.window_def(1)
        .unwrap()
        .find(70)
        .unwrap()
        .kind
        .as_element()
        .and_then(|el| el.as_any())
        .and_then(|any| any.downcast_ref())
        .expect("widget 70 is a keyboard")
}

fn piano_pressed(host: &Host) -> Vec<i32> {
    keys_of(host).pressed.clone()
}

#[test]
fn piano_press_glissando_and_release_emit_midi_shaped_notes() {
    let (mut host, l) = piano_host(r#","channel":2"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    // Press the front of C4: note-on, high velocity, channel carried.
    let c = piano::key_rect(&l, 60).unwrap();
    let effects = g.press(
        &mut host,
        &ctx,
        (c.x + c.w * 0.5) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 1);
    let (p, v, s, ch) = notes[0];
    assert_eq!((p, s, ch), (60, 1, 2));
    assert!(v > 120, "front-of-key press is loud, got {v}");
    assert_eq!(piano_pressed(&host), vec![60]);
    // Glissando onto D4: off 60, on 62 (the new key's own velocity).
    let d = piano::key_rect(&l, 62).unwrap();
    let effects = g.drag_to(
        &mut host,
        &ctx,
        (d.x + d.w * 0.5) as f64,
        (d.y + d.h * 0.5) as f64,
    );
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 2);
    assert_eq!((notes[0].0, notes[0].2), (60, 0));
    assert_eq!((notes[1].0, notes[1].2), (62, 1));
    assert_eq!(piano_pressed(&host), vec![62]);
    // Release: note-off of the held key.
    let effects = g.release(&mut host, &ctx, d.x as f64, d.y as f64);
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].0, notes[0].2), (62, 0));
    assert!(piano_pressed(&host).is_empty());
}

#[test]
fn piano_glissando_across_two_keys_releases_each_left_key() {
    // A glissando spanning more than one crossing: every key left behind
    // must get its note-off, and the final release offs the last key only.
    let (mut host, l) = piano_host("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    g.press(
        &mut host,
        &ctx,
        (c.x + c.w * 0.5) as f64,
        (c.y + c.h - 1.0) as f64,
    );
    let d = piano::key_rect(&l, 62).unwrap();
    g.drag_to(
        &mut host,
        &ctx,
        (d.x + d.w * 0.5) as f64,
        (d.y + d.h * 0.5) as f64,
    );
    let e = piano::key_rect(&l, 64).unwrap();
    let effects = g.drag_to(
        &mut host,
        &ctx,
        (e.x + e.w * 0.5) as f64,
        (e.y + e.h * 0.5) as f64,
    );
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 2, "second crossing: one off, one on");
    assert_eq!((notes[0].0, notes[0].2), (62, 0), "the key left is 62");
    assert_eq!((notes[1].0, notes[1].2), (64, 1));
    assert_eq!(piano_pressed(&host), vec![64]);
    let effects = g.release(&mut host, &ctx, e.x as f64, e.y as f64);
    let notes = note_emits(&effects);
    assert_eq!(notes.len(), 1);
    assert_eq!((notes[0].0, notes[0].2), (64, 0));
    assert!(piano_pressed(&host).is_empty());
}

#[test]
fn piano_wheel_pans_the_range_and_pan_zero_freezes_it() {
    let (mut host, l) = piano_host("");
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    let (cx, cy) = ((c.x + c.w * 0.5) as f64, (c.y + c.h - 1.0) as f64);
    let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
    assert!(has_emit_tag(&effects, 70, "range"));
    let k = keys_of(&host);
    assert_eq!((k.min, k.max), (62, 74));
    // `pan: 0` silences every range gesture.
    let (mut host, _) = piano_host(r#","pan":0"#);
    let effects = g.wheel(&mut host, &ctx, cx, cy, 1.0);
    assert!(effects.is_empty());
    let k = keys_of(&host);
    assert_eq!((k.min, k.max), (60, 72));
}

#[test]
fn piano_overview_drag_pans_and_wheel_zooms() {
    // With the overview on, the strip sits at the top of the widget rect.
    let host_json = r#"{"type":"window","children":[
        {"id":70,"type":"keys","min":60,"max":72}]}"#;
    let mut host = host_from(host_json);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let l = piano::layout(
        Rect::new(6.0, 6.0, 700.0, 120.0),
        60,
        72,
        true,
        false,
        &Metrics::default(),
    );
    let strip = l.overview.unwrap();
    let sy = (strip.y + strip.h * 0.5) as f64;
    // Drag along the strip: the window pans with the cursor.
    let x0 = piano::overview_key_x(strip, 66) as f64;
    let x1 = piano::overview_key_x(strip, 78) as f64;
    let effects = g.press(&mut host, &ctx, x0, sy);
    assert!(note_emits(&effects).is_empty(), "the strip plays no note");
    let effects = g.drag_to(&mut host, &ctx, x1, sy);
    assert!(has_emit_tag(&effects, 70, "range"));
    let k = keys_of(&host);
    assert_eq!(k.max - k.min, 12, "pan keeps the span");
    assert!(k.min > 60, "the window moved right");
    g.release(&mut host, &ctx, x1, sy);
    // Wheel over the strip zooms out (steps < 0 widens the span).
    let effects = g.wheel(&mut host, &ctx, x1, sy, -2.0);
    assert!(has_emit_tag(&effects, 70, "range"));
    let k = keys_of(&host);
    assert!(k.max - k.min > 12);
}

#[test]
fn piano_voice_mode_tracks_one_node_per_held_pitch() {
    let (mut host, l) = piano_host(r#","voice":"pv","voice_args":["pan",0.5]"#);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 712, 132);
    let c = piano::key_rect(&l, 60).unwrap();
    let (cx, cy) = ((c.x + 2.0) as f64, (c.y + c.h - 1.0) as f64);
    g.press(&mut host, &ctx, cx, cy);
    let voices = host.voices_of(70).to_vec();
    assert_eq!(voices.len(), 1);
    assert_eq!(voices[0].0, 60);
    // Glissando: the old voice is released, a new node sounds the new key.
    let d = piano::key_rect(&l, 62).unwrap();
    g.drag_to(
        &mut host,
        &ctx,
        (d.x + d.w * 0.5) as f64,
        (d.y + 1.0) as f64,
    );
    let after = host.voices_of(70).to_vec();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].0, 62);
    assert_ne!(after[0].1, voices[0].1, "a fresh node id per voice");
    // Release clears the bookkeeping.
    g.release(&mut host, &ctx, cx, cy);
    assert!(host.voices_of(70).is_empty());
    // A freed widget releases whatever is still held.
    g.press(&mut host, &ctx, cx, cy);
    assert!(!host.voices_of(70).is_empty());
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        }),
        from(),
    );
    assert!(host.voices_of(70).is_empty());
}
