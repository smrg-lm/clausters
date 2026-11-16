//! **What a post window holds**: lines of text, each made of runs of a
//! kind (output, an error, a result, a notice), appended as a session
//! writes them and kept up to a number of lines.
//!
//! The model of the `post` element ([`crate::host::elements`]), with no
//! window, no device and no wire: what an append does to the lines, which
//! lines are kept, where a selection is and what it copies, and where a
//! line wraps at a width. A line is addressed by its **number since the
//! post was cleared** (`first` is the number of the first line kept), so a
//! selection and a scroll position stay on their line while older lines
//! are dropped under them.
//!
//! The rules of an append are the ones the editor's post window has kept:
//! a result reads `-> value` on a line of its own ending, a notice always
//! starts on a new line, and output and errors continue whatever line is
//! open, so a `print(..., end="")` and the next one share a line.

use std::collections::VecDeque;

use unicode_segmentation::UnicodeSegmentation;

use super::textedit::{self, Unit};

/// What a run of a line is: how it is told apart when it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// What the session printed.
    Out,
    /// What it wrote to its error stream.
    Err,
    /// The value an evaluation answered, written `-> value`.
    Result,
    /// A notice of the program's own (started, interrupted, restarted).
    Info,
}

impl Kind {
    /// The kind a `/gui_set` key names: `out`, `err`, `result`, `info`.
    pub fn named(key: &str) -> Option<Kind> {
        Some(match key {
            "out" => Kind::Out,
            "err" => Kind::Err,
            "result" => Kind::Result,
            "info" => Kind::Info,
            _ => return None,
        })
    }
}

/// One line: its text and the runs it is made of, each from its first byte
/// to the next run's (the first at 0).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub text: String,
    pub runs: Vec<(usize, Kind)>,
}

impl Line {
    fn push(&mut self, kind: Kind, s: &str) {
        if self.runs.last().map(|r| r.1) != Some(kind) && !(s.is_empty() && !self.text.is_empty()) {
            self.runs.push((self.text.len(), kind));
        }
        self.text.push_str(s);
    }

    /// The runs as byte ranges with their kinds.
    pub fn spans(&self) -> impl Iterator<Item = (usize, usize, Kind)> + '_ {
        self.runs.iter().enumerate().map(|(i, &(at, kind))| {
            let end = self.runs.get(i + 1).map_or(self.text.len(), |r| r.0);
            (at, end, kind)
        })
    }
}

/// A place in the post: a line, by its number, and a byte of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u64,
    pub byte: usize,
}

/// The lines kept, the number of the first, and whether the last is open.
#[derive(Clone, Debug)]
pub struct Post {
    lines: VecDeque<Line>,
    first: u64,
    /// Whether the last line has not ended yet: what comes next continues it.
    open: bool,
    /// How many lines are kept: the oldest go past it.
    keep: usize,
}

/// The lines a post keeps unless told otherwise.
pub const KEEP: usize = 20_000;

impl Default for Post {
    fn default() -> Self {
        Self::new(KEEP)
    }
}

