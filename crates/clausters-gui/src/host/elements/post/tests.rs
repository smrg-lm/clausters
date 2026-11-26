//! The post window's tests: what a write shows, what a hand selects and
//! copies, where the wheel leaves the reader, and what ten thousand lines
//! at once cost.

use super::*;
use crate::host::clipboard::Clip;
use crate::host::widget::element::Mods;
use crate::host::world::World;
use clausters_gui_draw::font::Family;
use clausters_gui_draw::paint::Mesh;

const RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 400.0,
    h: 200.0,
};

fn view(json: &str) -> PostView {
    PostView::new(&serde_json::from_str(json).unwrap())
}

fn input<'a>(m: &'a Metrics, clicks: u32, shift: bool) -> Input<'a> {
    Input {
        metrics: m,
        indent: 0.0,
        rect: RECT,
        scale: 1.0,
        mods: Mods {
            shift,
            ..Mods::default()
        },
        viewport: (400.0, 200.0),
        clicks,
        time: None,
        header: None,
    }
}

fn geometry(v: &PostView, m: &Metrics) -> Geometry {
    v.geometry(RECT, 1.0, m, None)
}

/// The text of each row seen, top down.
fn seen(v: &PostView, m: &Metrics) -> Vec<String> {
    let g = geometry(v, m);
    v.visible(&g)
        .into_iter()
        .map(|(n, a, b, _)| v.post.line(n).unwrap().text[a..b].to_string())
        .collect()
}

fn write(v: &mut PostView, key: &str, text: &str) {
    assert!(v.set(key, &Value::from(text)));
}

