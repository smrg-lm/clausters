//! The keyboard: focus, the ring and the focused element.

use super::*;

/// Two fields side by side, which is the smallest tab ring there is.
fn two_field_host() -> Host {
    host_from(
        r#"{"type":"window","margin":0,"flow":"row","children":[
            {"id":5,"type":"text"},{"id":6,"type":"text"}]}"#,
    )
}

/// **A press on the axis' own ruler leaves the focus alone.** The position
/// cursor is placed on the ruler and nowhere else, so dropping the focus there
/// would take the view's keys away with every mark a reader puts down -- point
/// at a box, place the cursor, split. A ruler takes no focus of its own (it is
/// chrome, not a sink) and it takes none away from what it rules.
#[test]
fn the_ruler_of_the_focused_view_does_not_take_its_focus_away() {
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
    // The stack takes the focus, as any element does.
    let rect = placed_rect(&host, &ctx, 70);
    let body = {
        let h = interact::hit(
            &host,
            1,
            800,
            400,
            (rect.x + rect.w * 0.5) as f64,
            (rect.y + rect.h * 0.5) as f64,
            &|_, _| 1,
        )
        .unwrap();
        interact::time_of(&h.chain).unwrap().1.body
    };
    let on_clip = body.x as f64 + 4.0;
    let midy = (rect.y + rect.h * 0.5) as f64;
    g.press(&mut host, &ctx, on_clip, midy);
    g.release(&mut host, &ctx, on_clip, midy);
    assert_eq!(host.focused(), Some((1, 70)), "the stack has the keys");

    // A press on the ruler above it places the cursor and keeps them.
    let strip = placed_rect(&host, &ctx, 60);
    let on_ruler = (
        f64::from(strip.x + strip.w * 0.5),
        f64::from(strip.y + strip.h * 0.5),
    );
    g.press(&mut host, &ctx, on_ruler.0, on_ruler.1);
    let effects = g.release(&mut host, &ctx, on_ruler.0, on_ruler.1);
    assert!(has_emit_tag(&effects, 60, "locate"), "the mark was placed");
    assert_eq!(host.focused(), Some((1, 70)), "and the keys stayed");

    // **On a whole sample**, wherever between two the pointer landed: the
    // playhead that starts from the mark stands on one.
    for dx in [0.13, 0.37, 0.71] {
        g.press(&mut host, &ctx, on_ruler.0 + dx, on_ruler.1);
        g.release(&mut host, &ctx, on_ruler.0 + dx, on_ruler.1);
        let key = host.timeline_key(60).expect("the ruler is on a group");
        let cursor = host.timelines().state(key).expect("a group").cursor;
        assert!(
            cursor >= 0.0 && cursor.fract() == 0.0,
            "a whole sample: {cursor}"
        );
    }
}

fn text_value(host: &Host, id: i32) -> String {
    match host
        .window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .event_value()
    {
        Some(OscType::String(s)) => s,
        other => panic!("not a text field: {other:?}"),
    }
}

