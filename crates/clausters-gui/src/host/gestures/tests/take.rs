//! A take under the hand: the sweep, the marquee, copy and paste, and its two axes under the wheel.

use super::*;

/// **A selection an hour into a take names the frames it holds.** A float
/// keeps a whole frame only up to 2^24 of them -- under six minutes at 48 kHz
/// -- so the two numbers leave as doubles, as the `"locate"` beside them does.
#[test]
fn a_selection_far_into_a_take_is_reported_to_the_frame() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    // An hour at 48 kHz, and an odd frame in it: a float lands on a multiple
    // of sixteen there.
    host.set_timeline_total(50, 200_000_000);
    let (start, len) = (172_800_001.0, 4_801.0);
    let mut effects = Vec::new();
    // From its first sample to its last: a sweep holds the samples it passed
    // over.
    let last = start + len - 1.0;
    crate::host::gestures::nav::set_selection(&mut host, &mut effects, 1, 50, start, last, None);
    let args = emitted_args(&effects, 50).expect("a selection reports");
    assert_eq!(
        args,
        [
            OscType::String("selection".into()),
            OscType::Long(start as i64),
            OscType::Long(len as i64),
        ]
    );
}

#[test]
fn waveform_press_and_drag_select_a_range() {
    let mut host = host_from(
        &serde_json::json!({"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":vec![0.1; 1000],"base_bucket":2}]})
        .to_string(),
    );
    host.set_timeline_total(50, 1000);
    host.set_timeline_cursor(50, 100.0);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    // Press inside the view body (right of the y-ruler strip), then drag.
    let effects = g.press(&mut host, &ctx, 400.0, 150.0);
    assert!(has_emit_tag(&effects, 50, "selection"));
    let effects = g.drag_to(&mut host, &ctx, 600.0, 150.0);
    assert!(has_emit_tag(&effects, 50, "selection"));
    // The selection landed in the widget's navigation group -- where every
    // reader of it looks -- with a positive length.
    let key = host.timeline_key(50).unwrap();
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
    // Letting go keeps the cursor placed by the previous click. Copy keeps
    // this span; a later paste lands at that cursor, not a selection edge.
    let effects = g.release(&mut host, &ctx, 600.0, 150.0);
    assert!(!has_emit_tag(&effects, 50, "locate"));
    assert_eq!(
        emitted_args(&effects, 50).unwrap().last(),
        Some(&OscType::Bool(true))
    );
    let state = host.timelines().state(key).unwrap();
    assert_eq!(state.cursor(), Some(100.0));
    let selection = state.selection();
    let mut clip = crate::host::clipboard::Clip::default();
    g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, 600.0, 150.0, &mut clip)
        .unwrap();
    assert_eq!(host.timelines().state(key).unwrap().selection(), selection);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Paste, 600.0, 150.0, &mut clip)
        .unwrap();
    assert_eq!(
        emitted_args(&effects, 50).unwrap()[1],
        OscType::Double(100.0)
    );
}

/// A view whose vertical measures **frequency** has a second axis and it is not
/// a value: its selection is a band of bins, which is its own field in the
/// document's selection and a later milestone's gesture. So a marquee over a
/// spectrogram stays the one-axis sweep it was, rather than reporting hertz in
/// a range whose reader would take them for amplitudes.
#[test]
fn a_spectral_view_reports_no_value_range() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":51,"type":"signal","view":"spectrogram","data":[0.0,0.5,-0.5,1.0],
             "fft_size":4}]}"#,
    );
    host.set_timeline_total(51, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    let args = emitted_args(&effects, 51).expect("the sweep reports");
    assert_eq!(
        args.len(),
        3,
        "no value range on a frequency axis: {args:?}"
    );
}

