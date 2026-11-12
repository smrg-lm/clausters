//! A score under the hand: a press selects and a drag transposes.

use super::*;

/// A one-score window, the page fitted 1:1 into the child rect: a window of
/// 1012x412 gives the child (6,6,1000,400), matching the 1000x400 viewBox.
#[cfg(feature = "notation")]
fn score_host() -> Host {
    // Editable, so the drag tests exercise the transpose gesture; the
    // read-only default is covered by its own test below.
    host_from(
        r#"{"type":"window","children":[
            {"id":80,"type":"score","vb":[1000,400],"editable":true,
             "glyphs":{"E0A4":"M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
             "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"},
                      {"k":"glyph","cp":"E0A4","xf":[500,200,0.72,-0.72],"id":"n1"}],
             "kinds":{"n1":"note","staff":"staff"}}]}"#,
    )
}

/// What the page says is selected **now** -- read the way a `/gui_query` reads
/// it, since a ported leaf answers for itself rather than showing its variant.
#[cfg(feature = "notation")]
fn score_selected(host: &Host) -> Option<String> {
    let info = host.window_def(1).unwrap().find(80).unwrap().kind.info();
    match info.iter().find(|(k, _)| k == "selected") {
        Some((_, serde_json::Value::String(s))) if !s.is_empty() => Some(s.clone()),
        Some(_) => None,
        None => panic!("not a score: {info:?}"),
    }
}

#[cfg(feature = "notation")]
fn element_emits(effects: &[GestureEffect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("element".into())) =>
            {
                match &args[1..] {
                    [OscType::String(s)] => Some(s.clone()),
                    _ => panic!("malformed element payload: {args:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

#[cfg(feature = "notation")]
fn transpose_emits(effects: &[GestureEffect]) -> Vec<(String, i32)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. }
                if args.first() == Some(&OscType::String("transpose".into())) =>
            {
                match &args[1..] {
                    [OscType::String(s), OscType::Int(n)] => Some((s.clone(), *n)),
                    _ => panic!("malformed transpose payload: {args:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn a_press_that_does_not_move_the_note_stays_a_selection() {
    let mut host = score_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    g.press(&mut host, &ctx, 556.0, 196.0);
    // wandering back and forth within one step is not an edit
    g.drag_to(&mut host, &ctx, 556.0, 240.0);
    let effects = g.release(&mut host, &ctx, 556.0, 240.0);
    assert!(transpose_emits(&effects).is_empty(), "no step, no edit");
    assert_eq!(score_selected(&host).as_deref(), Some("n1"));
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn a_press_on_the_score_selects_the_element_and_emits_its_id() {
    let mut host = score_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    // the notehead sits at page (500, 200) -> child rect origin (6, 6)
    let effects = g.press(&mut host, &ctx, 556.0, 196.0);
    assert_eq!(element_emits(&effects), vec!["n1".to_string()]);
    assert_eq!(score_selected(&host).as_deref(), Some("n1"));
    // Clicking the same element again selects nothing new, so the script hears
    // nothing. Each click is a press **and** a release: the press alone is held,
    // because it may become a drag, and the machine refuses a second press while
    // it is.
    g.release(&mut host, &ctx, 556.0, 196.0);
    let again = g.press(&mut host, &ctx, 556.0, 196.0);
    assert!(
        element_emits(&again).is_empty(),
        "a re-press re-reported the selection: {again:?}"
    );
    // blank paper clears it, reported as an empty id
    g.release(&mut host, &ctx, 556.0, 196.0);
    let cleared = g.press(&mut host, &ctx, 106.0, 386.0);
    assert_eq!(element_emits(&cleared), vec![String::new()]);
    assert_eq!(score_selected(&host), None);
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn dragging_a_note_up_the_staff_transposes_it_in_diatonic_steps() {
    let mut host = score_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    // grab the notehead at page (500, 200); the page is fitted 1:1, so a
    // diatonic step is the default 90 page units = 90 px
    g.press(&mut host, &ctx, 556.0, 196.0);
    // Two steps up. The displacement is drawn while the drag lasts and reports
    // nothing on the way -- so what the machine owes it is the frame that draws
    // it, and the edit only travels on release.
    let moving = g.drag_to(&mut host, &ctx, 556.0, 16.0);
    assert!(transpose_emits(&moving).is_empty(), "nothing until release");
    assert!(moving.iter().any(|e| matches!(e, GestureEffect::Redraw(1))));
    // the release asks the client for the edit, in steps
    let effects = g.release(&mut host, &ctx, 556.0, 16.0);
    assert_eq!(transpose_emits(&effects), vec![("n1".to_string(), 2)]);
}

#[cfg(feature = "notation")]
#[test]
#[cfg(feature = "notation")]
fn a_read_only_score_selects_but_a_drag_does_not_transpose() {
    // The same host without `editable`: a press still selects and reports
    // the element (inspecting a page is not editing it), but dragging it a
    // full step neither previews nor emits a transpose.
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":80,"type":"score","vb":[1000,400],
             "glyphs":{"E0A4":"M0 -39c0 68 73 172 200 172c66 0 114 -37 114 -95c0 -84 -106 -171 -218 -171c-58 0 -96 34 -96 93Z"},
             "prims":[{"k":"line","pts":[[0,200],[1000,200]],"w":13,"id":"staff"},
                      {"k":"glyph","cp":"E0A4","xf":[500,200,0.72,-0.72],"id":"n1"}],
             "kinds":{"n1":"note","staff":"staff"}}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 1012, 412);
    let picked = g.press(&mut host, &ctx, 556.0, 196.0);
    assert_eq!(element_emits(&picked), vec!["n1".to_string()]);
    assert_eq!(score_selected(&host).as_deref(), Some("n1"));
    // two full steps up: nothing is displaced, and nothing is asked for
    let moving = g.drag_to(&mut host, &ctx, 556.0, 16.0);
    assert!(
        transpose_emits(&moving).is_empty(),
        "a read-only page edited: {moving:?}"
    );
    let effects = g.release(&mut host, &ctx, 556.0, 16.0);
    assert!(transpose_emits(&effects).is_empty(), "read-only: no edit");
}