/// The string of the single `Emit` in `effects`, if any.
fn emitted_string(effects: &[GestureEffect]) -> Option<String> {
    effects.iter().find_map(|e| match e {
        GestureEffect::Emit { args, .. } => match args.first() {
            Some(OscType::String(s)) if s != "focus" => Some(s.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// **A press on the axis' own ruler leaves the focus alone.** The position
/// cursor is placed on the ruler and nowhere else, so dropping the focus there
/// would take the multitrack's keys away with every mark a reader puts down -- point
/// at a box, place the cursor, split. A ruler takes no focus of its own (it is
/// **`A` in a track's header, through the whole machine** *(found 2026-09-12 by
/// the user: "el boton A no funciona")*.
///
/// The element's own test presses it at the indent the *element* asks for. What
/// a window actually hands it is the **navigation group's** gutter, and that is
/// the number the drawing and the hit test both have to be laid out at -- so a
/// fourth cell that fits one and not the other is a button drawn where nothing
/// answers. This presses the pixels a person presses.
#[test]
fn the_headers_automation_toggle_answers_where_it_is_drawn() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"layout":"col","children":[
            {"id":70,"type":"multitrack","link":9,"snap":0,"h":300,
             "sample_rate":48000,
             "tracks":["10", "bass", 120, 0, 0, 1.0, 0],
             "meters":["10", 0, 2, 2],
             "clips":["a","10",0,1000,0,"",-1]}]}"#,
    );
    host.sync_track_totals();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 900, 400);
    let rect = placed_rect(&host, &ctx, 70);
    let indent = interact::hit(
        &host,
        1,
        900,
        400,
        f64::from(rect.x) + 5.0,
        f64::from(rect.y) + 5.0,
        &|_, _| 1,
    )
    .map(|h| h.indent)
    .expect("the header band is the widget's");
    let band = crate::host::timeline::gutter_band(Rect::new(rect.x, rect.y, rect.w, 120.0), indent);
    let header = crate::host::graphics::track::Header {
        w: (indent > 0.0).then_some(indent),
        mute: Some(false),
        solo: Some(false),
        level: Some(1.0),
        live_level: false,
        curves: Some(false),
        meters: vec![(0.0, 0.0); 2],
    };
    let parts = crate::host::graphics::track::header_parts(band, &header, host.metrics_for(1));
    let cell = parts.curves.expect("the cell the `A` is drawn in");
    let at = (
        f64::from(cell.x + cell.w / 2.0),
        f64::from(cell.y + cell.h / 2.0),
    );
    let effects = g.press(&mut host, &ctx, at.0, at.1);
    let args = effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. } => Some(args.clone()),
            _ => None,
        })
        .find(|args| args.first() == Some(&OscType::String("tracks".into())))
        .expect("the press reports the lanes");
    assert_eq!(args[0], OscType::String("tracks".into()));
    assert_eq!(
        args[7],
        OscType::Int(1),
        "and the row now asks for its automation: {args:?}"
    );
}

/// **Two clicks select a word and three the line**, counted by the machine
/// from the front's clock and handed to the field -- what Ctrl+C then copies
/// says which.
#[test]
fn a_double_click_in_a_field_selects_a_word_and_a_triple_the_line() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 30.0, 15.0);
    g.release(&mut host, &ctx, 30.0, 15.0);
    for ch in "hello world".chars() {
        key(&g, &mut host, &ctx, Key::Char(ch));
    }
    let copied = |g: &mut Gestures, host: &mut Host, ctx: &mut GestureCtx| {
        ctx.ctrl = true;
        g.press_key(host, ctx, Key::Char('c'), None);
        ctx.ctrl = false;
        host.clipboard.text()
    };
    for (t, _) in [(1_000.0, 1), (1_150.0, 2)] {
        ctx.now_ms = t;
        g.press(&mut host, &ctx, 30.0, 15.0);
        g.release(&mut host, &ctx, 30.0, 15.0);
    }
    let word = copied(&mut g, &mut host, &mut ctx);
    assert!(
        word == "hello" || word == "world",
        "a double click took a word: {word:?}"
    );
    ctx.now_ms = 1_300.0;
    g.press(&mut host, &ctx, 30.0, 15.0);
    g.release(&mut host, &ctx, 30.0, 15.0);
    assert_eq!(
        copied(&mut g, &mut host, &mut ctx),
        "hello world",
        "a triple, the line"
    );
}

/// **A drag that reports nothing still repaints.** Extending a text selection
/// changes the picture and not the value, so there is nothing to deliver -- and
/// a window with no other frame source would have shown the old selection until
/// something else moved.
#[test]
fn dragging_a_selection_repaints_even_though_it_reports_nothing() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 10.0, 15.0);
    for ch in "hello".chars() {
        g.key(&mut host, &ctx, Key::Char(ch));
    }
    let out = g.drag_to(&mut host, &ctx, 200.0, 15.0);
    assert!(
        emitted_string(&out).is_none(),
        "a selection is not a value: {out:?}"
    );
    assert!(
        out.iter().any(|e| matches!(e, GestureEffect::Redraw(1))),
        "and it still asks for the frame that draws it: {out:?}"
    );
}

