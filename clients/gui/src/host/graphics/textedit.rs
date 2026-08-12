//! The pure text-editing model for the editable `text` widget.
//!
//! This is the `text` field's own complex logic, kept out of the widget and
//! drawing code so neither is obscured by caret arithmetic. It is deliberately
//! **std-only** (no `unicode-segmentation`): the caret and the selection anchor
//! are byte offsets into the widget's `value` string, always kept on `char`
//! boundaries, and every motion is by `char` — the right granularity for the
//! embedded bitmap font, which is per-cell. The operations mutate the string in
//! place (`String::insert_str`/`replace_range`) so the `value` stays the single
//! source of truth the widget already emits as its event value.
//!
//! The module is platform-agnostic and window-free, so it unit-tests without a
//! GPU and compiles to wasm unchanged. The gesture machine
//! ([`crate::host::gestures`]) drives these from key input, and the drawing/ hit-test
//! layout helpers at the bottom are shared by the renderer ([`super::controls`])
//! and the caret-placement door ([`crate::host::interact`]), so what is drawn and what
//! is clicked never disagree.

/// The caret and its optional selection anchor, as byte offsets into a `value`
/// string (each on a `char` boundary). A selection exists when `anchor` is set
/// and differs from `pos`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Caret {
    /// The insertion point (a byte offset on a char boundary).
    pub pos: usize,
    /// The other end of a selection, if one is being made.
    pub anchor: Option<usize>,
}

impl Caret {
    /// The selection as an ordered byte range `[start, end)`, or `None` when
    /// there is no selection (no anchor, or an empty one).
    pub fn selection(&self) -> Option<(usize, usize)> {
        match self.anchor {
            Some(a) if a != self.pos => Some((a.min(self.pos), a.max(self.pos))),
            _ => None,
        }
    }

    /// Places the caret at `pos` with no selection.
    fn set(&mut self, pos: usize) {
        self.pos = pos;
        self.anchor = None;
    }
}

/// The byte offset of the char boundary just before `pos` (or 0 at the start).
fn prev_boundary(value: &str, pos: usize) -> usize {
    value[..pos]
        .char_indices()
        .next_back()
        .map_or(0, |(i, _)| i)
}

/// The byte offset of the char boundary just after `pos` (or the end).
fn next_boundary(value: &str, pos: usize) -> usize {
    value[pos..]
        .char_indices()
        .nth(1)
        .map_or(value.len(), |(i, _)| pos + i)
}

/// Re-lands `pos`/`anchor` on valid char boundaries within `value` — called
/// after an external `/gui_set value` replaces the string under the caret.
pub fn clamp(value: &str, caret: &mut Caret) {
    fn fix(value: &str, p: usize) -> usize {
        let mut p = p.min(value.len());
        while p > 0 && !value.is_char_boundary(p) {
            p -= 1;
        }
        p
    }
    caret.pos = fix(value, caret.pos);
    caret.anchor = caret
        .anchor
        .map(|a| fix(value, a))
        .filter(|&a| a != caret.pos);
}

/// Deletes the current selection, if any, collapsing the caret to its start.
/// Returns whether anything was removed.
pub fn delete_selection(value: &mut String, caret: &mut Caret) -> bool {
    if let Some((s, e)) = caret.selection() {
        value.replace_range(s..e, "");
        caret.set(s);
        true
    } else {
        false
    }
}

/// The selected substring, for copy/cut (`None` when there is no selection).
pub fn selected<'a>(value: &'a str, caret: &Caret) -> Option<&'a str> {
    caret.selection().map(|(s, e)| &value[s..e])
}

/// Inserts `text` at the caret, replacing any selection first. Newlines pass
/// through (the caller gates them on a single-line field). Always changes the
/// content.
pub fn insert(value: &mut String, caret: &mut Caret, text: &str) -> bool {
    delete_selection(value, caret);
    value.insert_str(caret.pos, text);
    caret.set(caret.pos + text.len());
    true
}

/// Backspace: deletes the selection, else the char before the caret. Returns
/// whether the content changed.
pub fn backspace(value: &mut String, caret: &mut Caret) -> bool {
    if delete_selection(value, caret) {
        return true;
    }
    if caret.pos == 0 {
        return false;
    }
    let prev = prev_boundary(value, caret.pos);
    value.replace_range(prev..caret.pos, "");
    caret.set(prev);
    true
}

