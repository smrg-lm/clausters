//! The code element's tests: keys and the hand on the text, the dirty
//! marker it reports, and what a frame draws of a long file.

use super::*;
use crate::host::clipboard::Clip;
use crate::host::widget::element::Mods;
use crate::host::world::World;
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::theme::Theme;

const RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 480.0,
    h: 240.0,
};

fn view(json: &str) -> CodeView {
    CodeView::new(&serde_json::from_str(json).unwrap())
}

/// Presses `keys` with `mods` held, through the element's own `key`, and
/// answers the last one's events.
fn press(v: &mut CodeView, clip: &mut Clip, mods: Mods, keys: &[Key]) -> Option<Events> {
    let mut last = None;
    for k in keys {
        let mut input = KeyInput {
            mods,
            clipboard: &mut *clip,
            cursor: None,
        };
        last = v.key(k, &mut input);
    }
    last
}

fn typed(s: &str) -> Vec<Key> {
    s.chars()
        .map(|c| if c == '\n' { Key::Enter } else { Key::Char(c) })
        .collect()
}

const CTRL: Mods = Mods {
    shift: false,
    ctrl: true,
    alt: false,
};
const SHIFT: Mods = Mods {
    shift: true,
    ctrl: false,
    alt: false,
};

/// **Typing, selecting, cutting, pasting, undoing and indenting**, through
/// the keys, the text after each step what it should be.
#[test]
fn the_keys_edit_the_text() {
    let mut v = view(r#"{"language": "python"}"#);
    let mut clip = Clip::default();
    let none = Mods::default();
    press(&mut v, &mut clip, none, &typed("def f():\nreturn 1"));
    assert_eq!(v.code.text(), "def f():\n    return 1");
    // select "1" and cut it
    press(&mut v, &mut clip, SHIFT, &[Key::Left]);
    press(&mut v, &mut clip, CTRL, &[Key::Char('x')]);
    assert_eq!(v.code.text(), "def f():\n    return ");
    assert_eq!(clip.text(), "1");
    press(&mut v, &mut clip, CTRL, &[Key::Char('v'), Key::Char('v')]);
    assert_eq!(v.code.text(), "def f():\n    return 11");
    press(&mut v, &mut clip, CTRL, &[Key::Char('z')]);
    assert_eq!(v.code.text(), "def f():\n    return 1");
    // Shift+Tab takes the level off, Tab puts it back
    press(&mut v, &mut clip, SHIFT, &[Key::Tab]);
    assert_eq!(v.code.text(), "def f():\nreturn 1");
    press(&mut v, &mut clip, none, &[Key::Home, Key::Tab]);
    assert_eq!(v.code.text(), "def f():\n    return 1");
    press(&mut v, &mut clip, none, &[Key::End, Key::Tab]);
    assert_eq!(
        v.code.text(),
        "def f():\n    return 1    ",
        "Tab inserts a level"
    );
}

/// **The dirty marker is reported when it moves**, and an undo back to the
/// text as set clears it.
#[test]
fn dirty_is_reported_as_it_moves() {
    let mut v = view(r#"{"text": "x"}"#);
    let mut clip = Clip::default();
    let e = press(
        &mut v,
        &mut clip,
        Mods::default(),
        &[Key::End, Key::Char('y')],
    )
    .unwrap();
    assert!(!e.is_empty(), "dirty now");
    let e = press(&mut v, &mut clip, Mods::default(), &[Key::Char('z')]).unwrap();
    assert!(e.is_empty(), "still dirty: nothing to say");
    let e = press(&mut v, &mut clip, CTRL, &[Key::Char('z')]).unwrap();
    assert!(!e.is_empty(), "clean again");
    assert_eq!(v.code.text(), "x");
}

fn input<'a>(m: &'a Metrics, clicks: u32) -> Input<'a> {
    Input {
        metrics: m,
        indent: 0.0,
        rect: RECT,
        scale: 1.0,
        mods: Mods::default(),
        viewport: (480.0, 240.0),
        clicks,
        time: None,
    }
}

