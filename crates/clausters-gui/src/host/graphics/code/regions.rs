//! **What an evaluation reads**: the region of the text each of the
//! editor's evaluating keys sends, as the current editor finds it.
//!
//! - **The line or the selection** (Shift+Enter): the selection, from its
//!   line's start when only indentation stands before it, or the caret's
//!   line; nothing on a blank line.
//! - **The block** (Ctrl+Enter): the selection; else, in a text with cell
//!   markers (`# %%`, `// %%`), the cell the caret is in; else the top-level
//!   statement holding the caret (a whole `def`, `for`, `with`), or, as a
//!   preference, the run of top-level statements around it that no blank
//!   line separates. After a block the caret goes to the line past it, past
//!   the next cell's marker for a cell.
//!
//! A top-level statement is a child of the syntax tree's root, a comment
//! included; with no tree (plain text, or a build without the parser) the
//! statement is the caret's line.

use std::ops::Range;

use super::Code;

/// What a block is when nothing is selected and the text has no cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Block {
    /// The top-level statement holding the caret.
    #[default]
    Statement,
    /// The run of top-level statements around it no blank line separates.
    Paragraph,
}

/// Whether `line` marks a cell: a comment that starts with `%%`, as `# %%`
/// in Python and `// %%` in JavaScript, with a title or a kind after it or
/// not (`# %% Setup`, `# %% [markdown]`).
pub fn is_cell_marker(line: &str) -> bool {
    let rest = line.strip_prefix('#').or_else(|| line.strip_prefix("//"));
    rest.is_some_and(|r| r.trim_start().starts_with("%%"))
}

impl Code {
    /// **The selection or the caret's line**: the selection from its line's
    /// start when only indentation stands before it; none on a blank line.
    pub fn line_or_selection(&self) -> Option<Range<usize>> {
        match self.caret.selection() {
            Some((a, b)) => {
                let start = self.starts[self.line_of(a)];
                let from = if self.text[start..a].trim().is_empty() {
                    start
                } else {
                    a
                };
                Some(from..b)
            }
            None => {
                let (start, line) = self.line(self.line_of(self.caret.pos))?;
                (!line.trim().is_empty()).then(|| start..start + line.len())
            }
        }
    }

    /// The text's top-level statements in order, each from the start of
    /// its first line to the end of its code; none without a tree.
    fn top_level(&self) -> Vec<Range<usize>> {
        self.parse(None);
        let Some(nodes) = self.syntax.borrow().top_level() else {
            return Vec::new();
        };
        nodes
            .into_iter()
            .filter_map(|(from, to)| {
                let to = from + self.text[from..to].trim_end().len();
                (to > from).then(|| self.starts[self.line_of(from)]..to)
            })
            .collect()
    }

    /// The statement on the caret's line, by its index; none on a blank
    /// line.
    fn statement_here(&self, statements: &[Range<usize>]) -> Option<usize> {
        let (start, line) = self.line(self.line_of(self.caret.pos))?;
        if line.trim().is_empty() {
            return None;
        }
        let end = start + line.len();
        statements
            .iter()
            .position(|r| r.start <= end && r.end >= start)
    }

    /// The caret's line, when it has text: what a block is with no
    /// statement to read.
    fn text_line(&self) -> Option<Range<usize>> {
        let (start, line) = self.line(self.line_of(self.caret.pos))?;
        (!line.trim().is_empty()).then(|| start..start + line.len())
    }

    /// **The top-level statement holding the caret**: a whole `def`, `for`
    /// or `with`; the caret's line when no statement is read there.
    pub fn top_level_block(&self) -> Option<Range<usize>> {
        let statements = self.top_level();
        match self.statement_here(&statements) {
            Some(i) => Some(statements[i].clone()),
            None => self.text_line(),
        }
    }

    /// **The top-level statements around the caret no blank line
    /// separates**: a statement with blank lines inside it stays whole.
    pub fn paragraph_block(&self) -> Option<Range<usize>> {
        let statements = self.top_level();
        let Some(i) = self.statement_here(&statements) else {
            return self.top_level_block();
        };
        let touching =
            |a: &Range<usize>, b: &Range<usize>| self.line_of(b.start) <= self.line_of(a.end) + 1;
        let (mut first, mut last) = (i, i);
        while first > 0 && touching(&statements[first - 1], &statements[first]) {
            first -= 1;
        }
        while last + 1 < statements.len() && touching(&statements[last], &statements[last + 1]) {
            last += 1;
        }
        Some(statements[first].start..statements[last].end)
    }

    /// The lines that mark a cell, from 0.
    pub fn cell_markers(&self) -> Vec<usize> {
        (0..self.lines())
            .filter(|&n| self.line(n).is_some_and(|(_, l)| is_cell_marker(l)))
            .collect()
    }

    /// **The cell holding the caret**: the lines between the markers around
    /// it (or the text's ends), the blank lines at its edges left out; none
    /// for a cell with nothing in it.
    pub fn cell_at(&self, markers: &[usize]) -> Option<Range<usize>> {
        let here = self.line_of(self.caret.pos) as isize;
        let mut start = markers
            .iter()
            .rev()
            .map(|&m| m as isize)
            .find(|&m| m <= here)
            .unwrap_or(-1)
            + 1;
        let mut end = markers
            .iter()
            .map(|&m| m as isize)
            .find(|&m| m > here.max(start - 1))
            .unwrap_or(self.lines() as isize)
            - 1;
        let blank = |n: isize| {
            self.line(n as usize)
                .is_none_or(|(_, l)| l.trim().is_empty())
        };
        while start <= end && blank(start) {
            start += 1;
        }
        while end >= start && blank(end) {
            end -= 1;
        }
        if start > end {
            return None;
        }
        let (from, _) = self.line(start as usize)?;
        let (s, l) = self.line(end as usize)?;
        Some(from..s + l.len())
    }

    /// **The block Ctrl+Enter evaluates**, and whether it is a cell: the
    /// selection, the cell, or the statement or paragraph, as `block` asks.
    pub fn block(&self, block: Block) -> Option<(Range<usize>, bool)> {
        if self.caret.selection().is_some() {
            return self.line_or_selection().map(|r| (r, false));
        }
        let markers = self.cell_markers();
        if !markers.is_empty() {
            return self.cell_at(&markers).map(|r| (r, true));
        }
        let region = match block {
            Block::Statement => self.top_level_block(),
            Block::Paragraph => self.paragraph_block(),
        };
        region.map(|r| (r, false))
    }

    /// **The caret past `region`**: at the start of the line after it, or
    /// after the next cell's marker when it was a cell, so evaluating again
    /// goes on from there. Nothing past the text's last line.
    pub fn advance(&mut self, region: &Range<usize>, cell: bool) {
        let mut n = self.line_of(region.end) + 1;
        if cell {
            while n < self.lines() && !self.line(n).is_some_and(|(_, l)| is_cell_marker(l)) {
                n += 1;
            }
            n += 1;
        }
        if let Some((start, _)) = self.line(n) {
            self.select(start, start);
        }
    }
}
