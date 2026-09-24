//! The multitrack widget's tests, beside the widget.

use super::props::from_props;
use super::*;
use crate::host::widget::element::{Mods, TimeSpace};

/// A widget placed on a navigation group whose window is the whole multitrack,
/// with a header gutter wide enough to have a body beside it.
fn input<'a>(m: &'a Metrics, rect: Rect, len: f64) -> Input<'a> {
    Input {
        metrics: m,
        rect,
        indent: 100.0,
        scale: 1.0,
        mods: Mods::default(),
        viewport: (rect.w, rect.h),
        clicks: 1,
        time: Some(TimeSpace::of(View { start: 0.0, len }, len)),
    }
}

/// Two lanes of 100, and a clip on each: `a` over the first half of the
/// multitrack on `noise`, `b` over the second on `tone`.
fn multitrack() -> Multitrack {
    from_props(&props(
        r#"{"lanes": ["noise", "", 100, 0, 0, 1, 1, "tone", "", 100, 0, 0, 1, 1],
            "clips": ["a", "noise", 0, 500, 0, "", 0, "b", "tone", 500, 500, 0, "", 0]}"#,
    ))
}

/// The x a time lands at, and the y the middle of lane `i` is at.
fn xy(mt: &Multitrack, m: &Metrics, rect: Rect, t: f64, len: f64, i: usize) -> (f64, f64) {
    let body = track::lane_body(rect, false, 100.0, m);
    let x = f64::from(body.x) + t / len * f64::from(body.w);
    let at = mt.lane_rects(rect);
    (x, f64::from(at[i].y + at[i].h / 2.0))
}

fn props(json: &str) -> Map<String, Value> {
    match serde_json::from_str(json).expect("valid JSON") {
        Value::Object(m) => m,
        _ => panic!("an object"),
    }
}

const TWO: &str = r#"{
    "lanes": ["noise", "", 100, 0, 0, 0.8, 1, "tone", "Lead", 60, 1, 0, 0.5, 1],
    "clips": ["a", "noise", 0, 48000, 0, "", 0,
              "b", "tone", 96000, 48000, 0, "take 2", 0]
}"#;

/// **A metered track asks for its buses and reads them.** The declaration
/// is the subscription -- nothing else would ask the window for the frames
/// a level moves on -- and what a strip shows is the bus, read where it
/// stands rather than sent per block.
#[test]
fn a_metered_track_declares_its_buses_and_reads_them() {
    struct Buses;
    impl crate::host::BusSource for Buses {
        fn control(&self, index: usize) -> f32 {
            // Bus 10 is unity, 11 is silence, 12 is the mark above them.
            match index {
                10 => 1.0,
                12 => 1.0,
                _ => 0.0,
            }
        }
        fn level(&self, _bus: i32) -> f32 {
            0.0
        }
    }

    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1.0, 1, "two", "", 100, 0, 0, 1.0, 1],
            "meters": ["one", 10, 12, 2]}"#,
    ));
    assert_eq!(
        mt.needs().buses,
        vec![10, 11, 12, 13],
        "both runs, both channels, and nothing for the track with no meter"
    );

    let buses = Buses;
    let world = crate::host::world::World {
        bus: Some(&buses),
        ..Default::default()
    };
    let metrics = crate::host::metrics::Metrics::default();
    let ctx = Ctx {
        world: &world,
        metrics: &metrics,
        rect: Rect::new(0.0, 0.0, 800.0, 200.0),
        indent: 120.0,
        scale: 1.0,
        time: None,
        clip: None,
        focused: false,
        clock: 0.0,
    };
    let live = mt.live_header(&mt.lanes[0], &ctx);
    assert_eq!(live.meters, vec![(1.0, 1.0), (0.0, 0.0)]);
    assert!(
        mt.live_header(&mt.lanes[1], &ctx).meters.is_empty(),
        "a track with no meter draws no strip"
    );

    // **And the prop is set, not only built with.** A track's buses are
    // allocated when the track reaches the server, so one made after the
    // window opened -- a double click on the header -- names buses this
    // element never saw. Dropped, its strip never appeared and the name
    // spread over the space the strip should have taken.
    let mut mt = mt;
    assert!(mt.set(
        "meters",
        &Value::from(r#"["one", 10, 12, 2, "two", 10, 12, 2]"#)
    ));
    assert_eq!(
        mt.live_header(&mt.lanes[1], &ctx).meters,
        vec![(1.0, 1.0), (0.0, 0.0)],
        "the track that was told about later has its strip"
    );
}

/// **The strip is laid out from the channel count alone**, which is what
/// lets a press land on the pixels a control was drawn on: the hit test
/// never reads a bus, so its header must still be the width the drawing's
/// was.
#[test]
fn a_meter_takes_its_width_from_the_channels_and_not_from_the_level() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1.0, 1], "meters": ["one", 10, 12, 2]}"#,
    ));
    let m = crate::host::metrics::Metrics::default();
    let plain = from_props(&props(r#"{"lanes": ["one", "", 100, 0, 0, 1.0, 1]}"#));
    assert!(
        mt.gutter(&m) > plain.gutter(&m),
        "the band holds the strip beside the name rather than over it"
    );

    let mut quiet = mt.header(&mt.lanes[0], 0.0);
    let loud = {
        let mut h = quiet.clone();
        h.meters = vec![(1.0, 1.0); 2];
        h
    };
    assert_eq!(
        quiet.width(&m),
        loud.width(&m),
        "a level is not a size: a moving meter would move the name under it"
    );
    quiet.meters.clear();
    assert!(
        quiet.width(&m) < loud.width(&m),
        "and no meter takes no room"
    );
}

/// **A box's base view is what its contents are**, and it is drawn by the
/// element that draws it anywhere else: samples through the signal
/// element's body door, notes through the roll's. What the widget adds is
/// the axis -- a box is a window onto a picture, never a second
/// implementation of one.
#[test]
fn a_box_of_notes_is_a_roll_and_a_box_of_samples_is_a_take() {
    let mt = from_props(&props(
        r#"{
        "lanes": ["one", "", 100, 0, 0, 1.0, 1],
        "clips": ["a", "one", 0, 48000, 0, "", 0,
                  "b", "one", 96000, 48000, 0, "", -1],
        "notes": ["b", 0.0, 4800.0, 60.0, 100.0, 0.0,
                  "b", 4800.0, 4800.0, 72.0, 100.0, 0.0]
    }"#,
    ));
    assert!(mt.rolls.contains_key("b"), "the box the notes named");
    assert!(!mt.rolls.contains_key("a"), "and no other");
    assert!(
        mt.takes.is_empty(),
        "a take body is built when its samples arrive, not before"
    );
}

/// **A spectral box is a texture of its own**, named by the take it draws:
/// a time-frequency picture samples one, so it goes to the GPU pass rather
/// than into the mesh, and this element holds one per buffer its boxes are
/// windows onto.
#[test]
fn a_spectral_box_names_the_take_its_texture_is() {
    use crate::host::widget::element::SlotKey;
    let mut mt = from_props(&props(
        r#"{"view": "spectrogram",
            "lanes": ["one", "", 100, 0, 0, 1.0, 1],
            "clips": ["a", "one", 0, 48000, 0, "", 3]}"#,
    ));
    let world = crate::host::world::World::default();
    let metrics = crate::host::metrics::Metrics::default();
    let ctx = Ctx {
        world: &world,
        metrics: &metrics,
        rect: Rect::new(0.0, 0.0, 800.0, 200.0),
        indent: 0.0,
        scale: 1.0,
        time: None,
        clip: None,
        focused: false,
        clock: 0.0,
    };
    assert!(
        mt.texture_bodies(&ctx).is_empty(),
        "a picture of nothing is the frame around it"
    );
    mt.bulk_of(
        3,
        Loaded::Raw {
            samples: vec![0.0; 4096],
            channels: 1,
        },
    );
    let bodies = mt.texture_bodies(&ctx);
    assert_eq!(bodies.len(), 1, "one box, one picture");
    assert_eq!(bodies[0].key, SlotKey(3), "named by the take it draws");
    let fills = mt.fills();
    assert_eq!(fills.len(), 1, "and one upload, however many boxes read it");
    assert_eq!(fills[0].0, SlotKey(3));
}

/// The pitch window a roll body is fitted to is the crate's rule, so a box
/// and a window over the same notes are the same height.
#[test]
fn a_roll_body_is_fitted_by_the_crates_own_rule() {
    let notes = [0.0, 4800.0, 60.0, 100.0, 0.0];
    let (min, max) = clausters_document::view::catalogue::pitch_window(&notes);
    let mt = from_props(&props(
        r#"{"clips": ["b", "one", 0, 48000, 0, "", -1],
            "notes": ["b", 0.0, 4800.0, 60.0, 100.0, 0.0]}"#,
    ));
    let roll = mt.rolls.get("b").expect("the roll");
    assert_eq!((roll.range().0, roll.range().1), (min as f32, max as f32));
}

/// **A client describes the multitrack; it does not compose a tree of it.** The
/// two structures arrive as flat arrays, like a roll's notes, and the widget
/// is the one thing that holds them.
#[test]
fn the_lanes_and_the_clips_are_props_of_one_widget() {
    let mt = from_props(&props(TWO));
    assert_eq!(mt.lanes.len(), 2);
    assert_eq!(mt.clips.len(), 2);

    assert_eq!(mt.lanes[1].name, "tone");
    assert_eq!(
        mt.lanes[1].shown(),
        "Lead",
        "the label draws, the name addresses"
    );
    assert!(mt.lanes[1].mute);
    assert_eq!(mt.lanes[1].gain, 0.5);

    assert_eq!(mt.clips[1].lane, "tone");
    assert_eq!(mt.clips[1].place.offset, 96_000.0);
    assert_eq!(mt.clips[1].shown(), "take 2");
}

