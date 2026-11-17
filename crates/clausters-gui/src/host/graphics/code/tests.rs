//! The code model's tests: what each edit does to the text, the history and
//! the indentation.

use super::*;

/// `text` with the caret at the `|` in it, which is taken out.
fn at(text: &str) -> Code {
    let pos = text.find('|').expect("a caret");
    let mut code = Code::new(&text.replace('|', ""));
    code.set_caret(Caret { pos, anchor: None });
    code
}

/// The text with a `|` where the caret is.
fn shown(code: &Code) -> String {
    let mut s = code.text().to_string();
    s.insert(code.caret().pos, '|');
    s
}

/// **A run of typing is one step**, and undo returns to the text as it was
/// set, which is clean again; redo goes forward through it.
#[test]
fn a_run_of_typing_is_one_step_and_undo_makes_it_clean() {
    let mut c = at("|");
    for ch in ["d", "e", "f"] {
        c.insert(ch);
    }
    c.insert(" ");
    assert!(c.dirty());
    c.newline();
    c.insert("x");
    assert_eq!(c.text(), "def \nx");
    assert!(c.step(false), "the x");
    assert_eq!(c.text(), "def \n");
    assert!(c.step(false), "the line");
    assert!(c.step(false), "the typing");
    assert_eq!(c.text(), "");
    assert!(!c.dirty(), "back to the text as set");
    assert!(!c.step(false));
    c.step(true);
    assert_eq!(c.text(), "def ");
    c.mark_saved();
    assert!(!c.dirty());
    c.step(true);
    assert!(c.dirty());
    c.step(false);
    assert!(!c.dirty(), "an undo back to the saved text is clean");
}

/// **Enter keeps the indentation, and adds a level after a block's
/// opening** -- a colon in Python, a bracket in both -- with the closing
/// bracket on a line of its own when the caret stood before it.
#[test]
fn enter_keeps_and_opens_indentation() {
    let mut c = at("    x = 1|");
    c.language = Language::Python;
    c.newline();
    assert_eq!(shown(&c), "    x = 1\n    |");
    let mut c = at("def f():  # a comment|");
    c.language = Language::Python;
    c.newline();
    assert_eq!(shown(&c), "def f():  # a comment\n    |");
    let mut c = at("s = '#:'|");
    c.language = Language::Python;
    c.newline();
    assert_eq!(
        shown(&c),
        "s = '#:'\n|",
        "a colon in a string opens nothing"
    );
    let mut c = at("  if (a) {|}");
    c.language = Language::JavaScript;
    c.indent = Indent::Spaces(2);
    c.newline();
    assert_eq!(shown(&c), "  if (a) {\n    |\n  }");
    let mut c = at("plain:|");
    c.newline();
    assert_eq!(shown(&c), "plain:\n|");
}

/// **Tab inserts up to the next level, or indents the selected lines;
/// Shift+Tab takes a level off; Backspace in leading spaces goes back a
/// level** -- and a tab level when the preference is a tab.
#[test]
fn tab_shift_tab_and_backspace_work_by_level() {
    let mut c = at("ab|");
    c.indent_more();
    assert_eq!(shown(&c), "ab  |", "to the next multiple of four");
    let mut c = Code::new("a\nb\nc");
    c.set_caret(Caret {
        pos: 0,
        anchor: Some(4),
    });
    c.indent_more();
    assert_eq!(c.text(), "    a\n    b\nc", "the third line is not touched");
    assert_eq!(c.selected(), Some("    a\n    b\n"));
    c.indent_less();
    assert_eq!(c.text(), "a\nb\nc");
    let mut c = at("        |x");
    c.backspace(false);
    assert_eq!(shown(&c), "    |x");
    c.backspace(false);
    assert_eq!(shown(&c), "|x");
    let mut c = at("|x");
    c.indent = Indent::Tab;
    c.indent_more();
    assert_eq!(shown(&c), "\t|x");
    c.indent_less();
    assert_eq!(shown(&c), "|x");
}

/// **Cut, paste and select all go through the history as steps**, and
/// the line index follows every edit.
#[test]
fn cut_paste_and_the_line_index() {
    let mut c = at("one two|");
    c.motion(|t, k| textedit::move_word_left(t, k, true));
    assert_eq!(c.cut().as_deref(), Some("two"));
    assert_eq!(c.text(), "one ");
    c.paste("2\nthree\r\nfour");
    assert_eq!(c.text(), "one 2\nthree\nfour");
    assert_eq!(c.lines(), 3);
    assert_eq!(c.line(1), Some((6, "three")));
    assert_eq!(c.line_of(7), 1);
    assert_eq!(c.line_col(), (2, 4));
    c.step(false);
    assert_eq!(c.text(), "one ");
    assert_eq!(c.lines(), 1);
    c.select_all();
    assert_eq!(c.selected(), Some("one "));
}

/// **Home goes to the first character that is not indentation, then to the
/// line's start**; a page moves the caret by its rows.
#[test]
fn home_is_smart_and_a_page_moves_by_rows() {
    let mut c = at("    abc|");
    c.home(false);
    assert_eq!(c.caret().pos, 4);
    c.home(false);
    assert_eq!(c.caret().pos, 0);
    let mut c = at("|a\nb\nc\nd");
    c.rows(2, false);
    assert_eq!(c.line_col(), (2, 0));
    c.rows(-5, true);
    assert_eq!(c.selected(), Some("a\nb\n"));
}

/// **The difference of two texts is the one change between them**, on
/// character boundaries.
#[test]
fn a_difference_is_one_change() {
    assert_eq!(diff("abcdef", "abXYef"), (2, "cd", "XY"));
    assert_eq!(diff("aaa", "aaaa"), (3, "", "a"));
    assert_eq!(diff("\u{e9}", "\u{e8}"), (0, "\u{e9}", "\u{e8}"));
}

/// **An edit of a long file costs what it changes**: a key in a file of
/// five thousand lines is well inside a frame.
#[test]
fn a_key_in_five_thousand_lines() {
    let text: String = (0..5000)
        .map(|i| format!("    value_{i} = compute({i}, other)  # a comment\n"))
        .collect();
    let mut c = Code::new(&text);
    c.set_caret(Caret {
        pos: text.len() / 2,
        anchor: None,
    });
    let started = std::time::Instant::now();
    for _ in 0..20 {
        c.insert("x");
    }
    let each = started.elapsed() / 20;
    #[cfg(not(debug_assertions))]
    assert!(each < std::time::Duration::from_millis(2), "{each:?} a key");
    eprintln!("a key in 5000 lines: {each:?}");
    c.step(false);
    assert_eq!(c.text(), text, "the run undone");
}