/// **A sweep to the first sample leaves the pointer off the view, and the copy
/// is still the selection's.** Dragging to the very start or end of the contents
/// parks the pointer in the window's margin -- or outside the window, where there
/// is no pointer at all -- and a copy addressed only to what is under it answered
/// nothing, silently, over a selection plainly on screen. The window's most
/// recent selection is the fallback addressee, and the last one made wins.
#[test]
fn a_block_operation_falls_back_to_the_window_s_last_selection() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":54,"type":"signal","view":"trace",
             "data":[0.0,0.1,0.2,0.3,0.4,0.5,0.6,0.7],"base_bucket":2},
            {"id":55,"type":"signal","view":"trace",
             "data":[1.0,0.9,0.8,0.7,0.6,0.5,0.4,0.3],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(54, 8);
    host.set_timeline_total(55, 8);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();

    // Both views hold one, the second made last: that is the one addressed.
    host.select_timeline(54, 0.0, 4.0);
    host.select_timeline(55, 2.0, 5.0);
    // Off the window entirely -- what `CursorLeft` leaves behind.
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, -1.0, -1.0, &mut clip)
        .expect("the selection answers where the pointer does not");
    assert!(emitted_args(&effects, 55).is_none(), "{effects:?}");
    assert_eq!(&clip.blobs()[0][..], &[0.8, 0.7, 0.6, 0.5]);

    // The pointer still wins wherever it is over a view: it lands on the first.
    let mut clip = crate::host::clipboard::Clip::default();
    let rect = placed_rect(&host, &ctx, 54);
    let (cx, cy) = (
        (rect.x + rect.w / 2.0) as f64,
        (rect.y + rect.h / 2.0) as f64,
    );
    g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, cx, cy, &mut clip)
        .expect("the view under the pointer answers");
    assert_eq!(&clip.blobs()[0][..], &[0.0, 0.1, 0.2, 0.3, 0.4]);

    // A selection cleared gives the title up: nothing is addressed by accident.
    host.set_timeline_selection(55, None, Some(0.0));
    host.set_timeline_selection(54, None, Some(0.0));
    let mut clip = crate::host::clipboard::Clip::default();
    assert!(
        g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, -1.0, -1.0, &mut clip)
            .is_none()
    );
}

/// The amplitude axis zooms symmetrically: whatever lane the cursor is
/// over, the window keeps its centre -- so every channel's zero line stays
/// at its lane's centre instead of sliding out of the lane. The regression
/// this fixes: the anchor used to be the cursor's height within its lane,
/// which is meaningless for the *other* lanes of one shared window -- a
/// wheel near the top of channel 2 pushed every channel's wave to the
/// bottom of its lane, clipped.
#[test]
fn the_amplitude_axis_zooms_about_its_centre_whatever_lane_is_under_the_cursor() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":61,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(61, 1000);
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    // Four channels: the body splits into four lanes.
    ctx.slot_channels.insert(61, 4);
    // Wheel over the y-ruler strip (left of the body), high inside the
    // *last* lane -- the worst case for a cursor-derived anchor.
    let effects = g.wheel(&mut host, &ctx, 10.0, 212.0, 4.0);
    assert!(has_emit_tag(&effects, 61, "view_y"));
    let (start, len) = host
        .window_def(1)
        .unwrap()
        .find(61)
        .unwrap()
        .kind
        .editor()
        .unwrap()
        .y_view();
    assert!(len < 1.0, "wheel-in shrinks the amplitude window");
    // Zero (display 0.5) sits at the centre of the window, so it lands at
    // the centre of every lane.
    assert!(
        (start + len / 2.0 - 0.5).abs() < 1e-9,
        "the window stays centred on zero: got ({start}, {len})"
    );
}

/// **`select_box` declines where the picture has one measured axis**, so a plan
/// may name both steps and get a rectangle where there is one to draw and the
/// plain span where there is not -- the mixed stack an editor actually has.
#[test]
fn the_box_step_falls_through_to_the_plain_sweep() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":51,"type":"signal","view":"spectrogram","data":[0.0,0.5,-0.5,1.0],
             "fft_size":4,"gestures":{"drag":"select_box select"}}]}"#,
    );
    host.set_timeline_total(51, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    let args = emitted_args(&effects, 51).expect("the sweep reports");
    assert_eq!(
        args.len(),
        3,
        "the box step declined, the span ran: {args:?}"
    );
    let key = host.timeline_key(51).unwrap();
    assert!(host.timelines().state(key).unwrap().sel_len > 0.0);
}

