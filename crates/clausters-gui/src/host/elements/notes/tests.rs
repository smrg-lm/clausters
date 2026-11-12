//! The roll's tests: its picture, its gestures and what an edit of it
//! sends back.

use super::*;
use crate::host::widget::element::Mods;

fn props(json: &str) -> Map<String, Value> {
    serde_json::from_str(json).unwrap()
}

fn roll(json: &str) -> Notes {
    from_props(&props(json))
}

/// **A clip's notes are read through its window**, as a take is: a note
/// is a frame of the sequence, so a clip stretched from the front -- its
/// window's start pulled back with its edge -- draws every note where it
/// was, and a note before the window is none of the clip's.
#[test]
fn a_clip_s_notes_stand_where_they_were_played_whatever_its_window() {
    use crate::host::widget::SourceWindow;
    use crate::host::widget::element::TimeSpace;
    use clausters_gui_draw::draw::Draw;
    use clausters_gui_draw::metrics::Metrics;
    use clausters_gui_draw::paint::Mesh;
    use clausters_gui_draw::theme::Theme;

    let body = roll(
        r#"{"notes": [0, 4800, 60, 100, 0, 4800, 2400, 64, 100, 0],
                "min": 48, "max": 72, "ruler": "off", "osc_markers": false}"#,
    );
    let rect = Rect::new(0.0, 0.0, 960.0, 100.0);
    // the left edge of each bar the body draws, past the pitch labels
    let lefts = |start: f64| {
        let mut mesh = Mesh::new();
        let (m, theme) = (Metrics::default(), Theme::default());
        let time = TimeSpace::of(
            View {
                start: 0.0,
                len: 9600.0,
            },
            9600.0,
        )
        .with_window(SourceWindow {
            start,
            ..SourceWindow::default()
        });
        body.draw_body(&mut Draw::new(&mut mesh, &m, &theme), rect, &time);
        let mut xs: Vec<i32> = mesh
            .positions()
            .map(|(x, _)| x.round() as i32)
            .filter(|x| *x >= 100)
            .collect();
        xs.sort_unstable();
        xs.dedup();
        xs
    };
    // a clip over the sequence from its start: the second note at 480
    assert!(lefts(0.0).contains(&480));
    // stretched a half from the front: the same notes, a half later
    let stretched = lefts(-4800.0);
    assert_eq!(stretched.first(), Some(&480), "{stretched:?}");
    assert!(stretched.contains(&960) || stretched.contains(&959));
    // trimmed a quarter in: the first note starts before the window and
    // is not the clip's, the second stands a quarter earlier
    let trimmed = lefts(2400.0);
    assert_eq!(trimmed.first(), Some(&240), "{trimmed:?}");
}

/// **The notes a script marks are the ones a query names**, by the ids
/// the roll was handed -- the objects a script holds -- and by index on a
/// roll handed none.
#[test]
fn a_selection_is_set_and_queried_by_the_notes_ids() {
    let selected = |r: &Notes| -> Value {
        let (_, v) = r.info().into_iter().find(|(k, _)| k == "selected").unwrap();
        serde_json::from_str(v.as_str().unwrap()).unwrap()
    };
    let mut r = roll(
        r#"{"notes": [0, 1, 60, 100, 0, 1, 1, 62, 100, 0, 2, 1, 64, 100, 0],
                "note_ids": [7, 8, 9]}"#,
    );
    assert_eq!(selected(&r), serde_json::json!([]));
    assert!(r.set("selected", &Value::from("[9, 7]")));
    assert_eq!(
        selected(&r),
        serde_json::json!([7, 9]),
        "in the roll's order"
    );
    assert!(r.set("selected", &Value::from("[]")));
    assert_eq!(selected(&r), serde_json::json!([]));
    let mut bare = roll(r#"{"notes": [0, 1, 60, 100, 0, 1, 1, 62, 100, 0]}"#);
    assert!(bare.set("selected", &Value::from("[1]")));
    assert_eq!(
        selected(&bare),
        serde_json::json!([1]),
        "by index without ids"
    );
}

/// **A roll asks the window to follow the clock only while something is
/// sweeping.** It asked for it unconditionally once, and a page holding a
/// roll then repainted thirty times a second for as long as it was open,
/// with nothing moving in it -- 59% of a browser's main thread on the
/// composer example, against 3% once the anchor decides. The rule is the
/// `score`'s: the element carrying the anchor is the one that can answer.
#[test]
fn a_roll_follows_the_clock_only_while_a_playhead_is_anchored() {
    let idle = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0]}"#);
    assert!(
        !idle.needs().clock,
        "a roll with nothing sweeping in it keeps no window awake"
    );
    let sweeping = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"playhead_at":480.0}"#);
    assert!(
        sweeping.needs().clock,
        "an anchored roll does: the line has to move"
    );
}

/// A roll placed on a navigation group: the indent is its keyboard gutter,
/// the axis is the group's window over its content.
fn input<'a>(m: &'a Metrics, rect: Rect, time: Option<TimeSpace>) -> Input<'a> {
    Input {
        metrics: m,
        rect,
        indent: pianoroll::KEYBOARD_W,
        scale: 1.0,
        mods: Mods::default(),
        viewport: (rect.w, rect.h),
        clicks: 1,
        time,
    }
}

fn axis(len: f64) -> Option<TimeSpace> {
    Some(TimeSpace::of(View { start: 0.0, len }, len))
}

fn rect() -> Rect {
    Rect::new(0.0, 0.0, 500.0, 400.0)
}

/// The x pixel a time falls on in the grid -- what the drawing maps and what
/// a press has to invert.
fn x_of(r: &Notes, m: &Metrics, t: f64, len: f64) -> f64 {
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, m).grid;
    grid.x as f64 + t / len * grid.w as f64
}

fn y_of(r: &Notes, m: &Metrics, pitch: f32) -> f64 {
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, m).grid;
    let (lo, hi) = r.pitch_window();
    pianoroll::pitch_to_y(pitch, lo, hi, grid) as f64
}

#[test]
fn parses_defaults_and_the_wire_lists() {
    let r = roll("{}");
    assert_eq!((r.min, r.max), (PITCH_MIN, PITCH_MAX));
    assert!(!r.osc_markers && !r.midi_in && !r.ids);
    assert_eq!(r.snap, 0.0);
    assert!(r.notes.is_empty() && r.osc.is_empty());

    // The canonical quintuple form, and the OSC markers' strip opening because
    // there are markers.
    let r = roll(
        r#"{"notes":[0.0,100.0,60.0,90,2],"osc":[50.0,"hit"],
                "min":48,"max":72,"snap":25.0,"midi_in":true}"#,
    );
    assert_eq!(r.notes.len(), 1);
    assert_eq!((r.notes[0].velocity, r.notes[0].channel), (90, 2));
    assert_eq!(r.osc.len(), 1);
    // The same note written as floats, as the catalogue serializes it.
    let floats = roll(r#"{"notes":[0.0,100.0,60.0,90.0,2.0]}"#);
    assert_eq!(
        (floats.notes[0].velocity, floats.notes[0].channel),
        (90, 2),
        "a float velocity is the same velocity"
    );
    assert!(r.osc_markers, "markers open their lane");
    assert!(r.midi_in);
    assert_eq!(r.snap, 25.0);
}

/// A `/gui_set` of a list rides as its JSON string and drops the selection,
/// whose indices would dangle over the new list.
#[test]
fn apply_replaces_the_lists_and_drops_the_selection() {
    let mut r = roll(r#"{"notes":[0.0,10.0,60.0,100,0]}"#);
    r.selected = vec![0];
    assert!(r.set(
        "notes",
        &Value::from("[0.0,10.0,64.0,100,0,20.0,10.0,67.0,100,0]")
    ));
    assert_eq!(r.notes.len(), 2);
    assert!(r.selected.is_empty());
    assert!(r.set("osc", &Value::from("[5.0,\"a\"]")));
    assert_eq!(r.osc.len(), 1);
    assert!(r.set("snap", &Value::from(50.0)));
    // The editor chrome is the element's too, so its keys apply here.
    assert!(r.set("ruler", &Value::from("beats")));
    assert!(!r.set("nonesuch", &Value::from(1)));
}

/// The extent a group's axis reads off a roll is its content's, which is
/// what lets a roll being written into lengthen the timeline.
#[test]
fn the_content_span_is_the_end_of_the_last_thing_on_it() {
    let r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"osc":[400.0,""]}"#);
    assert_eq!(r.content_span(), Some(400.0));
    let r = roll(r#"{"notes":[100.0,300.0,60.0,100,0]}"#);
    assert_eq!(r.content_span(), Some(400.0));
    assert_eq!(roll("{}").content_span(), Some(0.0));
}

/// A note is grabbed by the pixels it is drawn on, and the drag reports the
/// whole list in the owner's own units.
#[test]
fn a_note_drag_moves_it_in_time_and_pitch_and_reports_the_list() {
    let m = Metrics::default();
    let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"min":48,"max":72}"#);
    let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
    assert!(matches!(
        r.press(at, &input(&m, rect(), axis(1000.0))),
        Claim::Take(_)
    ));
    assert!(matches!(r.drag, Some(Drag::Note { index: 0, .. })));

    let to = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 64.0));
    let events = r.drag(to, &input(&m, rect(), axis(1000.0)));
    assert!((r.notes[0].start - 200.0).abs() < 1.0, "{:?}", r.notes[0]);
    assert_eq!(r.notes[0].pitch, 64.0);
    assert_eq!(r.notes[0].dur, 100.0, "a move keeps the duration");
    // The note follows the hand and says nothing: one gesture is one edit.
    assert!(events.is_empty(), "a frame of a drag is not an edit");

    // The release is the edit, and it carries the whole list.
    let msgs = r
        .release(to, true, &input(&m, rect(), axis(1000.0)))
        .into_messages();
    assert_eq!(msgs[0][0], OscType::String("notes".into()));
    assert_eq!(msgs[0].len(), 1 + 5, "the tag plus a quintuple per note");
    assert!(r.drag.is_none());
}

