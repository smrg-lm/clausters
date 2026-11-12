//! A registered element: the press it claims, and the one it declines.

use super::*;

impl crate::Element for TestPad {
    fn set(&mut self, _key: &str, _v: &serde_json::Value) -> bool {
        false
    }

    fn draw(
        &self,
        _d: &mut clausters_gui_draw::draw::Draw,
        _ctx: &crate::host::widget::element::Ctx,
    ) {
    }

    fn value(&self) -> Option<OscType> {
        Some(OscType::Int(self.taken))
    }

    fn press(
        &mut self,
        _at: (f64, f64),
        _input: &crate::host::widget::element::Input,
    ) -> crate::Claim {
        if !self.claims {
            return crate::Claim::Decline;
        }
        self.taken += 1;
        crate::Claim::value(OscType::Int(self.taken))
    }

    fn drag(
        &mut self,
        at: (f64, f64),
        _input: &crate::host::widget::element::Input,
    ) -> crate::host::widget::element::Events {
        self.moved = at;
        self.steps += 1;
        crate::host::widget::element::Events::none()
    }

    fn release(
        &mut self,
        _at: (f64, f64),
        _inside: bool,
        _input: &crate::host::widget::element::Input,
    ) -> crate::host::widget::element::Events {
        crate::host::widget::element::Events::message(vec![
            OscType::String("pad".into()),
            OscType::Int(self.steps),
            OscType::Float(self.moved.1 as f32),
        ])
    }

    fn clone_box(&self) -> Box<dyn crate::Element> {
        Box::new(self.clone())
    }
}

fn pad_taken(host: &Host, id: i32) -> i32 {
    match host
        .window_def(1)
        .unwrap()
        .find(id)
        .unwrap()
        .kind
        .event_value()
    {
        Some(OscType::Int(n)) => n,
        other => panic!("not a pad: {other:?}"),
    }
}

/// A sample is a grabbable point. The whole route in one test -- press the
/// disc, drag it, and one intent leaves on release carrying both the value it
/// reached and the one it came from, so the owner can apply it and invert it
/// without remembering anything.
#[test]
fn a_dragged_sample_leaves_as_one_absolute_intent() {
    let def = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","navigable":1,
             "data":[0.0,0.5,-0.5,1.0,0.25,-0.25,0.75,0.0],"base_bucket":2,
             "gestures":{"drag":"sample"}}]}"#;
    let mut host = host_from(def);
    // Eight samples across the body: far enough apart that the trace marks
    // each one, which is the same question the grab asks.
    host.set_timeline_total(50, 8);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);

    g.press(&mut host, &ctx, 400.0, 100.0);
    // The grab shows at once: the hand sees the value it is holding before
    // anyone has applied anything.
    let held = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("the press takes hold of a sample")
        .clone();
    let effects = g.drag_to(&mut host, &ctx, 400.0, 220.0);
    let moved = host
        .widget_kind(1, 50)
        .and_then(|k| k.pending_edit())
        .expect("and keeps hold of it");
    assert_eq!(
        moved.start, held.start,
        "the drag moves the value, not which"
    );
    assert!(moved.values[0] < held.values[0], "dragging down lowers it");
    assert!(
        !has_emit_tag(&effects, 50, "sample"),
        "one gesture is one intent: nothing leaves along the way"
    );

    let effects = g.release(&mut host, &ctx, 400.0, 220.0);
    let args = emitted_args(&effects, 50).expect("the release reports");
    assert_eq!(
        args.len(),
        5,
        "tag, channel, frame, value, previous: {args:?}"
    );
    assert!(has_emit_tag(&effects, 50, "sample"));
    // The pending stays until the owner answers: dropping it here would snap
    // the picture back for the length of the round trip, which reads as a
    // refusal.
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_some(),
        "the edit is still in flight"
    );

    // And the acknowledgement is what lets go of it -- the rule *drop every pending
    // at or below the stamp*, with the drawing finally following the outbox.
    let seq = match &effects[..] {
        [.., crate::host::gestures::GestureEffect::Emit { seq, .. }] => *seq,
        _ => effects
            .iter()
            .find_map(|e| match e {
                crate::host::gestures::GestureEffect::Emit { seq, .. } => Some(*seq),
                _ => None,
            })
            .expect("the intent went out stamped"),
    };
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_ACK.into(),
            args: vec![
                clausters_core::osc::OscType::Int(seq),
                clausters_core::osc::OscType::Int(1),
            ],
        }),
        crate::host::ClientId::Udp(std::net::SocketAddr::from((
            std::net::Ipv4Addr::LOCALHOST,
            9000,
        ))),
    );
    assert!(
        host.widget_kind(1, 50)
            .and_then(|k| k.pending_edit())
            .is_none(),
        "the owner answered, so the hand lets go"
    );
}