/// **A press lands on the character under it; two select a word; a drag
/// selects to where it is.**
#[test]
fn the_hand_selects() {
    let m = Metrics::default();
    let mut v = view(r#"{"text": "alpha beta\ngamma"}"#);
    let g = v.geometry(RECT, 1.0, &m);
    let at = |col: f32, row: f32| {
        (
            f64::from(g.text.x + col * g.cell + 1.0),
            f64::from(g.text.y + (row + 0.5) * g.row_h),
        )
    };
    v.press(at(6.0, 0.0), &input(&m, 1));
    v.drag(at(3.0, 1.0), &input(&m, 1));
    v.release(at(3.0, 1.0), true, &input(&m, 1));
    assert_eq!(v.selected_text().as_deref(), Some("beta\ngam"));
    v.press(at(1.0, 1.0), &input(&m, 2));
    assert_eq!(v.selected_text().as_deref(), Some("gamma"));
}

/// **A frame of a long file draws the rows in view**, and the caret
/// brought into view scrolls it.
#[test]
fn a_frame_draws_the_rows_in_view() {
    let m = Metrics::default();
    let text: String = (0..5000).map(|i| format!("line {i}\n")).collect();
    let mut v = CodeView::new(&Map::new());
    v.set("text", &Value::from(text));
    let mut clip = Clip::default();
    press(&mut v, &mut clip, CTRL, &[Key::End]);
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
        focused: true,
        hovered: false,
        clock: 0.0,
        text_fit: 1.0,
    };
    let started = std::time::Instant::now();
    let mut mesh = Mesh::new();
    v.draw(&mut Draw::new(&mut mesh, &m, &theme), &ctx);
    eprintln!("a frame of 5000 lines: {:?}", started.elapsed());
    let (_, sy) = v.scroll.get();
    let g = v.geometry(RECT, 1.0, &m);
    assert!(
        sy > 4990.0 * g.row_h - g.text.h,
        "scrolled to the end: {sy}"
    );
}

/// **A key and the frame after it, in 5,000 lines of Python**: the edit
/// told to the tree, the rows in view parsed again and highlighted, and
/// drawn, with the find bar open on two thousand matches. The first frame
/// parses the whole file and is timed apart. The budget is 4 ms a key; a
/// release build is held to it.
#[test]
fn a_key_and_its_frame_in_five_thousand_lines_of_python() {
    let m = Metrics::default();
    let text: String = (0..1000)
        .map(|i| {
            format!(
                "def step_{i}(x, y=None):\n    \"\"\"Doc {i}.\"\"\"\n    if x > {i}:\n        return f\"{{x}}\" + 'a\\n'  # note\n    return compute(x, [1, 2.5])\n"
            )
        })
        .collect();
    let mut v = view(r#"{"language": "python"}"#);
    v.set("text", &Value::from(text));
    assert_eq!(v.code.lines(), 5001);
    let mut clip = Clip::default();
    // the caret in the middle of the file, in view
    v.code.place(v.code.line(2502).unwrap().0 + 8, false);
    v.reveal.set(true);
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
        focused: true,
        hovered: false,
        clock: 0.0,
        text_fit: 1.0,
    };
    let frame = |v: &CodeView| {
        let mut mesh = Mesh::new();
        v.draw(&mut Draw::new(&mut mesh, &m, &theme), &ctx);
        mesh
    };
    let started = std::time::Instant::now();
    let first = frame(&v);
    eprintln!("the first frame, the file parsed: {:?}", started.elapsed());
    // the find bar open on a word of every function, the keys the text's:
    // its matches are found again after every key
    v.bar.open_find(Some("return"));
    v.bar.unfocus();
    let mut worst = std::time::Duration::ZERO;
    let mut keys = typed("if (y): z = 'q' # w\n");
    keys.extend(std::iter::repeat_n(Key::Backspace, 6));
    for k in &keys {
        let started = std::time::Instant::now();
        press(&mut v, &mut clip, Mods::default(), std::slice::from_ref(k));
        frame(&v);
        worst = worst.max(started.elapsed());
    }
    eprintln!("the worst key and its frame in 5000 lines: {worst:?}");
    #[cfg(not(debug_assertions))]
    assert!(
        worst < std::time::Duration::from_millis(4),
        "{worst:?} a key"
    );
    // a parse left behind asks for the tick, and its ticks catch it up
    let live = Live {
        bus: None,
        sample_rate: 0.0,
        dt: 0.016,
        histories: &Default::default(),
    };
    let mut ticks = 0;
    let mut worst_tick = std::time::Duration::ZERO;
    while v.needs().animated {
        let started = std::time::Instant::now();
        v.tick(&live);
        frame(&v);
        worst_tick = worst_tick.max(started.elapsed());
        ticks += 1;
        assert!(ticks < 1000, "the parse catches up");
    }
    eprintln!("caught up in {ticks} ticks, the worst with its frame {worst_tick:?}");
    #[cfg(not(debug_assertions))]
    assert!(
        worst_tick < std::time::Duration::from_millis(4),
        "{worst_tick:?} a tick"
    );
    let mut fresh = Code::new(v.code.text());
    fresh.set_language(Language::Python);
    fresh.parse(None);
    assert_eq!(v.code.spans(0..5001), fresh.spans(0..5001));
    // the rows in view are drawn in more than one color
    let mut colors: std::collections::HashSet<[u32; 4]> = first
        .colors_by_y()
        .map(|(_, c)| c.map(f32::to_bits))
        .collect();
    #[cfg(feature = "font-atlas")]
    colors.extend(
        first
            .glyph_vertices()
            .chunks(8)
            .map(|v| [v[4], v[5], v[6], v[7]].map(f32::to_bits)),
    );
    for role in [
        theme.syntax_keyword,
        theme.syntax_string,
        theme.syntax_comment,
    ] {
        assert_eq!(
            colors.contains(&role.map(f32::to_bits)),
            cfg!(feature = "highlight"),
            "{role:?} drawn with highlighting and only then"
        );
    }
}