/// A trailing partial group is dropped rather than half-read, and a lane
/// with no name is not a lane -- the one field that is the identity.
#[test]
fn a_partial_group_is_dropped_rather_than_half_read() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, "two", ""],
            "clips": ["a", "one", 0, 10, 0, ""]}"#,
    ));
    assert_eq!(mt.lanes.len(), 1, "the second group is short");
    assert!(mt.clips.is_empty(), "so is the only clip");

    let unnamed = from_props(&props(r#"{"lanes": [7, "", 100, 0, 0, 1, 1]}"#));
    assert!(unnamed.lanes.is_empty(), "a lane with no name is not one");
}

/// **A `/gui_set` of a structure is the same parse**, so what a query
/// reports is what a set would take -- the round trip every non-scalar here
/// keeps.
#[test]
fn a_set_reads_what_a_query_reported() {
    let mut mt = from_props(&props(TWO));
    let reported: Map<String, Value> = mt.info().into_iter().collect();

    let mut empty = Multitrack::default();
    assert!(empty.set("lanes", &reported["lanes"]));
    assert!(empty.set("clips", &reported["clips"]));
    assert_eq!(empty.lanes, mt.lanes);
    assert_eq!(empty.clips, mt.clips);

    // And the same through the string carrier a `/gui_set` actually uses.
    let as_text = Value::from(serde_json::to_string(&reported["clips"]).unwrap());
    mt.clips.clear();
    assert!(mt.set("clips", &as_text));
    assert_eq!(mt.clips.len(), 2);
}

/// A clip naming a lane that is not here is **kept**, not dropped: what
/// cannot be placed can still be reported, so a script that renamed a lane
/// gets its clips back to re-home instead of losing them.
#[test]
fn a_clip_on_a_lane_that_is_gone_is_kept_and_not_placed() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 10, 0, "", 0, "b", "vanished", 0, 10, 0, "", 0]}"#,
    ));
    assert_eq!(mt.clips.len(), 2);
    assert!(mt.lane_of(&mt.clips[0]).is_some());
    assert!(mt.lane_of(&mt.clips[1]).is_none());
    // It is still in what the widget reports, which is how it comes back.
    let Value::Array(written) = model::clips_json(&mt.clips) else {
        panic!("an array");
    };
    assert_eq!(written.len(), 14);
}

/// **Its size is the caller's, never its content's.** A lane added by a
/// `/gui_set` must not relayout the window.
#[test]
fn a_lane_added_never_changes_how_big_the_widget_wants_to_be() {
    let m = Metrics::default();
    let mut mt = Multitrack::default();
    let bare = mt.natural(&m, 1.0);
    mt.set("lanes", &props(TWO)["lanes"]);
    assert_eq!(mt.natural(&m, 1.0), bare);
    // What *does* follow the content is how far the axis reaches.
    assert_eq!(mt.content_span(), Some(0.0));
    mt.set("clips", &props(TWO)["clips"]);
    assert_eq!(mt.content_span(), Some(144_000.0));
}
/// **The three tags are gone.** A move, a trim and a lane crossing each
/// leave as one `"clips"` payload carrying the multitrack as it now stands -- so
/// there is nothing for a reader to choose between, and no state a gesture
/// can put it in where the report changes shape.
#[test]
fn every_gesture_reports_the_multitrack_and_never_the_gesture() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;

    let tag = |ev: Events| match ev.into_messages().first() {
        Some(args) => match &args[0] {
            OscType::String(s) => s.clone(),
            _ => "?".into(),
        },
        None => "<nothing>".into(),
    };

    // A move inside its lane.
    let mut mt = multitrack();
    let from = xy(&mt, &m, rect, 250.0, len, 0);
    let to = xy(&mt, &m, rect, 350.0, len, 0);
    assert!(matches!(
        mt.press(from, &input(&m, rect, len)),
        Claim::Take(_)
    ));
    mt.drag(to, &input(&m, rect, len));
    let moved = mt.release(to, true, &input(&m, rect, len));
    assert_eq!(tag(moved), "clips");
    assert_eq!(mt.clips[0].place.offset, 100.0, "it moved by the travel");

    // A trim, by the right edge.
    let mut mt = multitrack();
    let edge = xy(&mt, &m, rect, 500.0, len, 0);
    let pulled = xy(&mt, &m, rect, 400.0, len, 0);
    assert!(matches!(
        mt.press(edge, &input(&m, rect, len)),
        Claim::Take(_)
    ));
    mt.drag(pulled, &input(&m, rect, len));
    let trimmed = mt.release(pulled, true, &input(&m, rect, len));
    assert_eq!(tag(trimmed), "clips");
    assert!(mt.clips[0].place.dur < 500.0, "it got shorter");
    assert_eq!(mt.clips[0].place.offset, 0.0, "and stayed where it began");

    // A lane crossed.
    let mut mt = multitrack();
    let from = xy(&mt, &m, rect, 250.0, len, 0);
    let down = xy(&mt, &m, rect, 250.0, len, 1);
    assert!(matches!(
        mt.press(from, &input(&m, rect, len)),
        Claim::Take(_)
    ));
    mt.drag(down, &input(&m, rect, len));
    let crossed = mt.release(down, true, &input(&m, rect, len));
    assert_eq!(tag(crossed), "clips", "the same one tag, again");
    assert_eq!(
        mt.clips[0].lane, "tone",
        "one field, not a remove and an add"
    );
}

/// **A gesture that changed nothing is not an edit.** A press and a release
/// with nothing between them is a click, and a drag that came back is the
/// same thing by another road: looking at four clips must not cost four
/// undos.
#[test]
fn a_drag_that_came_back_reports_nothing() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let from = xy(&mt, &m, rect, 250.0, len, 0);
    let away = xy(&mt, &m, rect, 400.0, len, 0);

    mt.press(from, &input(&m, rect, len));
    assert!(mt.release(from, true, &input(&m, rect, len)).is_empty());

    mt.press(from, &input(&m, rect, len));
    mt.drag(away, &input(&m, rect, len));
    mt.drag(from, &input(&m, rect, len));
    assert!(
        mt.release(from, true, &input(&m, rect, len)).is_empty(),
        "it is where the press found it"
    );
}

/// **A press that found no clip goes back to the chain.** The slack between
/// clips is the container's: that is where a click places the transport's
/// cursor and a sweep starts a marquee, and swallowing the press would take
/// both away.
#[test]
fn a_press_on_bare_lane_declines_rather_than_swallowing_it() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    // Past `a`'s end, on its lane -- bare lane, and `b` is on the other one.
    let bare = xy(&mt, &m, rect, 800.0, len, 0);
    assert!(matches!(
        mt.press(bare, &input(&m, rect, len)),
        Claim::Decline
    ));
    assert!(mt.grab.is_none());
}

/// A multitrack with both kinds of curve: a track automation under `noise` and
/// an envelope inside the box `a`.
fn curved() -> Multitrack {
    from_props(&props(
        r#"{"lanes": ["noise", "", 100, 0, 0, 1, 1, "tone", "", 100, 0, 0, 1, 1],
            "clips": ["a", "noise", 0, 500, 0, "", 0, "b", "tone", 500, 500, 0, "", 0],
            "curves": ["gain", "noise", "Gain", 0, 1, 40],
            "layers": ["env", "a", "", 0, 1],
            "points": ["gain", 0, 1, 1, 0, "gain", 1000, 0, 1, 0,
                       "env", 0, 0, 1, 0, "env", 500, 1, 1, 0]}"#,
    ))
}

/// **The same element in two places, and the places are the difference.**
/// A track automation takes a row of its own under its lane and runs the
/// whole timeline; a clip envelope is a layer inside its box and runs as
/// long as the box does.
#[test]
fn an_automation_is_a_row_and_an_envelope_is_a_layer() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let mt = curved();
    assert_eq!(mt.bodies.len(), 2, "one element per curve, however placed");

    // The row is under `noise`, and the second lane sits below it.
    let stack = mt.stack();
    assert_eq!(stack.len(), 3);
    assert_eq!(stack.row(1), Some(stack::Row::Curve(0)));
    assert_eq!(mt.lane_rects(rect)[1].y, 100.0 + 40.0 + 2.0 * GAP);

    let drawn = mt.curves_on_screen(rect, 100.0, &m, mt_time(500.0));
    let row = drawn.iter().find(|(n, ..)| *n == "gain").expect("the row");
    let layer = drawn.iter().find(|(n, ..)| *n == "env").expect("the layer");
    assert_eq!(row.2.span, 1000.0, "a row is as long as the multitrack");
    assert_eq!(layer.2.span, 500.0, "a layer is as long as its box");
    assert!(layer.1.w < row.1.w, "the box is narrower than the timeline");
}

/// The shared axis a curved multitrack is read against.
fn mt_time(len: f64) -> Option<TimeSpace> {
    Some(TimeSpace::of(
        View {
            start: 0.0,
            len: len * 2.0,
        },
        len * 2.0,
    ))
}

/// **A hidden layer is not drawn, and what is not drawn is not edited.**
#[test]
fn hiding_a_layer_takes_it_out_of_the_picture_and_out_of_reach() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let mut mt = curved();
    assert!(mt.set("hidden", &Value::from("env gain")));
    assert!(
        mt.curves_on_screen(rect, 100.0, &m, mt_time(500.0))
            .is_empty(),
        "neither is drawn"
    );
}

/// **A press lands on a curve's own points, never on the rectangle it
/// shares** -- so an envelope drawn across a box leaves the box draggable,
/// and the press that misses the line moves the box instead.
#[test]
fn a_press_beside_the_line_still_moves_the_box() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = curved();
    // The envelope on `a` runs from its floor to its ceiling, so the top
    // left corner of the box is far from the line.
    let at = mt.lane_rects(rect);
    let body = track::lane_body(at[0], false, 100.0, &m);
    let corner = (f64::from(body.x) + 2.0, f64::from(at[0].y) + 3.0);
    assert!(matches!(
        mt.press(corner, &input(&m, rect, len)),
        Claim::Take(_)
    ));
    assert!(mt.grab.is_some(), "the box took it");
    assert!(mt.holding.is_none(), "and no curve did");
    assert_eq!(mt.layer, None, "the placement is still what is in hand");
}

/// **A break-point is grabbed on the pixels it was drawn on**, and moving
/// one reports every curve there is -- the payload is the multitrack's, so a
/// `/gui_set points` of what came back is the identity.
#[test]
fn dragging_a_point_reports_every_curve() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = curved();
    let (_, row, space) = *mt
        .curves_on_screen(rect, 100.0, &m, mt_time(500.0))
        .iter()
        .find(|(n, ..)| *n == "gain")
        .expect("the row");
    // The first point of `gain` is at time zero and value one: the top
    // left of its row.
    let start = space.view.start;
    let x = f64::from(row.x) + (0.0 - start) / space.view.len * f64::from(row.w);
    let from = (x, f64::from(row.y) + 1.0);
    let inp = Input {
        time: mt_time(500.0),
        ..input(&m, rect, len)
    };
    assert!(matches!(mt.press(from, &inp), Claim::Take(_)));
    assert_eq!(
        mt.layer.as_deref(),
        Some("gain"),
        "the press took the layer"
    );
    let to = (x, f64::from(row.y + row.h) - 1.0);
    mt.drag(to, &inp);
    let msgs = mt.release(to, true, &inp).into_messages();
    let args = msgs.first().expect("an edit");
    assert_eq!(args[0], OscType::String("points".into()));
    assert_eq!(
        args.len(),
        1 + 5 * 4,
        "the tag, then a quintuple per point of every curve"
    );
    assert_eq!(args[1], OscType::String("gain".into()));
    assert_eq!(args[11], OscType::String("env".into()), "the layer too");
}

