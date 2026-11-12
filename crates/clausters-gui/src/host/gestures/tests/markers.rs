//! A marker, named where it is put.

use super::*;

/// **A marker is named before its owner hears of it.** A Ctrl+click on the
/// ruler puts one down numbered, with its number selected in a name typed
/// where its label stands: the keys type over it, Enter gives it the name and
/// only then are the markers reported. Escape takes a new one away again,
/// unreported; and a double click on a marker opens its name to rename it.
#[test]
fn a_marker_is_named_where_it_is_put() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"layout":"col","children":[
            {"id":60,"type":"field","link":9,"h":20},
            {"id":70,"type":"multitrack","link":9,"snap":0,"h":200,
             "sample_rate":48000,
             "tracks":["one", "", 120, 0, 0, 1.0, 1],
             "clips":["a","one",0,1000,0,"",-1]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let mut ctrl = GestureCtx::new(1, 800, 400);
    ctrl.ctrl = true;
    let strip = placed_rect(&host, &ctx, 60);
    let at = (
        f64::from(strip.x + strip.w * 0.25),
        f64::from(strip.y + strip.h * 0.5),
    );
    let type_in = |g: &Gestures, host: &mut Host, text: &str| {
        text.chars()
            .flat_map(|c| key(g, host, &ctx, Key::Char(c)).unwrap())
            .collect::<Vec<_>>()
    };

    let mut effects = g.press(&mut host, &ctrl, at.0, at.1);
    effects.extend(g.release(&mut host, &ctrl, at.0, at.1));
    assert_eq!(
        markers_of(&host, 60),
        (vec!["1".to_string()], Some("1".to_string())),
        "put down numbered, its name open"
    );
    assert!(!has_emit_tag(&effects, 60, "markers"), "and not told yet");
    let typed = type_in(&g, &mut host, "/cue");
    assert!(!has_emit_tag(&typed, 60, "markers"));
    assert_eq!(
        markers_of(&host, 60).1.as_deref(),
        Some("/cue"),
        "typed over"
    );
    let given = key(&g, &mut host, &ctx, Key::Enter).unwrap();
    assert!(has_emit_tag(&given, 60, "markers"), "Enter tells the owner");
    assert_eq!(markers_of(&host, 60), (vec!["/cue".to_string()], None));

    // a second one, left: it goes, and nobody heard of it
    let away = (f64::from(strip.x + strip.w * 0.75), at.1);
    g.press(&mut host, &ctrl, away.0, away.1);
    g.release(&mut host, &ctrl, away.0, away.1);
    assert_eq!(markers_of(&host, 60).0.len(), 2);
    let left = key(&g, &mut host, &ctx, Key::Escape).unwrap();
    assert!(!has_emit_tag(&left, 60, "markers"));
    assert_eq!(markers_of(&host, 60), (vec!["/cue".to_string()], None));

    // a double click on the first opens its name, and Enter renames it
    for now in [1000.0, 1100.0] {
        let mut clock = GestureCtx::new(1, 800, 400);
        clock.now_ms = now;
        g.press(&mut host, &clock, at.0, at.1);
        g.release(&mut host, &clock, at.0, at.1);
    }
    assert_eq!(markers_of(&host, 60).1.as_deref(), Some("/cue"));
    type_in(&g, &mut host, "/go");
    // a press elsewhere gives the name, as a field loses its edit
    let effects = g.press(&mut host, &ctx, away.0, away.1);
    g.release(&mut host, &ctx, away.0, away.1);
    assert!(has_emit_tag(&effects, 60, "markers"));
    assert_eq!(markers_of(&host, 60), (vec!["/go".to_string()], None));
}
