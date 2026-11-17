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
    c.set_language(Language::Python);
    c.newline();
    assert_eq!(shown(&c), "    x = 1\n    |");
    let mut c = at("def f():  # a comment|");
    c.set_language(Language::Python);
    c.newline();
    assert_eq!(shown(&c), "def f():  # a comment\n    |");
    let mut c = at("s = '#:'|");
    c.set_language(Language::Python);
    c.newline();
    assert_eq!(
        shown(&c),
        "s = '#:'\n|",
        "a colon in a string opens nothing"
    );
    let mut c = at("  if (a) {|}");
    c.set_language(Language::JavaScript);
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

/// The kinds of `code`'s text, each with the text it covers.
#[cfg(feature = "highlight")]
fn kinds(code: &Code) -> Vec<(String, syntax::Kind)> {
    assert!(code.parse(None));
    code.spans(0..code.lines())
        .into_iter()
        .map(|s| (code.text()[s.start..s.end].to_string(), s.kind))
        .collect()
}

/// **Python, by kind**: a definition's keyword and name, a parameter, a
/// keyword that returns, a string with an escape in it, a number, a
/// comment, a constant the language names.
#[cfg(feature = "highlight")]
#[test]
fn python_is_read_by_kind() {
    use syntax::Kind::*;
    let mut c = Code::new("def f(x):\n    return \"a\\n\" + 1  # one\nNone\n");
    c.set_language(Language::Python);
    let k = kinds(&c);
    let has = |text: &str, kind| k.iter().any(|(t, k)| t == text && *k == kind);
    assert!(has("def", Keyword), "{k:?}");
    assert!(has("f", Function), "{k:?}");
    assert!(has("x", Name), "{k:?}");
    assert!(has("return", Control), "{k:?}");
    assert!(has("\\n", Operator), "the escape inside the string: {k:?}");
    assert!(has("\"a", String), "{k:?}");
    assert!(has("1", Number), "{k:?}");
    assert!(has("# one", Comment), "{k:?}");
    assert!(has("None", Keyword), "{k:?}");
}

/// **JavaScript, by kind**: a regular expression, a keyword that branches,
/// a call, a name with a capital.
#[cfg(feature = "highlight")]
#[test]
fn javascript_is_read_by_kind() {
    use syntax::Kind::*;
    let mut c = Code::new("const re = /a+/;\nif (re) { run(new Thing()); }\n");
    c.set_language(Language::JavaScript);
    let k = kinds(&c);
    let has = |text: &str, kind| k.iter().any(|(t, k)| t == text && *k == kind);
    assert!(has("const", Keyword), "{k:?}");
    assert!(has("/a+/", Regexp), "{k:?}");
    assert!(has("if", Control), "{k:?}");
    assert!(has("run", Function), "{k:?}");
    assert!(has("Thing", Type), "{k:?}");
}

/// **Plain text has no kinds**, and a language set later reads the text.
#[cfg(feature = "highlight")]
#[test]
fn plain_text_has_no_kinds() {
    let mut c = Code::new("def f(): pass\n");
    assert!(kinds(&c).is_empty());
    c.set_language(Language::Python);
    assert!(!kinds(&c).is_empty());
}

/// **Only the lines asked about are read**, and a string that opened above
/// them is still a string in them.
#[cfg(feature = "highlight")]
#[test]
fn only_the_lines_asked_about_are_read() {
    let mut c = Code::new("x = 1\ns = \"\"\"one\ntwo\nthree\"\"\"\ny = 2\n");
    c.set_language(Language::Python);
    c.parse(None);
    let (start, _) = c.line(2).unwrap();
    let (end, three) = c.line(3).unwrap();
    let spans = c.spans(2..4);
    assert!(
        spans
            .iter()
            .all(|s| s.start >= start && s.end <= end + three.len())
    );
    assert_eq!(spans.len(), 1, "{spans:?}");
    assert_eq!(spans[0].kind, syntax::Kind::String);
    assert_eq!((spans[0].start, spans[0].end), (start, end + three.len()));
}

/// **The tree an edit is told to reads as a fresh one does**: after every
/// kind of edit -- typing, a new line, deleting across lines, pasting,
/// indenting, undoing and redoing -- the kinds of the whole text are the
/// ones a text parsed from nothing has.
#[cfg(feature = "highlight")]
#[test]
fn an_edited_tree_reads_as_a_fresh_one() {
    let mut c = at("def f(x):\n    return x|\n\nclass A:\n    pass\n");
    c.set_language(Language::Python);
    let check = |c: &Code, what: &str| {
        c.parse(None);
        let mut fresh = Code::new(c.text());
        fresh.set_language(Language::Python);
        fresh.parse(None);
        assert_eq!(
            c.spans(0..c.lines()),
            fresh.spans(0..fresh.lines()),
            "after {what}: {:?}",
            c.text()
        );
    };
    check(&c, "nothing");
    for ch in [" ", "+", " ", "\"", "s", "\""] {
        c.insert(ch);
        check(&c, "typing");
    }
    c.newline();
    check(&c, "a new line");
    c.insert("# note");
    check(&c, "a comment");
    c.set_caret(Caret {
        pos: 4,
        anchor: Some(20),
    });
    c.backspace(false);
    check(&c, "deleting across lines");
    c.paste("g(y):\n    '''doc\n    '''\n    return ");
    check(&c, "a paste");
    c.select_all();
    c.indent_more();
    check(&c, "indenting every line");
    c.indent_less();
    check(&c, "taking the level back");
    while c.step(false) {
        check(&c, "an undo");
    }
    while c.step(true) {
        check(&c, "a redo");
    }
    c.set_text("x = 'new'\n");
    check(&c, "a text set whole");
}

/// **A parse given no time stops, and the next carries it on**: with no
/// time at all each call reads a little, the tree meanwhile the one before
/// with its places moved, and when it catches up it reads as a fresh one --
/// also when an edit comes between two calls and the parse starts again.
#[cfg(feature = "highlight")]
#[test]
fn a_parse_out_of_time_is_carried_on() {
    let text: String = (0..300)
        .map(|i| format!("def f{i}(x):\n    return x + {i}  # c\n"))
        .collect();
    let mut c = Code::new(&text);
    c.set_language(Language::Python);
    assert!(c.parse(None));
    let before = c.spans(0..2);
    c.set_caret(Caret {
        pos: 0,
        anchor: None,
    });
    c.insert("'");
    assert!(c.parsing());
    let none = std::time::Duration::ZERO;
    assert!(!c.parse(Some(none)), "no time, not done");
    // the tree before, moved by the quote: the definition one byte on
    let moved: Vec<_> = c
        .spans(0..2)
        .iter()
        .map(|s| (s.start - 1, s.end - 1, s.kind))
        .collect();
    let was: Vec<_> = before.iter().map(|s| (s.start, s.end, s.kind)).collect();
    assert_eq!(moved, was);
    for _ in 0..3 {
        c.parse(Some(none));
    }
    // an edit between two calls: the parse starts again
    c.backspace(false);
    c.insert("#");
    let mut calls = 0;
    while !c.parse(Some(none)) {
        calls += 1;
        assert!(calls < 100_000, "the parse goes on");
    }
    assert!(calls > 1, "{calls} calls");
    assert!(!c.parsing());
    let mut fresh = Code::new(c.text());
    fresh.set_language(Language::Python);
    fresh.parse(None);
    assert_eq!(c.spans(0..c.lines()), fresh.spans(0..fresh.lines()));
}