/// **The velocity lane, one lane at a time**: a bar dragged up raises its
/// note's velocity, and a drag across the empty lane is a ramp over the
/// notes it crosses; each is one `notes` report at the release. A curve
/// removed names the lane shown.
#[test]
fn a_velocity_bar_is_dragged_and_a_ramp_drawn_across_the_lane() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,64,0, 300.0,100.0,62.0,64,0, 600.0,100.0,64.0,64,0],
                "min":48,"max":72,"lane":"velocity","note_ids":[1,2,3],"joins":[],
                "curves":["9","CC 1",0,127,40]}"#,
    );
    let input = input(&m, rect(), axis(1000.0));
    let (_, body) = r.velocity_rects(rect(), pianoroll::KEYBOARD_W, &m).unwrap();
    assert!(r.shown_rows().is_empty(), "the lane is the velocities");
    let xs = [0.0, 250.0, 650.0].map(|t| x_of(&r, &m, t, 1000.0));
    let x = |t: f64| xs[[0.0, 250.0, 650.0].iter().position(|w| *w == t).unwrap()];
    let y_of_velocity = |v: f64| f64::from(body.y) + (1.0 - v / 127.0) * f64::from(body.h);
    // a bar: pressed where it is and dragged to the top
    let press = (x(0.0), y_of_velocity(64.0));
    assert!(matches!(r.press(press, &input), Claim::Take(_)));
    assert!(matches!(r.drag, Some(Drag::Bars { .. })));
    r.drag((press.0, f64::from(body.y)), &input);
    assert_eq!(r.notes[0].velocity, 127);
    assert_eq!(r.notes[1].velocity, 64, "another bar stays");
    let msgs = r
        .release((press.0, f64::from(body.y)), true, &input)
        .into_messages();
    assert_eq!(msgs[0][0], OscType::String("notes".into()));
    // a ramp from the empty lane before the second note to past the third
    let from = (x(250.0), y_of_velocity(20.0));
    assert!(matches!(r.press(from, &input), Claim::Take(_)));
    assert!(matches!(r.drag, Some(Drag::Ramp { .. })));
    let to = (x(650.0), y_of_velocity(120.0));
    r.drag(to, &input);
    let (second, third) = (r.notes[1].velocity, r.notes[2].velocity);
    assert!(
        second > 20 && second < third && third <= 120,
        "{second} {third}"
    );
    assert_eq!(r.notes[0].velocity, 127, "outside the ramp");
    assert_eq!(r.release(to, true, &input).into_messages().len(), 1);
    // no curve is shown, so none is removed
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut keys = KeyInput {
        mods: Mods::default(),
        clipboard: &mut clipboard,
        cursor: None,
    };
    let refused = r
        .command("curve_remove", &mut keys)
        .unwrap()
        .into_messages();
    assert_eq!(refused[0][0], OscType::String("refused".into()));
    assert!(r.set("lane", &Value::from("9")));
    let named = r
        .command("curve_remove", &mut keys)
        .unwrap()
        .into_messages();
    assert_eq!(named[0][1], OscType::String("9".into()));
}

/// **The lane's tabs sit right over the lane**: the plane gives up their
/// row, the velocities start where the tabs end, and a press on a tab
/// asks the owner for its lane by name, editing nothing.
#[test]
fn the_lane_tabs_sit_over_the_lane_and_a_press_asks_for_one() {
    let m = Metrics::default();
    let r_json = r#"{"notes":[0.0,100.0,60.0,64,0],"min":48,"max":72,
            "lane":"velocity","note_ids":[1],"joins":[],
            "lane_tabs":["velocity","velocity","9","CC 1 ch 1"],
            "curves":["9","CC 1",0,127,40]}"#;
    let mut r = roll(r_json);
    let tabs = r
        .tabs_rect(rect(), pianoroll::KEYBOARD_W, &m)
        .expect("tabs over the lane");
    let (_, body) = r.velocity_rects(rect(), pianoroll::KEYBOARD_W, &m).unwrap();
    assert!(
        (tabs.y + tabs.h - body.y).abs() < 0.5,
        "right over the lane"
    );
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    assert!(
        grid.y + grid.h <= tabs.y + 0.5,
        "the plane ends over the tabs"
    );
    let (labels, index) = r.tabs_said();
    assert_eq!((labels.len(), index), (2, 0));
    let parts = crate::host::graphics::choice::parts(
        crate::host::graphics::choice::View::Tabs,
        &labels,
        tabs,
        clausters_gui_draw::font::DEFAULT_SIZE * m.ui_scale,
        &m,
    );
    let second = parts[1].1;
    let at = (
        f64::from(second.x + second.w * 0.5),
        f64::from(second.y + second.h * 0.5),
    );
    let input = input(&m, rect(), axis(1000.0));
    let Claim::Take(take) = r.press(at, &input) else {
        panic!("the tab takes the press");
    };
    let msgs = take.events.into_messages();
    assert_eq!(
        msgs[0],
        vec![OscType::String("lane".into()), OscType::String("9".into())]
    );
    assert_eq!(r.lane.as_deref(), Some("velocity"), "the owner shows it");
    // a roll with no tabs offered keeps its plane
    let mut bare = roll(r_json);
    assert!(bare.set("lane_tabs", &Value::Array(Vec::new())));
    assert!(bare.tabs_rect(rect(), pianoroll::KEYBOARD_W, &m).is_none());
}

/// **The pencil draws the velocities freehand**: a stroke across the
/// lane gives every bar it crosses the height it crossed it at, and the
/// pointer moving up or down past the last bar changes nothing it has
/// left behind -- where the pointer's ramp would have swung them all.
#[test]
fn the_pencil_draws_velocities_where_it_crosses_them() {
    let m = Metrics::default();
    let json = r#"{"notes":[0.0,100.0,60.0,64,0, 300.0,100.0,62.0,64,0, 600.0,100.0,64.0,64,0],
                "min":48,"max":72,"lane":"velocity","note_ids":[1,2,3],"joins":[],
                "tool":"draw"}"#;
    let mut r = roll(json);
    let probe = roll(json);
    let input = input(&m, rect(), axis(1000.0));
    let (_, body) = r.velocity_rects(rect(), pianoroll::KEYBOARD_W, &m).unwrap();
    let x = |t: f64| x_of(&probe, &m, t, 1000.0);
    let y = |v: f64| f64::from(body.y) + (1.0 - v / 127.0) * f64::from(body.h);
    let (start, end) = (x(-0.0) - 2.0, x(700.0));
    assert!(matches!(
        r.press((start.max(f64::from(body.x)), y(20.0)), &input),
        Claim::Take(_)
    ));
    assert!(matches!(r.drag, Some(Drag::Paint { .. })));
    r.drag((x(150.0), y(20.0)), &input);
    r.drag((end, y(20.0)), &input);
    let drawn: Vec<i32> = r.notes.iter().map(|n| n.velocity).collect();
    assert!(drawn.iter().all(|v| (*v - 20).abs() <= 1), "{drawn:?}");
    // up and down past the last bar: nothing left behind moves
    r.drag((end, y(120.0)), &input);
    r.drag((end, y(5.0)), &input);
    assert_eq!(
        r.notes.iter().map(|n| n.velocity).collect::<Vec<_>>(),
        drawn
    );
    // and back, rising, over the last two bars: each takes the height
    // the stroke had over it, the first left as drawn
    r.drag((x(250.0), y(100.0)), &input);
    let (second, third) = (r.notes[1].velocity, r.notes[2].velocity);
    assert!(second > third && third > drawn[2], "{second} {third}");
    assert_eq!(r.notes[0].velocity, drawn[0]);
    assert_eq!(
        r.release((x(250.0), y(100.0)), true, &input)
            .into_messages()
            .len(),
        1
    );
}