/// **A second press on a box is a press like the first**: the multitrack edits
/// non-destructively and opens nothing over what a box holds, so a double click
/// grabs the box and says nothing.
#[test]
fn a_second_press_on_a_box_is_a_press() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let on_a = xy(&mt, &m, rect, 250.0, len, 0);
    let twice = Input {
        clicks: 2,
        ..input(&m, rect, len)
    };
    let Claim::Take(take) = mt.press(on_a, &twice) else {
        panic!("the second press is taken");
    };
    assert!(
        take.events.into_messages().is_empty(),
        "and reports nothing"
    );
    assert!(mt.grab.is_some(), "it grabs the box, as one press does");
}

/// **A track is zoomed vertically by pulling its header's bottom edge**, and
/// the height a hand set survives what the multitrack says next: a `lanes`
/// payload states a height on every row, so a fader moved or a track added
/// would otherwise take the zoom away with it.
#[test]
fn a_track_is_zoomed_by_its_bottom_edge_and_keeps_it() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let inp = input(&m, rect, len);
    let band = crate::host::timeline::gutter_band(mt.lane_rects(rect)[0], 100.0);
    let edge = (f64::from(band.x) + 4.0, f64::from(band.y + band.h) - 1.0);
    let was = mt.lanes[0].height;

    assert!(matches!(mt.press(edge, &inp), Claim::Take(_)));
    assert_eq!(mt.lanes[0].height, was, "the press resizes nothing");
    mt.drag((edge.0, edge.1 + 60.0), &inp);
    assert_eq!(mt.lanes[0].height, was + 60.0, "the row follows the hand");
    assert_eq!(mt.lanes[1].height, was, "and only that row");
    assert!(
        mt.release((edge.0, edge.1 + 60.0), true, &inp)
            .into_messages()
            .is_empty(),
        "how tall a row is drawn is this window's: the multitrack is not asked"
    );

    // The client redraws its rows -- a fader moved, a track added -- and
    // the height a hand set is still the height.
    assert!(mt.set(
        "lanes",
        &serde_json::json!(["noise", "", 100, 0, 0, 0.5, 1, "tone", "", 100, 0, 0, 1, 1]),
    ));
    assert_eq!(
        mt.lanes[0].height,
        was + 60.0,
        "the zoom outlived the payload"
    );
    assert_eq!(mt.lanes[0].gain, 0.5, "and the payload landed");

    // A row that is gone takes its height with it.
    assert!(mt.set("lanes", &serde_json::json!(["tone", "", 100, 0, 0, 1, 1])));
    assert!(mt.zoom.is_empty());
}