impl Post {
    pub fn new(keep: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            first: 0,
            open: false,
            keep: keep.max(1),
        }
    }

    /// How many lines are kept now.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The number of the first line kept.
    pub fn first(&self) -> u64 {
        self.first
    }

    /// One past the number of the last line.
    pub fn end(&self) -> u64 {
        self.first + self.lines.len() as u64
    }

    /// The line numbered `n`, while it is kept.
    pub fn line(&self, n: u64) -> Option<&Line> {
        n.checked_sub(self.first)
            .and_then(|i| self.lines.get(usize::try_from(i).ok()?))
    }

    /// Keeps `keep` lines from now on, dropping the oldest past it.
    pub fn set_keep(&mut self, keep: usize) {
        self.keep = keep.max(1);
        self.trim();
    }

    /// **Writes `text` of `kind`**: output and errors continue the open
    /// line, a notice starts a line of its own, a result is written
    /// `-> text` and ends its line. A `\r\n` is a line's end.
    pub fn append(&mut self, kind: Kind, text: &str) {
        let mut text = text.replace("\r\n", "\n");
        match kind {
            Kind::Result => {
                text = format!("-> {text}");
                if !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            Kind::Info if self.open => text.insert(0, '\n'),
            _ => {}
        }
        if text.is_empty() {
            return;
        }
        let ends = text.ends_with('\n');
        let body = if ends { &text[..text.len() - 1] } else { &text };
        for (i, seg) in body.split('\n').enumerate() {
            match self.lines.back_mut() {
                Some(last) if i == 0 && self.open => last.push(kind, seg),
                _ => {
                    let mut line = Line::default();
                    line.push(kind, seg);
                    self.lines.push_back(line);
                }
            }
        }
        self.open = !ends;
        self.trim();
    }

    /// Everything goes; the numbers go on from where they were, so nothing
    /// that named a line names another one.
    pub fn clear(&mut self) {
        self.first = self.end();
        self.lines.clear();
        self.open = false;
    }

    fn trim(&mut self) {
        while self.lines.len() > self.keep {
            self.lines.pop_front();
            self.first += 1;
        }
    }

    /// `p` moved onto a line still kept and a byte of it.
    pub fn clamp(&self, p: Pos) -> Pos {
        if self.lines.is_empty() {
            return Pos {
                line: self.first,
                byte: 0,
            };
        }
        let line = p.line.clamp(self.first, self.end() - 1);
        let text = &self.line(line).map_or("", |l| l.text.as_str());
        let mut byte = p.byte.min(text.len());
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        Pos { line, byte }
    }

    /// **The text from `a` to `b`**, in either order: the lines between
    /// them joined by a line's end, exactly as they were written.
    pub fn text_between(&self, a: Pos, b: Pos) -> String {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        let mut out = String::new();
        for n in a.line..=b.line {
            let Some(line) = self.line(n) else { break };
            let from = if n == a.line { a.byte } else { 0 };
            let to = if n == b.line { b.byte } else { line.text.len() };
            if n > a.line {
                out.push('\n');
            }
            out.push_str(&line.text[from.min(to)..to]);
        }
        out
    }

    /// The whole of what is kept.
    pub fn text(&self) -> String {
        self.text_between(
            Pos {
                line: self.first,
                byte: 0,
            },
            Pos {
                line: self.end(),
                byte: usize::MAX,
            },
        )
    }

    /// **What a press at `p` selects by `unit`**: nothing for a character,
    /// the word under it for two clicks, its line for three.
    pub fn unit_at(&self, p: Pos, unit: Unit) -> (Pos, Pos) {
        let p = self.clamp(p);
        let text = self.line(p.line).map_or("", |l| l.text.as_str());
        let (a, b) = textedit::unit_at(text, p.byte, unit);
        (
            Pos {
                line: p.line,
                byte: a,
            },
            Pos {
                line: p.line,
                byte: b,
            },
        )
    }

    /// **A selection grown from what a press held to `p`**, by `unit`:
    /// `(anchor, head)`, the anchor at the far end of what was held.
    pub fn extend(&self, held: (Pos, Pos), p: Pos, unit: Unit) -> (Pos, Pos) {
        let (a, b) = self.unit_at(p, unit);
        if p < held.0 {
            (held.1, a)
        } else {
            (held.0, b.max(held.1))
        }
    }
}