#[test]
fn a_press_elsewhere_moves_the_focus_and_reports_both_ends() {
    let mut host = two_field_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 30.0, 30.0);
    // A click is a press and a release; a bare second press is not one.
    g.release(&mut host, &ctx, 30.0, 30.0);
    assert_eq!(host.focused(), Some((1, 5)));
    let e = g.press(&mut host, &ctx, 330.0, 30.0);
    assert_eq!(host.focused(), Some((1, 6)));
    assert_eq!(
        focus_events(&e),
        vec![(5, false), (6, true)],
        "the one that lost it is reported too, so a script can drop its caret"
    );
}

/// **What an element was in the middle of is settled as the focus leaves
/// it.** A text of an engraved page typed over where it is drawn is written
/// on Enter -- and when the hand goes to a tool instead, which is how most
/// edits end and a press the page never sees. The machine tells the element
/// (`Element::blur`) and reports what it answers, under the page's id.
#[test]
#[cfg(feature = "notation")]
fn a_text_typed_over_on_a_page_is_written_when_a_press_lands_elsewhere() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"score","h":300,"editable":true,"vb":[1000,500],"step":90,
             "glyphs":{},"kinds":{"t-title":"rend"},
             "prims":[{"k":"text","s":"A title","x":500,"y":300,"size":120,
                       "anchor":"middle","id":"t-title"}]},
            {"id":7,"type":"label","text":"a tool"}]}"#,
    );
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    // the page fits 1000x500 into 600x300: the title is about its middle
    let title = (300.0, 165.0);
    ctx.now_ms = 1_000.0;
    g.press(&mut host, &ctx, title.0, title.1);
    g.release(&mut host, &ctx, title.0, title.1);
    ctx.now_ms = 1_150.0;
    g.press(&mut host, &ctx, title.0, title.1);
    g.release(&mut host, &ctx, title.0, title.1);
    assert_eq!(host.focused(), Some((1, 5)), "the page takes the keyboard");
    assert!(key(&g, &mut host, &ctx, Key::Char('Z')).is_some());

    // a press on the tool under the page: the focus goes, and the text is written
    ctx.now_ms = 3_000.0;
    let out = g.press(&mut host, &ctx, 30.0, 350.0);
    assert_eq!(host.focused(), None);
    let written: Vec<&Vec<OscType>> = out
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id: 5, args, ..
            } => Some(args),
            _ => None,
        })
        .collect();
    assert_eq!(
        written,
        vec![&vec![
            OscType::String("text".into()),
            OscType::String("t-title".into()),
            OscType::String("Z".into()),
        ]],
        "and a page says nothing of the focus itself: {out:?}"
    );
}

/// **Past the last stop the focus leaves the tree**, and the front is told: in a
/// page that is what blurs the canvas, so a mounted GuiDef is an entrance and an
/// exit rather than a keyboard trap.
#[test]
fn tab_past_the_last_stop_hands_the_keyboard_back() {
    let mut host = two_field_host();
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    for _ in 0..2 {
        key(&g, &mut host, &ctx, Key::Tab);
    }
    assert_eq!(host.focused(), Some((1, 6)), "the last stop");
    let e = key(&g, &mut host, &ctx, Key::Tab).expect("Tab is always the ring's");
    assert_eq!(host.focused(), None);
    assert_eq!(focus_events(&e), vec![(6, false)]);
    assert!(
        e.iter().any(|f| matches!(f, GestureEffect::FocusOut(1))),
        "the front is told the focus left: {e:?}"
    );
}