/// Delete-forward: deletes the selection, else the char after the caret.
pub fn delete(value: &mut String, caret: &mut Caret) -> bool {
    if delete_selection(value, caret) {
        return true;
    }
    if caret.pos >= value.len() {
        return false;
    }
    let next = next_boundary(value, caret.pos);
    value.replace_range(caret.pos..next, "");
    true
}

/// Begins (or ends) a selection before a horizontal motion: with `select` a
/// missing anchor is seeded at the current position; without it, an existing
/// selection collapses toward `to_start` and the motion is skipped (the editor
/// convention that an unshifted arrow first collapses a selection). Returns
/// `true` when the motion should be skipped (a collapse happened).
fn begin_move(caret: &mut Caret, select: bool, to_start: bool) -> bool {
    if select {
        if caret.anchor.is_none() {
            caret.anchor = Some(caret.pos);
        }
        false
    } else if let Some((s, e)) = caret.selection() {
        caret.set(if to_start { s } else { e });
        true
    } else {
        caret.anchor = None;
        false
    }
}

/// Moves the caret one char left (extending the selection when `select`).
pub fn move_left(value: &str, caret: &mut Caret, select: bool) {
    if begin_move(caret, select, true) {
        return;
    }
    caret.pos = prev_boundary(value, caret.pos);
}

/// Moves the caret one char right (extending the selection when `select`).
pub fn move_right(value: &str, caret: &mut Caret, select: bool) {
    if begin_move(caret, select, false) {
        return;
    }
    caret.pos = next_boundary(value, caret.pos);
}

/// Whether `c` is part of a "word" for word-wise motion (alphanumeric or `_`).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The start of the **run** left of `pos`: a run of word chars or a run of
/// everything else, whichever the character before `pos` belongs to.
///
/// One run is the unit both word-wise operations are built from, and it is the
/// unit a **delete** takes: in `"a, b"` the first Ctrl+Backspace removes `b`
/// and the second removes `", "`, rather than swallowing the separators and
/// the word before them in one press. Deleting is destructive and a motion is
/// not, so the smaller step is the right default for it.
fn run_start_left(value: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let word = is_word(char_before(value, pos));
    let mut p = pos;
    while p > 0 && is_word(char_before(value, p)) == word {
        p = prev_boundary(value, p);
    }
    p
}

/// ...and its mirror: the end of the run right of `pos`.
fn run_end_right(value: &str, pos: usize) -> usize {
    if pos >= value.len() {
        return value.len();
    }
    let word = is_word(char_at(value, pos));
    let mut p = pos;
    while p < value.len() && is_word(char_at(value, p)) == word {
        p = next_boundary(value, p);
    }
    p
}

/// Where the start of the word left of `pos` is — **two** runs when the first
/// is separators, since a motion lands on a word rather than between two of
/// them. Built on the same scan the delete takes, so the two can never drift
/// into disagreeing about where a word begins.
fn word_start_left(value: &str, pos: usize) -> usize {
    let p = run_start_left(value, pos);
    if p > 0 && !is_word(char_before(value, pos)) {
        run_start_left(value, p)
    } else {
        p
    }
}

/// ...and its mirror, past the end of the word right of `pos`.
fn word_end_right(value: &str, pos: usize) -> usize {
    let p = run_end_right(value, pos);
    if p < value.len() && !is_word(char_at(value, pos)) {
        run_end_right(value, p)
    } else {
        p
    }
}

/// Moves the caret to the start of the word to its left (skipping any run of
/// non-word chars first) — Ctrl+Left.
pub fn move_word_left(value: &str, caret: &mut Caret, select: bool) {
    begin_move_extend(caret, select);
    caret.pos = word_start_left(value, caret.pos);
}

/// Moves the caret past the word to its right — Ctrl+Right.
pub fn move_word_right(value: &str, caret: &mut Caret, select: bool) {
    begin_move_extend(caret, select);
    caret.pos = word_end_right(value, caret.pos);
}