/// **The stack has vertical gestures of its own** *(asked for by the user
/// 2026-09-12, to make the example a multitrack editor worth trying)*: the
/// wheel scrolls it and zooms one row.
///
/// The plain wheel is left to the time axis, which is what it is over every
/// timeline view here -- so this element declines it, and the machine behind
/// it goes on to the axis.
#[test]
fn the_wheel_scrolls_the_stack_and_zooms_one_row() {
    let m = Metrics::default();
    // Short on purpose: three rows of a hundred-odd pixels do not fit, so
    // there is something below to scroll to.
    let rect = Rect::new(0.0, 0.0, 600.0, 150.0);
    let len = 1000.0;
    let mut mt = multitrack();
    mt.curves = vec![model::Curve {
        name: "c".into(),
        owner: "noise".into(),
        label: String::new(),
        min: 0.0,
        max: 1.0,
        height: CURVE_H,
    }];
    let inp = input(&m, rect, len);
    let plain = |mods: Mods| Input { mods, ..inp };
    let on_lane = (10.0, f64::from(mt.lane_rects(rect)[0].y) + 5.0);

    // The plain wheel is the axis', so nothing here answers it.
    assert!(
        mt.wheel(on_lane, (0.0, 1.0), &plain(Mods::default()))
            .is_none()
    );
    assert_eq!(mt.scroll, 0.0);

    // Shift scrolls, and only as far as there is stack below.
    let shift = Mods {
        shift: true,
        ..Mods::default()
    };
    assert!(mt.wheel(on_lane, (0.0, -1.0), &plain(shift)).is_some());
    assert!(mt.scroll > 0.0, "it moved down: {}", mt.scroll);
    let over = mt.stack().content_height() - rect.h;
    for _ in 0..50 {
        mt.wheel(on_lane, (0.0, -1.0), &plain(shift));
    }
    assert_eq!(mt.scroll, over, "and stops at the last row");
    assert!(
        mt.wheel(on_lane, (0.0, -1.0), &plain(shift)).is_some(),
        "and is still taken there: a stack at its end does nothing, rather \
         than letting the multitrack zoom under the hand"
    );
    assert_eq!(mt.scroll, over, "and nothing moved");
    for _ in 0..60 {
        mt.wheel(on_lane, (0.0, 1.0), &plain(shift));
    }
    assert_eq!(mt.scroll, 0.0, "and back at the top, never above it");

    // Ctrl zooms the row the pointer is on, a lane and a curve alike.
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    let was = mt.lanes[0].height;
    assert!(mt.wheel(on_lane, (0.0, 1.0), &plain(ctrl)).is_some());
    assert!(mt.lanes[0].height > was, "the lane grew");
    assert_eq!(mt.lanes[1].height, was, "and only that row");

    let row = mt.stack().rects(rect, mt.scroll)[1];
    let on_curve = (10.0, f64::from(row.y) + 5.0);
    assert!(matches!(
        mt.row_kind(&inp, on_curve.1),
        Some(stack::Row::Curve(0))
    ));
    let was = mt.curves[0].height;
    assert!(mt.wheel(on_curve, (0.0, 1.0), &plain(ctrl)).is_some());
    assert!(mt.curves[0].height > was, "a curve row has no edge to pull");

    // And both survive the payload that redraws them, the way a lane's
    // height set by a header drag does.
    let tall = (mt.lanes[0].height, mt.curves[0].height);
    assert!(mt.set(
        "lanes",
        &serde_json::json!(["noise", "", 100, 0, 0, 0.5, 1, "tone", "", 100, 0, 0, 1, 1]),
    ));
    assert!(mt.set("curves", &Value::from(r#"["c", "noise", "", 0, 1, 40]"#)));
    assert_eq!((mt.lanes[0].height, mt.curves[0].height), tall);
}

/// **The user's own sequence**: make a track, press its `A`, and the row
/// has to be there *(found 2026-09-12: "al crear un track y activar A no
/// hace nada, al crear otro track aparece la automatizacion del
/// anterior")*.
///
/// Every step of it passed on its own and the path did not, which is the
/// third time this seam has done that -- so this walks the whole thing: the
/// double click that makes the row, the correction that renames it from the
/// word this minted to the id the multitrack gave it, the press on `A`, and the
/// correction that carries the curve the owner made.
/// **The toggle hides and shows the row it is about** *(found 2026-09-12 by
/// the user: "la A sigue sin ocultar ni mostrar")*.
///
/// The press flipped `curves`, which nothing draws from: the stack is built
/// from `hidden`, and the owner answers with the picture only when a
/// **name** changes. So hiding stated nothing anybody could see, and the
/// first press on a bare track looked like it worked only because it mints a
/// curve.
#[test]
fn the_toggle_hides_the_row_and_shows_it_again() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = from_props(&props(
        r#"{"lanes": ["10", "noise", 96, 0, 0, 1, 1],
            "curves": ["100", "10", "gain", 0, 1, 40],
            "hidden": ""}"#,
    ));
    let indent = mt.gutter(&m);
    let inp = Input {
        indent,
        ..input(&m, rect, len)
    };
    let rows = |mt: &Multitrack| {
        let stack = mt.stack();
        (0..stack.len())
            .filter_map(|i| stack.row(i))
            .collect::<Vec<_>>()
    };
    let press = |mt: &mut Multitrack| {
        let band = crate::host::timeline::gutter_band(mt.lane_rects(rect)[0], indent);
        let cell = track::header_parts(band, &mt.header(&mt.lanes[0], indent), &m)
            .curves
            .expect("the toggle");
        let at = (
            f64::from(cell.x + cell.w / 2.0),
            f64::from(cell.y + cell.h / 2.0),
        );
        let Claim::Take(take) = mt.press(at, &inp) else {
            panic!("the header takes it")
        };
        take.events.into_messages()
    };

    assert_eq!(rows(&mt), vec![stack::Row::Lane(0), stack::Row::Curve(0)]);

    let msgs = press(&mut mt);
    assert_eq!(msgs[0][7], OscType::Int(0), "the row says it is hidden");
    assert_eq!(
        rows(&mt),
        vec![stack::Row::Lane(0)],
        "and the row is gone from the stack, under the hand"
    );

    // **And the owner says nothing**, because no name changed -- which is
    // exactly why the press has to state it here.
    let msgs = press(&mut mt);
    assert_eq!(msgs[0][7], OscType::Int(1));
    assert_eq!(rows(&mt), vec![stack::Row::Lane(0), stack::Row::Curve(0)]);
}

#[test]
fn a_track_made_here_shows_the_automation_its_toggle_asked_for() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = from_props(&props(
        r#"{"lanes": ["10", "noise", 96, 0, 0, 1, 0], "clips": []}"#,
    ));
    let indent = mt.gutter(&m);
    let inp = Input {
        indent,
        ..input(&m, rect, len)
    };

    // The double click under the last header: a track, at the end.
    let under = (
        f64::from(rect.x) + 5.0,
        f64::from(mt.lane_rects(rect)[0].y + mt.lanes[0].height) + 20.0,
    );
    let twice = Input { clicks: 2, ..inp };
    let Claim::Take(_) = mt.press(under, &twice) else {
        panic!("the band under the last header makes one")
    };
    assert_eq!(mt.lanes.len(), 2);
    assert_eq!(mt.lanes[1].name, "track 1", "a word this minted");
    assert!(!mt.lanes[1].curves, "and it asks for no automation");

    // The owner made the track and named it by its id.
    assert!(mt.set(
        "lanes",
        &Value::from(r#"["10", "noise", 96, 0, 0, 1, 0, "101", "track 101", 96, 0, 0, 1, 0]"#)
    ));

    // `A` on the new row.
    let band = crate::host::timeline::gutter_band(mt.lane_rects(rect)[1], indent);
    let cell = track::header_parts(band, &mt.header(&mt.lanes[1], indent), &m)
        .curves
        .expect("the toggle");
    let Claim::Take(take) = mt.press(
        (
            f64::from(cell.x + cell.w / 2.0),
            f64::from(cell.y + cell.h / 2.0),
        ),
        &inp,
    ) else {
        panic!("the header takes it")
    };
    let msgs = take.events.into_messages();
    assert_eq!(msgs[0][14], OscType::Int(1), "the row asks: {:?}", msgs[0]);

    // The owner made the curve and answers with the whole picture.
    assert!(mt.set(
        "lanes",
        &Value::from(r#"["10", "noise", 96, 0, 0, 1, 0, "101", "track 101", 96, 0, 0, 1, 1]"#)
    ));
    assert!(mt.set(
        "curves",
        &Value::from(r#"["103", "101", "gain", 0, 1, 40]"#)
    ));
    assert!(mt.set("hidden", &Value::from("")));

    // And the row is in the stack, under the track it belongs to.
    let stack = mt.stack();
    let rows: Vec<stack::Row> = (0..stack.len()).filter_map(|i| stack.row(i)).collect();
    assert_eq!(
        rows,
        vec![
            stack::Row::Lane(0),
            stack::Row::Lane(1),
            stack::Row::Curve(0)
        ],
        "the curve row is there, under its track"
    );
}

/// **A hidden curve is a row that is not there** *(found 2026-09-12 by the
/// user, pressing the header's toggle: "la A sigue sin ocultar ni
/// mostrar")*.
///
/// The stack reserved a band for every curve and the drawing skipped the
/// hidden ones, which is a hole exactly where the row was: the picture did
/// not change when a hand hid one and did not change when it showed one
/// either -- the same gap, with or without a line in it. The toggle was
/// working the whole way down and there was nothing to see at the end of
/// it, which is the most expensive kind of correct.
#[test]
fn a_hidden_curve_gives_its_row_back() {
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let mut mt = multitrack();
    mt.curves = vec![model::Curve {
        name: "c".into(),
        owner: "noise".into(),
        label: String::new(),
        min: 0.0,
        max: 1.0,
        height: CURVE_H,
    }];
    let with_row = mt.stack().content_height();
    let rows = mt.stack().len();

    assert!(mt.set("hidden", &Value::from("c")));
    assert_eq!(mt.stack().len(), rows - 1, "the row is gone, not blank");
    assert_eq!(
        mt.stack().content_height(),
        with_row - (CURVE_H + mt.gap),
        "and the stack is shorter by exactly that row"
    );
    // The lane under it moves up, which is the whole of what a hand sees.
    let after = mt.lane_rects(rect)[1].y;
    assert!(mt.set("hidden", &Value::from("")));
    assert!(
        mt.lane_rects(rect)[1].y > after,
        "and showing it puts the row back"
    );
    assert_eq!(mt.stack().len(), rows);
}

/// **`A` in a track's header shows and hides its automation rows** *(asked
/// for by the user 2026-09-12, as a facility for trying the example)*.
///
/// It rides the `lanes` report the mute and the solo beside it ride, and
/// the owner answers by saying which curves are visible -- a row a multitrack
/// shows is the multitrack's, and this is a control over that rather than a
/// second place for it to be recorded.
#[test]
fn the_headers_automation_toggle_reports_the_lanes() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = multitrack();
    // **The band the widget asks for**, which is what holds the controls:
    // a fourth cell is part of what the gutter has to be wide enough for,
    // and taking a number out of the air here would test a header nobody
    // lays out.
    let indent = mt.gutter(&m);
    let inp = Input {
        indent,
        ..input(&m, rect, len)
    };

    // **Offered on a bare track too**: the first press is what asks for the
    // automation, so a button only on tracks that have one would be a
    // button nobody could reach to make the first.
    assert!(
        track::header_parts(
            crate::host::timeline::gutter_band(mt.lane_rects(rect)[0], indent),
            &mt.header(&mt.lanes[0], indent),
            &m,
        )
        .curves
        .is_some(),
        "a track with nothing to show still offers the verb"
    );
    let band = crate::host::timeline::gutter_band(mt.lane_rects(rect)[0], indent);
    let cell = track::header_parts(band, &mt.header(&mt.lanes[0], indent), &m)
        .curves
        .expect("the track has automation, so it has the toggle");
    let at = (
        f64::from(cell.x + cell.w / 2.0),
        f64::from(cell.y + cell.h / 2.0),
    );
    let Claim::Take(take) = mt.press(at, &inp) else {
        panic!("the header takes it")
    };
    assert!(!mt.lanes[0].curves, "it flipped");
    let msgs = take.events.into_messages();
    assert_eq!(msgs[0][0], OscType::String("lanes".into()));
    assert_eq!(
        msgs[0][7],
        OscType::Int(0),
        "and the row says its automation is hidden"
    );
}

/// **A paste needs two coordinates, and they anchor at two different
/// boxes.** In time the block starts at the **earliest** of them; in rows
/// it starts at the **topmost**, so the selected track is where the block
/// begins and everything else lands on it or below it, keeping whatever
/// gaps the block had.
///
/// The defect this pins (found 2026-09-12 by the user, on the multitrack
/// example): the rows anchored at the earliest box *in time*, so which
/// track a block landed on depended on the order its boxes happened to be
/// recorded in -- the same block pasted onto the same track went up or down
/// according to that, and part of it landed **above** the track the hand
/// had pointed at. The user's own case: boxes from tracks 2, 4 and 1,
/// pasted onto track 3, belong on 4, 6 and 3.
#[test]
fn a_paste_starts_at_the_selected_track_and_never_above_it() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    // Seven tracks, and three boxes whose rows and whose onsets disagree:
    // the earliest box is on track 2 and the topmost is on track 1.
    let lanes: String = (0..7)
        .map(|i| format!(r#""t{i}", "", 100, 0, 0, 1, 1"#))
        .collect::<Vec<_>>()
        .join(", ");
    let mut mt = from_props(&props(&format!(
        r#"{{"lanes": [{lanes}],
             "clips": ["x", "t2", 200, 100, 0, "", 0,
                       "y", "t4", 400, 100, 0, "", 0,
                       "z", "t1", 100, 100, 0, "", 0]}}"#
    )));
    let mut keys = |mt: &mut Multitrack, key: Key, cursor: Option<f64>| {
        mt.key(
            &key,
            &mut KeyInput {
                mods: ctrl,
                clipboard: &mut clipboard,
                cursor,
            },
        )
    };
    mt.selected = vec![0, 1, 2];
    keys(&mut mt, Key::Char('c'), None).expect("copied");

    mt.track = Some(3);
    keys(&mut mt, Key::Char('v'), Some(0.0)).expect("pasted");
    assert_eq!(mt.clips.len(), 6);
    let landed: Vec<&str> = mt.clips[3..].iter().map(|c| c.lane.as_str()).collect();
    assert_eq!(
        landed,
        ["t4", "t6", "t3"],
        "the topmost box is the one on the selected track, and the gap is kept"
    );
    // The time anchor is the other box: the earliest onset lands on the
    // cursor, and the rest keep their distances from it.
    let offsets: Vec<f64> = mt.clips[3..].iter().map(|c| c.place.offset).collect();
    assert_eq!(offsets, [100.0, 300.0, 0.0]);

    // With no track selected the rows are the ones it came from: a paste
    // back into the same multitrack is the block where it was.
    mt.track = None;
    keys(&mut mt, Key::Char('v'), Some(0.0)).expect("pasted");
    let landed: Vec<&str> = mt.clips[6..].iter().map(|c| c.lane.as_str()).collect();
    assert_eq!(landed, ["t2", "t4", "t1"]);
}

/// **A block that does not fit is refused, not flattened.**
///
/// It used to clamp every row past the last track onto that track, which
/// turned a block several tracks tall into a pile on one -- the one thing a
/// paste promises not to do. The multitrack gains no track either: making one is
/// a verb of its own, and a paste is not the place to grow the thing it is
/// pasting into.
#[test]
fn a_paste_that_runs_past_the_last_track_says_so() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    let mut mt = multitrack();
    let mut keys = |mt: &mut Multitrack, key: Key, cursor: Option<f64>| {
        mt.key(
            &key,
            &mut KeyInput {
                mods: ctrl,
                clipboard: &mut clipboard,
                cursor,
            },
        )
    };
    // Two tracks, a block two tracks tall, pasted onto the second: the
    // second half has nowhere to go.
    mt.selected = vec![0, 1];
    keys(&mut mt, Key::Char('c'), None).expect("copied");
    mt.track = Some(1);
    let said = refusal(keys(&mut mt, Key::Char('v'), Some(100.0)));
    assert_eq!(
        said,
        Some("this block is 2 track(s) tall and needs 3 here; the multitrack has 2".to_string())
    );
    assert_eq!(mt.clips.len(), 2, "and nothing was pasted");

    // Onto the first, the same block fits exactly.
    mt.track = Some(0);
    keys(&mut mt, Key::Char('v'), Some(100.0)).expect("pasted");
    assert_eq!(mt.clips.len(), 4);
    assert_eq!(mt.clips[2].lane, "noise");
    assert_eq!(mt.clips[3].lane, "tone");
}

/// **An automation row's header is the track's picture, not the track.** A
/// curve is drawn in a row of its own under the lane it belongs to, so a
/// press on the band beside it addresses no track: it selects none, lets go
/// of none, and asks for none.
#[test]
fn an_automation_rows_header_addresses_no_track() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
    let len = 1000.0;
    let mut mt = curved();
    let inp = input(&m, rect, len);
    // The curve row is the second entry of the stack: `noise`, its `gain`
    // row, then `tone`.
    let rows = mt.stack().rects(rect, 0.0);
    let curve_row = rows[1];
    let on = (f64::from(rect.x) + 4.0, f64::from(curve_row.y) + 2.0);

    mt.track = Some(0);
    assert!(matches!(mt.press(on, &inp), Claim::Take(_)));
    assert_eq!(mt.track, Some(0), "the track a hand had is still in hand");
    assert_eq!(mt.lanes.len(), 2, "and nothing was added");

    // Nor does a double click there ask for a track: the band under the
    // *last* header is where there is nothing to point at.
    let twice = Input {
        clicks: 2,
        ..input(&m, rect, len)
    };
    mt.press(on, &twice);
    assert_eq!(mt.lanes.len(), 2, "a curve row is not empty header space");
}