/// **A roll under the window's ruler draws no strip of its own** and
/// still counts in the unit: the plane takes the room, and a `/gui_set`
/// gives the strip back.
#[test]
fn a_roll_under_the_window_s_ruler_reserves_no_strip() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,64,0],"min":48,"max":72,
                "ruler":"samples","ruler_strip":false}"#,
    );
    assert_eq!(r.editor.ruler, Ruler::Samples, "the unit stays");
    assert!(!r.editor.strip());
    let bare = r.regions(rect(), pianoroll::KEYBOARD_W, &m);
    assert!(r.set("ruler_strip", &Value::from(true)));
    let ruled = r.regions(rect(), pianoroll::KEYBOARD_W, &m);
    assert!(
        bare.grid.h > ruled.grid.h,
        "the strip's room is the plane's"
    );
}

/// **The ids name the new list whichever came first**: a sender whose map
/// sorts its keys sets `note_ids` before `notes`, and the list that
/// arrives second must not lose the ids that arrived first -- or the next
/// report names every note 0, as one the hand made.
#[test]
fn a_list_set_after_its_ids_keeps_them() {
    let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"note_ids":[7]}"#);
    assert!(r.set("note_ids", &Value::from("[7,9]")));
    assert!(r.set(
        "notes",
        &Value::from("[0.0,10.0,62.0,100,0,20.0,10.0,64.0,100,0]")
    ));
    assert_eq!(r.notes.iter().map(|n| n.id).collect::<Vec<_>>(), vec![7, 9]);
    // And the other way round.
    assert!(r.set("notes", &Value::from("[0.0,10.0,65.0,100,0]")));
    assert!(r.set("note_ids", &Value::from("[11]")));
    assert_eq!(r.notes[0].id, 11);
}

/// **A roll in hertz reads, draws and reports hertz**, and a pitch moves
/// continuously: its rows are the pitches the frequencies are, so the
/// note is where a MIDI roll would draw it, and a drag snaps to nothing.
#[test]
fn a_roll_in_hertz_reads_and_reports_hertz_and_moves_continuously() {
    let m = Metrics::default();
    let mut r =
        roll(r#"{"notes":[0.0,100.0,440.0,100,0],"min":110.0,"max":1760.0,"ruler_y":"hz"}"#);
    assert!((r.notes[0].pitch - 69.0).abs() < 1e-4, "{:?}", r.notes[0]);
    assert!((r.min - 45.0).abs() < 1e-4 && (r.max - 93.0).abs() < 1e-4);
    let info = r.info();
    let wire: Vec<f64> = serde_json::from_str(info[0].1.as_str().unwrap()).unwrap();
    assert!(
        (wire[2] - 440.0).abs() < 1e-2,
        "a query answers hertz: {wire:?}"
    );

    let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 69.0));
    r.press(at, &input(&m, rect(), axis(1000.0)));
    let to = (at.0, y_of(&r, &m, 69.5));
    r.drag(to, &input(&m, rect(), axis(1000.0)));
    let pitch = r.notes[0].pitch;
    assert!(
        (pitch - 69.5).abs() < 0.1,
        "not snapped to a semitone: {pitch}"
    );
    let msgs = r
        .release(to, true, &input(&m, rect(), axis(1000.0)))
        .into_messages();
    let OscType::Float(hz) = msgs[0][3] else {
        panic!("a float pitch: {:?}", msgs[0]);
    };
    let expected = clausters_core::scale::midi_to_hz(f64::from(pitch));
    assert!(
        (f64::from(hz) - expected).abs() < 1e-2,
        "reported in hertz: {hz}"
    );
}

/// **A note in hertz keeps its height at any zoom**, and on the keys it is
/// its row, which grows with the zoom.
#[test]
fn a_note_in_hertz_keeps_its_height_at_any_zoom() {
    let m = Metrics::default();
    let r = roll(r#"{"notes":[0.0,100.0,440.0,100,0],"ruler_y":"hz"}"#);
    let grid = Rect::new(0.0, 0.0, 400.0, 400.0);
    let bar = r.bar(&m);
    assert!(bar.is_some());
    let bar = bar.unwrap();
    let wide = Pitches::line(20.0, 120.0, bar).note_height(grid);
    let close = Pitches::line(60.0, 72.0, bar).note_height(grid);
    assert_eq!(wide, close, "the same bar whatever the window");
    let keys = roll(r#"{"notes":[0.0,100.0,69.0,100,0]}"#);
    assert!(keys.bar(&m).is_none());
    assert!(
        Pitches::rows(60.0, 72.0).note_height(grid) > Pitches::rows(20.0, 120.0).note_height(grid)
    );
}

/// **In hertz a drag reaches the window's edges and lands on round
/// frequencies**: the note's centre goes where the hand is, to the very
/// edge of a zoomed window, and its frequency is rounded to what a pixel
/// spans there.
#[test]
fn a_drag_in_hertz_reaches_the_edges_on_round_frequencies() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,440.0,100,0],"min":8.175798915643707,"max":20000.0,
                "ruler_y":"hz","y_start":0.5,"y_len":0.03}"#,
    );
    let pitches = r.axis(&m);
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    let x = x_of(&r, &m, 50.0, 1000.0);
    let at = (x, f64::from(pitches.y(r.notes[0].pitch, grid)));
    assert!(
        matches!(
            r.press(at, &input(&m, rect(), axis(1000.0))),
            Claim::Take(_)
        ),
        "the note is under {at:?} in a window {pitches:?}"
    );
    // Past the top: the centre is at the window's top edge.
    r.drag(
        (x, f64::from(grid.y) - 20.0),
        &input(&m, rect(), axis(1000.0)),
    );
    let per_px = pitches.span() / grid.h;
    assert!(
        (r.notes[0].pitch - pitches.hi).abs() <= per_px,
        "{}",
        r.notes[0].pitch
    );
    // Past the bottom: its bottom edge.
    r.drag(
        (x, f64::from(grid.y + grid.h) + 20.0),
        &input(&m, rect(), axis(1000.0)),
    );
    assert!(
        (r.notes[0].pitch - pitches.lo).abs() <= per_px,
        "{}",
        r.notes[0].pitch
    );
    // In the middle: a round frequency, to what a pixel spans there -- a
    // step of a half hertz at this zoom.
    r.drag(
        (x, f64::from(grid.y + grid.h * 0.37)),
        &input(&m, rect(), axis(1000.0)),
    );
    let f = clausters_core::scale::midi_to_hz(f64::from(r.notes[0].pitch));
    assert!((f * 2.0 - (f * 2.0).round()).abs() < 1e-2, "{f}");
}

/// **On the keys a drag transposes a microtone and keeps its bend**: the
/// box moves by whole semitones, the cents stay.
#[test]
fn a_drag_on_the_keys_keeps_a_notes_bend() {
    let m = Metrics::default();
    let mut r = roll(r#"{"notes":[0.0,100.0,60.37,100,0],"min":48,"max":72}"#);
    let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
    assert!(matches!(
        r.press(at, &input(&m, rect(), axis(1000.0))),
        Claim::Take(_)
    ));
    let to = (at.0, y_of(&r, &m, 62.0));
    r.drag(to, &input(&m, rect(), axis(1000.0)));
    assert!(
        (r.notes[0].pitch - 62.37).abs() < 1e-4,
        "{}",
        r.notes[0].pitch
    );
}

/// **A roll navigates its compass and opens on its window**: `min`/`max`
/// are the whole domain, and the view starts on the slice `y_start` and
/// `y_len` name -- in hertz, read as the pitches its rows are.
#[test]
fn a_roll_opens_on_its_window_inside_the_whole_compass() {
    let keys = roll(
        r#"{"notes":[0.0,100.0,60.0,100,0],"min":0,"max":127,
                             "y_start":0.4,"y_len":0.2}"#,
    );
    let (lo, hi) = keys.pitch_window();
    assert!(
        (lo - 50.8).abs() < 1e-3 && (hi - 76.2).abs() < 1e-3,
        "{lo} {hi}"
    );
    let hz = roll(
        r#"{"notes":[0.0,100.0,440.0,100,0],"min":8.175798915643707,
                           "max":20000.0,"ruler_y":"hz","y_start":0.5,"y_len":0.1}"#,
    );
    let (lo, hi) = hz.pitch_window();
    let top = clausters_core::scale::hz_to_midi(20_000.0) as f32;
    assert!((lo - top * 0.5).abs() < 1e-2, "{lo}");
    assert!((hi - top * 0.6).abs() < 1e-2, "{hi}");
}

/// The hertz ruler marks round frequencies over the roll's window, each
/// where its pitch is.
#[test]
fn the_hertz_ruler_marks_round_frequencies_at_their_pitches() {
    let ticks = pianoroll::hz_ticks(Pitches::line(45.0, 93.0, 8.0), 400.0, &Metrics::default());
    let labelled: Vec<_> = ticks.iter().filter(|(_, l)| l.is_some()).collect();
    assert!(!labelled.is_empty());
    for (pitch, label) in &labelled {
        assert!((44.5..=93.5).contains(pitch), "{pitch} {label:?}");
    }
    assert!(
        ticks.iter().any(|(p, l)| l.as_deref() == Some("200")
            && (f64::from(*p) - clausters_core::scale::hz_to_midi(200.0)).abs() < 0.01),
        "200 Hz is marked at its pitch: {ticks:?}"
    );
}

