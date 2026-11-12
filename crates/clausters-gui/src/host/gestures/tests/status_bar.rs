//! The status bar (`crate::host::status`): the band it takes, the press it
//! consumes, and the two things that write to it.
//!
//! It is here rather than beside the model because everything worth checking
//! about it is a *host* fact -- the area the tree is laid out in, an emitted
//! event, an acknowledgement -- and none of it is reachable from a `Status`
//! alone.

use super::*;
use crate::host::status::Kind;

/// A window with a bar and one widget in it. Deliberately not through
/// `host_from`, which turns the bar off for every other test in this file.
fn barred_host() -> Host {
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(1),
                OscType::String(
                    r#"{"type":"window","children":[
                            {"id":9,"type":"toggle","value":0}]}"#
                        .into(),
                ),
            ],
        }),
        from(),
    );
    host
}

#[test]
fn the_band_comes_off_the_bottom_and_the_tree_gets_the_rest() {
    let host = barred_host();
    let full = Rect::new(0.0, 0.0, 800.0, 400.0);
    let band = host
        .status_bar_rect(1, 800, 400)
        .expect("the window has one");
    let content = host.content_area(1, 800, 400);
    assert_eq!(band.h, host.metrics_for(1).status_h, "one line, closed");
    assert_eq!(band.y + band.h, full.h, "along the bottom edge");
    assert_eq!(content.h + band.h, full.h, "and the tree gets the rest");
    assert_eq!(content.w, full.w);
}

#[test]
fn a_window_that_declines_the_bar_keeps_every_pixel() {
    let mut host = barred_host();
    no_status_bar(&mut host, 1);
    assert_eq!(host.status_bar_rect(1, 800, 400), None);
    assert_eq!(
        host.content_area(1, 800, 400),
        Rect::new(0.0, 0.0, 800.0, 400.0)
    );
}

#[test]
fn a_press_on_the_band_opens_the_log_and_reaches_no_widget() {
    let mut host = barred_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let band = host.status_bar_rect(1, 800, 400).unwrap();
    let (px, py) = ((band.x + 4.0) as f64, (band.y + band.h * 0.5) as f64);

    let effects = g.press(&mut host, &ctx, px, py);
    assert!(host.status_open(1), "the click opened it");
    assert!(!g.dragging(), "and started no gesture");
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { .. })),
        "the band is chrome: nothing under it was pressed"
    );
    // Open, it is taller -- and the press that closes it is the one aimed
    // at where it now is.
    let open_band = host.status_bar_rect(1, 800, 400).unwrap();
    assert!(open_band.h > band.h, "the log area is several lines");
    g.press(
        &mut host,
        &ctx,
        px,
        (open_band.y + open_band.h * 0.5) as f64,
    );
    assert!(!host.status_open(1), "and the next click closed it");
}

#[test]
fn what_the_host_emits_is_what_the_bar_says() {
    let mut host = barred_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let toggle = placed_rect(&host, &ctx, 9);
    g.press(
        &mut host,
        &ctx,
        (toggle.x + 4.0) as f64,
        (toggle.y + toggle.h * 0.5) as f64,
    );
    let statuses = host.statuses();
    let line = statuses.get(&1).and_then(|s| s.last()).expect("a line");
    assert_eq!(line.kind, Kind::Did);
    assert_eq!(line.widget, Some(9));
    assert_eq!(
        line.text, "value 1",
        "the toggle's own value, said out loud"
    );
}

#[test]
fn an_owners_reason_lands_on_the_window_of_the_edit_it_answered() {
    let mut host = barred_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let toggle = placed_rect(&host, &ctx, 9);
    g.press(
        &mut host,
        &ctx,
        (toggle.x + 4.0) as f64,
        (toggle.y + toggle.h * 0.5) as f64,
    );
    let seq = host.outbox.borrow().pending().last().unwrap().seq;
    assert!(host.settle(crate::host::ack::Acked {
        seq,
        reason: Some("this toggle is the rendering, not the thing".into()),
        ..Default::default()
    }));
    let statuses = host.statuses();
    let line = statuses.get(&1).and_then(|s| s.last()).expect("a line");
    assert_eq!(line.kind, Kind::Refused);
    assert_eq!(
        line.text, "refused: this toggle is the rendering, not the thing",
        "the `/gui_ack` reason, which nothing used to read"
    );
}

/// **A verb nothing claimed says so**, on the bar and as the machine's own
/// note rather than as something the hand did -- and then goes on to the
/// window's owner, since a verb the host did not perform is the
/// application's to answer.
///
/// The defect this holds is the one reported twice on 2026-09-12: a verb
/// that refused correctly and a letter no element answers to were the same
/// silence at the window. The element's refusals were given words that day;
/// this is the other case.
#[test]
fn a_key_no_element_took_leaves_a_note_and_reaches_the_owner() {
    let mut host = barred_host();
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let toggle = placed_rect(&host, &ctx, 9);
    let pointer = ((toggle.x + 4.0) as f64, (toggle.y + toggle.h * 0.5) as f64);
    let effects = g
        .press_key(&mut host, &ctx, Key::Char('q'), Some(pointer))
        .expect("a bound key is consumed");
    assert_eq!(
        emitted_args(&effects, 1),
        Some(vec![OscType::String("quantize".into())]),
        "a toggle has no quantize, so the window's owner is told"
    );
    let statuses = host.statuses();
    let line = statuses
        .get(&1)
        .and_then(|s| s.lines().find(|l| l.kind == Kind::Note))
        .expect("a note");
    assert_eq!(line.widget, None, "a note is the window's");
    assert!(
        line.text.contains("did not take it"),
        "and it names what happened: {}",
        line.text
    );
}

/// **A bound value has a history too.** It leaves by the one road that
/// never passes through `emit`, which is why the bar used to go quiet for
/// the most direct control in the window.
#[test]
fn a_bound_widget_says_what_it_did() {
    let mut host = Host::new();
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(1),
                OscType::String(
                    r#"{"type":"window","children":[
                            {"id":9,"type":"toggle","value":0,
                             "bind":["server","/node_set",1000,"gate"]}]}"#
                        .into(),
                ),
            ],
        }),
        from(),
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 400);
    let toggle = placed_rect(&host, &ctx, 9);
    let effects = g.press(
        &mut host,
        &ctx,
        (toggle.x + 4.0) as f64,
        (toggle.y + toggle.h * 0.5) as f64,
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, GestureEffect::Emit { .. })),
        "bound: nothing was reported to a script"
    );
    let statuses = host.statuses();
    let line = statuses.get(&1).and_then(|s| s.last()).expect("a line");
    assert_eq!(line.kind, Kind::Did);
    assert_eq!(line.widget, Some(9));
    assert_eq!(line.text, "value 1 (bound)");
}