/// **A plain drag over a waveform is the time span it has always been**, and
/// the value band is a step a plan asks for.
///
/// The default is not a matter of taste: a drag on a waveform means *this
/// stretch of time* in every editor there has ever been, and a marquee that
/// also cut a band of amplitudes out of it would answer a question nobody
/// asked. What a band is good for is the script's business, so the script names
/// the step -- which is the D track's own rule about modes.
#[test]
fn a_marquee_restricts_in_value_only_where_the_plan_asks_for_it() {
    let plain = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#;
    let mut host = host_from(plain);
    host.set_timeline_total(50, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    // The default plan, swept with height: the span, and nothing else.
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    let args = emitted_args(&effects, 50).expect("the sweep reports");
    assert_eq!(args.len(), 3, "a plain drag is a time span: {args:?}");
    let editor = host.widget_kind(1, 50).unwrap().editor().unwrap();
    assert_eq!(editor.value_range(), None);

    // The same sweep where the plan asked for the rectangle.
    let boxed = r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2,
             "gestures":{"drag":"select_box"}}]}"#;
    let mut host = host_from(boxed);
    host.set_timeline_total(50, 1000);
    let mut g = Gestures::default();
    g.press(&mut host, &ctx, 400.0, 80.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 200.0);
    assert!(has_emit_tag(&effects, 50, "selection"));
    // The payload grew by the two numbers, and only when there are two.
    let args = emitted_args(&effects, 50).expect("the sweep reports");
    assert_eq!(args.len(), 5, "start, len and the range: {args:?}");
    let editor = host.widget_kind(1, 50).unwrap().editor().unwrap();
    let (min, max) = editor.value_range().expect("a rectangle restricts");
    // The default domain is full-scale amplitude, and the sweep ran from above
    // the centre line to below it: a band inside [-1, 1], the higher edge
    // coming from the *upper* pixel.
    assert!(min > -1.0 && max < 1.0 && max > min, "{min}..{max}");
    // A sweep along one height, under the same plan, clears it: a new selection
    // replaces the old one whole rather than keeping a finished gesture's band.
    g.release(&mut host, &ctx, 600.0, 200.0);
    g.press(&mut host, &ctx, 400.0, 150.0);
    let effects = g.drag_to(&mut host, &ctx, 600.0, 150.0);
    let args = emitted_args(&effects, 50).expect("the second sweep reports");
    assert_eq!(args.len(), 3, "one axis is the two numbers it always was");
    let editor = host.widget_kind(1, 50).unwrap().editor().unwrap();
    assert_eq!(editor.value_range(), None, "the old range does not linger");
}

/// **A mapped take is readable, and a slot does not take that away.** The
/// navigable views are the ones the clipboard was written for, and they are also
/// the ones whose data a loader routes into a GPU slot -- so for a while a copy
/// over the very source it was meant for refused, the element holding nothing
/// while the picture on screen was drawn from the samples. The element keeps the
/// pyramid the slot draws (`frame::keep_data`), and the copy reads it.
#[test]
fn a_take_routed_into_a_slot_is_still_the_elements_samples() {
    use crate::host::widget::element::Loaded;
    use crate::waveform::WaveformData;
    use std::sync::Arc;

    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":53,"type":"signal","view":"trace","navigable":1,
             "path":"take.f32","bulk":true,"base_bucket":2}]}"#,
    );
    // What a loader resolved: the mapped file's samples, summarized. It fills
    // the slot, and the element keeps the same pyramid.
    let peaks = Arc::new(WaveformData::from_interleaved(
        &[0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7],
        1,
        2,
    ));
    let widget = host
        .window_def_mut(1)
        .and_then(|t| t.find_mut(53))
        .expect("the view is in the tree");
    crate::host::frame::keep_data(widget, &Loaded::Peaks(peaks));
    host.set_timeline_total(53, 8);

    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    host.select_timeline(53, 2.0, 5.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("a view under the cursor answers");
    assert!(
        emitted_args(&effects, 53).is_none(),
        "the copy was made, so nothing was refused: {effects:?}"
    );
    assert_eq!(&clip.blobs()[0][..], &[0.2, 0.3, 0.4, 0.5]);
}