/// **A roll given ids names every note in its report**, the id first: a
/// note the owner named keeps its id, and one the hand made -- or a copy
/// that repeats an id, as a split's second half does -- is 0.
#[test]
fn a_roll_with_ids_reports_each_note_by_its_id() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,90,1],"note_ids":[7,9],
                "min":48,"max":72}"#,
    );
    assert!(r.ids);
    assert_eq!((r.notes[0].id, r.notes[1].id), (7, 9));
    let copy = r.notes[1];
    r.notes.push(copy);
    let msgs = r.notes_event().into_messages();
    assert_eq!(msgs[0].len(), 1 + 3 * 6, "the tag plus a sextuple per note");
    assert_eq!(msgs[0][1], OscType::Double(7.0));
    assert_eq!(msgs[0][7], OscType::Double(9.0));
    assert_eq!(msgs[0][13], OscType::Double(0.0), "the copy is a new note");
    // A new list drops the ids until its own arrive.
    assert!(r.set("notes", &Value::from("[0.0,10.0,60.0,100,0]")));
    assert_eq!(r.notes[0].id, 0);
    assert!(r.set("note_ids", &Value::from("[3]")));
    assert_eq!(r.notes[0].id, 3);
    let _ = m;
}

/// **Shift and a vertical drag is a note's velocity**, drawn in the note
/// rather than in a lane: one step a pixel, the grabbed note's alone when
/// it is not selected, and one edit at the release.
#[test]
fn shift_and_a_vertical_drag_sets_the_velocity_on_the_note() {
    let m = Metrics::default();
    let mut r =
        roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
    let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
    let mut shifted = input(&m, rect(), axis(1000.0));
    shifted.mods.shift = true;
    assert!(matches!(r.press(at, &shifted), Claim::Take(_)));
    let events = r.drag((at.0, at.1 - 20.0), &shifted);
    assert!(events.is_empty());
    assert_eq!(r.notes[0].velocity, 120);
    assert_eq!(r.notes[1].velocity, 100, "only the grabbed note");
    assert_eq!(r.notes[0].pitch, 60.0, "and it does not move");
    r.drag((at.0, at.1 + 200.0), &shifted);
    assert_eq!(r.notes[0].velocity, 1, "never a note-off");
    let msgs = r.release(at, true, &shifted).into_messages();
    assert_eq!(msgs[0][0], OscType::String("notes".into()));
}

/// **A note drag is measured against the axis it is handed each step**, not
/// against a press-time copy of it -- which is what lets the machine scroll
/// the axis under a drag held past a lane's edge.
#[test]
fn a_drag_follows_an_axis_that_moves_under_it() {
    let m = Metrics::default();
    let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"min":48,"max":72}"#);
    let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
    r.press(at, &input(&m, rect(), axis(1000.0)));
    // The same cursor, against a window that has panned 500 forward: the
    // note lands 500 later, because the pixel now names a later sample.
    let panned = Some(TimeSpace::of(
        View {
            start: 500.0,
            len: 1000.0,
        },
        2000.0,
    ));
    r.drag(at, &input(&m, rect(), panned));
    assert!((r.notes[0].start - 500.0).abs() < 1.0, "{:?}", r.notes[0]);
}

/// **The marquee is the machine's; what it caught is the roll's.** The
/// gesture, the anchor and the shared time span belong to the container --
/// every linked view follows that selection -- and the one question left
/// for this element is which notes the rectangle covered, and in what band
/// of its own axis.
///
/// The press declines, because there is nothing of the roll's on empty
/// grid; and a rectangle of no size is what lets go of the notes, which is
/// the same rule on every view that sweeps.
#[test]
fn the_roll_answers_what_the_rectangle_caught() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0,
                         500.0,100.0,80.0,100,0],"min":48,"max":84}"#,
    );
    // A press on empty grid is not the roll's: the container sweeps.
    let at = (x_of(&r, &m, 0.0, 1000.0), y_of(&r, &m, 58.0));
    assert_eq!(
        r.press(at, &input(&m, rect(), axis(1000.0))),
        Claim::Decline
    );

    let to = (x_of(&r, &m, 400.0, 1000.0), y_of(&r, &m, 66.0));
    let swept = r.select_in(at, to, &input(&m, rect(), axis(1000.0)));
    // The pitch axis is discrete, so the band is the semitones the sweep
    // passed over -- 58 to 66 read as whole steps, both ends included.
    let (lo_p, hi_p) = swept.band.expect("a rectangle restricts the pitch axis");
    assert!(
        (58.0..=59.0).contains(&lo_p) && (65.0..=66.0).contains(&hi_p),
        "{lo_p} {hi_p}"
    );
    assert!(swept.changed);
    assert_eq!(r.selected, vec![0, 1], "the third note is out of the band");

    // A rectangle of no size covers nothing, which is the press: the notes
    // are let go of, and no band is reported.
    let swept = r.select_in(at, at, &input(&m, rect(), axis(1000.0)));
    assert!(r.selected.is_empty() && swept.changed);
    assert_eq!(swept.band, None, "a click restricts nothing vertically");
}

/// Grabbing a **selected** note moves the whole selection rigidly; grabbing
/// an unselected one selects it alone and moves it.
#[test]
fn a_block_moves_together_and_an_unselected_note_alone() {
    let m = Metrics::default();
    let mut r =
        roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
    r.selected = vec![0, 1];
    let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
    r.press(at, &input(&m, rect(), axis(1000.0)));
    assert!(matches!(r.drag, Some(Drag::Block { .. })));
    let to = (x_of(&r, &m, 150.0, 1000.0), y_of(&r, &m, 60.0));
    r.drag(to, &input(&m, rect(), axis(1000.0)));
    assert!((r.notes[0].start - 100.0).abs() < 1.0);
    assert!((r.notes[1].start - 300.0).abs() < 1.0, "rigid");

    // The other half: an unselected note is selected alone, the set let
    // go of -- a click selects what it is on.
    let mut r =
        roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
    r.selected = vec![1];
    r.press(at, &input(&m, rect(), axis(1000.0)));
    assert!(matches!(r.drag, Some(Drag::Note { index: 0, .. })));
    assert_eq!(r.selected, vec![0]);
}

/// Ctrl adds a note where there is none and removes the one under the
/// cursor; the add drags its end, and the removal erases on.
#[test]
fn ctrl_adds_and_removes_a_note() {
    let m = Metrics::default();
    let mut r = roll(r#"{"min":48,"max":72,"snap":100.0}"#);
    let mut ctrl = input(&m, rect(), axis(1000.0));
    ctrl.mods = Mods {
        ctrl: true,
        ..Mods::default()
    };
    let at = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 60.0));
    assert!(matches!(r.press(at, &ctrl), Claim::Take(_)));
    assert_eq!(r.notes.len(), 1);
    assert_eq!(r.notes[0].start, 300.0, "snapped to the note grid");
    assert!(matches!(
        r.drag,
        Some(Drag::Note {
            part: boxes::Part::End,
            ..
        })
    ));

    r.drag = None;
    // The note it just added spans 300..400 on the grid.
    let on_note = (x_of(&r, &m, 350.0, 1000.0), y_of(&r, &m, 60.0));
    assert!(matches!(r.press(on_note, &ctrl), Claim::Take(_)));
    assert!(r.notes.is_empty());
}

