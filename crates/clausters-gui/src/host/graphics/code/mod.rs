//! **What a code element holds**: a text, the caret and the selection over
//! it, the history of what was done to it, and the editing that is code's
//! own -- indentation by a level, a new line that keeps it, the line index a
//! view reads its rows by.
//!
//! The text is one `String` and the caret is [`textedit`]'s, so every motion
//! and every plain edit is the one the `text` field already makes: by
//! grapheme, by word, by line, keeping the column up and down. What this adds
//! is what a field has no use for:
//!
//! - **a line index** ([`Code::line`]): the byte each line starts at, kept
//!   after every edit, so a view draws the rows it shows without scanning
//!   the text;
//! - **a history of changes**, not of copies: each step keeps what it
//!   removed and what it inserted where, so a long file and a long session
//!   cost what was typed, a run of typing is one step, and the text is
//!   **dirty** while it is not the one last marked saved -- an undo back to
//!   it makes it clean again;
//! - **indentation by the preference** ([`Indent`]): Tab inserts a level or
//!   indents the lines selected, Shift+Tab takes one off, Backspace in a
//!   line's leading spaces takes a level back, Enter keeps the line's
//!   indentation and adds a level after a line that opens a block
//!   ([`Language`]);
//! - **its syntax** ([`syntax`]): every change is told to the tree as it is
//!   made, [`Code::parse`] reads again what was touched, within a time if
//!   asked, and [`Code::spans`] says what each part of a range of the text
//!   is;
//! - **what typing code expects** ([`Code::type_char`]): a bracket or a
//!   quote typed brings its closing one, which is typed over and taken with
//!   it by a Backspace; a selection typed over with one is put between the
//!   two. A line comment is put on and taken off the lines selected
//!   ([`Code::toggle_comment`]), and the bracket by the caret is paired
//!   with its other ([`Code::bracket_pair`]);
//! - **finding** ([`find`]): the next and the previous match of a query,
//!   one replaced or all of them as one step, a line gone to, and the other
//!   places the selected text stands ([`Code::occurrences`]).
//!
//! No window, no device, no wire: what a key does to the text is tested
//! here as text.

use std::cell::RefCell;

use unicode_segmentation::UnicodeSegmentation;

use super::textedit::{self, Caret, Unit};

pub mod find;
pub mod regions;
pub mod syntax;

use find::Finder;
use syntax::{Bracket, Span, Syntax};

/// **One level of indentation**: a tab, or a number of spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Tab,
    Spaces(u8),
}

impl Default for Indent {
    fn default() -> Self {
        Indent::Spaces(4)
    }
}

impl Indent {
    /// The level a prop names: `"tab"`, or a number of spaces (1 to 8).
    pub fn parse(v: &serde_json::Value) -> Option<Indent> {
        if v.as_str() == Some("tab") {
            return Some(Indent::Tab);
        }
        let n = v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))?;
        (1..=8).contains(&n).then_some(Indent::Spaces(n as u8))
    }

    /// The level as text.
    pub fn unit(self) -> String {
        match self {
            Indent::Tab => "\t".into(),
            Indent::Spaces(n) => " ".repeat(n as usize),
        }
    }

    /// How many columns a level is: what a tab stands for when spaces are
    /// counted against it.
    fn width(self) -> usize {
        match self {
            Indent::Tab => 4,
            Indent::Spaces(n) => n as usize,
        }
    }
}

/// **What a line opening a block looks like**, which is all a language is
/// asked here: a new line after one is indented a level more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Plain,
    Python,
    JavaScript,
}

impl Language {
    /// The language a prop names: `python` or `javascript` (also `js`,
    /// `typescript`, `ts`); anything else is plain text.
    pub fn named(name: &str) -> Language {
        match name.to_ascii_lowercase().as_str() {
            "python" | "py" => Language::Python,
            "javascript" | "js" | "typescript" | "ts" => Language::JavaScript,
            _ => Language::Plain,
        }
    }

    /// Whether `line` (what precedes the caret on it) opens a block: a
    /// bracket left open at its end in both, and a colon in Python, a
    /// comment after it set aside.
    fn opens_block(self, line: &str) -> bool {
        let code = match self {
            Language::Python => strip_comment(line, '#'),
            Language::JavaScript => line.split("//").next().unwrap_or(line),
            Language::Plain => return false,
        };
        let end = code.trim_end();
        end.ends_with(['(', '[', '{']) || (self == Language::Python && end.ends_with(':'))
    }

    /// What starts a comment that runs to the end of the line; none in
    /// plain text.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Language::Python => Some("#"),
            Language::JavaScript => Some("//"),
            Language::Plain => None,
        }
    }

    /// The quotes a string opens with, which typing closes.
    fn quotes(self) -> &'static [char] {
        match self {
            Language::JavaScript => &['\'', '"', '`'],
            _ => &['\'', '"'],
        }
    }

    /// Whether three quotes open a string, which typing closes with three.
    fn triple_quotes(self) -> bool {
        self == Language::Python
    }

    /// What may stand before a string's quote and still open it (`f"`,
    /// `rb'`).
    fn string_prefixes(self) -> &'static [&'static str] {
        match self {
            Language::Python => &[
                "f", "fr", "rf", "r", "u", "b", "br", "rb", "F", "FR", "RF", "R", "U", "B", "BR",
                "RB",
            ],
            _ => &[],
        }
    }
}