/// **The header's level is a knob, and a knob turns by a drag.** A header
/// is a narrow band and a groove long enough to be read takes the width the
/// name needs; a dial reads and turns in the space there actually is. The
/// press changes nothing -- a dial has no left and right end to put the
/// pointer between -- and the turn is measured from where it landed.
#[test]
fn the_headers_level_is_a_knob_and_turns_by_the_drag() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    mt.lanes[0].gain = 0.5;
    let inp = input(&m, rect, len);
    let at = mt.lane_rects(rect);
    let band = crate::host::timeline::gutter_band(at[0], 100.0);
    let parts = track::header_parts(band, &mt.header(&mt.lanes[0], 100.0), &m);
    let cell = parts.level.expect("the lane offers a level");
    assert!(
        (cell.w - cell.h).abs() < 1.0,
        "a square cell, like the toggles beside it"
    );

    let on = (
        f64::from(cell.x + cell.w * 0.5),
        f64::from(cell.y + cell.h * 0.5),
    );
    assert!(matches!(mt.press(on, &inp), Claim::Take(_)));
    assert_eq!(mt.lanes[0].gain, 0.5, "the press turns nothing");

    // Up raises, and by the distance travelled rather than to where the
    // pointer is.
    mt.drag((on.0, on.1 - f64::from(cell.h)), &inp);
    assert!(mt.lanes[0].gain > 0.5, "a turn upward raises it");
    let raised = mt.lanes[0].gain;
    mt.drag((on.0, on.1 + f64::from(cell.h)), &inp);
    assert!(mt.lanes[0].gain < raised, "and back down lowers it");
    // It reports as it goes, like every other control.
    assert!(!mt.lanes_event().into_messages().is_empty());
}

/// **A drag snaps to the edges of the boxes already on the lane**, which is
/// what makes two of them meetable at the sample: with no quantization a
/// hand never lands one box exactly where another ends, so `j` never had
/// two boxes to join. A hand that keeps pulling past the tolerance goes on
/// through and overlaps them, which is a crossfade and legal.
#[test]
fn a_drag_meets_the_box_beside_it_and_j_joins_the_two() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    // The two halves of a cut: `b` reads on from where `a` stops, which is
    // the other half of what a join needs.
    let mut mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 200, 0, "", 0, "b", "one", 500, 200, 200, "", 0]}"#,
    ));
    let inp = input(&m, rect, len);

    // Drag `b` back to *near* where `a` ends: near enough to mean it, and
    // not near enough for a hand to land it by aim.
    let from = xy(&mt, &m, rect, 550.0, len, 0);
    let to = xy(&mt, &m, rect, 253.0, len, 0);
    assert!(matches!(mt.press(from, &inp), Claim::Take(_)));
    mt.drag(to, &inp);
    assert_eq!(
        mt.clips[1].place.offset, 200.0,
        "it met the box beside it exactly"
    );

    // Pull well past the tolerance and it goes on through: an overlap is a
    // crossfade, not a refusal.
    let over = xy(&mt, &m, rect, 400.0, len, 0);
    mt.drag(over, &inp);
    assert_eq!(
        mt.clips[1].place.offset, 350.0,
        "the hand went on through: an overlap is a crossfade, not a refusal"
    );
    mt.release(over, true, &inp);

    // Meeting is what `j` needs: with the two touching, it asks for them.
    mt.clips[1].place.offset = 200.0;
    mt.selected = vec![0, 1];
    let mut clipboard = crate::host::clipboard::Clip::default();
    assert_eq!(
        joined(mt.key(
            &Key::Char('j'),
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: None,
            },
        )),
        vec!["a".to_string(), "b".to_string()],
        "it names the boxes in hand and leaves what they become to the owner"
    );
    assert_eq!(mt.clips.len(), 2, "and edits nothing itself");
}

// **What a join may be over is the material's question, and it moved.**
// Two boxes that read different runs of a source, two on two lanes, a hand
// holding one: the answers are
// `clausters_document::multitrack::picture::read_join`'s, which is the only
// place that knows what each box *reads* rather than what it is called. The
// tests that stood here asserted a second copy of them.

/// **A box is a window onto a source, and an edge stops where the source
/// does** -- unless the multitrack says the box wraps, where past the end is the
/// beginning again. The `loops` prop is that statement, a name set like
/// `hidden`, and what it decides is what a drag may do and what is drawn
/// under a box longer than its samples.
#[test]
fn a_box_that_loops_may_be_pulled_past_its_source_and_one_that_does_not_may_not() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 500, 0, "", 0, "b", "one", 500, 500, 0, "", 0],
            "loops": "a"}"#,
    ));
    assert!(mt.wraps("a"), "the multitrack said this one wraps");
    assert!(!mt.wraps("b"));
    assert!(mt.contents_of(0).looping);
    assert!(!mt.contents_of(1).looping);
    // Nobody loaded the samples here, so there is no length to stop at --
    // which is the silence an edge used to leave in every case.
    assert!(mt.contents_of(1).total.is_none());
}

/// **The trim grip stops blinking.** An edge drag takes the pointer off the
/// box it is resizing -- that is what pulling an edge is -- so a mark drawn
/// only where the pointer is disappeared under the hand that was using it.
#[test]
fn a_held_edge_draws_its_grip_wherever_the_pointer_went() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let inp = input(&m, rect, len);
    let (cr, ends) = {
        let (n, cr, local) = mt
            .boxes_on_screen(rect, 100.0, &m, inp.time)
            .into_iter()
            .find(|(n, ..)| *n == 0)
            .expect("the first box is on screen");
        assert_eq!(n, 0);
        (
            cr,
            track::clip_ends_on_screen(&local, mt.clips[0].place.dur),
        )
    };
    let ends_x = f64::from(cr.x + cr.w) - 1.0;
    let midy = f64::from(cr.y + cr.h * 0.5);

    // Nothing held: the pointer's own side, and nothing off the box.
    assert!(mt.lit_grip(0, cr, ends, &m, Some((ends_x, midy))).is_some());
    assert!(mt.lit_grip(0, cr, ends, &m, None).is_none());
    let outside = (ends_x + 40.0, midy + 80.0);
    assert!(mt.lit_grip(0, cr, ends, &m, Some(outside)).is_none());

    // Held: the edge in hand keeps its mark wherever the pointer got to.
    assert!(matches!(mt.press((ends_x, midy), &inp), Claim::Take(_)));
    assert_eq!(mt.grab.expect("an edge").part, Part::End);
    let (_, side) = mt
        .lit_grip(0, cr, ends, &m, Some(outside))
        .expect("the held edge is still lit");
    assert_eq!(side, track::ClipSide::End);
    // And the box that is *not* held draws nothing from someone else's grab.
    assert!(mt.lit_grip(1, cr, ends, &m, Some(outside)).is_none());
}

/// **The header is a surface, and the track is what it addresses.** A click
/// on the space beside its three controls selects that track -- the second
/// coordinate a paste needs, since the position cursor only says *when* --
/// a double click there makes one, and Delete takes the selected one away
/// with everything on it.
#[test]
fn the_header_selects_a_track_makes_one_and_delete_takes_it_away() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let inp = input(&m, rect, len);
    // The band beside the controls, on the second lane's row: the top of it,
    // which is where the name is drawn and no control is.
    let at = mt.lane_rects(rect);
    let beside = |i: usize| (f64::from(rect.x) + 4.0, f64::from(at[i].y) + 2.0);

    assert!(matches!(mt.press(beside(1), &inp), Claim::Take(_)));
    assert_eq!(
        mt.track,
        Some(1),
        "a click on the header points at the track"
    );

    // A double click on a header asks for a track, and it lands after the
    // one the hand was pointing at.
    let twice = Input {
        clicks: 2,
        ..input(&m, rect, len)
    };
    let Claim::Take(take) = mt.press(beside(0), &twice) else {
        panic!("the header takes it");
    };
    let msgs = take.events.into_messages();
    let args = msgs.first().expect("the rows as they now stand");
    assert_eq!(args[0], OscType::String("lanes".into()));
    assert_eq!(args.len(), 1 + 3 * 7, "three rows, seven fields each");
    assert_eq!(mt.lanes.len(), 3);
    assert_eq!(mt.lanes[1].name, "track 1", "a word, never an id");
    assert_eq!(mt.track, Some(1), "and the hand holds what it asked for");

    // Delete with a track in hand is the track's, and the boxes on it go
    // with it -- one payload, because the rows report is the multitrack's tracks
    // and a track that is not in it is gone with its contents.
    mt.track = Some(0);
    let mut clipboard = crate::host::clipboard::Clip::default();
    let events = mt
        .key(
            &Key::Delete,
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: None,
            },
        )
        .expect("a track in hand is what Delete acts on");
    let msgs = events.into_messages();
    assert_eq!(msgs[0][0], OscType::String("lanes".into()));
    assert_eq!(mt.lanes.len(), 2);
    assert!(
        !mt.clips.iter().any(|c| c.lane == "noise"),
        "and the boxes on it went with it"
    );
}

/// A double click on bare stack enters nothing: there is no box there, and
/// the press goes back to the container the way a single one does.
#[test]
fn a_double_press_off_the_boxes_still_declines() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let bare = xy(&mt, &m, rect, 800.0, len, 0);
    let twice = Input {
        clicks: 2,
        ..input(&m, rect, len)
    };
    assert!(matches!(mt.press(bare, &twice), Claim::Decline));
}

/// A `/gui_set` of what a query reported is the identity, for the curves as
/// for the two lists that were here before them.
#[test]
fn the_curves_read_back_as_they_were_reported() {
    let mt = curved();
    let reported: Map<String, Value> = mt.info().into_iter().collect();
    let mut echo = Multitrack::default();
    assert!(echo.set("lanes", &reported["lanes"]));
    assert!(echo.set("clips", &reported["clips"]));
    assert!(echo.set("curves", &reported["curves"]));
    assert!(echo.set("layers", &reported["layers"]));
    assert!(echo.set("points", &reported["points"]));
    assert_eq!(echo.curves, mt.curves);
    assert_eq!(echo.layers, mt.layers);
    assert_eq!(echo.points_json(), mt.points_json());
}