/// **What a plain press does is the tool's**: the pointer makes a note
/// on a double click alone, the pencil on a press, the eraser takes the
/// note away, the scissors cut it where it was clicked, the stretch scales
/// the selection from its first onset; and Shift with a click that moves
/// no velocity adds a note to the selection.
#[test]
fn a_plain_press_does_what_the_tool_says() {
    let m = Metrics::default();
    let two = r#"{"notes":[0.0,100.0,60.0,100,0, 200.0,100.0,64.0,100,0],
            "min":48,"max":72,"snap":100.0}"#;
    let plain = input(&m, rect(), axis(1000.0));
    let empty = (
        x_of(&roll(two), &m, 650.0, 1000.0),
        y_of(&roll(two), &m, 67.0),
    );
    // the pointer: a click on empty grid makes nothing, a double click a note
    let mut r = roll(two);
    assert!(matches!(r.press(empty, &plain), Claim::Decline));
    let mut double = input(&m, rect(), axis(1000.0));
    double.clicks = 2;
    assert!(matches!(r.press(empty, &double), Claim::Take(_)));
    assert_eq!(r.notes.len(), 3);
    assert_eq!(r.selected, vec![2], "the note made is selected");
    // the pencil makes one on a press
    let mut r = roll(two);
    assert!(r.set("tool", &Value::from("draw")));
    // nothing is reported at the press: an owner's correction mid-drag
    // would reorder the notes under the index the drag holds
    let Claim::Take(take) = r.press(empty, &plain) else {
        panic!("the pencil takes the press");
    };
    assert!(take.events.into_messages().is_empty());
    assert_eq!(r.notes.len(), 3);
    // the eraser takes the note pressed
    let mut r = roll(two);
    r.set("tool", &Value::from("erase"));
    let first = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
    r.press(first, &plain);
    assert_eq!(r.notes.len(), 1);
    // the scissors cut the note clicked where it was clicked, on the grid
    let mut r = roll(r#"{"notes":[0.0,400.0,60.0,100,0],"min":48,"max":72,"snap":100.0}"#);
    r.set("tool", &Value::from("split"));
    let inside = (x_of(&r, &m, 210.0, 1000.0), y_of(&r, &m, 60.0));
    assert!(matches!(r.press(inside, &plain), Claim::Take(_)));
    assert_eq!(r.notes.len(), 2);
    assert!((r.notes[1].start - 200.0).abs() < 1e-6);
    // the stretch: the selection's end dragged from 300 to 600 doubles it
    let mut r = roll(two);
    r.set("tool", &Value::from("stretch"));
    r.selected = vec![0, 1];
    let second = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 64.0));
    r.press(second, &plain);
    r.drag((x_of(&r, &m, 550.0, 1000.0), y_of(&r, &m, 64.0)), &plain);
    assert!(
        (r.notes[1].start - 400.0).abs() < 1.0,
        "{}",
        r.notes[1].start
    );
    assert!((r.notes[1].dur - 200.0).abs() < 1.0 && r.notes[0].start == 0.0);
    // Shift and a click: the note joins the selection, and leaves it
    let mut r = roll(two);
    let mut shift = input(&m, rect(), axis(1000.0));
    shift.mods = Mods {
        shift: true,
        ..Mods::default()
    };
    r.press(first, &plain);
    r.release(first, true, &plain);
    assert_eq!(r.selected, vec![0], "a click selects");
    r.press(second, &shift);
    r.release(second, true, &shift);
    assert_eq!(r.selected, vec![0, 1]);
    r.press(first, &shift);
    r.release(first, true, &shift);
    assert_eq!(r.selected, vec![1]);
}

/// **A Ctrl drag from a note erases what it crosses**, and the list is
/// reported once, at the release.
#[test]
fn a_ctrl_drag_from_a_note_erases_what_it_crosses() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100,0, 200.0,100.0,60.0,100,0, 400.0,100.0,64.0,100,0],
                "min":48,"max":72}"#,
    );
    let mut ctrl = input(&m, rect(), axis(1000.0));
    ctrl.mods = Mods {
        ctrl: true,
        ..Mods::default()
    };
    let at = |t: f64, p: f32, r: &Notes| (x_of(r, &m, t, 1000.0), y_of(r, &m, p));
    assert!(matches!(r.press(at(50.0, 60.0, &r), &ctrl), Claim::Take(_)));
    assert!(matches!(r.drag, Some(Drag::Erase)));
    let crossed = at(250.0, 60.0, &r);
    assert!(
        r.drag(crossed, &ctrl).into_messages().is_empty(),
        "nothing yet"
    );
    assert_eq!(r.notes.len(), 1);
    let off = at(450.0, 60.0, &r);
    r.drag(off, &ctrl);
    assert_eq!(r.notes.len(), 1, "the third is on another row");
    let told = r.release(off, true, &ctrl).into_messages();
    assert_eq!(told.len(), 1, "one edit");
}

/// **An edge of a selected note resizes the selection**: each note's
/// same edge travels as far, and a note too short to follow stops alone.
#[test]
fn an_edge_of_a_selected_note_resizes_the_selection() {
    let m = Metrics::default();
    let mut r =
        roll(r#"{"notes":[0.0,200.0,60.0,100,0, 300.0,100.0,64.0,100,0],"min":48,"max":72}"#);
    r.selected = vec![0, 1];
    let plain = input(&m, rect(), axis(1000.0));
    // the first note's end edge, grabbed
    let edge = (x_of(&r, &m, 199.0, 1000.0), y_of(&r, &m, 60.0));
    r.press(edge, &plain);
    assert!(matches!(r.drag, Some(Drag::Resize { .. })));
    let to = (x_of(&r, &m, 249.0, 1000.0), y_of(&r, &m, 60.0));
    r.drag(to, &plain);
    let grew = r.notes[0].dur - 200.0;
    assert!(grew > 30.0, "{grew}");
    assert!((r.notes[1].dur - (100.0 + grew)).abs() < 1.0, "as far");
    assert_eq!(r.notes[1].start, 300.0, "an end edge keeps the start");
}

/// The block keys: `q` quantizes the selection, Delete removes it, and
/// cut/paste travel through the host-wide clipboard in the same JSON a
/// `/gui_set notes` takes.
#[test]
fn the_block_keys_quantize_delete_and_travel_through_the_clipboard() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut r = roll(r#"{"notes":[90.0,50.0,60.0,100,0,260.0,50.0,64.0,100,0],"snap":100.0}"#);
    r.selected = vec![0];
    fn ki(clip: &mut crate::host::clipboard::Clip, ctrl: bool) -> KeyInput<'_> {
        at_cursor(clip, ctrl, None)
    }
    fn at_cursor(
        clip: &mut crate::host::clipboard::Clip,
        ctrl: bool,
        cursor: Option<f64>,
    ) -> KeyInput<'_> {
        KeyInput {
            mods: Mods {
                ctrl,
                ..Mods::default()
            },
            clipboard: clip,
            cursor,
        }
    }
    assert!(
        r.verb(Verb::Quantize, &mut ki(&mut clipboard, false))
            .is_some()
    );
    assert_eq!(r.notes[0].start, 100.0);
    assert_eq!(r.notes[1].start, 260.0, "unselected, untouched");

    // Cut: the block lands on the clipboard and leaves the roll.
    r.selected = vec![0];
    assert!(r.verb(Verb::Cut, &mut ki(&mut clipboard, true)).is_some());
    assert_eq!(r.notes.len(), 1);
    let block = clipboard.text();
    assert!(block.starts_with('['), "{block}");

    // ...and pastes back at the window's cursor, keeping its pitch: the
    // block starts where the cursor is and not where it was copied from.
    assert!(
        r.verb(
            Verb::Paste,
            &mut at_cursor(&mut clipboard, true, Some(400.0))
        )
        .is_some()
    );
    assert_eq!(r.notes.len(), 2);
    assert_eq!(r.notes[1].pitch, 60.0);
    assert_eq!(
        r.notes[1].start, 400.0,
        "at the cursor, snapped to the grid"
    );
    assert_eq!(r.selected, vec![1], "the pasted block is selected");
    // A roll on no axis keeps step entry's own position as the anchor,
    // which is where this one still stands.
    notes::remove_notes(&mut r.notes, &[1]);
    r.selected.clear();
    assert!(r.verb(Verb::Paste, &mut ki(&mut clipboard, true)).is_some());
    assert_eq!(r.notes[1].start, r.step, "no cursor: the step position");

    // Delete takes the selection away; a verb it has no arm for falls
    // through to the window.
    assert!(
        r.verb(Verb::Delete, &mut ki(&mut clipboard, false))
            .is_some()
    );
    assert_eq!(r.notes.len(), 1);
    assert!(r.verb(Verb::Play, &mut ki(&mut clipboard, false)).is_none());
    // Text on the clipboard is not a note block, so a paste declines it.
    let mut text = crate::host::clipboard::Clip::default();
    text.set_text("hola");
    assert!(r.verb(Verb::Paste, &mut ki(&mut text, true)).is_none());
}

/// Live MIDI: a note-on paints a held note, the matching note-off closes it
/// -- at the running playhead when recording, on the step cursor when the
/// transport is stopped (and the last key up advances it).
/// **A lane sits under the plane and a Ctrl press on it adds a point**,
/// reported once as every curve's points.
#[test]
fn a_lane_takes_the_bottom_of_the_plane_and_a_ctrl_press_adds_a_point() {
    let m = Metrics::default();
    let flat = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0]}"#);
    let mut r = roll(
        r#"{"notes":[0.0,50.0,60.0,100.0,0.0],
                "curves":["cc74","CC 74",0.0,127.0,40.0]}"#,
    );
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    let whole = flat.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    assert_eq!(
        grid.h,
        whole.h - 40.0,
        "the lane took its height off the plane"
    );
    let (_, body) = r.row_rects(rect(), pianoroll::KEYBOARD_W, &m)[0];
    let at = (
        (body.x + body.w * 0.5) as f64,
        (body.y + body.h * 0.5) as f64,
    );
    let mut held = input(&m, rect(), axis(100.0));
    held.mods = Mods {
        ctrl: true,
        ..Mods::default()
    };
    let Claim::Take(take) = r.press(at, &held) else {
        panic!("the lane takes the press");
    };
    let args = take.events.into_messages()[0].clone();
    assert_eq!(args[0], OscType::String("points".into()));
    assert_eq!(args[1], OscType::String("cc74".into()));
    assert_eq!(r.release(at, true, &held), Events::none(), "reported once");
}

