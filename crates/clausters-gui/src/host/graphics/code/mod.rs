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
//!   is.
//!
//! No window, no device, no wire: what a key does to the text is tested
//! here as text.

use std::cell::RefCell;

use unicode_segmentation::UnicodeSegmentation;

use super::textedit::{self, Caret, Unit};

pub mod syntax;

use syntax::{Span, Syntax};

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
        self.reindex();
        true
    }

    // -- typing ---------------------------------------------------------------

    /// **`s` typed** in place of the selection: a run of single characters
    /// is one step of the history.
    pub fn insert(&mut self, s: &str) -> bool {
        let typed = s.graphemes(true).count() == 1 && s != "\n";
        self.edit(typed, |text, caret| textedit::insert(text, caret, s))
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
    /// or, in a line's leading spaces, back to the level before; with
    /// `word`, the word before.
    pub fn backspace(&mut self, word: bool) -> bool {
        let indent = self.indent;
        self.edit(false, |text, caret| {
            if word {
                return textedit::backspace_word(text, caret);
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
        if plans.is_empty() {
            return false;
        }
        self.edit(false, |text, caret| {
            let map = |p: usize| -> usize {
                let mut out = p as isize;
                // a place at a line's start stays there, so a selection
                // from it takes in what was put before the line
                for (start, cut, put) in &plans {
                    if p > *start || (p == *start && *cut > 0) {
                        let inside = (p - start).min(*cut);
                        out += put.len() as isize - inside as isize;
                    }
                }
                out.max(0) as usize
            };
            let pos = map(caret.pos);
            let anchor = caret.anchor.map(map);
            for (start, cut, put) in plans.iter().rev() {
                text.replace_range(*start..start + cut, put);
            }
            caret.pos = pos;
            caret.anchor = anchor;
            true
        })
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