/// **A frequency axis does not zoom past its own analysis.** Below a few FFT
/// bins across the whole body the curve is the interpolation between two
/// neighbours -- a flat line that no longer answers to the signal -- so the
/// zoom stops at a floor derived from `fft_size` and the sample rate rather
/// than at a constant fraction of the display axis.
#[test]
fn the_frequency_zoom_stops_at_the_analysis_resolution() {
    let mut host = spectrum_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.sample_rate = 48_000.0;
    for _ in 0..40 {
        g.wheel(&mut host, &ctx, 400.0, 150.0, 2.0);
    }
    let (start, len) = x_window(&host, 80);
    // The window in bins, through the geometry the curve is drawn with.
    let (nyquist, f_lo) = crate::host::graphics::signal::spectrum::axis_geometry(
        48_000.0,
        crate::spectrogram::FreqScale::Log,
        440.0,
    );
    let bins = |d: f64| {
        crate::host::ruler::display_to_hz(d, nyquist, crate::spectrogram::FreqScale::Log, f_lo)
            * 2048.0
            / 48_000.0
    };
    let span = bins(start + len) - bins(start);
    assert!(
        (3.5..=4.5).contains(&span),
        "the window bottomed out at {span:.2} bins, not at the analysis floor"
    );
    // Far short of what the display axis alone would have allowed.
    assert!(
        len > crate::viewport::MIN_SPAN * 5.0,
        "the floor is still the display constant ({len})"
    );
}

/// The `(widget, gained)` pairs the `"focus"` events in `effects` report.
fn focus_events(effects: &[GestureEffect]) -> Vec<(i32, bool)> {
    effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit {
                widget_id, args, ..
            } => match args.as_slice() {
                [OscType::String(tag), OscType::Int(on)] if tag == "focus" => {
                    Some((*widget_id, *on == 1))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// A window with one editable `text` field (id 5) filling it. It is
/// **natural-sized**, so it is a control-high strip at the top of the window
/// rather than the whole pane -- every press below aims inside that strip.
fn text_host() -> Host {
    host_from(r#"{"type":"window","margin":0,"children":[{"id":5,"type":"text"}]}"#)
}

/// Tab walks the ring in **layout order**, and Shift+Tab back along it.
#[test]
fn tab_walks_the_ring_in_layout_order() {
    let mut host = two_field_host();
    let g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    key(&g, &mut host, &ctx, Key::Tab);
    assert_eq!(
        host.focused(),
        Some((1, 5)),
        "the ring is entered at its first stop"
    );
    key(&g, &mut host, &ctx, Key::Tab);
    assert_eq!(host.focused(), Some((1, 6)));
    ctx.shift = true;
    key(&g, &mut host, &ctx, Key::Tab);
    assert_eq!(host.focused(), Some((1, 5)));
}

#[test]
fn a_press_focuses_the_field_and_typing_emits_on_every_keystroke() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // A press focuses the field and says so -- but does not edit it, so there
    // is no value event: a click is not an edit.
    let e = g.press(&mut host, &ctx, 30.0, 15.0);
    assert_eq!(host.focused(), Some((1, 5)));
    assert_eq!(focus_events(&e), vec![(5, true)]);
    assert!(emitted_string(&e).is_none());
    // Each character is delivered as the field's whole string, ungated.
    for (ch, expect) in [('h', "h"), ('i', "hi")] {
        let e =
            key(&g, &mut host, &ctx, Key::Char(ch)).expect("the focused field consumes the key");
        assert_eq!(emitted_string(&e).as_deref(), Some(expect));
        assert_eq!(text_value(&host, 5), expect);
    }
    // Backspace edits and re-emits.
    let e = key(&g, &mut host, &ctx, Key::Backspace).unwrap();
    assert_eq!(emitted_string(&e).as_deref(), Some("h"));
}

#[test]
fn keys_are_ignored_when_nothing_is_focused() {
    let mut host = text_host();
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    // Nothing focused: the focus declines the key (the key table reads it
    // instead).
    assert!(key(&g, &mut host, &ctx, Key::Char('x')).is_none());
    assert_eq!(text_value(&host, 5), "");
}

/// A script may point the keyboard itself -- and is refused when it points at
/// something that reads none, rather than being left waiting for keystrokes
/// that cannot arrive.
#[test]
fn a_script_can_set_the_focus_and_a_widget_that_reads_no_keys_refuses_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"text"},{"id":7,"type":"label","text":"x"}]}"#,
    );
    let mut effects = Vec::new();
    assert!(host.set_props(
        5,
        vec![("focus".into(), serde_json::json!(1))],
        &mut effects
    ));
    assert_eq!(host.focused(), Some((1, 5)));
    // ...and the key is not a prop: a query answers what the widget is, and
    // `focus` is not part of that.
    assert!(
        !host
            .registry()
            .get(5)
            .is_some_and(|w| w.props.contains_key("focus")),
        "focus was written into the document"
    );
    host.set_props(
        7,
        vec![("focus".into(), serde_json::json!(1))],
        &mut effects,
    );
    assert_eq!(host.focused(), Some((1, 5)), "the label refused it");
    // `focus 0` gives it up.
    host.set_props(
        5,
        vec![("focus".into(), serde_json::json!(0))],
        &mut effects,
    );
    assert_eq!(host.focused(), None);
}