/// **A bend is drawn in the plane**, over the pitches its range spans from
/// its note; any other expression inside the note's box.
#[test]
fn a_bend_layer_spans_its_pitches_and_another_sits_in_the_box() {
    let m = Metrics::default();
    let r = roll(
        r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"note_ids":[7],
                "layers":["b","7","bend",-2.0,2.0,true,"p","7","pressure",0.0,1.0,false]}"#,
    );
    let placed = r.curves_on_screen(rect(), pianoroll::KEYBOARD_W, &m, axis(100.0));
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    let axis_y = r.axis(&m);
    let (_, bend, _) = placed.iter().find(|(n, ..)| *n == "b").unwrap();
    let (_, inside, _) = placed.iter().find(|(n, ..)| *n == "p").unwrap();
    assert!((bend.y - axis_y.y(62.0, grid)).abs() < 1e-3);
    assert!((bend.y + bend.h - axis_y.y(58.0, grid)).abs() < 1e-3);
    assert!(inside.h < bend.h, "the pressure is inside the box");
}

/// **The hover reads what is under the pointer**: over a lane its whole
/// label and the curve's value there, a break-point's own on a point, the
/// label alone over its narrow cell; over a note's layer line the
/// control's; and nothing of a curve over empty grid.
#[test]
fn the_hover_reads_a_curve_under_the_pointer() {
    let m = Metrics::default();
    let r = roll(
        r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"note_ids":[7],
                "curves":["vol","CC 7 ch 1 volume",0.0,127.0,40.0],
                "layers":["b","7","bend",-12.0,12.0,true],
                "points":["vol",0.0,0.0,1,0.0,"vol",100.0,127.0,1,0.0,
                          "b",0.0,1.0,1,0.0,"b",50.0,1.0,1,0.0]}"#,
    );
    let time = axis(100.0);
    let read = |at: (f64, f64)| r.curve_readout(at, rect(), pianoroll::KEYBOARD_W, &m, time);
    let (label, body) = r.row_rects(rect(), pianoroll::KEYBOARD_W, &m)[0];
    let mid = (body.y + body.h * 0.5) as f64;
    // Half way along the ramp the lane reads half of it, wherever the
    // pointer is in the row's height.
    let half = (body.x + body.w * 0.5) as f64;
    assert_eq!(read((half, mid)).as_deref(), Some("CC 7 ch 1 volume  64"));
    assert_eq!(
        read((half, body.y as f64 + 2.0)).as_deref(),
        Some("CC 7 ch 1 volume  64")
    );
    // The cell under the keyboard is narrow: the whole label is read there.
    let cell = ((label.x + 2.0) as f64, mid);
    assert_eq!(read(cell).as_deref(), Some("CC 7 ch 1 volume"));

    // The bend's line holds a semitone up across the note.
    let placed = r.curves_on_screen(rect(), pianoroll::KEYBOARD_W, &m, time);
    let (_, bend, _) = placed.iter().find(|(n, ..)| *n == "b").unwrap();
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    let up = r.axis(&m).y(61.0, grid) as f64;
    let along = (bend.x + bend.w * 0.5) as f64;
    assert_eq!(read((along, up)).as_deref(), Some("bend  +1.00 st"));
    assert_eq!(
        read(((bend.x + 1.0) as f64, up)).as_deref(),
        Some("bend  point +1.00 st"),
        "on its first point"
    );
    // Off the line, inside the note: the note's, not the curve's.
    assert_eq!(read((along, r.axis(&m).y(50.0, grid) as f64)), None);
}

/// **A roll says which MIDI its notes are written for**, set and cleared
/// like any prop -- empty for notes for the server.
#[test]
fn a_roll_carries_the_midi_spec_it_shows() {
    let mut r = roll(r#"{"midi":"MPE"}"#);
    assert_eq!(r.midi, "MPE");
    assert!(r.set("midi", &Value::from("MIDI 1.0")));
    assert_eq!(r.midi, "MIDI 1.0");
    assert!(r.set("midi", &Value::from("")));
    assert!(r.midi.is_empty());
    assert!(roll("{}").midi.is_empty());
}

/// **A note's curve runs past its off**, into the release: the layer
/// reaches its last point while the note's box stays its on-to-off span.
#[test]
fn a_layer_reaches_past_its_note_and_the_box_does_not() {
    let m = Metrics::default();
    let r = roll(
        r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"note_ids":[7],
                "layers":["p","7","pressure",0.0,1.0,false],
                "points":["p",0.0,0.5,1,0.0,"p",80.0,0.0,1,0.0]}"#,
    );
    let placed = r.curves_on_screen(rect(), pianoroll::KEYBOARD_W, &m, axis(100.0));
    let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
    let nav = r.view(axis(100.0));
    let boxed = pianoroll::note_rect(grid, &nav, 0.0, &r.notes[0], r.axis(&m)).unwrap();
    let (_, layer, space) = placed.iter().find(|(n, ..)| *n == "p").unwrap();
    assert!((layer.x - boxed.x).abs() < 1e-3);
    assert!(
        (layer.w - boxed.w * 80.0 / 50.0).abs() < 1e-2,
        "the layer reaches its last point"
    );
    assert!(space.span >= 80.0);
    assert_eq!(r.notes[0].dur, 50.0, "the note is still its on and its off");
}

/// **An MPE note is painted at its bend, and follows it**: a zone note
/// starts at its key plus its bend, and a retune moves the held note.
#[test]
fn an_mpe_note_is_painted_at_its_bend_and_follows_it() {
    let mut r = roll(r#"{"midi_in":true}"#);
    let on = MidiNote {
        on: true,
        channel: 3,
        pitch: 60,
        velocity: 90,
        bend: 0.5,
        retune: false,
    };
    r.midi(on, Some(0.0));
    assert_eq!(r.notes[0].pitch, 60.5);
    r.midi(
        MidiNote {
            bend: 2.0,
            retune: true,
            ..on
        },
        Some(10.0),
    );
    assert_eq!(r.notes[0].pitch, 62.0);
    assert_eq!(r.notes.len(), 1, "a retune starts no note");
}

#[test]
fn live_midi_records_at_the_playhead_and_steps_when_stopped() {
    let mut r = roll(r#"{"midi_in":true,"snap":100.0}"#);
    assert!(r.needs().midi);
    assert!(!roll("{}").needs().midi);

    // Recording: the key is held from 200 to 350.
    let on = MidiNote {
        on: true,
        channel: 0,
        pitch: 60,
        velocity: 90,
        bend: 0.0,
        retune: false,
    };
    assert!(r.midi(on, Some(200.0)).is_some());
    assert_eq!(r.notes[0].start, 200.0);
    assert!(
        r.midi(
            MidiNote {
                on: false,
                velocity: 0,
                ..on
            },
            Some(350.0)
        )
        .is_some()
    );
    assert_eq!(r.notes[0].dur, 150.0, "the key was held this long");

    // Stopped: a chord lands on the step cursor and advances it once.
    let mut r = roll(r#"{"midi_in":true,"snap":100.0}"#);
    for pitch in [60, 64] {
        r.midi(
            MidiNote {
                on: true,
                channel: 0,
                pitch,
                velocity: 90,
                bend: 0.0,
                retune: false,
            },
            None,
        );
    }
    assert_eq!((r.notes[0].start, r.notes[1].start), (0.0, 0.0));
    for pitch in [60, 64] {
        r.midi(
            MidiNote {
                on: false,
                channel: 0,
                pitch,
                velocity: 0,
                bend: 0.0,
                retune: false,
            },
            None,
        );
    }
    assert_eq!(r.step, 100.0, "one step for the whole chord");
    // A note-off nobody is holding is tolerated, not a panic.
    assert!(
        r.midi(
            MidiNote {
                on: false,
                channel: 9,
                pitch: 1,
                velocity: 0,
                bend: 0.0,
                retune: false,
            },
            None
        )
        .is_none()
    );
}

/// **The OSC markers show and do not write.**
///
/// A roll is the editor of things that have a pitch; the other items a
/// timeline holds have none and are drawn below it as markers. The lane
/// had grown a `Ctrl`-press that added and removed them, against the
/// decision this widget was built with, and it could not have worked: a
/// marker *is* the message it sends, the lane draws only its address, and
/// there is no way to type one here, so an added marker was a message with
/// no destination. Both clients saw the same press and answered
/// differently -- one refusing it with a sentence, the other keeping the
/// addressless marker -- which is how it was found.
///
/// Nothing tested this lane's editing at all, which is why the gesture
/// could contradict a recorded decision and stay.
#[test]
fn the_osc_markers_refuse_the_press_that_meant_to_edit_them() {
    let m = Metrics::default();
    let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"osc":[50.0,"/bar"],"osc_markers":1}"#);
    assert_eq!(r.osc.len(), 1, "the marker it was given");
    let mut i = input(&m, rect(), axis(1000.0));
    let lane = r.regions(rect(), pianoroll::KEYBOARD_W, &m).osc;
    let on = (x_of(&r, &m, 50.0, 1000.0), (lane.y + lane.h * 0.5) as f64);
    let empty = (x_of(&r, &m, 700.0, 1000.0), (lane.y + lane.h * 0.5) as f64);

    // The gesture that meant to edit is told, and consumed so nothing
    // behind it turns a refused edit into a sweep.
    i.mods.ctrl = true;
    for (what, at) in [("on a marker", on), ("on empty lane", empty)] {
        let Claim::Take(take) = r.press(at, &i) else {
            panic!("{what}: a refusal consumes the press");
        };
        let msgs = take.events.into_messages();
        assert_eq!(msgs[0][0], OscType::String("refused".into()), "{what}");
        assert_eq!(msgs[0][1], OscType::String("osc".into()), "{what}");
    }
    assert_eq!(r.osc.len(), 1, "nothing was added and nothing removed");

    // And a plain press hands the lane back to the container, the way the
    // axis strip beside it does -- so a sweep across the roll still works.
    i.mods.ctrl = false;
    assert_eq!(r.press(on, &i), Claim::Decline);
    assert!(r.drag.is_none(), "and no marker is being slid");
}

/// The reason a verb gave for refusing, or `None` when it did the thing --
/// the same reader the multitrack's own verb test uses, because a refusal
/// is an ordinary event in both.
fn refusal(events: Option<Events>) -> Option<String> {
    let msg = events?.into_messages().into_iter().next()?;
    (msg.first() == Some(&OscType::String("refused".into()))).then(|| match msg.get(2) {
        Some(OscType::String(s)) => s.clone(),
        _ => String::new(),
    })
}

/// **A roll's verbs say why they did nothing**, exactly as a lane's do.
///
/// The drift this holds: the two implement one set of verbs over one
/// reading, and a multitrack that could not quantize said so while a roll
/// that could not returned silence -- which is the thing two reports in one
/// day settled as a defect rather than as a quiet success.
#[test]
fn a_verb_that_acts_on_nothing_says_why_rather_than_nothing() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut press = |roll: &mut Notes, verb: Verb| {
        refusal(roll.verb(
            verb,
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: Some(100.0),
            },
        ))
    };

    // Two notes on one pitch, already on the grid, and the cursor between
    // them: every verb has something to act on and nothing to do.
    let mut r =
        roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0, 200.0,50.0,60.0,100.0,0.0],"snap":100.0}"#);
    r.selected = vec![0, 1];
    assert_eq!(
        press(&mut r, Verb::Quantize),
        Some("these notes are already on the grid".to_string())
    );
    assert_eq!(
        press(&mut r, Verb::Split),
        Some("the cursor is not inside a held note".to_string()),
        "the cursor falls in the gap between them"
    );
    assert_eq!(
        press(&mut r, Verb::Join),
        Some("a join is one pitch's, and these notes do not touch on one".to_string())
    );
}

