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

/// `text` with the selection from the `[` to the `]` in it, which are
/// taken out, the caret at the `]`.
fn selecting(text: &str) -> Code {
    let a = text.find('[').expect("a start");
    let b = text.find(']').expect("an end") - 1;
    let mut code = Code::new(&text.replace(['[', ']'], ""));
    code.select(a, b);
    code
}

fn python(mut c: Code) -> Code {
    c.set_language(Language::Python);
    c.parse(None);
    c
}

fn type_all(c: &mut Code, s: &str) {
    for ch in s.chars() {
        c.type_char(ch);
        c.parse(None);
    }
}

/// **A bracket typed brings its closer, which is typed over**, and the
/// run is one step of the history; Backspace between the two takes both.
/// Before a word the bracket comes alone, and a closer is not typed over
/// once the caret has left its line.
#[test]
fn brackets_typed_close_and_are_typed_over() {
    let mut c = python(at("|"));
    type_all(&mut c, "print(x");
    assert_eq!(shown(&c), "print(x|)");
    type_all(&mut c, ")");
    assert_eq!(shown(&c), "print(x)|", "typed over");
    assert!(c.step(false));
    assert_eq!(c.text(), "", "one run of typing");
    let mut c = python(at("|"));
    type_all(&mut c, "[");
    assert_eq!(shown(&c), "[|]");
    c.backspace(false);
    assert_eq!(shown(&c), "|", "both go");
    let mut c = python(at("|abc"));
    type_all(&mut c, "(");
    assert_eq!(shown(&c), "(|abc", "a word follows");
    let mut c = python(at("a|\nb"));
    type_all(&mut c, "(");
    assert_eq!(shown(&c), "a(|)\nb");
    c.set_caret(Caret {
        pos: 5,
        anchor: None,
    });
    c.set_caret(Caret {
        pos: 2,
        anchor: None,
    });
    type_all(&mut c, ")");
    assert_eq!(shown(&c), "a()|)\nb", "the line was left");
}

/// **A quote opens a string where one may start**: after a word it is an
/// apostrophe, after a prefix it opens one, inside a string it closes it,
/// and in Python three of them open three that close.
#[test]
fn quotes_typed_close_where_a_string_starts() {
    let mut c = python(at("x = |"));
    type_all(&mut c, "\"ab");
    assert_eq!(shown(&c), "x = \"ab|\"");
    type_all(&mut c, "\"");
    assert_eq!(shown(&c), "x = \"ab\"|");
    let mut c = python(at("# don|"));
    type_all(&mut c, "'");
    assert_eq!(shown(&c), "# don'|");
    let mut c = python(at("x = f|"));
    type_all(&mut c, "'");
    assert_eq!(shown(&c), "x = f'|'");
    let mut c = python(at("x = \"a | b\""));
    type_all(&mut c, "\"");
    assert_eq!(shown(&c), "x = \"a \"| b\"", "inside a string, one quote");
    let mut c = python(at("|"));
    type_all(&mut c, "\"\"\"");
    assert_eq!(shown(&c), "\"\"\"|\"\"\"");
    type_all(&mut c, "doc\"");
    assert_eq!(shown(&c), "\"\"\"doc\"\"\"|", "the three typed over");
    let mut c = at("|");
    c.set_language(Language::JavaScript);
    type_all(&mut c, "`");
    assert_eq!(shown(&c), "`|`");
}

/// **A selection typed over with a bracket or a quote is put between the
/// two**, and stays selected.
#[test]
fn a_selection_is_put_between_a_pair() {
    let mut c = python(selecting("x = [a + b]"));
    type_all(&mut c, "(");
    assert_eq!(c.text(), "x = (a + b)");
    assert_eq!(c.selected(), Some("a + b"));
    type_all(&mut c, "\"");
    assert_eq!(c.text(), "x = (\"a + b\")");
    assert_eq!(c.selected(), Some("a + b"));
}