/// **Tab is the element's**, so the focus ring leaves it on Ctrl+Tab.
#[test]
fn tab_is_kept() {
    let v = view("{}");
    assert!(v.takes_tab());
}

/// **A line's cells are counted the same by its bytes and by its
/// graphemes**: a tab to its stop either way.
#[test]
fn cells_count_a_tab_to_its_stop() {
    assert_eq!(cells("abc"), 3);
    assert_eq!(cells("a\tb"), TAB_COLS + 1);
    assert_eq!(cells("\u{e9}\tb"), TAB_COLS + 1);
    assert_eq!(cells("abcd\t"), 2 * TAB_COLS);
}

const CTRL_ALT: Mods = Mods {
    shift: false,
    ctrl: true,
    alt: true,
};

/// One frame of `v` in `RECT`, focused or not.
fn frame(v: &CodeView, focused: bool) -> Mesh {
    let m = Metrics::default();
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
        focused,
        hovered: false,
        clock: 0.0,
        text_fit: 1.0,
    };
    let mut mesh = Mesh::new();
    v.draw(&mut Draw::new(&mut mesh, &m, &theme), &ctx);
    mesh
}

/// Whether `mesh` fills something in `color`.
fn fills(mesh: &Mesh, color: Color) -> bool {
    mesh.colors_by_y().any(|(_, c)| c == color)
}

fn found(v: &CodeView) -> Value {
    v.info()
        .into_iter()
        .find(|(k, _)| k == "found")
        .map(|(_, v)| v)
        .unwrap()
}