/// A curve that survives a new list keeps its points: replacing the
/// declarations is not an edit of what they hold.
#[test]
fn a_curve_that_survives_a_redeclaration_keeps_its_points() {
    let mut mt = curved();
    let before = mt.points_of("gain");
    assert!(mt.set(
        "curves",
        &Value::from(r#"["gain", "noise", "Gain", 0, 1, 60, "pan", "tone", "", -1, 1, 30]"#)
    ));
    assert_eq!(mt.points_of("gain"), before);
    assert_eq!(mt.curves[0].height, 60.0, "and takes its new row height");
    assert!(mt.bodies.contains_key("pan"), "the new one is built empty");
}

/// The report is the same list a `/gui_set clips` would take, so applying
/// what came back is the identity -- which is what makes the payload a
/// *state* rather than a description of a gesture.
#[test]
fn what_comes_back_is_what_a_set_would_take() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let from = xy(&mt, &m, rect, 250.0, len, 0);
    let to = xy(&mt, &m, rect, 350.0, len, 0);
    mt.press(from, &input(&m, rect, len));
    mt.drag(to, &input(&m, rect, len));
    let reported = mt.release(to, true, &input(&m, rect, len));

    let mut echo = multitrack();
    let msgs = reported.into_messages();
    let args = msgs.first().expect("an edit");
    assert!(echo.set("clips", &model::clips_json(&mt.clips)));
    assert_eq!(echo.clips, mt.clips);
    assert_eq!(args.len(), 1 + 7 * 2, "the tag, then a septuple per clip");
}
/// **A marquee catches the clips it covered, of every lane it crossed** -- a
/// selection the stack's sweep made is not one lane's. And it writes no
/// band: the second axis here is the stack, not a value.
#[test]
fn a_sweep_catches_what_it_covered_across_the_stack() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();

    let from = xy(&mt, &m, rect, 100.0, len, 0);
    let to = xy(&mt, &m, rect, 900.0, len, 1);
    let swept = mt.select_in(from, to, &input(&m, rect, len));
    assert_eq!(mt.selected.len(), 2, "one from each lane");
    assert!(swept.changed);
    assert!(swept.band.is_none(), "the stack is not a value axis");

    // A sweep over one lane's time only catches that lane's.
    let a = xy(&mt, &m, rect, 100.0, len, 0);
    let b = xy(&mt, &m, rect, 400.0, len, 0);
    mt.select_in(a, b, &input(&m, rect, len));
    assert_eq!(mt.selected, vec![0]);
}

/// **A click selects the box it landed on, alone.** A press is not yet a
/// gesture -- the same movement is a click or a drag depending on what
/// happens next -- so it is decided on release: a press that moved nothing
/// meant *this one*, and a hand that can point at a box is a hand that can
/// then place the cursor and split it.
#[test]
fn a_click_on_a_box_selects_that_one_and_nothing_leaves() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    mt.selected = vec![0, 1];

    // Press and release on the same pixel: the block is let go of and the
    // box under the hand is what is held.
    let on_a = xy(&mt, &m, rect, 250.0, len, 0);
    mt.press(on_a, &input(&m, rect, len));
    let events = mt.release(on_a, true, &input(&m, rect, len));
    assert_eq!(mt.selected, vec![0], "the one it landed on, alone");
    assert!(
        events.into_messages().is_empty(),
        "a selection is the hand's, not the document's"
    );
    assert_eq!(mt.clips[0].place.offset, 0.0, "and nothing moved");

    // A press that *did* move is a drag, and a drag reports the multitrack.
    let over = xy(&mt, &m, rect, 350.0, len, 0);
    mt.press(on_a, &input(&m, rect, len));
    mt.drag(over, &input(&m, rect, len));
    let events = mt.release(over, true, &input(&m, rect, len));
    assert!(!events.into_messages().is_empty(), "a move is an edit");
}

/// **A block travels rigidly, and grabbing an unselected clip lets go of
/// it** -- the hand that reached past its selection meant the box it reached
/// for.
#[test]
fn a_block_moves_as_one_and_an_unselected_grab_moves_alone() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    mt.selected = vec![0, 1];

    let from = xy(&mt, &m, rect, 250.0, len, 0);
    let to = xy(&mt, &m, rect, 350.0, len, 0);
    mt.press(from, &input(&m, rect, len));
    mt.drag(to, &input(&m, rect, len));
    mt.release(to, true, &input(&m, rect, len));
    assert_eq!(mt.clips[0].place.offset, 100.0);
    assert_eq!(mt.clips[1].place.offset, 600.0, "the other one came too");

    // Now grab the one that is *not* selected.
    let mut mt = multitrack();
    mt.selected = vec![1];
    let on_a = xy(&mt, &m, rect, 250.0, len, 0);
    let over = xy(&mt, &m, rect, 350.0, len, 0);
    mt.press(on_a, &input(&m, rect, len));
    mt.drag(over, &input(&m, rect, len));
    mt.release(over, true, &input(&m, rect, len));
    assert_eq!(mt.clips[0].place.offset, 100.0);
    assert_eq!(mt.clips[1].place.offset, 500.0, "it stayed where it was");
}

/// **The mixer is the second payload.** A fader and the two toggles report
/// `"lanes"` -- the lanes as they now stand -- and never the clips, which is
/// the whole reason the multitrack is written as two structures.
#[test]
fn the_header_reports_the_lanes_and_never_the_clips() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let at = mt.lane_rects(rect);
    let band = crate::host::timeline::gutter_band(at[0], 100.0);
    let parts = track::header_parts(band, &mt.header(&mt.lanes[0], 100.0), &m);

    let mute = parts.mute.expect("the lane offers a mute");
    let claim = mt.press(
        (
            f64::from(mute.x + mute.w / 2.0),
            f64::from(mute.y + mute.h / 2.0),
        ),
        &input(&m, rect, len),
    );
    let Claim::Take(take) = claim else {
        panic!("the header takes the press")
    };
    let msgs = take.events.into_messages();
    assert_eq!(msgs[0][0], OscType::String("lanes".into()));
    assert!(mt.lanes[0].mute, "and it flipped");
    assert_eq!(
        msgs[0].len(),
        1 + 7 * 2,
        "the tag, then seven fields per lane"
    );
}

/// `q` quantizes what the hand holds, across the stack, and Delete removes
/// it -- both reporting the clips as they now stand, which is the one
/// payload every edit here has.
#[test]
fn the_key_verbs_act_on_the_held_set_and_report_the_multitrack() {
    let mut mt = multitrack();
    mt.snap = 400.0;
    mt.selected = vec![0, 1];
    let mut clipboard = crate::host::clipboard::Clip::default();
    fn ki(clip: &mut crate::host::clipboard::Clip) -> KeyInput<'_> {
        KeyInput {
            mods: Mods::default(),
            clipboard: clip,
            cursor: None,
        }
    }

    let quantized = mt
        .key(&Key::Char('q'), &mut ki(&mut clipboard))
        .expect("something moved");
    assert_eq!(
        quantized.into_messages()[0][0],
        OscType::String("clips".into())
    );
    assert_eq!(mt.clips[1].place.offset, 400.0, "500 onto a 400 grid");

    let removed = mt
        .key(&Key::Delete, &mut ki(&mut clipboard))
        .expect("they went");
    assert_eq!(
        removed.into_messages()[0][0],
        OscType::String("clips".into())
    );
    assert!(mt.clips.is_empty());
    assert!(mt.selected.is_empty());

    // With nothing held, the keys are not this widget's.
    assert!(mt.key(&Key::Char('q'), &mut ki(&mut clipboard)).is_none());
}
/// **`e` cuts at the window's cursor and `j` asks for the two back.** The
/// window over the contents moves with the cut, so the second half reads on
/// from where the first stopped -- which is what makes the pair joinable,
/// and a fact about the *material*, so what they become is the document's
/// and this only names them.
#[test]
fn a_clip_splits_at_the_cursor_and_joins_back() {
    let mut mt = multitrack();
    let mut clipboard = crate::host::clipboard::Clip::default();
    fn ki(clip: &mut crate::host::clipboard::Clip, cursor: Option<f64>) -> KeyInput<'_> {
        KeyInput {
            mods: Mods::default(),
            clipboard: clip,
            cursor,
        }
    }
    mt.selected = vec![0];

    let cut = mt
        .key(&Key::Char('e'), &mut ki(&mut clipboard, Some(200.0)))
        .expect("it cut");
    assert_eq!(cut.into_messages()[0][0], OscType::String("clips".into()));
    assert_eq!(mt.clips.len(), 3);
    assert_eq!(mt.clips[0].place.dur, 200.0);
    let tail = mt.clips.last().expect("the second half");
    assert_eq!((tail.place.offset, tail.place.dur), (200.0, 300.0));
    assert_eq!(
        tail.place.start, 200.0,
        "it reads on rather than restarting"
    );
    assert_eq!(
        tail.name, "a 2",
        "a name the client never said, minted here"
    );
    assert_eq!(tail.lane, "noise", "and it stayed on its lane");

    // Both halves are in the hand, so `j` names both -- the one the client
    // said and the one this minted.
    assert_eq!(
        joined(mt.key(&Key::Char('j'), &mut ki(&mut clipboard, None))),
        vec!["a".to_string(), "a 2".to_string()]
    );
    assert_eq!(mt.clips.len(), 3, "and the picture waits for the answer");
}