/// **Where `text` breaks into rows no wider than `max_w`**: the first byte
/// of each row, the first at 0. A row breaks after the last space that
/// fits, or -- a word longer than a row -- inside it, at a grapheme; each
/// row holds one grapheme at least. `advance` is a grapheme's width.
pub fn rows(text: &str, max_w: f32, advance: impl Fn(&str) -> f32) -> Vec<usize> {
    let mut out = vec![0];
    let mut x = 0.0f32;
    // the byte after the last space of this row, and the width up to it
    let mut space: Option<(usize, f32)> = None;
    for (at, g) in text.grapheme_indices(true) {
        let w = advance(g);
        let start = *out.last().unwrap_or(&0);
        if x + w > max_w && at > start {
            match space {
                Some((after, upto)) if after > start && after <= at => {
                    out.push(after);
                    x -= upto;
                }
                _ => {
                    out.push(at);
                    x = 0.0;
                }
            }
            space = None;
        }
        x += w;
        if g == " " || g == "\t" {
            space = Some((at + g.len(), x));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(p: &Post) -> Vec<&str> {
        (p.first()..p.end())
            .map(|n| p.line(n).unwrap().text.as_str())
            .collect()
    }

    /// **Output continues the open line; a notice and a result do not run
    /// into it.** What `print(x, end="")` leaves open, the next write
    /// carries on; a notice starts below it, a result reads `-> value`.
    #[test]
    fn an_append_follows_the_post_windows_rules() {
        let mut p = Post::default();
        p.append(Kind::Out, "a");
        p.append(Kind::Out, "b\nc\n");
        assert_eq!(lines(&p), ["ab", "c"]);
        p.append(Kind::Out, "d");
        p.append(Kind::Info, "restarted\n");
        assert_eq!(lines(&p), ["ab", "c", "d", "restarted"]);
        p.append(Kind::Result, "42");
        assert_eq!(lines(&p), ["ab", "c", "d", "restarted", "-> 42"]);
        p.append(Kind::Out, "x\r\n");
        p.append(Kind::Err, "boom");
        p.append(Kind::Out, "!\n");
        let last = p.line(p.end() - 1).unwrap();
        assert_eq!(last.text, "boom!");
        assert_eq!(
            last.spans().collect::<Vec<_>>(),
            [(0, 4, Kind::Err), (4, 5, Kind::Out)]
        );
        p.append(Kind::Out, "\n");
        assert_eq!(p.line(p.end() - 1).unwrap().text, "", "an empty line");
    }

    /// **The oldest lines go past `keep`**, and the numbers of the rest stay
    /// theirs: a place named before still names its line.
    #[test]
    fn lines_past_keep_go_and_the_rest_keep_their_numbers() {
        let mut p = Post::new(3);
        for i in 0..5 {
            p.append(Kind::Out, &format!("{i}\n"));
        }
        assert_eq!(lines(&p), ["2", "3", "4"]);
        assert_eq!((p.first(), p.end()), (2, 5));
        assert_eq!(p.line(3).unwrap().text, "3");
        p.clear();
        assert!(p.is_empty());
        assert_eq!(p.first(), 5);
        p.append(Kind::Out, "again\n");
        assert_eq!(p.line(5).unwrap().text, "again");
    }

    /// **A copy is exactly what was selected**, either way round, across
    /// lines, with the line's ends written as they were.
    #[test]
    fn the_text_between_two_places_is_exactly_what_they_hold() {
        let mut p = Post::default();
        p.append(Kind::Out, "one two\nthree\nfour five\n");
        let a = Pos { line: 0, byte: 4 };
        let b = Pos { line: 2, byte: 4 };
        assert_eq!(p.text_between(a, b), "two\nthree\nfour");
        assert_eq!(p.text_between(b, a), "two\nthree\nfour");
        assert_eq!(p.text(), "one two\nthree\nfour five");
        let (w0, w1) = p.unit_at(Pos { line: 2, byte: 6 }, Unit::Word);
        assert_eq!(p.text_between(w0, w1), "five");
        let (l0, l1) = p.unit_at(Pos { line: 1, byte: 1 }, Unit::Line);
        assert_eq!(p.text_between(l0, l1), "three");
        // a word held, the pointer gone back a line: the selection keeps the word
        let held = p.unit_at(Pos { line: 1, byte: 2 }, Unit::Word);
        let (anchor, head) = p.extend(held, Pos { line: 0, byte: 1 }, Unit::Word);
        assert_eq!(p.text_between(anchor, head), "one two\nthree");
    }

    /// **A line wraps after the last space that fits**, and a word longer
    /// than a row breaks inside it.
    #[test]
    fn a_line_wraps_at_spaces_and_a_long_word_breaks() {
        let one = |_: &str| 1.0;
        assert_eq!(rows("abc def", 10.0, one), [0]);
        assert_eq!(rows("abc def ghi", 8.0, one), [0, 8]);
        assert_eq!(rows("abc def ghi", 5.0, one), [0, 4, 8]);
        assert_eq!(rows("abcdefgh", 3.0, one), [0, 3, 6]);
        assert_eq!(rows("", 3.0, one), [0]);
    }
}