/// Ctrl+Backspace: deletes the selection, else the **run** to the caret's left
/// — the word it is sitting after, or the separators between it and the
/// previous word (see [`run_start_left`]). Returns whether the content changed.
pub fn backspace_word(value: &mut String, caret: &mut Caret) -> bool {
    if delete_selection(value, caret) {
        return true;
    }
    let start = run_start_left(value, caret.pos);
    if start == caret.pos {
        return false;
    }
    value.replace_range(start..caret.pos, "");
    caret.set(start);
    true
}

/// Ctrl+Delete: the same forward — the run to the caret's right.
pub fn delete_word(value: &mut String, caret: &mut Caret) -> bool {
    if delete_selection(value, caret) {
        return true;
    }
    let end = run_end_right(value, caret.pos);
    if end == caret.pos {
        return false;
    }
    value.replace_range(caret.pos..end, "");
    true
}

/// Seeds/clears the anchor for a word/vertical motion (no collapse-skip: these
/// always move).
fn begin_move_extend(caret: &mut Caret, select: bool) {
    if select {
        if caret.anchor.is_none() {
            caret.anchor = Some(caret.pos);
        }
    } else {
        caret.anchor = None;
    }
}

/// The char just before byte `p` (assumes `p > 0` on a boundary).
fn char_before(value: &str, p: usize) -> char {
    value[..p].chars().next_back().unwrap_or(' ')
}

/// The char at byte `p` (assumes `p < len` on a boundary).
fn char_at(value: &str, p: usize) -> char {
    value[p..].chars().next().unwrap_or(' ')
}