/// Two of the smallest possible elements: one that takes every press and
/// reports how many it has taken, one that never takes any.
/// A registered element that exercises the **whole** gesture sequence: it takes
/// a press, follows the cursor while it is held, and reports the total on
/// release. What a real leaf makes of those positions is its own business; what
/// the machine promises is that all three arrive.
#[derive(Debug, Clone)]
struct TestPad {
    taken: i32,
    claims: bool,
    /// Where the cursor last was.
    moved: (f64, f64),
    /// How many `drag` steps arrived.
    steps: i32,
}

fn pad(
    props: &serde_json::Map<String, serde_json::Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn crate::Element>, String> {
    Ok(Box::new(TestPad {
        taken: 0,
        claims: props
            .get("claims")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        moved: (0.0, 0.0),
        steps: 0,
    }))
}

/// The default gesture table hands a leaf the press, so a registered element
/// gets one with no `gestures` prop and nothing else to configure -- and what
/// it claims leaves as the widget's value, on the same `/gui_event` path a
/// built-in control's does.
#[test]
fn a_registered_element_takes_the_press_and_its_value_leaves() {
    crate::register("test_pad", pad);
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"test_pad"}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    let effects = g.press(&mut host, &ctx, 300.0, 200.0);
    assert_eq!(pad_taken(&host, 5), 1, "the element saw the press");
    assert!(
        effects.iter().any(|e| matches!(
            e,
            GestureEffect::Emit { widget_id: 5, args, .. }
                if args.first() == Some(&OscType::Int(1))
        )),
        "and its claim left as a value: {effects:?}"
    );
    crate::unregister("test_pad");
}

/// The press is **held**: a claim opens a drag that carries no geometry, so
/// every motion and the release land on the element that took it. What it
/// reports on release travels as an edit-back payload (a tagged list), not as a
/// value -- the element says which by how many arguments it sends.
#[test]
fn a_claim_holds_the_press_through_the_drag_and_the_release() {
    crate::register("test_pad", pad);
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":5,"type":"test_pad"}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 300.0, 200.0);
    assert!(g.dragging(), "a taken press is held");
    g.drag_to(&mut host, &ctx, 300.0, 260.0);
    g.drag_to(&mut host, &ctx, 300.0, 290.0);
    let effects = g.release(&mut host, &ctx, 300.0, 290.0);
    assert!(!g.dragging(), "the release ends it");
    assert_eq!(
        emitted(&effects, 5),
        vec![vec![
            OscType::String("pad".into()),
            OscType::Int(2),
            OscType::Float(290.0)
        ]],
        "two drag steps, at the last cursor: {effects:?}"
    );
    crate::unregister("test_pad");
}

/// Declining is the other half of the contract: the press goes back to the
/// chain, which here is the plane under it -- so it pans, exactly as a press on
/// a lane's empty space or a patcher's bare canvas does.
#[test]
fn a_declined_press_falls_through_to_the_plane() {
    crate::register("test_pad", pad);
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":4,"type":"plane","children":[
                {"id":5,"type":"test_pad","claims":false,"x":0,"y":0,"w":400,"h":300}]}]}"#,
    );
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 600, 400);
    g.press(&mut host, &ctx, 100.0, 100.0);
    assert_eq!(pad_taken(&host, 5), 0, "it declined");
    g.drag_to(&mut host, &ctx, 140.0, 100.0);
    g.release(&mut host, &ctx, 140.0, 100.0);
    let panned = match &host.window_def(1).unwrap().find(4).unwrap().kind {
        WidgetKind::Scroll { view, .. } => view.view_x,
        other => panic!("not a plane: {other:?}"),
    };
    assert!(panned != 0.0, "the plane never got the press back");
    crate::unregister("test_pad");
}