/// **A comment goes on in one column and comes off again**: the least
/// indentation of the lines that have text, empty lines left alone; a line
/// already commented among others that are not is commented again.
#[test]
fn a_comment_is_toggled_on_the_lines_selected() {
    let text = "def f():\n    x = 1\n\n    y = 2\n";
    let mut c = python(Code::new(text));
    c.select(0, text.len() - 1);
    assert!(c.toggle_comment());
    assert_eq!(c.text(), "# def f():\n#     x = 1\n\n#     y = 2\n");
    assert!(c.toggle_comment());
    assert_eq!(c.text(), text);
    let mut c = python(at("def f():\n    x = |1\n\n    y = 2\n"));
    c.toggle_comment();
    assert_eq!(
        shown(&c),
        "def f():\n    # x = |1\n\n    y = 2\n",
        "the caret stays"
    );
    let mut c = python(selecting("[    # a\n    b]\n"));
    c.toggle_comment();
    assert_eq!(c.text(), "    # # a\n    # b\n");
    let mut c = at("|x");
    c.set_language(Language::JavaScript);
    c.toggle_comment();
    assert_eq!(c.text(), "// x");
    assert!(!at("|x").toggle_comment(), "plain text has none");
}

fn finder(text: &str) -> Finder {
    Finder::new(&find::Query {
        text: text.into(),
        ..Default::default()
    })
}

/// **The next and the previous match, round past either end**: case does
/// not count unless asked, a whole word is one, and a line end is
/// written `\n`.
#[test]
fn find_goes_to_the_next_and_previous_match() {
    let mut c = at("|Alpha alphabet\nalpha");
    let f = finder("alpha");
    assert!(c.find(&f, true));
    assert_eq!(c.caret().selection(), Some((0, 5)));
    c.find(&f, true);
    assert_eq!(c.caret().selection(), Some((6, 11)));
    c.find(&f, true);
    c.find(&f, true);
    assert_eq!(c.caret().selection(), Some((0, 5)), "round past the end");
    c.find(&f, false);
    assert_eq!(
        c.caret().selection(),
        Some((15, 20)),
        "round past the start"
    );
    let word = Finder::new(&find::Query {
        text: "alpha".into(),
        case: true,
        word: true,
        ..Default::default()
    });
    assert_eq!(word.within(c.text(), 0, c.text().len()), vec![(15, 20)]);
    assert_eq!(finder("bet\\nal").within(c.text(), 0, 99), vec![(11, 17)]);
    assert!(
        Finder::new(&find::Query {
            text: "(".into(),
            regexp: true,
            ..Default::default()
        })
        .error()
        .is_some()
    );
    assert!(finder("").error().is_none() && !finder("").ready());
}

/// **A replacement replaces the match selected and selects the next**;
/// with none selected it only finds. All of them are one step, and a
/// regular expression's groups are read into the replacement.
#[test]
fn matches_are_replaced_one_or_all() {
    let mut c = at("|a1 b2 a3");
    let f = finder("a");
    assert!(c.replace(&f, "x"));
    assert_eq!(c.text(), "a1 b2 a3", "the first replace finds");
    c.replace(&f, "xy");
    assert_eq!(c.text(), "xy1 b2 a3");
    assert_eq!(c.caret().selection(), Some((7, 8)), "the next selected");
    let re = Finder::new(&find::Query {
        text: "([a-z])(\\d)".into(),
        regexp: true,
        ..Default::default()
    });
    let mut c = at("|a1 b2 a3");
    assert_eq!(c.replace_all(&re, "$2$1$$"), 3);
    assert_eq!(c.text(), "1a$ 2b$ 3a$");
    assert!(c.step(false));
    assert_eq!(c.text(), "a1 b2 a3", "one step");
    let mut c = at("|a b");
    c.replace_all(&finder(" "), "\\n");
    assert_eq!(c.text(), "a\nb");
}

/// **A line is gone to by its number**, on or back from the caret's, as a
/// share of the text, and with a column.
#[test]
fn go_to_a_line() {
    let text: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    let mut c = Code::new(&text);
    assert!(c.go_to("3"));
    assert_eq!(c.line_col(), (2, 0));
    c.go_to("+4");
    assert_eq!(c.line_col(), (6, 0));
    c.go_to("-2:3");
    assert_eq!(c.line_col(), (4, 3));
    c.go_to("50%");
    assert_eq!(c.line_col(), (5, 0), "half of eleven lines, rounded");
    c.go_to("99");
    assert_eq!(c.line_col(), (10, 0), "the last");
    assert!(!c.go_to("x"));
    assert!(!c.go_to("3:x"));
}