/// The byte offset where the line containing `pos` begins (just after the
/// previous newline, or 0).
fn line_start(value: &str, pos: usize) -> usize {
    value[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// The byte offset where the line containing `pos` ends (at the next newline, or
/// the end).
fn line_end(value: &str, pos: usize) -> usize {
    value[pos..].find('\n').map_or(value.len(), |i| pos + i)
}

/// Moves the caret to the start of its line — Home.
pub fn move_home(value: &str, caret: &mut Caret, select: bool) {
    begin_move_extend(caret, select);
    caret.pos = line_start(value, caret.pos);
}

/// Moves the caret to the end of its line — End.
pub fn move_end(value: &str, caret: &mut Caret, select: bool) {
    begin_move_extend(caret, select);
    caret.pos = line_end(value, caret.pos);
}

/// Moves the caret up one line, keeping its column (clamped to the target
/// line's length) — multiline only; a no-op on the first line.
pub fn move_up(value: &str, caret: &mut Caret, select: bool) {
    begin_move_extend(caret, select);
    let start = line_start(value, caret.pos);
    if start == 0 {
        return; // already on the first line
    }
    let col = value[start..caret.pos].chars().count();
    let prev_start = line_start(value, start - 1);
    caret.pos = col_to_byte(value, prev_start, start - 1, col);
}

/// Moves the caret down one line, keeping its column — multiline only; a no-op
/// on the last line.
pub fn move_down(value: &str, caret: &mut Caret, select: bool) {
    begin_move_extend(caret, select);
    let start = line_start(value, caret.pos);
    let end = line_end(value, caret.pos);
    if end >= value.len() {
        return; // already on the last line
    }
    let col = value[start..caret.pos].chars().count();
    let next_start = end + 1;
    let next_end = line_end(value, next_start);
    caret.pos = col_to_byte(value, next_start, next_end, col);
}

/// The byte offset of column `col` within the line `[start, end)` (clamped to
/// the line's char count).
fn col_to_byte(value: &str, start: usize, end: usize, col: usize) -> usize {
    value[start..end]
        .char_indices()
        .nth(col)
        .map_or(end, |(i, _)| start + i)
}

/// Selects the whole string — Ctrl+A.
pub fn select_all(value: &str, caret: &mut Caret) {
    caret.anchor = Some(0);
    caret.pos = value.len();
}

// --- Layout helpers shared by the renderer and the hit-test ----------------

/// The `(line, column)` of byte `pos`: the line is the count of newlines before
/// it, the column the char count since the line start.
pub fn line_col(value: &str, pos: usize) -> (usize, usize) {
    let line = value[..pos].bytes().filter(|&b| b == b'\n').count();
    let col = value[line_start(value, pos)..pos].chars().count();
    (line, col)
}

/// The byte offset of `(line, col)`, each clamped into range — the inverse of
/// [`line_col`], used to turn a click's row/column into a caret position.
pub fn offset_of(value: &str, line: usize, col: usize) -> usize {
    let mut start = 0;
    for _ in 0..line {
        match value[start..].find('\n') {
            Some(i) => start += i + 1,
            None => {
                // Past the last line: clamp to the end.
                return value.len();
            }
        }
    }
    let end = line_end(value, start);
    col_to_byte(value, start, end, col)
}

/// The first visible column so a caret at `caret_col` stays within a `cols`-wide
/// window (right-anchored once it overflows). Stateless: recomputed each frame
/// from the caret alone, so there is no scroll offset to keep in sync.
pub fn h_scroll(caret_col: usize, cols: usize) -> usize {
    if cols == 0 || caret_col < cols {
        0
    } else {
        caret_col + 1 - cols
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret(pos: usize) -> Caret {
        Caret { pos, anchor: None }
    }

    #[test]
    fn insert_and_backspace_move_the_caret() {
        let mut s = String::new();
        let mut c = caret(0);
        insert(&mut s, &mut c, "abc");
        assert_eq!((s.as_str(), c.pos), ("abc", 3));
        move_left(&s, &mut c, false);
        insert(&mut s, &mut c, "X");
        assert_eq!((s.as_str(), c.pos), ("abXc", 3));
        assert!(backspace(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), ("abc", 2));
        assert!(delete(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), ("ab", 2));
        assert!(!delete(&mut s, &mut c)); // nothing after the caret
    }

    #[test]
    fn backspace_at_start_is_a_no_op() {
        let mut s = "hi".to_string();
        let mut c = caret(0);
        assert!(!backspace(&mut s, &mut c));
        assert_eq!(s, "hi");
    }

    #[test]
    fn selection_replace_and_delete() {
        let mut s = "hello".to_string();
        let mut c = Caret {
            pos: 4,
            anchor: Some(1),
        };
        assert_eq!(selected(&s, &c), Some("ell"));
        // Typing over a selection replaces it.
        insert(&mut s, &mut c, "i");
        assert_eq!((s.as_str(), c.pos), ("hio", 2));
        // Backspace deletes a selection whole.
        let mut c = Caret {
            pos: 0,
            anchor: Some(3),
        };
        assert!(backspace(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), ("", 0));
    }

    #[test]
    fn shift_arrows_extend_then_unshifted_collapses() {
        let s = "abcd".to_string();
        let mut c = caret(1);
        move_right(&s, &mut c, true);
        move_right(&s, &mut c, true);
        assert_eq!(c.selection(), Some((1, 3)));
        // An unshifted left collapses to the selection start (no move past it).
        move_left(&s, &mut c, false);
        assert_eq!((c.pos, c.anchor), (1, None));
    }

    #[test]
    fn word_motion_skips_runs() {
        let s = "foo_bar baz".to_string();
        let mut c = caret(s.len());
        move_word_left(&s, &mut c, false);
        assert_eq!(&s[c.pos..], "baz");
        move_word_left(&s, &mut c, false);
        assert_eq!(&s[c.pos..], "foo_bar baz"); // whole first word (with `_`)
        move_word_right(&s, &mut c, false);
        assert_eq!(&s[..c.pos], "foo_bar");
    }

    /// A word-wise delete takes **one run** per press — the word, then the
    /// separators — where the motion crosses both in one step. Deleting is
    /// destructive, so it steps smaller than the caret does.
    #[test]
    fn word_delete_takes_one_run_per_press() {
        let mut s = "a, b".to_string();
        let mut c = caret(s.len());
        assert!(backspace_word(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), ("a, ", 3), "the word alone");
        assert!(backspace_word(&mut s, &mut c));
        assert_eq!(
            (s.as_str(), c.pos),
            ("a", 1),
            "then the comma and the space"
        );
        assert!(backspace_word(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), ("", 0));
        // Nothing left to remove: no change, and no event behind it.
        assert!(!backspace_word(&mut s, &mut c));

        let mut s = "a, b".to_string();
        let mut c = caret(0);
        assert!(delete_word(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), (", b", 0));
        assert!(delete_word(&mut s, &mut c));
        assert_eq!((s.as_str(), c.pos), ("b", 0));
        assert!(delete_word(&mut s, &mut c));
        assert_eq!(s, "");
        assert!(!delete_word(&mut s, &mut c));
    }

    /// ...and the motion still crosses to a word, which is the one place the
    /// two deliberately differ.
    #[test]
    fn word_motion_still_lands_on_a_word() {
        let s = "a, b".to_string();
        let mut c = caret(s.len());
        move_word_left(&s, &mut c, false);
        assert_eq!(c.pos, 3, "to the start of \"b\"");
        move_word_left(&s, &mut c, false);
        assert_eq!(c.pos, 0, "past the separators, to the start of \"a\"");
    }

    /// A selection wins over the word, in both directions — the rule the plain
    /// Backspace/Delete already follow.
    #[test]
    fn word_delete_takes_the_selection_when_there_is_one() {
        for forward in [false, true] {
            let mut s = "hello world".to_string();
            let mut c = Caret {
                pos: 5,
                anchor: Some(0),
            };
            assert!(if forward {
                delete_word(&mut s, &mut c)
            } else {
                backspace_word(&mut s, &mut c)
            });
            assert_eq!((s.as_str(), c.pos), (" world", 0));
        }
    }

    #[test]
    fn home_end_are_line_aware() {
        let s = "one\ntwo".to_string();
        let mut c = caret(5); // between t and w on line 2
        move_home(&s, &mut c, false);
        assert_eq!(c.pos, 4); // start of "two"
        move_end(&s, &mut c, false);
        assert_eq!(c.pos, 7); // end of "two"
    }

    #[test]
    fn vertical_motion_keeps_the_column() {
        let s = "hello\nhi\nworld".to_string();
        let mut c = caret(4); // "hell|o" on line 1, col 4
        move_down(&s, &mut c, false);
        // Line 2 "hi" is shorter than col 4 → clamps to its end.
        assert_eq!(c.pos, 8); // end of "hi"
        move_down(&s, &mut c, false);
        // Column carried is the clamped one (2) into "world".
        assert_eq!(&s[line_start(&s, c.pos)..c.pos], "wo");
        move_up(&s, &mut c, false);
        assert_eq!(c.pos, 8); // back to end of "hi"
    }

    #[test]
    fn move_up_on_first_line_and_down_on_last_are_no_ops() {
        let s = "abc".to_string();
        let mut c = caret(1);
        move_up(&s, &mut c, false);
        assert_eq!(c.pos, 1);
        move_down(&s, &mut c, false);
        assert_eq!(c.pos, 1);
    }

    #[test]
    fn clamp_relands_on_char_boundaries() {
        let s = "áé".to_string(); // each char is 2 bytes
        let mut c = Caret {
            pos: 3,
            anchor: Some(1),
        };
        clamp(&s, &mut c);
        assert!(s.is_char_boundary(c.pos));
        assert_eq!(c.pos, 2);
        assert!(c.anchor.is_none_or(|a| s.is_char_boundary(a)));
    }

    #[test]
    fn line_col_and_offset_round_trip() {
        let s = "ab\ncde\nf";
        assert_eq!(line_col(s, 5), (1, 2)); // "cd|e"
        assert_eq!(offset_of(s, 1, 2), 5);
        // A column past a line's end clamps to the line end.
        assert_eq!(offset_of(s, 0, 9), 2);
        // A line past the last clamps to the string end.
        assert_eq!(offset_of(s, 9, 0), s.len());
    }

    #[test]
    fn h_scroll_keeps_the_caret_visible() {
        assert_eq!(h_scroll(3, 10), 0); // fits: no scroll
        assert_eq!(h_scroll(12, 10), 3); // caret past the window: right-anchored
        assert_eq!(h_scroll(9, 10), 0); // exactly the last visible cell
        assert_eq!(h_scroll(5, 0), 0); // degenerate width
    }

    #[test]
    fn select_all_spans_the_string() {
        let s = "abc".to_string();
        let mut c = caret(1);
        select_all(&s, &mut c);
        assert_eq!(c.selection(), Some((0, 3)));
    }
}