/// **A correction does not empty the hand.** The client answers a name the
/// host minted with the whole picture, so a `clips` payload arrives right
/// after a split -- between the cut and the `j` that would put it back.
/// Clearing the selection there let the two halves go, and a join had
/// nothing to join.
#[test]
fn a_clips_correction_leaves_the_selection_where_it_was() {
    let mut mt = multitrack();
    mt.selected = vec![0, 1];
    assert!(mt.set(
        "clips",
        &Value::from(r#"["b", "tone", 500, 500, 0, "", 0, "a", "noise", 0, 500, 0, "", 0]"#)
    ));
    assert_eq!(
        mt.selected
            .iter()
            .map(|i| mt.clips[*i].name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"],
        "the same boxes, wherever the new list put them"
    );

    // And a box the picture no longer has is simply not held any more.
    assert!(mt.set("clips", &Value::from(r#"["a", "noise", 0, 500, 0, "", 0]"#)));
    assert_eq!(mt.selected, vec![0]);
}

/// **A split, the correction it draws, and then the join** *(found
/// 2026-09-12 by the user: "j para join no hace nada")*.
///
/// The whole round trip, which the two halves of it passed separately and
/// failed together. The host mints the tail's word (`a 2`) and the document
/// mints its id, so the correction that comes back calls the same box
/// something else -- and a hand re-found by **name** alone let exactly that
/// box go. One box held is nothing to join, and a join that joins nothing
/// said nothing, so the key looked dead.
#[test]
fn a_split_survives_the_correction_that_renames_its_half_and_joins() {
    let mut mt = multitrack();
    let mut clipboard = crate::host::clipboard::Clip::default();
    mt.selected = vec![0];
    mt.key(
        &Key::Char('e'),
        &mut KeyInput {
            mods: Mods::default(),
            clipboard: &mut clipboard,
            cursor: Some(200.0),
        },
    )
    .expect("it cut");
    assert_eq!(mt.selected.len(), 2, "both halves stay in the hand");

    // The client applied the split, the multitrack minted an id for the half,
    // and the whole picture comes back with that id in place of `a 2`.
    assert!(mt.set(
        "clips",
        &Value::from(
            r#"["a", "noise", 0, 200, 0, "", 0, "20", "noise", 200, 300, 200, "", 0,
                "b", "tone", 500, 500, 0, "", 0]"#
        )
    ));
    assert_eq!(mt.selected.len(), 2, "and the hand still holds both");
    assert!(
        mt.selected.iter().any(|i| mt.clips[*i].name == "20"),
        "the half is held under the name the multitrack kept"
    );

    // So `j` names both -- the head under the name it kept and the half
    // under the one the multitrack gave it, which is the whole point: a hand
    // re-found by name alone would have asked for a join of one box.
    let mut named = joined(mt.key(
        &Key::Char('j'),
        &mut KeyInput {
            mods: Mods::default(),
            clipboard: &mut clipboard,
            cursor: None,
        },
    ));
    named.sort();
    assert_eq!(named, vec!["20".to_string(), "a".to_string()]);
}

/// **A verb that acts on nothing says why** *(found 2026-09-12 by the user,
/// the second report of the same silence: "toco ctrl-j pero no hace nada.
/// Siguen separados")*.
///
/// A verb that finds nothing to do used to return `false`, and `false`
/// reaches nobody: the key looked dead, which is how a correct refusal
/// teaches "it sometimes does not work". `q` and `e` are the two whose
/// answer is the *picture's* and so is decided here; `j`'s is the
/// material's, and it refuses through the document with the same word
/// (`clausters_document::multitrack::picture::read_join`).
#[test]
fn a_verb_that_acts_on_nothing_says_why_rather_than_nothing() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut press = |mt: &mut Multitrack, k: char| {
        refusal(mt.key(
            &Key::Char(k),
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: Some(200.0),
            },
        ))
    };

    let mut mt = multitrack();
    mt.selected = vec![0];
    assert_eq!(
        press(&mut mt, 'q'),
        Some("these boxes are already on the grid".to_string()),
    );
    assert_eq!(
        press(&mut mt, 'e'),
        None,
        "the cursor is inside the held box, so this one cuts"
    );
    assert_eq!(
        press(&mut mt, 'e'),
        Some("the cursor is not inside a held box".to_string()),
        "and cutting at the seam again has nothing to cut"
    );
}

/// **A cut half is a clip like any other, and it changes lanes.** The old
/// projection stopped emitting lane changes once a split had happened,
/// because the half was a box the view had minted and the gesture state
/// still named the lane the press had captured. Here the halves are clips
/// on the stack the drag reads, so the second one drags across like the
/// first.
#[test]
fn a_half_left_by_a_split_still_changes_lanes() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let mut clipboard = crate::host::clipboard::Clip::default();
    mt.selected = vec![0];
    mt.key(
        &Key::Char('e'),
        &mut KeyInput {
            mods: Mods::default(),
            clipboard: &mut clipboard,
            cursor: Some(200.0),
        },
    )
    .expect("it cut");

    let from = xy(&mt, &m, rect, 350.0, len, 0);
    let to = xy(&mt, &m, rect, 350.0, len, 1);
    mt.press(from, &input(&m, rect, len));
    mt.drag(to, &input(&m, rect, len));
    mt.release(to, true, &input(&m, rect, len));

    let half = mt
        .clips
        .iter()
        .find(|c| c.name == "a 2")
        .expect("the second half");
    assert_eq!(half.lane, "tone", "the half went to the lane under it");
    assert_eq!(half.place.offset, 200.0, "and it did not move in time");
}

/// The boxes a `join` report names, in the order it names them.
fn joined(events: Option<Events>) -> Vec<String> {
    events
        .map(Events::into_messages)
        .and_then(|m| m.into_iter().next())
        .filter(|msg| msg.first() == Some(&OscType::String("join".into())))
        .map(|msg| {
            msg.into_iter()
                .skip(1)
                .filter_map(|arg| match arg {
                    OscType::String(s) => Some(s),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The reason a verb gave for refusing, or `None` when it did the thing.
/// A refusal is an ordinary event -- `"refused" <verb> <why>` -- so this is
/// how a test reads what the status bar will draw.
fn refusal(events: Option<Events>) -> Option<String> {
    let msg = events?.into_messages().into_iter().next()?;
    (msg.first() == Some(&OscType::String("refused".into()))).then(|| match msg.get(2) {
        Some(OscType::String(s)) => s.clone(),
        _ => String::new(),
    })
}

/// **A block travels through the clipboard keeping its own shape**: the
/// earliest lands on the cursor and the rest keep their distances, which is
/// what makes a pasted block the same block.
#[test]
fn a_block_is_cut_and_pasted_at_the_cursor_with_its_shape() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut mt = multitrack();
    mt.selected = vec![0, 1];
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };

    let cut = mt
        .key(
            &Key::Char('x'),
            &mut KeyInput {
                mods: ctrl,
                clipboard: &mut clipboard,
                cursor: None,
            },
        )
        .expect("they left");
    assert_eq!(cut.into_messages()[0][0], OscType::String("clips".into()));
    assert!(mt.clips.is_empty());

    let pasted = mt
        .key(
            &Key::Char('v'),
            &mut KeyInput {
                mods: ctrl,
                clipboard: &mut clipboard,
                cursor: Some(100.0),
            },
        )
        .expect("they came back");
    assert_eq!(
        pasted.into_messages()[0][0],
        OscType::String("clips".into())
    );
    assert_eq!(mt.clips.len(), 2);
    assert_eq!(
        mt.clips[0].place.offset, 100.0,
        "the earliest on the cursor"
    );
    assert_eq!(mt.clips[1].place.offset, 600.0, "and the shape kept");
    assert_eq!(
        mt.clips[0].lane, "noise",
        "each on the lane it was cut from"
    );
    assert_eq!(mt.clips[1].lane, "tone");
}
/// **A drag over the gap between two lanes must not jump.** The pointer
/// crosses pixels no lane is drawn on, and a hit test that answers
/// "nowhere" there makes the block snap back to the row the press found --
/// for those frames only, so it flickers, and it jumps two rows at once
/// when the gap is not the one it started beside. That is the glitch the
/// window's own edges had, twice.
///
/// The rule is `graphics::multitrack::bands`', and it is `gestures/nav.rs`'
/// for the widget-tree stack: **a gap belongs to the lane above it**, so a
/// pointer between two lanes is on one rather than on nothing.
#[test]
fn a_drag_through_the_gap_between_lanes_does_not_jump() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 420.0);
    let len = 1000.0;
    let mut mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1, "two", "", 100, 0, 0, 1, 1, "three", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 500, 0, "", 0]}"#,
    ));
    mt.gap = 8.0;

    let at = mt.lane_rects(rect);
    let from = xy(&mt, &m, rect, 250.0, len, 0);
    mt.press(from, &input(&m, rect, len));

    // The gap between the **second** and the third lane: two rows from
    // where the press was, so answering "nowhere" reads as the origin and
    // jumps two rows rather than none.
    let in_gap = f64::from(at[1].y + at[1].h + mt.gap / 2.0);
    mt.drag((from.0, in_gap), &input(&m, rect, len));
    assert_eq!(mt.clips[0].lane, "two", "the gap is the lane above's");

    // On into the third, and back through the gap: one step each way, and
    // never a return to where the press was.
    mt.drag(xy(&mt, &m, rect, 250.0, len, 2), &input(&m, rect, len));
    assert_eq!(mt.clips[0].lane, "three");
    mt.drag((from.0, in_gap), &input(&m, rect, len));
    assert_eq!(mt.clips[0].lane, "two", "and not back to \"one\"");
}

/// **Held past the end of the stack, a block stops rather than folding.**
/// The continuous index is clamped, so a hand dragged off the bottom leaves
/// the clip on the last lane instead of oscillating back to the first.
#[test]
fn a_drag_past_the_last_lane_stops_at_it() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 320.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let from = xy(&mt, &m, rect, 250.0, len, 0);

    mt.press(from, &input(&m, rect, len));
    mt.drag((from.0, 10_000.0), &input(&m, rect, len));
    assert_eq!(mt.clips[0].lane, "tone", "the last one, not the first");
    mt.drag((from.0, -10_000.0), &input(&m, rect, len));
    assert_eq!(
        mt.clips[0].lane, "noise",
        "and the first going the other way"
    );
}