/// **Ctrl+F opens the bar on the text selected; Enter finds the next match
/// and Shift+Enter the previous**, round past either end; Escape closes
/// it, and F3 goes on finding with it closed.
#[test]
fn the_bar_finds() {
    let mut v = view(r#"{"text": "one two one\nthree one"}"#);
    let mut clip = Clip::default();
    v.code.select(0, 3);
    press(&mut v, &mut clip, CTRL, &[Key::Char('f')]);
    assert_eq!(v.bar.mode(), Some(Mode::Find));
    assert_eq!(v.bar.finder().query().text, "one");
    assert_eq!(found(&v), 3);
    press(&mut v, &mut clip, Mods::default(), &[Key::Enter]);
    assert_eq!(v.code.caret().selection(), Some((8, 11)));
    press(&mut v, &mut clip, Mods::default(), &[Key::Enter]);
    assert_eq!(v.code.caret().selection(), Some((18, 21)));
    press(&mut v, &mut clip, Mods::default(), &[Key::Enter]);
    assert_eq!(
        v.code.caret().selection(),
        Some((0, 3)),
        "round past the end"
    );
    press(&mut v, &mut clip, SHIFT, &[Key::Enter]);
    assert_eq!(v.code.caret().selection(), Some((18, 21)));
    press(&mut v, &mut clip, Mods::default(), &[Key::Escape]);
    assert_eq!(v.bar.mode(), None);
    press(&mut v, &mut clip, Mods::default(), &[Key::F(3)]);
    assert_eq!(
        v.code.caret().selection(),
        Some((0, 3)),
        "F3, the bar closed"
    );
    press(&mut v, &mut clip, SHIFT, &[Key::F(3)]);
    assert_eq!(v.code.caret().selection(), Some((18, 21)));
    // the keys back to the text: typing replaces the selection
    press(&mut v, &mut clip, Mods::default(), &typed("1"));
    assert_eq!(v.code.text(), "one two one\nthree 1");
}

/// **Replace and All**: Tab moves to the replacement, Enter replaces the
/// match selected and selects the next, All replaces the rest as one step.
#[test]
fn the_bar_replaces() {
    let mut v = view(r#"{"text": "a1 a2 a3"}"#);
    let mut clip = Clip::default();
    let none = Mods::default();
    press(&mut v, &mut clip, CTRL, &[Key::Char('f')]);
    press(&mut v, &mut clip, none, &typed("a"));
    press(&mut v, &mut clip, none, &[Key::Enter]);
    assert_eq!(v.code.caret().selection(), Some((0, 1)));
    press(&mut v, &mut clip, none, &[Key::Tab]);
    press(&mut v, &mut clip, none, &typed("b"));
    let e = press(&mut v, &mut clip, none, &[Key::Enter]).unwrap();
    assert!(!e.is_empty(), "the text went dirty");
    assert_eq!(v.code.text(), "b1 a2 a3");
    assert_eq!(
        v.code.caret().selection(),
        Some((3, 4)),
        "the next selected"
    );
    let mut input = KeyInput {
        mods: none,
        clipboard: &mut clip,
        cursor: None,
    };
    v.command("replace_all", &mut input);
    assert_eq!(v.code.text(), "b1 b2 b3");
    assert_eq!(found(&v), 0);
    // the field's keys while it has them; the text's once the bar closes
    press(&mut v, &mut clip, CTRL, &[Key::Char('z')]);
    assert_eq!(v.code.text(), "b1 b2 b3");
    press(&mut v, &mut clip, none, &[Key::Escape]);
    press(&mut v, &mut clip, CTRL, &[Key::Char('z')]);
    assert_eq!(v.code.text(), "b1 a2 a3", "All was one step");
}

/// **Ctrl+Alt+G goes to a line**: the line typed, Enter, the caret there
/// and the bar gone.
#[test]
fn the_bar_goes_to_a_line() {
    let text: String = (1..=100).map(|n| format!("line {n}\n")).collect();
    let mut v = view("{}");
    v.set("text", &Value::from(text));
    let mut clip = Clip::default();
    press(&mut v, &mut clip, CTRL_ALT, &[Key::Char('g')]);
    assert_eq!(v.bar.mode(), Some(Mode::Line));
    press(&mut v, &mut clip, Mods::default(), &typed("60:2"));
    press(&mut v, &mut clip, Mods::default(), &[Key::Enter]);
    assert_eq!(v.code.line_col(), (59, 2));
    assert_eq!(v.bar.mode(), None);
    frame(&v, true);
    let g = v.geometry(RECT, 1.0, &Metrics::default());
    let (_, sy) = v.scroll.get();
    let middle = sy + g.text.h * 0.5;
    assert!(
        (middle - 59.5 * g.row_h).abs() < g.row_h,
        "the line in the middle of the view"
    );
}

/// **The bar answers the hand**: a press on its field gives it the keys,
/// on a switch turns it, on the next match finds it; a press on the text
/// gives the text the keys back.
#[test]
fn the_bar_is_pressed() {
    let m = Metrics::default();
    let mut v = view(r#"{"text": "Ab ab AB"}"#);
    let mut clip = Clip::default();
    press(&mut v, &mut clip, CTRL, &[Key::Char('f')]);
    let layout = v.bar.layout(RECT, 1.0, &m).unwrap();
    let centre = |part: Part| {
        let r = layout.rect_of(part).unwrap();
        (f64::from(r.x + r.w * 0.5), f64::from(r.y + r.h * 0.5))
    };
    press(&mut v, &mut clip, Mods::default(), &[Key::Escape]);
    press(&mut v, &mut clip, CTRL, &[Key::Char('f')]);
    v.press(centre(Part::Field(Field::Find)), &input(&m, 1));
    press(&mut v, &mut clip, Mods::default(), &typed("ab"));
    assert_eq!(found(&v), 3, "case does not count");
    v.press(centre(Part::Case), &input(&m, 1));
    assert_eq!(found(&v), 1, "it does now");
    v.press(centre(Part::Next), &input(&m, 1));
    assert_eq!(v.code.caret().selection(), Some((3, 5)));
    let g = v.geometry(RECT, 1.0, &m);
    let on_text = (f64::from(g.text.x + 1.0), f64::from(g.text.y + 1.0));
    v.press(on_text, &input(&m, 1));
    v.release(on_text, true, &input(&m, 1));
    press(&mut v, &mut clip, Mods::default(), &typed("x"));
    assert_eq!(v.code.text(), "xAb ab AB", "the text's keys");
    v.press(centre(Part::Close), &input(&m, 1));
    assert_eq!(v.bar.mode(), None);
}

/// **Code is typed as code**: a bracket brings its closer and is typed
/// over, and Ctrl+/ comments the line and uncomments it.
#[test]
fn the_keys_type_code() {
    let mut v = view(r#"{"language": "python"}"#);
    let mut clip = Clip::default();
    let none = Mods::default();
    press(&mut v, &mut clip, none, &typed("f("));
    assert_eq!(v.code.text(), "f()");
    press(&mut v, &mut clip, none, &typed("x)"));
    assert_eq!(v.code.text(), "f(x)");
    assert_eq!(v.code.caret().pos, 4);
    press(&mut v, &mut clip, CTRL, &[Key::Char('/')]);
    assert_eq!(v.code.text(), "# f(x)");
    press(&mut v, &mut clip, CTRL, &[Key::Char('/')]);
    assert_eq!(v.code.text(), "f(x)");
}

/// **The marks are drawn**: the matches in their color and the selected
/// one in its own, the other places the selected text stands, the bracket
/// by the caret with its pair (or alone, in its own color), and that
/// only while the element has the keys.
#[test]
fn the_marks_are_drawn() {
    let theme = Theme::default();
    let mut clip = Clip::default();
    let mut v = view(r#"{"text": "f(x) + x\nx", "language": "python"}"#);
    v.code.select(2, 3);
    let mesh = frame(&v, true);
    assert!(fills(&mesh, with_alpha(theme.selection, 0.2)), "the others");
    press(&mut v, &mut clip, CTRL, &[Key::Char('f')]);
    press(&mut v, &mut clip, Mods::default(), &[Key::Enter]);
    let mesh = frame(&v, true);
    assert!(fills(&mesh, theme.find_match));
    assert!(fills(&mesh, theme.find_current));
    press(&mut v, &mut clip, Mods::default(), &[Key::Escape]);
    v.code.place(4, false);
    assert!(fills(&frame(&v, true), theme.bracket_match));
    assert!(
        !fills(&frame(&v, false), theme.bracket_match),
        "not focused"
    );
    let mut v = view(r#"{"text": "f(x", "language": "python"}"#);
    v.code.place(2, false);
    assert!(fills(&frame(&v, true), theme.bracket_mismatch));
}