/// The bracket that closes `open`.
fn closing(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// What a bracket typed before it is closed at once: the end of the line,
/// a space, or what ends an expression.
fn closes_before(next: Option<char>) -> bool {
    next.is_none_or(|c| c.is_whitespace() || ")]}:;>".contains(c))
}

/// `line` up to a comment that starts with `mark` outside a string.
fn strip_comment(line: &str, mark: char) -> &str {
    let mut quote: Option<char> = None;
    for (at, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == mark => return &line[..at],
            None => {}
        }
    }
    line
}

/// One change: at byte `at`, `removed` became `inserted`, the caret
/// standing where it was before and where it is after.
#[derive(Clone, Debug)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
    before: Caret,
    after: Caret,
}

/// One step of the history: its changes in the order they were made, and
/// the revision the text is at after it.
#[derive(Clone, Debug)]
struct Step {
    changes: Vec<Change>,
    rev: u64,
}

/// The text of a code element, the caret over it and what was done to it.
#[derive(Clone, Debug)]
pub struct Code {
    text: String,
    caret: Caret,
    /// The byte each line starts at; the first is 0.
    starts: Vec<usize>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Whether the last change was a character typed: a run of them is one
    /// step.
    typing: bool,
    /// The revision the text is at, the next one to give, and the one last
    /// marked saved.
    rev: u64,
    next_rev: u64,
    saved: u64,
    /// The revision the text was set at, where undoing everything returns.
    base: u64,
    pub indent: Indent,
    /// The tree of the text, in its language. In a cell because a parse is
    /// carried on by whoever asks, a view's tick included.
    syntax: RefCell<Syntax>,
    /// Where the closing brackets and quotes typing put stand: one typed is
    /// typed over them. Kept while the caret stays on their line.
    closers: Vec<usize>,
}

impl Default for Code {
    fn default() -> Self {
        Self::new("")
    }
}

impl Code {
    /// `text`, clean, the caret at its start and nothing to undo.
    pub fn new(text: &str) -> Self {
        let mut code = Code {
            text: String::new(),
            caret: Caret::default(),
            starts: vec![0],
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            rev: 0,
            next_rev: 1,
            saved: 0,
            base: 0,
            indent: Indent::default(),
            syntax: RefCell::new(Syntax::new(Language::default())),
            closers: Vec::new(),
        };
        code.set_text(text);
        code
    }