/// **A drag must never read an axis it is moving.** A widget on no
/// navigation group rules itself by its own extent, and a drag *changes*
/// the extent -- so re-deriving the axis per frame stretches the
/// pixel-to-time map under the hand, the next step reads further out, and
/// the box accelerates away from the pointer.
///
/// Measured before the fix: 40, 80, 400, 1600, **8675** for equal steps.
/// It is the same miscalculation the window's own edges had.
#[test]
fn a_drag_off_a_group_reads_the_axis_the_press_found() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();
    let mut bare = input(&m, rect, len);
    bare.time = None;

    let from = xy(&mt, &m, rect, 250.0, len, 0);
    mt.press(from, &bare);
    let mut seen = Vec::new();
    for step in [1.0, 2.0, 10.0, 40.0, 100.0] {
        mt.drag((from.0 + step * 20.0, from.1), &bare);
        seen.push((step, mt.clips[0].place.offset));
    }
    // Equal ratios of travel give equal ratios of offset: the map held
    // still even as the multitrack grew under it.
    let (first_step, first) = seen[0];
    for (step, offset) in &seen {
        let want = first * step / first_step;
        assert!(
            (offset - want).abs() < 1.0,
            "step {step}: {offset} against {want} -- the axis moved"
        );
    }
    assert!(
        model::extent(&mt.clips) > len,
        "and the multitrack did grow"
    );
}
/// **A grip is hit on the pixels it is drawn on.** The press asks the same
/// call the drawing made, so the handle and its hit area cannot disagree --
/// which is exactly the case nobody tests.
#[test]
fn an_edge_is_grabbed_by_the_grip_that_is_drawn_there() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mt = multitrack();

    let body = track::lane_body(mt.lane_rects(rect)[0], false, 100.0, &m);
    let nav = View { start: 0.0, len };
    let (x0, x1) = stack::clip_x(&mt.clips[0], body, &nav, MIN_CLIP_W).expect("on screen");
    let cr = track::clip_rect(body, x0, x1);
    let local = track::clip_local_view(
        body,
        &nav,
        mt.clips[0].place.offset,
        mt.clips[0].place.dur,
        cr,
    );
    let ends = track::clip_ends_on_screen(&local, mt.clips[0].place.dur);
    let (left, right) = track::clip_grips(cr, ends, &m);
    let left = left.expect("its start is on screen");
    let right = right.expect("and so is its end");

    let mid_y = f64::from(cr.y + cr.h / 2.0);
    let on = |r: Rect| (f64::from(r.x + r.w / 2.0), mid_y);
    assert_eq!(
        mt.clip_at(&input(&m, rect, len), on(left)),
        Some((0, Part::Start))
    );
    assert_eq!(
        mt.clip_at(&input(&m, rect, len), on(right)),
        Some((0, Part::End))
    );
    // And the middle is the body, which is what moves it.
    let middle = (f64::from(cr.x + cr.w / 2.0), mid_y);
    assert_eq!(
        mt.clip_at(&input(&m, rect, len), middle),
        Some((0, Part::Body))
    );
}
/// **A take is asked for once, and again when it is forgotten.** A front
/// asks on every repaint; asking for every take each time re-mapped and
/// re-summarized all of them per frame (found 2026-09-13, reading the walk).
/// A box over a new buffer is asked for on the next repaint, and a take whose
/// samples were just made is asked for again after the host forgets it.
#[test]
fn a_take_is_asked_for_once_and_again_when_forgotten() {
    let mut mt = multitrack();
    assert_eq!(mt.ask_takes(true), vec![0], "a window being built asks all");
    assert!(
        mt.ask_takes(false).is_empty(),
        "a repaint asks nothing more"
    );
    assert_eq!(mt.ask_takes(true), vec![0], "a rebuild still asks all");

    // A join names a new buffer: the next repaint asks for it and no other.
    let joined = from_props(&props(
        r#"{"lanes": ["noise", "", 100, 0, 0, 1, 1],
            "clips": ["a", "noise", 0, 500, 0, "", 0, "j", "noise", 500, 500, 0, "", 3]}"#,
    ));
    mt.clips = joined.clips;
    assert_eq!(mt.ask_takes(false), vec![3]);
    assert!(mt.ask_takes(false).is_empty());

    // Its stitch is done: forgotten, it is asked for once more.
    assert!(mt.forget_take(3), "it had been asked for");
    assert!(!mt.forget_take(7), "a take it never asked for is not its");
    assert_eq!(mt.ask_takes(false), vec![3]);
    assert!(mt.ask_takes(false).is_empty());
}

/// **Buffer 0 is a buffer.** It is the first one an allocator hands out, so
/// a zero sentinel would make the first take a script loads the one take it
/// cannot draw -- which is exactly how this was found, by eye, on the first
/// clip of a multitrack.
#[test]
fn a_clip_over_buffer_zero_has_a_source() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 10, 0, "", 0,
                      "b", "one", 20, 10, 0, "", -1]}"#,
    ));
    assert_eq!(mt.clips[0].source, 0, "a window onto buffer 0");
    assert_eq!(mt.clips[1].source, model::NO_SOURCE, "and one onto nothing");
    assert_eq!(mt.needs().takes, vec![0], "so exactly one take is fetched");

    // A clip written with no source at all is a window onto nothing, not
    // onto buffer 0.
    let bare = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1], "clips": ["a", "one", 0, 10, 0, ""]}"#,
    ));
    assert!(bare.clips.is_empty(), "six fields is a partial septuple");
}
/// **A lane's header is a gutter the group reserves.** It is asked of the
/// element and stamped by the layout as the widest wish on the axis, so a
/// ruler stacked with these lanes starts its ticks over the same sample.
/// Answering zero is a stack with no names and no controls on it.
#[test]
fn the_lanes_ask_for_the_band_their_headers_need() {
    let m = Metrics::default();
    let mt = multitrack();
    assert!(
        mt.gutter(&m) >= m.header_w,
        "a header carrying a name, two toggles and a fader is at least the role"
    );
}

/// **A swept line is a picture driven by the clock**, so the window has to
/// be told: an anchored playhead moves with no message, and nothing else in
/// a multitrack would ask for the frame it moves on.
#[test]
fn an_anchored_playhead_asks_the_window_for_frames() {
    let mut mt = multitrack();
    assert!(!mt.needs().clock, "a stopped transport drives nothing");
    mt.set("playhead_at", &Value::from(0.0));
    assert!(mt.needs().clock, "and an anchored one drives the window");
}
/// **A held grip is shown because it is held.** The edge moves under the
/// hand, so asking where the pointer is each frame makes the mark blink as
/// the box catches up with it -- which is what a trim looked like.
#[test]
fn the_grip_a_drag_is_holding_does_not_depend_on_the_pointer() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    let mut mt = multitrack();

    let body = track::lane_body(mt.lane_rects(rect)[0], false, 100.0, &m);
    let nav = View { start: 0.0, len };
    let (x0, x1) = stack::clip_x(&mt.clips[0], body, &nav, MIN_CLIP_W).expect("on screen");
    let cr = track::clip_rect(body, x0, x1);
    let (_, right) = {
        let local = track::clip_local_view(
            body,
            &nav,
            mt.clips[0].place.offset,
            mt.clips[0].place.dur,
            cr,
        );
        let ends = track::clip_ends_on_screen(&local, mt.clips[0].place.dur);
        track::clip_grips(cr, ends, &m)
    };
    let right = right.expect("its end is on screen");
    let on_grip = (
        f64::from(right.x + right.w / 2.0),
        f64::from(cr.y + cr.h / 2.0),
    );

    mt.press(on_grip, &input(&m, rect, len));
    assert!(matches!(mt.grab.map(|g| g.part), Some(Part::End)));
    // Pull it well left: the edge is now nowhere near where the press was,
    // and the drag still knows which side it took.
    mt.drag((on_grip.0 - 60.0, on_grip.1), &input(&m, rect, len));
    assert!(mt.clips[0].place.dur < 500.0);
    assert!(
        matches!(mt.grab.map(|g| g.part), Some(Part::End)),
        "the side is the drag's, not the pointer's"
    );
}

/// **A join asks for the takes it reads, not for itself** -- and draws from
/// them -- while a box that is not a join asks for its own buffer as always.
#[test]
fn a_join_asks_for_the_takes_its_spans_read_and_not_for_its_own_buffer() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1.0, 1],
            "clips": ["join", "one", 0, 400, 0, "", 9,
                      "plain", "one", 400, 100, 0, "", 5],
            "segments": ["join", 7, 200, 200, 1, "join", 7, 0, 200, 1]}"#,
    ));
    assert_eq!(
        mt.needs().takes,
        vec![5, 7],
        "the join's spans read buffer 7, and buffer 9 is never fetched"
    );
    let spans = &mt.segments["join"];
    assert_eq!(spans.len(), 2, "in the order they play");
    assert_eq!(
        (spans[0].source, spans[0].start, spans[0].frames),
        (7, 200.0, 200.0)
    );
    assert_eq!(
        (spans[1].source, spans[1].start, spans[1].frames),
        (7, 0.0, 200.0)
    );
}

/// **A span that reads nothing is dropped**, and a box whose spans are all
/// dropped is not named, so it is drawn from its own buffer.
#[test]
fn a_span_that_reads_nothing_is_dropped() {
    let mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1.0, 1],
            "clips": ["join", "one", 0, 400, 0, "", 9],
            "segments": ["join", -1, 0, 200, 1, "join", 7, 0, 0, 1, "join", 7]}"#,
    ));
    assert!(mt.segments.is_empty());
    assert_eq!(mt.needs().takes, vec![9], "back to the box's own buffer");
}

/// **A join stops at its spans.** A join's own buffer is never fetched -- it is
/// drawn from the takes its spans read -- so a bound asked of the takes found
/// nothing for it, and an edge drag pulled a join past the end of its samples
/// while every other box stopped at its last frame. Its length is the sum of
/// its spans, known before any sample has arrived.
#[test]
fn a_join_is_trimmed_no_further_than_its_spans() {
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 1000.0;
    // Two 300-frame spans of take 0; the join's own buffer, 5, is never asked for.
    let mut mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 400, 0, "", 5],
            "segments": ["a", 0, 0, 300, 1, "a", 0, 300, 300, 1]}"#,
    ));
    assert_eq!(
        mt.needs().takes,
        vec![0],
        "a join asks for the takes it reads"
    );
    assert_eq!(mt.contents_of(0).total, Some(600.0));

    let edge = xy(&mt, &m, rect, 400.0, len, 0);
    let far = xy(&mt, &m, rect, 900.0, len, 0);
    assert!(matches!(
        mt.press(edge, &input(&m, rect, len)),
        Claim::Take(_)
    ));
    mt.drag(far, &input(&m, rect, len));
    mt.release(far, true, &input(&m, rect, len));
    assert_eq!(
        mt.clips[0].place.dur, 600.0,
        "the edge stops where the spans end"
    );
}

/// **A box whose source was written at another rate is bounded in its own
/// samples.** The box is placed and trimmed on the multitrack's axis and the
/// take is counted in its own frames, and an edge that compared the two
/// directly stopped where neither is: a 44.1 kHz take on a 48 kHz multitrack
/// reaches 8.8% further than its frame count says.
#[test]
fn an_edge_stops_where_the_samples_do_at_the_sources_own_rate() {
    use crate::host::widget::element::Loaded;
    let m = Metrics::default();
    let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
    let len = 100_000.0;
    // 44100 frames of a 44.1 kHz take, drawn on a 48 kHz axis: 0.91875 frames
    // of source per sample of box, so the whole take is 48000 samples long.
    let mut mt = from_props(&props(
        r#"{"lanes": ["one", "", 100, 0, 0, 1, 1],
            "clips": ["a", "one", 0, 20000, 0, "", 0],
            "rates": ["a", 0.91875]}"#,
    ));
    mt.bulk_of(
        0,
        Loaded::Raw {
            samples: vec![0.25; 44_100],
            channels: 1,
        },
    );
    let contents = mt.contents_of(0);
    assert_eq!(contents.total, Some(44_100.0), "the take's own frames");
    assert!(
        (contents.rate - 0.91875).abs() < 1e-9,
        "and the box's own rate"
    );

    let edge = xy(&mt, &m, rect, 20_000.0, len, 0);
    let far = xy(&mt, &m, rect, 90_000.0, len, 0);
    assert!(matches!(
        mt.press(edge, &input(&m, rect, len)),
        Claim::Take(_)
    ));
    mt.drag(far, &input(&m, rect, len));
    mt.release(far, true, &input(&m, rect, len));
    let dur = mt.clips[0].place.dur;
    assert!(
        (dur - 48_000.0).abs() < 1.0,
        "the whole take, in the multitrack's samples: {dur}"
    );
}