/// **The three verbs split where the host's authority does.** A copy is a read
/// and the host may do it: the selected span leaves the element's own contents
/// and lands on the clipboard, typed and with the rate it was taken at. A cut
/// and a paste change data the host does not own, so they leave as intents.
#[test]
fn copy_reads_the_samples_and_cut_and_paste_leave_as_intents() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":50,"type":"signal","view":"trace",
             "data":[0.0,0.1,0.2,0.3,0.4,0.5,0.6,0.7],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(50, 8);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    // Nothing selected: there is nothing to copy, and nothing is claimed.
    assert!(
        g.clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
            .is_none()
    );
    // Select samples 2..=5, then copy: the block is the contents itself.
    host.select_timeline(50, 2.0, 5.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("a view under the cursor answers");
    assert!(
        emitted_args(&effects, 50).is_none(),
        "a copy changed nothing, so it reports nothing: {effects:?}"
    );
    assert!(clip.is_whole());
    assert_eq!(clip.doc().unwrap().kind(), "samples");
    assert_eq!(&clip.blobs()[0][..], &[0.2, 0.3, 0.4, 0.5]);

    // Cut: a copy and a removal. The copy is the host's, so the block is on
    // the clipboard first; the removal is the owner's, so what leaves is the
    // request.
    let mut cut = crate::host::clipboard::Clip::default();
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Cut, 400.0, 150.0, &mut cut)
        .expect("answered");
    assert_eq!(&cut.blobs()[0][..], &[0.2, 0.3, 0.4, 0.5], "a cut copies");
    let args = emitted_args(&effects, 50).expect("a cut reports");
    assert_eq!(args[0], OscType::String("cut".into()));
    assert_eq!(args.len(), 3, "the span it names: {args:?}");

    // Paste: the position, the kind, the document, and the payload beside it --
    // the clipboard travels *with* the intent, because it is the host's and the
    // owner may never have seen what is on it.
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Paste, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 50).expect("a paste reports");
    assert_eq!(args[0], OscType::String("paste".into()));
    assert_eq!(args[2], OscType::String("samples".into()));
    assert!(matches!(args[4], OscType::Blob(ref b) if b.len() == 16));

    // A click leaves a selection of zero length, and a paste lands where it
    // put the cursor -- not at frame 0, which is what reading only a span
    // answered.
    host.select_timeline(50, 6.0, 6.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Paste, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 50).expect("a paste reports");
    assert_eq!(args[1], OscType::Double(6.0), "at the cursor");

    // **The position cursor wins over a selection swept elsewhere**: a paste
    // lands where the reader put the cursor, not at the selection's start.
    host.set_timeline_cursor(50, 1.0);
    host.select_timeline(50, 3.0, 5.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Paste, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 50).expect("a paste reports");
    assert_eq!(args[1], OscType::Double(1.0), "at the position cursor");

    // **A paste with the pointer over nothing** (from the menu) goes to the
    // view the cursor was put on, with nothing selected.
    host.select_timeline(50, 0.0, 0.0);
    host.set_timeline_cursor(50, 4.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Paste, -10.0, -10.0, &mut clip)
        .expect("the view the cursor was put on answers");
    let args = emitted_args(&effects, 50).expect("a paste reports");
    assert_eq!(args[1], OscType::Double(4.0));

    // A mix is the same payload under its own word: the owner adds the block
    // onto what is there rather than putting it in.
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Mix, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 50).expect("a mix reports");
    assert_eq!(args[0], OscType::String("mix".into()));
    assert!(matches!(args[4], OscType::Blob(ref b) if b.len() == 16));
}

/// **A source the host cannot read declines, out loud.** A mapped pyramid is an
/// overview and a live view has no addressable past; putting silence on the
/// clipboard would be the one answer worse than saying no, and saying nothing
/// at all teaches "sometimes copy does not work".
#[test]
fn a_copy_the_host_cannot_honestly_make_is_refused() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":52,"type":"signal","view":"trace","bus":0,"rate":"audio"}]}"#,
    );
    host.set_timeline_total(52, 1000);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    host.select_timeline(52, 10.0, 40.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 52).expect("the refusal is reported");
    assert_eq!(args[0], OscType::String("refused".into()));
    assert_eq!(args[1], OscType::String("copy".into()));
    assert!(clip.is_empty(), "and nothing was put on the clipboard");
}