    /// **The whole text replaced** by `text`: clean, its history gone, the
    /// caret kept where it was if it still falls inside.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.replace("\r\n", "\n");
        textedit::clamp(&self.text, &mut self.caret);
        self.caret.anchor = None;
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
        self.rev = self.next_rev;
        self.next_rev += 1;
        self.saved = self.rev;
        self.base = self.rev;
        self.syntax.get_mut().reset();
        self.closers.clear();
        self.reindex();
    }

    /// The language the text is written in.
    pub fn language(&self) -> Language {
        self.syntax.borrow().language()
    }

    /// The text read as `language` from now on.
    pub fn set_language(&mut self, language: Language) {
        if language != self.language() {
            self.syntax = RefCell::new(Syntax::new(language));
        }
    }

    /// **The tree brought up to the text**, within `budget` if one is
    /// given: answers whether it is, or whether the parse stopped at its time
    /// and is to be carried on by the next call.
    pub fn parse(&self, budget: Option<std::time::Duration>) -> bool {
        self.syntax.borrow_mut().parse(&self.text, budget)
    }

    /// Whether the tree is behind the text.
    pub fn parsing(&self) -> bool {
        self.syntax.borrow().parsing()
    }

    /// **What each part of lines `lines` is**, by the tree as it stands:
    /// runs of one kind, in order and apart, absolute in the text; what
    /// nothing names is left out.
    pub fn spans(&self, lines: std::ops::Range<usize>) -> Vec<Span> {
        let Some(&from) = self.starts.get(lines.start) else {
            return Vec::new();
        };
        let to = self
            .starts
            .get(lines.end)
            .map_or(self.text.len(), |next| next - 1);
        self.syntax.borrow().spans(&self.text, from, to)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> Caret {
        self.caret
    }

    /// Puts the caret (and the selection) at `caret`, landed on the text.
    pub fn set_caret(&mut self, caret: Caret) {
        self.caret = caret;
        textedit::clamp(&self.text, &mut self.caret);
        self.typing = false;
        self.keep_closers();
    }

    /// **Bytes `a` to `b` selected**, the caret at `b`.
    pub fn select(&mut self, a: usize, b: usize) {
        self.set_caret(Caret {
            pos: b,
            anchor: (a != b).then_some(a),
        });
    }

    /// The closers typed over kept only on the caret's line, as typing
    /// over them is only what was just typed.
    fn keep_closers(&mut self) {
        if self.closers.is_empty() {
            return;
        }
        let line = self.line_of(self.caret.pos);
        let (start, text) = self.line(line).unwrap_or((0, ""));
        let end = start + text.len();
        self.closers.retain(|&p| p >= start && p < end);
    }

    /// How many lines the text has: one more than its line ends.
    pub fn lines(&self) -> usize {
        self.starts.len()
    }

    /// Line `n` (from 0), without its end, and the byte it starts at.
    pub fn line(&self, n: usize) -> Option<(usize, &str)> {
        let start = *self.starts.get(n)?;
        let end = self
            .starts
            .get(n + 1)
            .map_or(self.text.len(), |next| next - 1);
        Some((start, &self.text[start..end]))
    }

    /// The line byte `at` is on.
    pub fn line_of(&self, at: usize) -> usize {
        self.starts.partition_point(|&s| s <= at).saturating_sub(1)
    }

    /// The caret's line and its column, in graphemes, both from 0.
    pub fn line_col(&self) -> (usize, usize) {
        let line = self.line_of(self.caret.pos);
        let start = self.starts[line];
        (
            line,
            self.text[start..self.caret.pos].graphemes(true).count(),
        )
    }

    /// What is selected, if anything is.
    pub fn selected(&self) -> Option<&str> {
        textedit::selected(&self.text, &self.caret)
    }

    /// Whether the text is not the one last marked saved.
    pub fn dirty(&self) -> bool {
        self.rev != self.saved
    }

    /// The text as it is now is the one saved.
    pub fn mark_saved(&mut self) {
        self.saved = self.rev;
    }

    /// Whether there is a step to undo, and one to redo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn reindex(&mut self) {
        self.starts.clear();
        self.starts.push(0);
        self.starts.extend(
            self.text
                .bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
    }

    /// **An edit, recorded**: `f` changes the text and the caret, and what
    /// it changed becomes a step of the history -- or joins the last one,
    /// for a character `typed` after another. Answers whether anything
    /// changed.
    fn edit(&mut self, typed: bool, f: impl FnOnce(&mut String, &mut Caret) -> bool) -> bool {
        let before_text = self.text.clone();
        let before = self.caret;
        if !f(&mut self.text, &mut self.caret) || self.text == before_text {
            self.typing = false;
            return false;
        }
        let (at, removed, inserted) = diff(&before_text, &self.text);
        self.syntax
            .get_mut()
            .edit(&before_text, at, removed, inserted);
        // a closer follows its character, and goes with it
        let (cut, put) = (removed.len(), inserted.len());
        self.closers.retain_mut(|p| {
            if *p >= at + cut {
                *p = *p + put - cut;
                true
            } else {
                *p < at
            }
        });
        let change = Change {
            at,
            removed: removed.to_string(),
            inserted: inserted.to_string(),
            before,
            after: self.caret,
        };
        let rev = self.next_rev;
        self.next_rev += 1;
        match self.undo.last_mut() {
            Some(step) if typed && self.typing => {
                step.changes.push(change);
                step.rev = rev;
            }
            _ => self.undo.push(Step {
                changes: vec![change],
                rev,
            }),
        }
        self.rev = rev;
        self.redo.clear();
        self.typing = typed;
        self.reindex();
        self.keep_closers();
        true
    }

    /// **One step back**, or forward with `redo`: the text and the caret
    /// as they were. Answers whether there was one.
    pub fn step(&mut self, redo: bool) -> bool {
        let Some(step) = (if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        }) else {
            return false;
        };
        if redo {
            for c in &step.changes {
                self.syntax
                    .get_mut()
                    .edit(&self.text, c.at, &c.removed, &c.inserted);
                self.text
                    .replace_range(c.at..c.at + c.removed.len(), &c.inserted);
            }
            self.caret = step.changes.last().map_or(self.caret, |c| c.after);
            self.rev = step.rev;
            self.undo.push(step);
        } else {
            for c in step.changes.iter().rev() {
                self.syntax
                    .get_mut()
                    .edit(&self.text, c.at, &c.inserted, &c.removed);
                self.text
                    .replace_range(c.at..c.at + c.inserted.len(), &c.removed);
            }
            self.caret = step.changes.first().map_or(self.caret, |c| c.before);
            self.rev = self.undo.last().map_or(self.base, |s| s.rev);
            self.redo.push(step);
        }
        self.typing = false;
        self.closers.clear();
        self.reindex();
        true
    }

    /// **Byte `at` as a session's code intelligence reads a place**: a line
    /// from 1 and a column from 0, in characters.
    pub fn position(&self, at: usize) -> (usize, usize) {
        let line = self.line_of(at);
        let start = self.starts[line];
        (
            line + 1,
            self.text[start..at.min(self.text.len())].chars().count(),
        )
    }

    /// The byte a line from 1 and a column from 0, in characters, name:
    /// held to the text and to the line.
    pub fn at_position(&self, line: usize, column: usize) -> usize {
        let (start, text) = self
            .line(line.clamp(1, self.lines()) - 1)
            .unwrap_or((0, ""));
        start
            + text
                .char_indices()
                .nth(column)
                .map_or(text.len(), |(i, _)| i)
    }

    /// The word byte `at` is in or just after: its bytes, a name's
    /// characters (`$` too, as JavaScript writes names).
    pub fn word_at(&self, at: usize) -> Option<(usize, usize)> {
        let named = |c: char| find::is_word(c) || c == '$';
        let at = at.min(self.text.len());
        let start = self.text[..at]
            .char_indices()
            .rev()
            .take_while(|(_, c)| named(*c))
            .last()
            .map_or(at, |(i, _)| i);
        let end = at
            + self.text[at..]
                .char_indices()
                .find(|(_, c)| !named(*c))
                .map_or(self.text.len() - at, |(i, _)| i);
        (end > start).then_some((start, end))
    }

    /// Whether byte `at` is inside a string or a comment, by the tree; no
    /// when there is none to say.
    pub fn in_text(&self, at: usize) -> bool {
        self.syntax
            .borrow()
            .in_text(&self.text, at)
            .unwrap_or(false)
    }

    /// **Bytes `from` to `to` replaced by `with`**, as one step, the caret
    /// after it.
    pub fn replace_range(&mut self, from: usize, to: usize, with: &str) -> bool {
        let done = self.apply(vec![(from, to - from, with.to_string())], true);
        if done {
            self.select(from + with.len(), from + with.len());
        }
        done
    }

    /// The text's revision: two that are equal are the same text.
    pub fn revision(&self) -> u64 {
        self.rev
    }

    // -- typing ---------------------------------------------------------------

    /// **`s` typed** in place of the selection: a run of single characters
    /// is one step of the history.
    pub fn insert(&mut self, s: &str) -> bool {
        let typed = s.graphemes(true).count() == 1 && s != "\n";
        self.edit(typed, |text, caret| textedit::insert(text, caret, s))
    }

    /// **A character typed, as code is typed**: a bracket or a quote brings
    /// the one that closes it when nothing is written after the caret, and
    /// a closer just put there is typed over; with a selection, the
    /// selection is put between the two. Anything else is typed as
    /// [`insert`](Self::insert) types it.
    pub fn type_char(&mut self, c: char) -> bool {
        let pos = self.caret.pos;
        let next = self.text[pos..].chars().next();
        let language = self.language();
        let is_quote = language.quotes().contains(&c);
        let close = closing(c).or(is_quote.then_some(c));
        // a closer typing put, typed over
        let over =
            self.caret.selection().is_none() && next == Some(c) && self.closers.contains(&pos);
        if over && (is_quote || ")]}".contains(c)) {
            let three: String = [c; 3].iter().collect();
            let n = if is_quote && language.triple_quotes() && self.text[pos..].starts_with(&three)
            {
                3
            } else {
                1
            };
            self.closers.retain(|&p| p != pos);
            self.caret = Caret {
                pos: pos + n,
                anchor: None,
            };
            return true;
        }
        let Some(close) = close else {
            return self.insert(c.encode_utf8(&mut [0; 4]));
        };
        if let Some((a, b)) = self.caret.selection() {
            // around the selection, which stays selected
            let backwards = self
                .caret
                .anchor
                .is_some_and(|anchor| anchor > self.caret.pos);
            let done = self.edit(false, |text, caret| {
                text.insert(b, close);
                text.insert(a, c);
                let (from, to) = (a + c.len_utf8(), b + c.len_utf8());
                *caret = if backwards {
                    Caret {
                        pos: from,
                        anchor: Some(to),
                    }
                } else {
                    Caret {
                        pos: to,
                        anchor: Some(from),
                    }
                };
                true
            });
            if done {
                self.closers.push(b + c.len_utf8());
            }
            return done;
        }
        // what typing it puts down; the caret goes after the first
        let put = if is_quote {
            self.quote_pair(pos, c, next)
        } else {
            closes_before(next).then(|| format!("{c}{close}"))
        };
        let Some(put) = put else {
            return self.insert(c.encode_utf8(&mut [0; 4]));
        };
        let after = pos + c.len_utf8();
        let done = self.edit(true, |text, caret| {
            text.insert_str(pos, &put);
            *caret = Caret {
                pos: after,
                anchor: None,
            };
            true
        });
        if done {
            self.closers.push(after);
        }
        done
    }

    /// **What quote `c` typed at `pos` puts down when it opens a string**:
    /// the pair, or (after two of it that open one, in a language with
    /// strings of three) the three more that close three. None where it
    /// closes or sits in a string, or runs into a word.
    fn quote_pair(&self, pos: usize, c: char, next: Option<char>) -> Option<String> {
        if next == Some(c) {
            return None;
        }
        let two: String = [c; 2].iter().collect();
        if self.language().triple_quotes() && self.text[..pos].ends_with(&two) {
            let start = pos - two.len();
            if !self.text[..start].ends_with(c) && self.can_start_string(start) {
                return Some([c; 4].iter().collect());
            }
        }
        if next.is_some_and(find::is_word) {
            return None;
        }
        (self.can_start_string(pos) && !self.in_string(pos, c)).then_some(two)
    }

    /// Whether a quote at `pos` would open a string: no word runs into it,
    /// but for one of the language's prefixes (`f`, `rb`).
    fn can_start_string(&self, pos: usize) -> bool {
        let before = &self.text[..pos];
        let word_ends = |s: &str| s.chars().next_back().is_some_and(find::is_word);
        !word_ends(before)
            || self
                .language()
                .string_prefixes()
                .iter()
                .any(|p| before.ends_with(p) && !word_ends(&before[..before.len() - p.len()]))
    }

    /// **Whether `pos` is inside a string or a comment**: the tree's answer
    /// when it has the text, else whether quote `c` stands an odd number of
    /// times on the line before it.
    fn in_string(&self, pos: usize, c: char) -> bool {
        if let Some(inside) = self.syntax.borrow().in_text(&self.text, pos) {
            return inside;
        }
        let start = textedit::line_start(&self.text, pos);
        let mut odd = false;
        let mut escaped = false;
        for ch in self.text[start..pos].chars() {
            match ch {
                '\\' if !escaped => escaped = true,
                _ => {
                    odd ^= ch == c && !escaped;
                    escaped = false;
                }
            }
        }
        odd
    }

    /// **`s` pasted**: one step, whatever its length.
    pub fn paste(&mut self, s: &str) -> bool {
        let s = s.replace("\r\n", "\n");
        self.edit(false, |text, caret| textedit::insert(text, caret, &s))
    }

    /// **Enter**: a new line with the indentation of the one it leaves, a
    /// level more after a line that opens a block -- and, between a bracket
    /// and the one that closes it, the closing one on a line of its own.
    pub fn newline(&mut self) -> bool {
        let indent = self.indent;
        let language = self.language();
        self.edit(false, |text, caret| {
            textedit::delete_selection(text, caret);
            let start = textedit::line_start(text, caret.pos);
            let before = &text[start..caret.pos];
            let lead: String = before
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let opens = language.opens_block(before);
            let closer = text[caret.pos..].chars().next();
            let pair = opens
                && matches!(
                    (before.trim_end().chars().last(), closer),
                    (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}'))
                );
            let mut inner = format!("\n{lead}");
            if opens {
                inner.push_str(&indent.unit());
            }
            if pair {
                let at = caret.pos;
                let all = format!("{inner}\n{lead}");
                text.insert_str(at, &all);
                caret.pos = at + inner.len();
                caret.anchor = None;
                true
            } else {
                textedit::insert(text, caret, &inner)
            }
        })
    }

    /// **Backspace**: the selection, or the grapheme before the caret --
    /// with the one that closes it, between a bracket or a quote and its
    /// closer; in a line's leading spaces, back to the level before; with
    /// `word`, the word before.
    pub fn backspace(&mut self, word: bool) -> bool {
        let indent = self.indent;
        let quotes = self.language().quotes();
        self.edit(false, |text, caret| {
            if word {
                return textedit::backspace_word(text, caret);
            }
            // between a bracket or a quote and the one that closes it, both
            if caret.selection().is_none() {
                let before = text[..caret.pos].chars().next_back();
                let after = text[caret.pos..].chars().next();
                if let (Some(b), Some(a)) = (before, after)
                    && (closing(b) == Some(a) || (a == b && quotes.contains(&b)))
                {
                    let from = caret.pos - b.len_utf8();
                    text.replace_range(from..caret.pos + a.len_utf8(), "");
                    caret.pos = from;
                    return true;
                }
            }
            if caret.selection().is_none()
                && let Indent::Spaces(n) = indent
            {
                let start = textedit::line_start(text, caret.pos);
                let before = &text[start..caret.pos];
                if !before.is_empty() && before.bytes().all(|b| b == b' ') {
                    let n = n as usize;
                    let to = (before.len() - 1) / n * n;
                    text.replace_range(start + to..caret.pos, "");
                    caret.pos = start + to;
                    caret.anchor = None;
                    return true;
                }
            }
            textedit::backspace(text, caret)
        })
    }

    /// **Delete**: the selection, or the grapheme after the caret; with
    /// `word`, the word after.
    pub fn delete(&mut self, word: bool) -> bool {
        self.edit(false, |text, caret| {
            if word {
                textedit::delete_word(text, caret)
            } else {
                textedit::delete(text, caret)
            }
        })
    }

    /// **Cut**: what is selected leaves the text, and is answered.
    pub fn cut(&mut self) -> Option<String> {
        let taken = self.selected()?.to_string();
        self.edit(false, textedit::delete_selection);
        Some(taken)
    }

    // -- indentation ----------------------------------------------------------

    /// The lines a selection touches, or the caret's: a selection that ends
    /// at the start of a line does not touch that one.
    fn touched(&self) -> (usize, usize) {
        let (a, b) = self
            .caret
            .selection()
            .unwrap_or((self.caret.pos, self.caret.pos));
        let first = self.line_of(a);
        let mut last = self.line_of(b);
        if last > first && self.starts[last] == b {
            last -= 1;
        }
        (first, last)
    }

    /// **Tab**: a level inserted at the caret (spaces up to the next level),
    /// or, with a selection, a level put before every line it touches.
    pub fn indent_more(&mut self) -> bool {
        if self.caret.selection().is_some() {
            let unit = self.indent.unit();
            return self.reindent(|_| (0, unit.clone()));
        }
        let unit = match self.indent {
            Indent::Tab => "\t".to_string(),
            Indent::Spaces(n) => {
                let (_, col) = self.line_col();
                " ".repeat(n as usize - col % n as usize)
            }
        };
        self.edit(false, |text, caret| textedit::insert(text, caret, &unit))
    }

    /// **Shift+Tab**: a level taken off every line the selection touches, or
    /// the caret's -- a tab, or up to a level's spaces.
    pub fn indent_less(&mut self) -> bool {
        let width = self.indent.width();
        self.reindent(|line| {
            let n = if line.starts_with('\t') {
                1
            } else {
                line.bytes().take(width).take_while(|b| *b == b' ').count()
            };
            (n, String::new())
        })
    }

    /// Every line touched: `per(line)` says how many of its first bytes go
    /// and what goes before the rest. The caret and the selection stay on
    /// the text they were on.
    fn reindent(&mut self, per: impl Fn(&str) -> (usize, String)) -> bool {
        let (first, last) = self.touched();
        let mut plans = Vec::new();
        for n in first..=last {
            let (start, line) = self.line(n).unwrap_or((0, ""));
            let (cut, put) = per(line);
            if cut > 0 || !put.is_empty() {
                plans.push((start, cut, put));
            }
        }
        // a place at a line's start stays there, so a selection from it
        // takes in what was put before the line
        self.apply(plans, false)
    }

    /// **Changes at several places, as one step**: each plan cuts `cut`
    /// bytes at `at` and puts `put` there, in order and apart. The caret and
    /// the selection stay on the text they were on; a place where something
    /// is only put goes after it with `after`, and stays before it without.
    fn apply(&mut self, plans: Vec<(usize, usize, String)>, after: bool) -> bool {
        if plans.is_empty() {
            return false;
        }
        self.edit(false, |text, caret| {
            let map = |p: usize| -> usize {
                let mut out = p as isize;
                for (at, cut, put) in &plans {
                    if p > *at || (p == *at && (*cut > 0 || after)) {
                        let inside = (p - at).min(*cut);
                        out += put.len() as isize - inside as isize;
                    }
                }
                out.max(0) as usize
            };
            let pos = map(caret.pos);
            let anchor = caret.anchor.map(map);
            for (at, cut, put) in plans.iter().rev() {
                text.replace_range(*at..at + cut, put);
            }
            caret.pos = pos;
            caret.anchor = anchor;
            true
        })
    }

    // -- comments -------------------------------------------------------------

    /// **A line comment put on the lines selected, or taken off**: off when
    /// every line that has text starts with one, on otherwise: the
    /// language's mark and a space, at the least indentation among them, so
    /// the comment stands in one column. Plain text has no comment.
    pub fn toggle_comment(&mut self) -> bool {
        let Some(mark) = self.language().line_comment() else {
            return false;
        };
        let (first, last) = self.touched();
        // each line: where it starts, its indentation, whether it is empty,
        // and where its comment is, if it has one
        let lines: Vec<(usize, usize, bool, Option<usize>)> = (first..=last)
            .map(|n| {
                let (start, line) = self.line(n).unwrap_or((0, ""));
                let indent = line.len() - line.trim_start().len();
                let empty = indent == line.len();
                let comment = line[indent..].starts_with(mark).then_some(indent);
                (start, indent, empty, comment)
            })
            .collect();
        let single = lines.len() == 1;
        let least = lines
            .iter()
            .filter(|l| !l.2)
            .map(|l| l.1)
            .min()
            .unwrap_or(0);
        let plans: Vec<(usize, usize, String)> =
            if lines.iter().any(|l| l.3.is_none() && (!l.2 || single)) {
                let at = |l: &(usize, usize, bool, Option<usize>)| {
                    if l.2 { l.1 } else { least }
                };
                lines
                    .iter()
                    .filter(|l| single || !l.2)
                    .map(|l| (l.0 + at(l), 0, format!("{mark} ")))
                    .collect()
            } else {
                lines
                    .iter()
                    .filter_map(|l| {
                        let from = l.0 + l.3?;
                        let to = from + mark.len();
                        let cut = mark.len() + usize::from(self.text[to..].starts_with(' '));
                        Some((from, cut, String::new()))
                    })
                    .collect()
            };
        self.apply(plans, true)
    }

    // -- finding --------------------------------------------------------------

    /// **The next match of `finder` selected**, or the previous one: after
    /// the selection, or before it, round past the end. Answers whether
    /// there was one.
    pub fn find(&mut self, finder: &Finder, forward: bool) -> bool {
        let (a, b) = self
            .caret
            .selection()
            .unwrap_or((self.caret.pos, self.caret.pos));
        let found = if forward {
            finder.next(&self.text, a, b)
        } else {
            finder.previous(&self.text, a, b)
        };
        match found {
            Some((from, to)) => {
                self.select(from, to);
                true
            }
            None => false,
        }
    }

    /// **The match selected replaced by `with`**, and the next one
    /// selected; with no match selected, the next one is only selected.
    /// Answers whether anything was done.
    pub fn replace(&mut self, finder: &Finder, with: &str) -> bool {
        let (a, b) = self
            .caret
            .selection()
            .unwrap_or((self.caret.pos, self.caret.pos));
        let selected = finder.from(&self.text, a).next() == Some((a, b));
        if !selected {
            return self.find(finder, true);
        }
        let put = finder.replacement(&self.text, (a, b), with);
        let next = finder.next(&self.text, a, b);
        if !self.apply(vec![(a, b - a, put.clone())], true) {
            return false;
        }
        // the next match, where the replacement moved it
        if let Some((from, to)) = next {
            let shift = |p: usize| if p >= b { p + put.len() - (b - a) } else { p };
            self.select(shift(from), shift(to));
        }
        true
    }

    /// **Every match replaced by `with`**, as one step: answers how many.
    pub fn replace_all(&mut self, finder: &Finder, with: &str) -> usize {
        let plans: Vec<(usize, usize, String)> = finder
            .from(&self.text, 0)
            .map(|m| (m.0, m.1 - m.0, finder.replacement(&self.text, m, with)))
            .collect();
        let n = plans.len();
        if self.apply(plans, true) { n } else { 0 }
    }

    /// **The caret on the line `spec` names**: a number (from 1), a
    /// number of lines on (`+3`) or back (`-3`), a share of the text
    /// (`50%`), each with a column after a colon (`12:4`, from 0). Answers
    /// whether `spec` named one.
    pub fn go_to(&mut self, spec: &str) -> bool {
        let spec = spec.trim();
        let (head, column) = match spec.split_once(':') {
            Some((h, c)) => match c.parse::<usize>() {
                Ok(c) => (h, Some(c)),
                Err(_) => return false,
            },
            None => (spec, None),
        };
        let (sign, head) = match head.chars().next() {
            Some(s @ ('+' | '-')) => (Some(s), &head[1..]),
            _ => (None, head),
        };
        let (number, percent) = match head.strip_suffix('%') {
            Some(n) => (n, true),
            None => (head, false),
        };
        if !number.bytes().all(|b| b.is_ascii_digit())
            || (number.is_empty() && (sign.is_some() || percent))
        {
            return false;
        }
        let here = self.line_of(self.caret.pos) as f64 + 1.0;
        let lines = self.lines() as f64;
        let line = match number.parse::<f64>() {
            Err(_) => here,
            Ok(n) if percent => {
                let share = match sign {
                    Some('-') => here / lines - n / 100.0,
                    Some(_) => here / lines + n / 100.0,
                    None => n / 100.0,
                };
                (lines * share).round()
            }
            Ok(n) => match sign {
                Some('-') => here - n,
                Some(_) => here + n,
                None => n,
            },
        };
        let line = (line.clamp(1.0, lines) as usize) - 1;
        let (start, text) = self.line(line).unwrap_or((0, ""));
        let col = column.unwrap_or(0);
        let at = text.char_indices().nth(col).map_or(text.len(), |(i, _)| i);
        self.select(start + at, start + at);
        true
    }

    /// **The other places the selected text stands** in lines `lines`: none
    /// when nothing is selected, the selection is longer than a line of
    /// code, or the places are too many to be worth pointing at.
    pub fn occurrences(&self, lines: std::ops::Range<usize>) -> Vec<(usize, usize)> {
        const LONGEST: usize = 200;
        const MOST: usize = 100;
        let Some((a, b)) = self.caret.selection() else {
            return Vec::new();
        };
        let sought = &self.text[a..b];
        if sought.len() > LONGEST || sought.trim().is_empty() {
            return Vec::new();
        }
        let Some(&from) = self.starts.get(lines.start) else {
            return Vec::new();
        };
        let mut to = self
            .starts
            .get(lines.end)
            .map_or(self.text.len(), |&next| next)
            .saturating_add(sought.len())
            .min(self.text.len());
        while !self.text.is_char_boundary(to) {
            to -= 1;
        }
        let found: Vec<(usize, usize)> = self.text[from..to]
            .match_indices(sought)
            .map(|(i, _)| (from + i, from + i + sought.len()))
            .filter(|&(x, y)| y <= a || x >= b)
            .take(MOST + 1)
            .collect();
        if found.len() > MOST {
            Vec::new()
        } else {
            found
        }
    }

    // -- brackets -------------------------------------------------------------

    /// **The bracket by the caret and the one it pairs with**: the one
    /// before the caret, else the one after; its pair, or none when nothing
    /// closes it. A bracket in a string or a comment is not one.
    pub fn bracket_pair(&self) -> Option<(usize, Option<usize>)> {
        let pos = self.caret.pos;
        let bytes = self.text.as_bytes();
        let is_bracket = |at: usize| bytes.get(at).is_some_and(|b| b"()[]{}".contains(b));
        for at in [pos.checked_sub(1), Some(pos)].into_iter().flatten() {
            if !is_bracket(at) {
                continue;
            }
            match self.syntax.borrow().bracket(&self.text, at) {
                Some(Bracket::Text) => continue,
                Some(Bracket::Code(other)) => return Some((at, other)),
                None => return Some((at, scan_bracket(bytes, at))),
            }
        }
        None
    }

    // -- motion and selection -------------------------------------------------

    /// A motion over the text, as the field makes it ([`textedit`]).
    pub fn motion(&mut self, f: impl FnOnce(&str, &mut Caret)) {
        f(&self.text, &mut self.caret);
        self.typing = false;
    }

    /// **Home**: the first character of the line that is not indentation,
    /// or -- from there -- the line's start.
    pub fn home(&mut self, select: bool) {
        let line = self.line_of(self.caret.pos);
        let (start, text) = self.line(line).unwrap_or((0, ""));
        let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
        let to = if self.caret.pos == start + lead {
            start
        } else {
            start + lead
        };
        self.motion(|_, caret| {
            if select {
                caret.anchor.get_or_insert(caret.pos);
            } else {
                caret.anchor = None;
            }
            caret.pos = to;
        });
    }

    /// **`rows` lines up (negative) or down**, keeping the column: a page.
    pub fn rows(&mut self, rows: isize, select: bool) {
        for _ in 0..rows.unsigned_abs() {
            if rows < 0 {
                self.motion(|text, caret| textedit::move_up(text, caret, select));
            } else {
                self.motion(|text, caret| textedit::move_down(text, caret, select));
            }
        }
    }

    /// The caret at byte `at`, extending the selection with `select`.
    pub fn place(&mut self, at: usize, select: bool) {
        self.motion(|_, caret| {
            if select {
                caret.anchor.get_or_insert(caret.pos);
            } else {
                caret.anchor = None;
            }
            caret.pos = at;
        });
        textedit::clamp(&self.text, &mut self.caret);
    }

    /// What a press at byte `at` selects by `unit`.
    pub fn unit_at(&self, at: usize, unit: Unit) -> (usize, usize) {
        textedit::unit_at(&self.text, at, unit)
    }

    /// A selection grown from what a press held to `at`, by `unit`.
    pub fn extend(&mut self, held: (usize, usize), at: usize, unit: Unit) {
        self.caret = textedit::extend_by(&self.text, held, at, unit);
        self.typing = false;
    }

    pub fn select_all(&mut self) {
        self.motion(textedit::select_all);
    }
}