/// **A roll does not take a note of a voice or a channel other than the
/// one edited**: the lists it is drawn with ride beside the notes, a
/// dimmed note is no hit and no marquee takes it, MIDI mode's empty lists
/// take them away, and a channel edited veils the others' again.
#[test]
fn a_note_of_another_voice_is_drawn_faint_and_not_taken() {
    let m = Metrics::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,90,0, 0.0,100.0,64.0,90,1],"note_ids":[1,2],
                "voices":[0,0,0,1],"dimmed":[0,1],"held":[100.0,40.0],"labels":["p",""]}"#,
    );
    assert_eq!(r.written.voice(1), Some([0, 1]));
    assert!(r.written.dimmed(1) && !r.written.dimmed(0));
    let len = 400.0;
    let input = input(&m, rect(), axis(len));
    let (x, x0, x1) = (
        x_of(&r, &m, 50.0, len),
        x_of(&r, &m, 0.0, len),
        x_of(&r, &m, 200.0, len),
    );
    let (low, high) = (y_of(&r, &m, 60.0), y_of(&r, &m, 64.0));
    assert!(r.hit((x, low), &input).note.is_some(), "the voice edited");
    assert!(r.hit((x, high), &input).note.is_none(), "another voice");
    let swept = r.select_in((x0, 0.0), (x1, 399.0), &input);
    assert!(swept.changed);
    assert_eq!(r.selected, vec![0]);
    assert!(r.set("voices", &serde_json::json!([])) && r.set("dimmed", &serde_json::json!([])));
    assert!(
        r.hit((x, high), &input).note.is_some(),
        "in MIDI mode every note is taken"
    );
    // a channel edited in MIDI mode veils the others' notes the same way
    assert!(r.set("dimmed", &serde_json::json!([0, 1])));
    assert!(r.hit((x, high), &input).note.is_none(), "another channel");
    assert_eq!(r.velocity_shown(), vec![0], "nor has a velocity bar");
}

/// **A page's marks are read as the owner draws them**: the
/// articulations over each box, the breath at its end and the tint each
/// note is drawn with, set again by a `/gui_set`; without `tints` a note
/// is tinted by its voice.
#[test]
fn a_roll_reads_the_marks_the_breaths_and_the_tints() {
    let mut r = roll(
        r#"{"notes": [0, 100, 60, 100, 0, 100, 100, 64, 100, 0],
                "voices": [0, 0, 0, 1],
                "marks": ["> -", ""], "breaths": ["", ","]}"#,
    );
    assert_eq!(r.written.marks, vec!["> -".to_string(), String::new()]);
    assert_eq!(r.written.breaths[1], ",");
    assert!(r.written.tints.is_empty());
    assert!(r.set("tints", &serde_json::json!([0, 2])));
    assert_eq!(r.written.tints, vec![0, 2]);
}

/// **A pitch line is drawn through the pitch it sounds, cut at the
/// grid**: a run from 60 bending to 62 and on into 64 starts on the row of
/// 60, and a run reaching past the grid draws nothing outside it. A
/// `/gui_set` replaces the runs, an empty list taking them away.
#[test]
fn a_pitch_line_follows_its_run_inside_the_grid() {
    use clausters_gui_draw::draw::Draw;
    use clausters_gui_draw::metrics::Metrics;
    use clausters_gui_draw::paint::Mesh;
    use clausters_gui_draw::theme::Theme;

    let mut r = roll(
        r#"{"notes": [0, 100, 60, 100, 0, 100, 100, 64, 100, 0],
                "min": 48, "max": 72,
                "lines": [[0, 60, 100, 62, 100, 64, 200, 64], [150, 70, 400, 90]]}"#,
    );
    assert_eq!(r.lines.len(), 2);
    assert_eq!(r.lines[0][1], [100.0, 62.0]);
    let grid = Rect::new(0.0, 0.0, 300.0, 240.0);
    let nav = View {
        start: 0.0,
        len: 300.0,
    };
    let axis = pianoroll::Pitches::rows(48.0, 72.0);
    let (m, theme) = (Metrics::default(), Theme::default());
    let mut mesh = Mesh::new();
    r.draw_lines(&mut Draw::new(&mut mesh, &m, &theme), grid, &nav, axis);
    let points: Vec<(f32, f32)> = mesh.positions().collect();
    assert!(!points.is_empty());
    assert!(
        points.iter().all(|(x, y)| *x >= grid.x - 0.01
            && *x <= grid.x + grid.w + 0.01
            && *y >= grid.y - 0.01
            && *y <= grid.y + grid.h + 0.01),
        "every vertex inside the grid"
    );
    let first = axis.y(60.0, grid);
    assert!(
        points
            .iter()
            .any(|(x, y)| x.abs() < 2.0 && (y - first).abs() < 2.0)
    );
    assert!(r.set("lines", &serde_json::json!("[]")));
    assert!(r.lines.is_empty());
}