/// A press on something that reads no keyboard drops the focus -- which is how
/// a caret disappears when you click away from a field.
#[test]
fn a_press_on_a_widget_that_takes_no_focus_clears_it() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"flow":"col","children":[
            {"id":5,"type":"text"},{"id":7,"type":"label","text":"x"}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 30.0, 10.0);
    // A click is a press and a release; a bare second press is not one.
    g.release(&mut host, &ctx, 30.0, 10.0);
    assert_eq!(host.focused(), Some((1, 5)));
    let e = g.press(&mut host, &ctx, 30.0, 60.0);
    assert_eq!(host.focused(), None);
    assert_eq!(focus_events(&e), vec![(5, false)]);
}

/// A window with nothing focusable hands Tab straight back, rather than
/// swallowing it -- the same exit, reached without ever entering.
#[test]
fn tab_in_a_window_with_no_ring_leaves_at_once() {
    // A label reads no keys; every control does, so a window holding one
    // control has a ring.
    let mut host =
        host_from(r#"{"type":"window","children":[{"id":9,"type":"label","text":"x"}]}"#);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let e = key(&g, &mut host, &ctx, Key::Tab).unwrap();
    assert_eq!(host.focused(), None);
    assert!(e.iter().any(|f| matches!(f, GestureEffect::FocusOut(1))));
}

#[test]
fn enter_inserts_a_newline_only_in_a_multiline_field() {
    let ctx = GestureCtx::new(1, 600, 400);
    // Single-line: Enter is inert (no send-on-Enter).
    let mut host = text_host();
    let mut g = Gestures::default();
    g.press(&mut host, &ctx, 30.0, 15.0);
    let e = key(&g, &mut host, &ctx, Key::Enter).unwrap();
    assert!(emitted_string(&e).is_none());
    assert_eq!(text_value(&host, 5), "");
    // Multiline: Enter inserts a newline and emits.
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[{"id":5,"type":"text","multiline":true}]}"#,
    );
    let mut g = Gestures::default();
    g.press(&mut host, &ctx, 30.0, 30.0);
    // A click is a press and a release; a bare second press is not one.
    g.release(&mut host, &ctx, 30.0, 30.0);
    key(&g, &mut host, &ctx, Key::Char('a'));
    let e = key(&g, &mut host, &ctx, Key::Enter).unwrap();
    assert_eq!(emitted_string(&e).as_deref(), Some("a\n"));
}

#[test]
fn cut_and_paste_move_text_through_the_clipboard() {
    let mut host = text_host();
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 30.0, 15.0);
    for ch in "abc".chars() {
        g.key(&mut host, &ctx, Key::Char(ch));
    }
    // Select all, then cut to the clipboard.
    ctx.ctrl = true;
    g.key(&mut host, &ctx, Key::Char('a'));
    g.key(&mut host, &ctx, Key::Char('x'));
    assert_eq!(host.clipboard.text(), "abc");
    assert_eq!(text_value(&host, 5), "");
    // Paste it back twice.
    g.key(&mut host, &ctx, Key::Char('v'));
    g.key(&mut host, &ctx, Key::Char('v'));
    assert_eq!(text_value(&host, 5), "abcabc");
}