/// **The other places the selected text stands**, the selection left out.
#[test]
fn the_other_occurrences_of_the_selection() {
    let c = selecting("[x] = x + y\nx");
    assert_eq!(c.occurrences(0..2), vec![(4, 5), (10, 11)]);
    assert_eq!(c.occurrences(1..2), vec![(10, 11)], "the lines asked");
    assert!(at("|x x").occurrences(0..1).is_empty(), "nothing selected");
    assert!(selecting("[ ] x").occurrences(0..1).is_empty(), "blank");
}

/// **The bracket by the caret is paired**: the one before it first, the
/// nested ones counted, none when nothing closes it; and, read by the
/// tree, one inside a string is not a bracket.
#[test]
fn the_bracket_by_the_caret_is_paired() {
    let c = python(at("f(a[1]|)"));
    assert_eq!(c.bracket_pair(), Some((5, Some(3))));
    let c = python(at("f|(a[1])"));
    assert_eq!(c.bracket_pair(), Some((1, Some(6))));
    let c = python(at("f(a|"));
    assert_eq!(c.bracket_pair(), None);
    let c = python(at("f(|a"));
    assert_eq!(c.bracket_pair(), Some((1, None)));
    let c = at("(a]|");
    assert_eq!(c.bracket_pair(), Some((2, None)), "another kind closes");
    let c = at("{[(|)]}");
    assert_eq!(c.bracket_pair(), Some((2, Some(3))));
    #[cfg(feature = "highlight")]
    {
        let c = python(at("s = \"(|\" + f(x)"));
        assert_eq!(c.bracket_pair(), None, "in a string");
        let c = python(at("|(x)  # )"));
        assert_eq!(c.bracket_pair(), Some((0, Some(2))));
    }
}

// -- the current editor's region and indentation tests, ported case by case

/// `text` with the caret at the `|`, read as `language` and parsed.
fn read_as(text: &str, language: Language) -> Code {
    let mut c = at(text);
    c.set_language(language);
    c.parse(None);
    c
}

fn slice(c: &Code, r: Option<std::ops::Range<usize>>) -> Option<&str> {
    r.map(|r| &c.text()[r])
}

/// The line under the caret, or nothing on a blank one.
#[test]
fn regions_the_line_under_the_caret() {
    let c = read_as("a = 1\nb = |2\n", Language::Python);
    assert_eq!(slice(&c, c.line_or_selection()), Some("b = 2"));
    let c = read_as("a = 1\n|\nb = 2", Language::Python);
    assert_eq!(c.line_or_selection(), None);
}

/// A selection starts at its line when only indentation precedes it.
#[test]
fn regions_a_selection_starts_at_its_line() {
    let text = "def f():\n    x = 1\n    return x\n";
    let mut c = Code::new(text);
    let from = text.find("x = 1").unwrap();
    let to = text[from + 5..].find("x\n").unwrap() + from + 5 + 1;
    c.select(from, to);
    assert_eq!(
        slice(&c, c.line_or_selection()),
        Some("    x = 1\n    return x")
    );
}

/// The top-level statement holds the whole def the caret is in.
#[cfg(feature = "highlight")]
#[test]
fn regions_the_top_level_statement() {
    let c = read_as(
        "import os\n\ndef f():\n    a = 1\n\n    return |a\n\nprint(f())\n",
        Language::Python,
    );
    assert_eq!(
        slice(&c, c.top_level_block()),
        Some("def f():\n    a = 1\n\n    return a")
    );
}

/// Contiguous statements run until a blank line, and a def stays whole.
#[cfg(feature = "highlight")]
#[test]
fn regions_contiguous_statements() {
    let c = read_as("x = 1\ny = |2\nz = 3\n\nw = 4\n", Language::Python);
    assert_eq!(slice(&c, c.paragraph_block()), Some("x = 1\ny = 2\nz = 3"));
    let c = read_as(
        "def f():\n    a = 1\n\n    return a\ng = |f()\n\nh = 0\n",
        Language::Python,
    );
    assert_eq!(
        slice(&c, c.paragraph_block()),
        Some("def f():\n    a = 1\n\n    return a\ng = f()")
    );
}