/// **The bracket that pairs with the one at `at`**, read off the text
/// alone: the first of its other kind with the nested ones counted, not
/// further than a long function's length away; none when a bracket of
/// another kind closes first, or nothing does.
fn scan_bracket(bytes: &[u8], at: usize) -> Option<usize> {
    const FURTHEST: usize = 10_000;
    let (open, close, forward) = match bytes[at] {
        b'(' => (b'(', b')', true),
        b'[' => (b'[', b']', true),
        b'{' => (b'{', b'}', true),
        b')' => (b'(', b')', false),
        b']' => (b'[', b']', false),
        _ => (b'{', b'}', false),
    };
    let mut depth = 0usize;
    let range: Box<dyn Iterator<Item = usize>> = if forward {
        Box::new(at + 1..bytes.len().min(at + 1 + FURTHEST))
    } else {
        Box::new((at.saturating_sub(FURTHEST)..at).rev())
    };
    let (same, other) = if forward {
        (open, close)
    } else {
        (close, open)
    };
    for i in range {
        let b = bytes[i];
        if b == same || (b"([{".contains(&b) == forward && b"()[]{}".contains(&b) && b != other) {
            depth += 1;
        } else if b"()[]{}".contains(&b) {
            if depth == 0 {
                return (b == other).then_some(i);
            }
            depth -= 1;
        }
    }
    None
}

/// **What changed between `before` and `after`**: the byte where they part,
/// what `before` had there and what `after` has instead -- the common
/// beginning and end left out, on character boundaries.
fn diff<'a>(before: &'a str, after: &'a str) -> (usize, &'a str, &'a str) {
    let mut start = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
        start -= 1;
    }
    let room = before.len().min(after.len()) - start;
    let mut end = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(before.len() - end) || !after.is_char_boundary(after.len() - end)
    {
        end -= 1;
    }
    (
        start,
        &before[start..before.len() - end],
        &after[start..after.len() - end],
    )
}

#[cfg(test)]
mod tests;