/// **An owner that copies is asked to**: the copy and the cut leave as
/// intents, the clipboard is the owner's to fill (`/gui_clipboard`), and a
/// source the host cannot read is not refused.
#[test]
fn a_copy_goes_to_an_owner_that_copies() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":53,"type":"signal","view":"trace","bus":0,"rate":"audio",
             "copies":"owner"}]}"#,
    );
    host.set_timeline_total(53, 1000);
    let g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let mut clip = crate::host::clipboard::Clip::default();
    host.select_timeline(53, 10.0, 40.0);
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Copy, 400.0, 150.0, &mut clip)
        .expect("answered");
    let args = emitted_args(&effects, 53).expect("the copy is asked of the owner");
    assert_eq!(args[0], OscType::String("copy".into()));
    let effects = g
        .clipboard_key(&mut host, &ctx, ClipVerb::Cut, 400.0, 150.0, &mut clip)
        .expect("answered");
    let tags: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            GestureEffect::Emit { args, .. } => args.first().cloned(),
            _ => None,
        })
        .collect();
    assert_eq!(
        tags,
        [OscType::String("cut".into())],
        "the owner completes copy and publication before removal"
    );

    // The owner's document goes on the host's clipboard.
    let doc = r#"{"content":{"kind":"buffer","channels":2,"frames":40,"sample_rate":48000.0,"buffer":70}}"#;
    host.handle_packet(
        OscPacket::Message(OscMessage {
            addr: crate::host::GUI_CLIPBOARD.into(),
            args: vec![OscType::String(doc.into())],
        }),
        from(),
    );
    assert_eq!(host.clipboard.doc().map(|d| d.kind()), Some("buffer"));
}

#[test]
fn wheel_zooms_the_time_axis_and_emits_the_view() {
    let mut host = host_from(
        r#"{"type":"window","children":[
            {"id":60,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
    );
    host.set_timeline_total(60, 1000);
    let mut g = Gestures::default();
    let ctx = GestureCtx::new(1, 800, 300);
    let before = host.timeline_nav(60).unwrap().0.len;
    let effects = g.wheel(&mut host, &ctx, 400.0, 150.0, 1.0);
    let after = host.timeline_nav(60).unwrap().0.len;
    assert!(after < before, "wheel-in shrinks the visible window");
    assert!(has_emit_tag(&effects, 60, "view"));
}

/// The lane count is two halves of one answer: the front knows how many
/// channels reached the card, the widget knows what it does with them. An
/// overlaid trace draws one lane out of four channels, a stacked one four --
/// and the machine, which divides by it, asks rather than matching on a kind.
#[test]
fn a_widget_says_how_it_stacks_what_the_front_uploaded() {
    let host = host_from(
        r#"{"type":"window","children":[
            {"id":60,"type":"signal","view":"trace","data":[0.0,1.0],"navigable":1},
            {"id":61,"type":"signal","view":"trace","data":[0.0,1.0],"navigable":1,
             "overlay":1},
            {"id":62,"type":"label","text":"nothing on the card"}]}"#,
    );
    let tree = host.window_def(1).unwrap();
    let mut ctx = GestureCtx::new(1, 800, 300);
    ctx.slot_channels.insert(60, 4);
    ctx.slot_channels.insert(61, 4);
    assert_eq!(ctx.rows(60, &tree.find(60).unwrap().kind), 4);
    assert_eq!(ctx.rows(61, &tree.find(61).unwrap().kind), 1);
    // A widget with no slot was given nothing, and nothing is one lane.
    assert_eq!(ctx.rows(62, &tree.find(62).unwrap().kind), 1);
}

/// **A roll's keyboard scrolls through the octaves under the wheel**, as a
/// piano's range does, and Ctrl zooms them: the window keeps its height and
/// moves up for a wheel up, where a waveform's strip would zoom.
#[test]
fn a_rolls_keyboard_scrolls_under_the_wheel_and_ctrl_zooms() {
    let mut host = host_from(
        r#"{"type":"window","margin":0,"children":[
            {"id":70,"type":"notes","notes":[0.0,100.0,60.0,100,0],"min":0,"max":127,
             "axes":{"y":{"start":0.4,"len":0.2}}}]}"#,
    );
    let view = |host: &Host| {
        host.window_def(1)
            .unwrap()
            .find(70)
            .unwrap()
            .kind
            .editor()
            .unwrap()
            .y_view()
    };
    let mut g = Gestures::default();
    let mut ctx = GestureCtx::new(1, 800, 300);
    // Over the keyboard, the gutter at the left of the roll.
    let effects = g.wheel(&mut host, &ctx, 10.0, 100.0, 1.0);
    assert!(has_emit_tag(&effects, 70, "view_y"));
    let (start, len) = view(&host);
    assert!(
        (len - 0.2).abs() < 1e-9,
        "a scroll keeps the window's height"
    );
    assert!((start - 0.42).abs() < 1e-9, "up a tenth of it: {start}");
    ctx.ctrl = true;
    g.wheel(&mut host, &ctx, 10.0, 100.0, 1.0);
    let (_, zoomed) = view(&host);
    assert!(zoomed < 0.2, "Ctrl zooms: {zoomed}");
}