/// **A roll whose owner speaks joins asks it to join a line**: `join`
/// names the selection to the owner rather than refusing -- notes of one
/// pitch that touch too, which the owner makes one with their curves --
/// `unjoin` names them too, and both refuse with no selection. The joins
/// are drawn from the `joins` prop.
#[test]
fn a_roll_that_speaks_joins_asks_its_owner_to_join_the_selection() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut r = roll(
        r#"{"notes":[0.0,50.0,60.0,100.0,0.0, 100.0,50.0,62.0,100.0,0.0],
                "note_ids":[7,9],"joins":[0,1]}"#,
    );
    assert_eq!(r.joins, Some(vec![(0, 1)]));
    let mut input = KeyInput {
        mods: Mods::default(),
        clipboard: &mut clipboard,
        cursor: Some(100.0),
    };
    assert!(
        refusal(r.command("unjoin", &mut input)).is_some(),
        "nothing selected"
    );
    r.selected = vec![1, 0];
    let said = |events: Option<Events>| -> Vec<OscType> {
        events.unwrap().into_messages().into_iter().next().unwrap()
    };
    assert_eq!(
        said(r.verb(Verb::Join, &mut input)),
        vec![
            OscType::String("join".into()),
            OscType::Double(7.0),
            OscType::Double(9.0)
        ]
    );
    assert_eq!(r.notes.len(), 2, "the roll joined nothing itself");
    assert_eq!(
        said(r.command("unjoin", &mut input))[0],
        OscType::String("unjoin".into())
    );
    // one pitch touching is the owner's join too: the boxes here hold
    // no curves to keep
    let mut held = roll(
        r#"{"notes":[0.0,100.0,60.0,100.0,0.0, 100.0,100.0,60.0,100.0,0.0],
                "note_ids":[7,9],"joins":[]}"#,
    );
    held.selected = vec![0, 1];
    assert_eq!(
        said(held.verb(Verb::Join, &mut input))[0],
        OscType::String("join".into())
    );
    assert_eq!(held.notes.len(), 2, "the roll joined nothing itself");
    // a roll that was sent no joins keeps the refusal it had
    let mut plain = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0, 100.0,50.0,62.0,100.0,0.0]}"#);
    plain.selected = vec![0, 1];
    assert!(refusal(plain.verb(Verb::Join, &mut input)).is_some());
    assert!(plain.command("unjoin", &mut input).is_none());
}

/// **A roll whose owner holds events asks it to split them**: the cursor
/// and the selected ids, the roll's own notes left as they are; a cursor
/// outside every selected note is refused here as before.
#[test]
fn a_roll_that_speaks_joins_asks_its_owner_to_split_the_selection() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100.0,0.0, 200.0,50.0,62.0,100.0,0.0],
                "note_ids":[7,9],"joins":[]}"#,
    );
    r.selected = vec![0];
    let mut at = |cursor: f64, r: &mut Notes| {
        r.verb(
            Verb::Split,
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: Some(cursor),
            },
        )
    };
    let said = at(40.0, &mut r).unwrap().into_messages();
    assert_eq!(
        said[0],
        vec![
            OscType::String("split".into()),
            OscType::Double(40.0),
            OscType::Double(7.0)
        ]
    );
    assert_eq!(r.notes.len(), 2, "the roll cut nothing itself");
    assert_eq!(
        refusal(at(150.0, &mut r)),
        Some("the cursor is not inside a held note".to_string())
    );
}

/// **A transposition is the owner's**, of the selection's ids or, with
/// nothing selected, of no ids: every note the roll takes.
#[test]
fn a_roll_asks_its_owner_to_transpose_the_selection_or_every_note() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100.0,0.0, 200.0,50.0,62.0,100.0,0.0],
                "note_ids":[7,9],"joins":[]}"#,
    );
    let mut input = KeyInput {
        mods: Mods::default(),
        clipboard: &mut clipboard,
        cursor: None,
    };
    let said = |events: Option<Events>| events.unwrap().into_messages().remove(0);
    assert_eq!(
        said(r.command("step_up", &mut input)),
        vec![OscType::String("step_up".into())]
    );
    r.selected = vec![1];
    assert_eq!(
        said(r.command("octave_down", &mut input)),
        vec![OscType::String("octave_down".into()), OscType::Double(9.0)]
    );
    assert_eq!(r.notes[1].pitch, 62.0, "the roll moved nothing itself");
}

/// **A selection by data is the owner's to answer**: the selected ids
/// go out, and with nothing selected the roll says why.
#[test]
fn a_roll_asks_its_owner_to_select_by_what_the_selection_holds() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100.0,0.0, 200.0,50.0,60.0,100.0,0.0],
                "note_ids":[7,9],"joins":[]}"#,
    );
    let mut input = KeyInput {
        mods: Mods::default(),
        clipboard: &mut clipboard,
        cursor: None,
    };
    assert!(refusal(r.command("select_pitch", &mut input)).is_some());
    r.selected = vec![0];
    let said = r
        .command("select_pitch", &mut input)
        .unwrap()
        .into_messages();
    assert_eq!(
        said[0],
        vec![OscType::String("select_pitch".into()), OscType::Double(7.0)]
    );
    // the answer is a correction of the roll's `selected`
    assert!(r.set("selected", &serde_json::json!([7, 9])));
    assert_eq!(r.selected, vec![0, 1]);
}

/// **A roll asked to tell its marks says what the hand marked**, once per
/// change, and never what its owner set.
#[test]
fn a_roll_that_tells_its_marks_says_each_change_once() {
    let mut r = roll(
        r#"{"notes":[0.0,100.0,60.0,100.0,0.0, 200.0,50.0,62.0,100.0,0.0],
                "note_ids":[7,9],"tell_marks":1}"#,
    );
    assert_eq!(
        r.take_marks(),
        Some(vec![OscType::String("marked".into())]),
        "nothing marked is said once"
    );
    assert_eq!(r.take_marks(), None);
    r.selected = vec![1, 0];
    assert_eq!(
        r.take_marks(),
        Some(vec![
            OscType::String("marked".into()),
            OscType::Double(7.0),
            OscType::Double(9.0)
        ])
    );
    assert!(r.set("selected", &serde_json::json!([9])));
    assert_eq!(r.take_marks(), None, "the owner's own marks");
    let mut quiet = roll(r#"{"notes":[0.0,100.0,60.0,100.0,0.0],"note_ids":[7]}"#);
    quiet.selected = vec![0];
    assert_eq!(quiet.take_marks(), None, "a roll not asked tells nothing");
}

/// **The cut is the box arithmetic's, and the identity is the roll's.**
///
/// A note's second half keeps the pitch, the velocity and the channel of
/// the one it came from, which is what `Holder::duplicate` answers for here;
/// everything else about the cut is what a clip's split does, through the
/// same function.
#[test]
fn a_cut_note_leaves_two_halves_that_are_still_the_same_note() {
    let mut r = roll(r#"{"notes":[0.0,200.0,64.0,90,3]}"#);
    r.selected = vec![0];
    let cut = notes::split_notes(&mut r.notes, &r.selected, 50.0);
    assert_eq!(cut, vec![0, 1], "the head it was, and the tail it made");
    assert_eq!((r.notes[0].start, r.notes[0].dur), (0.0, 50.0));
    assert_eq!((r.notes[1].start, r.notes[1].dur), (50.0, 150.0));
    assert_eq!(
        (r.notes[1].pitch, r.notes[1].velocity, r.notes[1].channel),
        (64.0, 90, 3),
        "the tail is the same note, cut"
    );
    let mut held = roll(r#"{"notes":[0.0,200.0,64.0,90,3],"note_ids":[7]}"#);
    held.selected = vec![0];
    notes::split_notes(&mut held.notes, &held.selected, 50.0);
    assert_eq!(
        (held.notes[0].id, held.notes[1].id),
        (7, 0),
        "the tail is a new note, for its owner to name"
    );
}

/// **A grid in beats is read through the tempo map**: a note quantizes to
/// the beat nearest it, and through a tempo change the beats are where
/// the map puts them -- one a second up to beat 4, two a second after.
#[test]
fn a_beat_grid_quantizes_through_the_tempo_map() {
    let mut clipboard = crate::host::clipboard::Clip::default();
    let mut r = roll(
        r#"{"notes":[130.0,50.0,60.0,100,0,440.0,30.0,64.0,100,0],
                "sample_rate":100.0,"grid":1.0,
                "tempo_map":[{"beats":0,"tempo":1},{"beats":4,"tempo":2}]}"#,
    );
    let input = &mut KeyInput {
        mods: Mods::default(),
        clipboard: &mut clipboard,
        cursor: None,
    };
    assert!(r.verb(Verb::Quantize, input).is_some());
    let starts: Vec<f64> = r.notes.iter().map(|n| n.start).collect();
    assert_eq!(starts, [100.0, 450.0], "beat 1, and beat 5 at 4.5 seconds");
    assert_eq!(r.notes[1].dur, 30.0, "a quantize moves onsets only");
    // with snap to grid off a hand lands where it is put, and a quantize
    // still moves onto the grid: the switch is the hand's, not the verb's
    r.editor.grid_snap = false;
    assert!(!r.grid().is_set(), "a drag lands where the hand puts it");
    r.notes[0].start = 130.0;
    assert!(r.verb(Verb::Quantize, input).is_some());
    assert_eq!(r.notes[0].start, 100.0, "the quantize is on the grid still");
    // and with no grid at all, it says there is none to quantize to
    let mut bare = roll(r#"{"notes":[130.0,50.0,60.0,100,0]}"#);
    let refused = bare.verb(Verb::Quantize, input).expect("an answer");
    assert!(format!("{refused:?}").contains("no grid"), "{refused:?}");
}
