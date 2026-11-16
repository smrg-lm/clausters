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

/// **Tab is the element's**, so the focus ring leaves it on Ctrl+Tab.
#[test]
fn tab_is_kept() {
    let v = view("{}");
    assert!(v.takes_tab());
}