/// A cell marker is a comment that starts with %%, with or without a title.
#[test]
fn regions_a_cell_marker() {
    use regions::is_cell_marker;
    assert!(is_cell_marker("# %%"));
    assert!(is_cell_marker("#%% Setup"));
    assert!(is_cell_marker("# %% [markdown]"));
    assert!(!is_cell_marker("x = 1  # %%"));
    assert!(!is_cell_marker("# % not a cell"));
    assert!(is_cell_marker("// %% Setup"));
    assert!(is_cell_marker("//%%"));
    assert!(!is_cell_marker("x = 1 // %%"));
}

/// In JavaScript, the top-level statement and the cell are read the same
/// way.
#[cfg(feature = "highlight")]
#[test]
fn regions_in_javascript() {
    let c = read_as(
        "const a = 1;\nfunction f(x) {\n  return |x;\n}\nf(a);\n",
        Language::JavaScript,
    );
    assert_eq!(
        slice(&c, c.top_level_block()),
        Some("function f(x) {\n  return x;\n}")
    );
    let c = read_as(
        "// %% one\nconst a = 1;\n\n// %% two\nconst |b = 2;\nb * a\n",
        Language::JavaScript,
    );
    assert_eq!(
        slice(&c, c.cell_at(&c.cell_markers())),
        Some("const b = 2;\nb * a")
    );
}

/// The cell under the caret, without the blank lines at its edges.
#[test]
fn regions_the_cell_under_the_caret() {
    let c = read_as(
        "# %% one\na = 1\n\n# %% two\n\nb = |2\nc = 3\n\n# %% three\nd = 4\n",
        Language::Python,
    );
    assert_eq!(c.cell_markers(), vec![0, 3, 8]);
    assert_eq!(
        slice(&c, c.cell_at(&c.cell_markers())),
        Some("b = 2\nc = 3")
    );
    let c = read_as("# %% one\n|\n# %% two\nx = 1\n", Language::Python);
    assert_eq!(c.cell_at(&c.cell_markers()), None);
}

/// **A block moves the caret past it**: to the next line, or past the next
/// cell's marker; with no tree the statement is the line.
#[test]
fn regions_a_block_and_the_caret_past_it() {
    let mut c = read_as("# %% one\na = |1\n\n# %% two\nb = 2\n", Language::Python);
    let (r, cell) = c.block(regions::Block::Statement).unwrap();
    assert!(cell);
    assert_eq!(&c.text()[r.clone()], "a = 1");
    c.advance(&r, cell);
    assert_eq!(c.line_col(), (4, 0), "past the next marker");
    let mut c = read_as("x = |1\ny = 2\n", Language::Plain);
    let (r, cell) = c.block(regions::Block::Statement).unwrap();
    assert_eq!((&c.text()[r.clone()], cell), ("x = 1", false));
    c.advance(&r, cell);
    assert_eq!(c.line_col(), (1, 0));
}

/// A level is the unit the preference names.
#[test]
fn indent_a_level_is_the_unit_named() {
    let unit = |v: serde_json::Value| Indent::parse(&v).unwrap().unit();
    assert_eq!(unit(serde_json::json!("tab")), "\t");
    assert_eq!(unit(serde_json::json!(2)), "  ");
    assert_eq!(unit(serde_json::json!("4")), "    ");
}

/// Tab with spaces reaches the next stop, not a whole level more; with
/// tabs it inserts one tab character.
#[test]
fn indent_tab_reaches_the_next_stop() {
    let tabbed = |text: &str, indent: Indent| {
        let mut c = at(text);
        c.indent = indent;
        assert!(c.indent_more());
        c.text().to_string()
    };
    assert_eq!(tabbed("|x", Indent::Spaces(4)), "    x");
    assert_eq!(tabbed("  |x", Indent::Spaces(4)), "    x");
    assert_eq!(tabbed("ab|c", Indent::Spaces(4)), "ab  c");
    assert_eq!(tabbed("  |x", Indent::Tab), "  \tx");
}