/// **A write shows the newest lines**: the last line written is the last
/// row seen, and nothing past it.
#[test]
fn a_write_shows_the_newest_lines() {
    let m = Metrics::default();
    let mut v = view(r#"{}"#);
    for i in 0..100 {
        write(&mut v, "out", &format!("line {i}\n"));
    }
    write(&mut v, "result", "42");
    let rows = seen(&v, &m);
    assert_eq!(rows.last().unwrap(), "-> 42");
    assert_eq!(rows[rows.len() - 2], "line 99");
    assert!(rows.len() < 30, "only what fits: {}", rows.len());
}

/// **The wheel up leaves the reader where they read**, a write does not
/// move them, and coming back down to the end follows again.
#[test]
fn reading_up_stops_following_and_the_end_resumes_it() {
    let m = Metrics::default();
    let mut v = view(r#"{}"#);
    for i in 0..100 {
        write(&mut v, "out", &format!("line {i}\n"));
    }
    let inp = input(&m, 1, false);
    assert!(v.wheel((10.0, 10.0), (0.0, 5.0), &inp).is_some());
    assert!(!v.follow);
    let before = seen(&v, &m);
    write(&mut v, "out", "line 100\n");
    assert_eq!(seen(&v, &m), before, "a write does not move a reader");
    v.wheel((10.0, 10.0), (0.0, -50.0), &inp);
    assert!(v.follow, "back at the end, it follows");
    assert_eq!(seen(&v, &m).last().unwrap(), "line 100");
}

/// **The scroll bar is the host's one bar**: a press on its thumb holds it
/// where it was taken and leaves the view, a drag up reads back and stops
/// following, a drag back to the end follows again, and the thumb is no
/// shorter than a hand's grip.
#[test]
fn the_scroll_bar_is_held_by_its_thumb_and_the_end_follows() {
    let m = Metrics::default();
    let mut v = view(r#"{}"#);
    for i in 0..500 {
        write(&mut v, "out", &format!("line {i}\n"));
    }
    let inp = input(&m, 1, false);
    let g = geometry(&v, &m);
    let bar = v.bar(&g, &m).expect("more than is seen");
    assert!(
        bar.thumb.h >= m.handle_grip.min(bar.track.h),
        "a grip a hand can take"
    );
    assert_eq!(
        bar.thumb.y + bar.thumb.h,
        bar.track.y + bar.track.h,
        "following: at the end"
    );
    let on = (f64::from(bar.thumb.x + 1.0), f64::from(bar.thumb.y + 2.0));
    let before = seen(&v, &m);
    v.press(on, &inp);
    assert_eq!(seen(&v, &m), before, "a press on the thumb keeps the view");
    v.drag((on.0, on.1 - 60.0), &inp);
    assert!(!v.follow, "read back");
    assert_ne!(seen(&v, &m), before);
    v.drag((on.0, on.1 + 400.0), &inp);
    v.release((on.0, on.1 + 400.0), true, &inp);
    assert!(v.follow, "back at the end, it follows");
    assert_eq!(seen(&v, &m), before);
}

/// **A press and a drag select, and a copy is exactly that text**: across
/// lines, by character; two clicks take a word.
#[test]
fn a_drag_selects_and_a_copy_is_exactly_what_was_selected() {
    let m = Metrics::default();
    let mut v = view(r#"{"autoscroll": false}"#);
    write(&mut v, "out", "alpha beta\ngamma delta\nepsilon\n");
    let g = geometry(&v, &m);
    let rows = v.visible(&g);
    assert_eq!(rows.len(), 3);
    // from inside "beta" on the first row to inside "delta" on the second
    let x_of = |row: &str, at: usize| {
        f64::from(g.text.x + font::width_in(Family::Mono, &row[..at], g.size)) + 0.1
    };
    let y_of = |i: usize| f64::from(rows[i].3 + g.row_h * 0.5);
    let inp = input(&m, 1, false);
    v.press((x_of("alpha beta", 6), y_of(0)), &inp);
    v.drag((x_of("gamma delta", 8), y_of(1)), &inp);
    v.release((0.0, 0.0), true, &inp);
    let mut clip = Clip::default();
    let mut keys = KeyInput {
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
        clipboard: &mut clip,
        cursor: None,
    };
    v.key(&Key::Char('c'), &mut keys).unwrap();
    assert_eq!(clip.text(), "beta\ngamma de");
    let two = input(&m, 2, false);
    v.press((x_of("epsilon", 2), y_of(2)), &two);
    assert_eq!(v.selected_text().as_deref(), Some("epsilon"));
}

/// **Ten thousand lines at once cost the rows seen**: the write measures
/// nothing, and a frame measures and draws only what fits.
#[test]
fn ten_thousand_lines_at_once_measure_only_the_rows_seen() {
    let m = Metrics::default();
    let mut v = view(r#"{}"#);
    let block: String = (0..10_000).map(|i| format!("line number {i}\n")).collect();
    let started = std::time::Instant::now();
    write(&mut v, "out", &block);
    assert_eq!(v.post.len(), 10_000);
    assert!(
        v.rows.borrow().lines.is_empty(),
        "the write measured nothing"
    );
    let theme = Theme::default();
    let world = World::default();
    let ctx = Ctx {
        world: &world,
        metrics: &m,
        rect: RECT,
        indent: 0.0,
        clip: None,
        scale: 1.0,
        time: None,
        focused: false,
        hovered: false,
        clock: 0.0,
        text_fit: 1.0,
        header: None,
    };
    let mut mesh = Mesh::new();
    v.draw(&mut Draw::new(&mut mesh, &m, &theme), &ctx);
    let took = started.elapsed();
    // the frame budget, held where it is measured: an optimized build
    #[cfg(not(debug_assertions))]
    assert!(
        took < std::time::Duration::from_millis(16),
        "the write and a frame took {took:?}"
    );
    eprintln!("10000 lines written and drawn in {took:?}");
    let measured = v.rows.borrow().lines.len();
    assert!(measured < 40, "measured {measured} lines");
    assert_eq!(seen(&v, &m).last().unwrap(), "line number 9999");
}

/// **The menu clears, and the oldest lines go past `keep`.**
#[test]
fn clear_and_keep() {
    let mut v = view(r#"{"keep": 3}"#);
    for i in 0..5 {
        write(&mut v, "out", &format!("{i}\n"));
    }
    assert_eq!(v.post.text(), "2\n3\n4");
    let m = Metrics::default();
    let entries = v.context_menu((0.0, 0.0), &input(&m, 1, false)).unwrap();
    assert!(entries.iter().any(|e| e.label == "Clear"));
    let mut clip = Clip::default();
    let mut keys = KeyInput {
        mods: Mods::default(),
        clipboard: &mut clip,
        cursor: None,
    };
    v.command("clear", &mut keys).unwrap();
    assert!(v.post.is_empty());
    assert!(v.set("clear", &Value::from(1)));
}
