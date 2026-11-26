//! **The popup layer**: what opens *over* a window's tree -- a chooser's list,
//! a menu and its submenus, a tip -- and the one function that places all of
//! it.
//!
//! A popup is the host's, not an element's. An element used to declare a
//! rectangle of its own and draw into it, and the rectangle was resolved once,
//! at the press, against the whole window: so the list of a chooser near the
//! bottom edge was drawn under the status bar, a list taller than the window
//! ran off it, a resize left it standing where the field had been, and only a
//! press ever reached it -- no wheel, no key, no pointer motion. None of that is
//! one element's to fix, because none of it is one element's: a menu bar and a
//! context menu have no element at all.
//!
//! So the **stack** lives on the host, one per window ([`Popups`]), and it is
//! placed by **one function** ([`layout`]) that both the frame and the gesture
//! machine call -- which is what makes a row hit where it was drawn. Placement
//! is against the **work area** (the window minus the host's own bands), it
//! flips to the side that has room, it holds the far edge, and what does not
//! fit scrolls. It runs at every frame, so a popup follows the thing it hangs
//! off.
//!
//! What is listed is a [`menu`] tree: a chooser's options are entries with no
//! verb, answered by the element that opened the list
//! ([`Element::picked`](super::widget::Element::picked)); a menu's entries
//! carry a verb and the host reports it.

use super::graphics::controls::ScrollBar;
use super::menu::{self, Entry};
use super::menubar::{self, Title};
use super::widget::Widget;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// Who opened the stack, and so who answers a pick.
#[derive(Debug, Clone, PartialEq)]
pub enum Owner {
    /// An element's own list -- a chooser. The pick is the element's
    /// (`Element::picked`), addressed by the row's position.
    Element(i32),
    /// The window's menu bar: the list hangs off this title.
    Bar(Title),
    /// The `context` menu of this widget, opened at the pointer.
    Context(i32),
    /// The `menu` of this button, opened under it.
    Button(i32),
    /// The **edit menu** of an element that takes text -- Cut, Copy, Paste,
    /// Delete, Select all -- the host's own, opened at the pointer where the
    /// element carries no `context` of its own. A pick is an edit of that
    /// element, not a report.
    Edit(i32),
    /// **An element's own context menu**
    /// ([`Element::context_menu`](super::widget::Element::context_menu)),
    /// opened at the pointer where the widget carries no `context`. A pick is
    /// the element's command, performed rather than reported.
    Own(i32),
    /// **The window's key sheet** (the `keys` verb, F1): what each key does
    /// in the window, read and never picked. It stands in the middle of the
    /// window under a title strip with a close mark, and only that mark and
    /// Escape take it down -- a press anywhere else is swallowed.
    Keys,
    /// **The dock's own menu** -- a panel's or a page's -- opened over the
    /// dock's surface rather than a window's, and answered by the dock
    /// ([`crate::host::dock`]).
    Dock,
}

/// Where the first list of a stack hangs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// Under this rectangle, or over it when there is no room below -- a
    /// field's list, a title's menu.
    Below(Rect),
    /// At this point: a context menu.
    At(f32, f32),
    /// In the middle of the work area: the key sheet.
    Centre,
}

/// One open list: the entries it shows and what the hand is doing in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    /// The indices from the owner's root down to this list -- empty for the
    /// first one.
    pub path: Vec<usize>,
    pub entries: Vec<Entry>,
    /// The row under the pointer, or the one the keys walked to.
    pub hover: Option<usize>,
    /// How far the list is scrolled, in pixels, when it is taller than its
    /// place.
    pub scroll: f32,
    /// Whether the scroll follows the hovered row -- set by a key, cleared by
    /// the wheel, so a list walked from the keyboard keeps its row on screen
    /// and one scrolled by hand stays where the hand put it.
    pub reveal: bool,
}

impl Level {
    fn new(path: Vec<usize>, entries: Vec<Entry>) -> Self {
        Self {
            path,
            entries,
            hover: None,
            scroll: 0.0,
            reveal: false,
        }
    }
}

/// The open lists of one window: the first, and the submenus opened from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    pub owner: Owner,
    pub anchor: Anchor,
    /// Where the owner's own rectangle stood when the stack opened, so the
    /// anchor follows it: a resize or a pan moves the widget, and the list
    /// hangs off the widget.
    pub origin: Option<(f32, f32)>,
    /// The least a list may be wide -- a chooser's list is at least its field.
    pub min_w: f32,
    /// The most rows the first list shows, the rest scrolled; all of them
    /// when none.
    pub max_rows: Option<usize>,
    pub text_size: f32,
    pub levels: Vec<Level>,
    /// What has been typed to find an entry by its name, and when the last
    /// letter came: letters a second apart start a new search.
    pub typed: (String, f64),
}

impl Stack {
    pub fn new(owner: Owner, entries: Vec<Entry>, anchor: Anchor, text_size: f32) -> Self {
        Self {
            owner,
            anchor,
            origin: None,
            min_w: 0.0,
            max_rows: None,
            text_size,
            levels: vec![Level::new(Vec::new(), entries)],
            typed: (String::new(), f64::NEG_INFINITY),
        }
    }

    /// The widget the stack hangs off, when it hangs off one.
    pub fn owner_widget(&self) -> Option<i32> {
        match self.owner {
            Owner::Element(id)
            | Owner::Context(id)
            | Owner::Button(id)
            | Owner::Edit(id)
            | Owner::Own(id) => Some(id),
            Owner::Bar(_) | Owner::Keys | Owner::Dock => None,
        }
    }

    /// The path of row `row` of list `level`, from the root.
    pub fn path_of(&self, level: usize, row: usize) -> Vec<usize> {
        let mut path = self.levels[level].path.clone();
        path.push(row);
        path
    }

    /// Opens the submenu of `row` of list `level`, closing whatever was open
    /// past that list. Answers whether anything changed.
    pub fn open_child(&mut self, level: usize, row: usize) -> bool {
        let Some(sub) = self.levels[level]
            .entries
            .get(row)
            .filter(|e| e.enabled)
            .and_then(Entry::submenu)
        else {
            return false;
        };
        let path = self.path_of(level, row);
        if self
            .levels
            .get(level + 1)
            .is_some_and(|next| next.path == path)
        {
            return false;
        }
        let child = Level::new(path, sub.to_vec());
        self.levels.truncate(level + 1);
        self.levels.push(child);
        true
    }

    /// Closes every list past `keep` (which stays). Answers whether any did.
    pub fn close_past(&mut self, keep: usize) -> bool {
        let had = self.levels.len();
        self.levels.truncate((keep + 1).max(1));
        self.levels.len() != had
    }

    /// The pointer is over `row` of list `level` (or over none of that list):
    /// the hover moves, a submenu row opens its list, and a plain row closes
    /// the lists past its own. Answers whether the picture changed.
    pub fn point(&mut self, level: usize, row: Option<usize>) -> bool {
        let mut changed = self.levels[level].hover != row;
        self.levels[level].hover = row;
        self.levels[level].reveal = false;
        match row {
            Some(r) if self.levels[level].entries[r].submenu().is_some() => {
                changed |= self.open_child(level, r);
            }
            Some(_) => changed |= self.close_past(level),
            // Off the rows of this list -- a separator, its margin: the
            // submenu stays, since the hand is usually on its way to it.
            None => {}
        }
        changed
    }
}

/// How long letters typed to find an entry keep adding to one search, in
/// milliseconds.
const TYPED_MS: f64 = 1000.0;

/// How long the pointer may cross other rows on its way to an open submenu
/// before they take it, in milliseconds.
pub const AIM_MS: f64 = 300.0;

impl Stack {
    /// **A letter typed on the deepest list**: the hover goes to the next
    /// entry whose label starts with what was typed in the last second.
    /// One letter typed again steps through the entries starting with it.
    /// Answers whether the hover moved.
    pub fn type_ahead(&mut self, c: char, now_ms: f64) -> bool {
        if now_ms - self.typed.1 > TYPED_MS {
            self.typed.0.clear();
        }
        self.typed.1 = now_ms;
        self.typed.0.extend(c.to_lowercase());
        let first = self.typed.0.chars().next().unwrap_or(c);
        let same = self.typed.0.chars().all(|x| x == first);
        let needle = if same {
            first.to_string()
        } else {
            self.typed.0.clone()
        };
        let top = self.levels.len() - 1;
        let level = &mut self.levels[top];
        let n = level.entries.len();
        // a new letter starts the search after the hover; a word goes on
        // from it, so the row it already found stays found
        let start = match level.hover {
            Some(h) if same => h + 1,
            Some(h) => h,
            None => 0,
        };
        let found = (0..n).map(|k| (start + k) % n).find(|&i| {
            let e = &level.entries[i];
            e.pickable() && e.label.to_lowercase().starts_with(&needle)
        });
        if let Some(i) = found {
            level.hover = Some(i);
            level.reveal = true;
        }
        found.is_some()
    }
}

/// **Whether the pointer is on its way to an open submenu**: it moved from
/// `from` to `to` over a row of list `level` other than the one whose
/// submenu is open, inside the triangle from where it was to the near edge
/// of that submenu. Such a crossing does not take the rows it passes over.
pub fn aiming(
    stack: &Stack,
    placed: &[Placed],
    level: usize,
    row: Option<usize>,
    from: Option<(f64, f64)>,
    to: (f64, f64),
) -> bool {
    let (Some(from), Some(child)) = (from, placed.get(level + 1)) else {
        return false;
    };
    let opener = stack
        .levels
        .get(level + 1)
        .and_then(|next| next.path.last().copied());
    if row.is_some() && row == opener {
        return false;
    }
    let r = child.rect;
    let near = if f64::from(r.x) >= from.0 {
        f64::from(r.x)
    } else {
        f64::from(r.x + r.w)
    };
    let (top, bottom) = (f64::from(r.y), f64::from(r.y + r.h));
    let tri = [from, (near, top), (near, bottom)];
    let side = |a: (f64, f64), b: (f64, f64), p: (f64, f64)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let d = [
        side(tri[0], tri[1], to),
        side(tri[1], tri[2], to),
        side(tri[2], tri[0], to),
    ];
    let neg = d.iter().any(|&v| v < 0.0);
    let pos = d.iter().any(|&v| v > 0.0);
    // moving at all, and toward it
    to != from && !(neg && pos)
}

/// What a key did to an open stack.
#[derive(Debug, Clone, PartialEq)]
pub enum Keyed {
    /// The hover moved, or a list opened or closed: repaint.
    Moved,
    /// The row at this path was picked.
    Pick(Vec<usize>),
    /// The last list was closed: the stack is gone.
    Closed,
    /// Left or right at the first list of a menu bar: the neighbouring title.
    Step(i32),
    /// Not a key a list answers; it is still swallowed, a popup being modal.
    Ignored,
}

/// The keys that walk an open stack. They are the standard ones and are not
/// bindings: a list is modal, so nothing else is listening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
}

/// The next pickable row from `from` in direction `dir`, wrapping; `None` when
/// the list has nothing to land on.
fn step(entries: &[Entry], from: Option<usize>, dir: i32) -> Option<usize> {
    let n = entries.len() as i32;
    if n == 0 {
        return None;
    }
    let start = match (from, dir > 0) {
        (Some(i), _) => i as i32,
        (None, true) => -1,
        (None, false) => n,
    };
    (1..=n)
        .map(|k| (start + dir * k).rem_euclid(n) as usize)
        .find(|&i| entries[i].pickable())
}

impl Stack {
    /// One walking key, on the deepest open list.
    pub fn key(&mut self, key: Walk) -> Keyed {
        let top = self.levels.len() - 1;
        let level = &mut self.levels[top];
        let moved = |level: &mut Level, to: Option<usize>| {
            level.hover = to.or(level.hover);
            level.reveal = true;
            Keyed::Moved
        };
        match key {
            Walk::Down => {
                let to = step(&level.entries, level.hover, 1);
                moved(level, to)
            }
            Walk::Up => {
                let to = step(&level.entries, level.hover, -1);
                moved(level, to)
            }
            Walk::Home => {
                let to = step(&level.entries, None, 1);
                moved(level, to)
            }
            Walk::End => {
                let to = step(&level.entries, None, -1);
                moved(level, to)
            }
            Walk::Right => match level.hover {
                Some(row) if level.entries[row].submenu().is_some() => {
                    self.open_child(top, row);
                    let child = self.levels.last_mut().expect("a stack has a list");
                    child.hover = step(&child.entries, None, 1);
                    child.reveal = true;
                    Keyed::Moved
                }
                _ if matches!(self.owner, Owner::Bar(_)) => Keyed::Step(1),
                _ => Keyed::Ignored,
            },
            Walk::Left => {
                if top > 0 {
                    self.levels.pop();
                    Keyed::Moved
                } else if matches!(self.owner, Owner::Bar(_)) {
                    Keyed::Step(-1)
                } else {
                    Keyed::Ignored
                }
            }
            Walk::Enter => match level.hover {
                Some(row) if level.entries[row].submenu().is_some() => self.key(Walk::Right),
                Some(row) if level.entries[row].pickable() => Keyed::Pick(self.path_of(top, row)),
                _ => Keyed::Ignored,
            },
            Walk::Escape => {
                if top > 0 {
                    self.levels.pop();
                    Keyed::Moved
                } else {
                    Keyed::Closed
                }
            }
        }
    }
}

/// **A tooltip**: a short text over what it is the tip of, with the key that
/// does the same, when one does.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    /// The widget whose tip this is.
    pub widget: i32,
    pub text: String,
    /// The chord bound to what the widget does, drawn as a key cap.
    pub key: Option<String>,
    /// What it is the tip of: the widget's rectangle, as far as it shows.
    pub anchor: Rect,
}

pub use super::widget::own::{Note, Own, OwnList};

impl OwnList {
    /// The list as a stack of one level, which the layout places and the
    /// frame draws as it does every list.
    pub fn stack(&self, id: i32) -> Stack {
        let mut stack = Stack::new(
            Owner::Element(id),
            self.entries.clone(),
            Anchor::Below(self.anchor),
            self.text_size,
        );
        stack.max_rows = Some(self.rows);
        stack.levels[0].hover = self.hover;
        stack.levels[0].reveal = true;
        stack
    }
}

/// One line of a note's documentation as drawn, and whether the text it
/// was wrapped out of ends with it (a copy joins it to the next with a line
/// break, else with a space).
#[derive(Debug, Clone, PartialEq)]
pub struct NoteLine {
    pub text: String,
    pub ends: bool,
    /// **The runs it is drawn in**, left to right: where each starts in
    /// `text`, how far right of the line's start it stands, its face and
    /// its size. One run of a plain line, several of a line of Markdown.
    pub runs: Vec<NoteRun>,
    /// Its top and its height, from the documentation's top; for the
    /// head's lines, from the head's.
    pub y: f32,
    pub h: f32,
}

/// One run of a [`NoteLine`]: from byte `from` of its text to the next
/// run's, at `x`, in `family` at `size`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteRun {
    pub from: usize,
    pub x: f32,
    pub family: font::Family,
    pub size: f32,
}

impl NoteLine {
    /// A line written in one face at one size, `y` from its top.
    fn plain(text: String, ends: bool, family: font::Family, size: f32, y: f32, h: f32) -> Self {
        Self {
            text,
            ends,
            runs: vec![NoteRun {
                from: 0,
                x: 0.0,
                family,
                size,
            }],
            y,
            h,
        }
    }

    /// The runs with the end of each, as `(run, to)`.
    fn spans(&self) -> impl Iterator<Item = (&NoteRun, usize)> {
        self.runs.iter().enumerate().map(|(i, r)| {
            let to = self.runs.get(i + 1).map_or(self.text.len(), |n| n.from);
            (r, to)
        })
    }

    /// The x of byte `at`, from the line's start.
    pub fn x_of(&self, at: usize) -> f32 {
        let at = at.min(self.text.len());
        let mut last = 0.0;
        for (r, to) in self.spans() {
            if at <= to {
                let piece = self.text.get(r.from..at.max(r.from)).unwrap_or_default();
                return r.x + font::width_in(r.family, piece, r.size);
            }
            last = r.x + font::width_in(r.family, &self.text[r.from..to], r.size);
        }
        last
    }

    /// The byte nearest `dx` right of the line's start.
    pub fn byte_at(&self, dx: f32) -> usize {
        for (r, to) in self.spans() {
            let piece = &self.text[r.from..to];
            let end = r.x + font::width_in(r.family, piece, r.size);
            if dx >= end {
                continue;
            }
            for (i, c) in piece.char_indices() {
                let left = r.x + font::width_in(r.family, &piece[..i], r.size);
                let w = font::width_in(r.family, &piece[i..i + c.len_utf8()], r.size);
                if dx < left + w * 0.5 {
                    return r.from + i;
                }
            }
            return to;
        }
        self.text.len()
    }
}

/// **A note, placed**: its card, the band of its head and the lines it is
/// written on, the window its documentation is read through and the lines
/// of it, all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct NotePlace {
    pub rect: Rect,
    pub head: Rect,
    /// The head's lines: one, or a card's long signature wrapped.
    pub heads: Vec<NoteLine>,
    pub head_line: f32,
    /// Where the documentation shows: its lines scroll through it.
    pub doc: Rect,
    pub lines: Vec<NoteLine>,
    pub line_h: f32,
    /// **A card's documentation as Markdown**, laid out as the
    /// documentation panel lays a page out; its lines are its rows of
    /// text, what a selection walks. `None` for a note that is no card.
    pub laid: Option<crate::host::graphics::docs::layout::Laid>,
    /// How tall the documentation is, all of it.
    pub doc_full: f32,
}

impl NotePlace {
    /// The furthest the documentation scrolls.
    pub fn max_scroll(&self) -> f32 {
        (self.doc_full - self.doc.h).max(0.0)
    }

    /// **The documentation's scroll bar**, in the card's right inset beside
    /// it, with it scrolled by `scroll`; `None` when all of it is seen.
    pub fn bar(&self, scroll: f32, m: &Metrics) -> Option<ScrollBar> {
        let w = m.scrollbar_w.min(m.card_x);
        let track = Rect::new(self.rect.x + self.rect.w - w, self.doc.y, w, self.doc.h);
        ScrollBar::new(
            true,
            track,
            self.doc.h,
            self.max_scroll(),
            scroll,
            self.line_h,
        )
    }

    /// **The code block of the documentation at `(x, y)` that scrolls
    /// across**, with the documentation scrolled by `scroll`: its index and
    /// how far it scrolls at most.
    pub fn pan_at(&self, scroll: f32, x: f32, y: f32) -> Option<(usize, f32)> {
        let laid = self.laid.as_ref()?;
        let (left, top) = (self.doc.x, self.doc.y - scroll);
        laid.pans.iter().enumerate().find_map(|(i, b)| {
            let r = Rect::new(left + b.rect.x, top + b.rect.y, b.rect.w, b.rect.h);
            (b.most() > 0.0 && r.contains(f64::from(x), f64::from(y))).then(|| (i, b.most()))
        })
    }

    /// How far line `i` is scrolled across: its code block's, when it is in
    /// one that scrolls ([`Card::pans`]).
    pub fn line_pan(&self, i: usize, pans: &[f32]) -> f32 {
        let (Some(laid), Some((l, false))) = (self.laid.as_ref(), self.line(i)) else {
            return 0.0;
        };
        laid.pans
            .iter()
            .position(|b| l.y >= b.rect.y && l.y < b.rect.y + b.rect.h)
            .map_or(0.0, |b| {
                pans.get(b)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, laid.pans[b].most())
            })
    }

    /// Every line a selection walks: the head's, then the documentation's.
    pub fn count(&self) -> usize {
        self.heads.len() + self.lines.len()
    }

    /// Line `i` of them, and whether it is the head's.
    pub fn line(&self, i: usize) -> Option<(&NoteLine, bool)> {
        match self.heads.get(i) {
            Some(l) => Some((l, true)),
            None => self.lines.get(i - self.heads.len()).map(|l| (l, false)),
        }
    }

    /// Where line `i` is drawn, with the documentation scrolled by `scroll`:
    /// its top and its height.
    pub fn line_top(&self, i: usize, scroll: f32) -> (f32, f32) {
        match self.line(i) {
            Some((l, true)) => (self.head.y + l.y, l.h),
            Some((l, false)) => (self.doc.y + l.y - scroll, l.h),
            None => (self.doc.y, self.line_h),
        }
    }
}

/// **Where in a note's text**: a line (the head's lines first, the
/// documentation's after them) and a byte in it.
pub type NotePos = (usize, usize);

/// **Rows of text read as one text**: each row's text in order, a line
/// break after the row that ends its paragraph and a space after one the
/// paragraph was wrapped from. So a place in the rows is a place in one
/// string, which the text model selects over as it selects a field
/// (`textedit`: a word, a paragraph as its line, a drag growing by either),
/// and what a selection copies is a slice of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub rows: Vec<NoteLine>,
    text: String,
    /// Where each row starts in `text`.
    starts: Vec<usize>,
}

impl Flow {
    pub fn of(rows: Vec<NoteLine>) -> Self {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            starts.push(text.len());
            text.push_str(&row.text);
            if i + 1 < rows.len() {
                text.push(if row.ends { '\n' } else { ' ' });
            }
        }
        Self { rows, text, starts }
    }

    /// All of it, as one text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The place `pos` of the rows, in the text.
    pub fn offset(&self, (row, at): NotePos) -> usize {
        match (self.starts.get(row), self.rows.get(row)) {
            (Some(start), Some(r)) => start + at.min(r.text.len()),
            _ => self.text.len(),
        }
    }

    /// The place in the rows of `offset` in the text: a row's end for the
    /// break or the space after it.
    pub fn pos(&self, offset: usize) -> NotePos {
        let row = self
            .starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let start = self.starts.get(row).copied().unwrap_or(0);
        let len = self.rows.get(row).map_or(0, |r| r.text.len());
        (row, (offset - start.min(offset)).min(len))
    }

    /// The row at `y`, from the rows' top: the one it is on, or the last
    /// above it.
    pub fn row_at(&self, y: f32) -> usize {
        self.rows.iter().rposition(|l| l.y <= y).unwrap_or(0)
    }

    /// **What is selected of each row** from `a` to `b` (text offsets, in
    /// either order): the row, and its byte range. A row the selection goes
    /// on past ends selected, the break or the space after it included.
    pub fn spans(&self, a: usize, b: usize) -> Vec<(usize, usize, usize, bool)> {
        let (a, b) = (a.min(b), a.max(b));
        let ((r0, from), (r1, to)) = (self.pos(a), self.pos(b));
        (r0..=r1)
            .filter_map(|i| {
                let len = self.rows.get(i)?.text.len();
                let s = if i == r0 { from } else { 0 };
                let e = if i == r1 { to } else { len };
                Some((i, s, e, i < r1))
            })
            .collect()
    }
}

/// **A hover card's own state**, the host's, since the element only says what
/// it shows: how far its documentation is scrolled and what of its text is
/// selected, from the press to where the drag is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Card {
    /// The element whose note it is.
    pub owner: i32,
    pub scroll: f32,
    pub select: Option<(NotePos, NotePos)>,
    /// **How far each code block of its documentation wider than the card
    /// is scrolled across**, by the block's index in the layout's `pans`, as
    /// the documentation panel scrolls one.
    pub pans: Vec<f32>,
    /// The copy button of its documentation a press holds, drawn held
    /// down until the release.
    pub copy_held: Option<usize>,
}

impl Card {
    /// How far code block `i` is scrolled across.
    pub fn pan(&self, i: usize) -> f32 {
        self.pans.get(i).copied().unwrap_or(0.0)
    }
}

/// The widest a note is that is no card, in the documentation's characters.
const NOTE_CHARS: f32 = 72.0;

/// The share of the work area's width a hover card may take.
const CARD_SHARE: f32 = 0.9;

/// **A note's head, wrapped to `w`**: its text as one flowing line
/// ([`Note::head_flow`]) cut where it no longer fits, each line in runs of
/// the code's face, bold where the head marks it, `line` apart.
fn head_lines(note: &Note, w: f32, line: f32) -> Vec<NoteLine> {
    let (text, marks) = note.head_flow();
    let size = note.code_size;
    let cut = font::wrap_in(font::Family::Mono, &text, w, size);
    let n = cut.len();
    let mut at = 0;
    let mut out = Vec::with_capacity(n);
    for (i, piece) in cut.into_iter().enumerate() {
        // where the line stands in the head's text: the wrap cut it at a
        // space (taken out) or inside a word too wide for a line
        let from = text[at..].find(&piece).map_or(at, |o| at + o);
        let to = from + piece.len();
        at = to;
        // the line's runs: plain and marked by turns, at the marks' bounds
        let mut bounds = vec![0];
        for &(a, b) in &marks {
            for edge in [a, b] {
                if edge > from && edge < to {
                    bounds.push(edge - from);
                }
            }
        }
        bounds.sort_unstable();
        bounds.dedup();
        let mut runs = Vec::with_capacity(bounds.len());
        for &start in &bounds {
            let marked = marks
                .iter()
                .any(|&(a, b)| from + start >= a && from + start < b);
            let family = if marked {
                font::Family::MonoBold
            } else {
                font::Family::Mono
            };
            let x = runs.last().map_or(0.0, |r: &NoteRun| {
                r.x + font::width_in(r.family, &piece[r.from..start], size)
            });
            runs.push(NoteRun {
                from: start,
                x,
                family,
                size,
            });
        }
        out.push(NoteLine {
            text: piece,
            ends: i + 1 == n,
            runs,
            y: i as f32 * line,
            h: line,
        });
    }
    if out.is_empty() {
        out.push(NoteLine::plain(
            String::new(),
            true,
            font::Family::Mono,
            size,
            0.0,
            line,
        ));
    }
    out
}

/// `text` wrapped to `w` in `family`, each line saying whether the text
/// ends with it, `line` apart from `y`.
fn wrapped(
    family: font::Family,
    text: &str,
    w: f32,
    size: f32,
    y: f32,
    line: f32,
) -> Vec<NoteLine> {
    let lines = font::wrap_in(family, text, w, size);
    let n = lines.len();
    lines
        .into_iter()
        .enumerate()
        .map(|(i, text)| NoteLine::plain(text, i + 1 == n, family, size, y + i as f32 * line, line))
        .collect()
}

/// **The rows of text of a laid-out document**, each its runs in the
/// order they stand: what a selection in a card or in the documentation
/// walks. A row ends where the stretch of text the layout laid it from
/// ends -- a paragraph, a heading, a line of code -- and a stretch wrapped
/// onto the next row goes on there.
pub(crate) fn rows_of(laid: &crate::host::graphics::docs::layout::Laid) -> Vec<NoteLine> {
    use crate::host::graphics::docs::layout::Item;
    let mut texts: Vec<(f32, f32, &str, font::Family, f32, usize)> = laid
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Text {
                x,
                y,
                text,
                family,
                size,
                para: Some(para),
                ..
            } => Some((*y, *x, text.as_str(), *family, *size, *para)),
            _ => None,
        })
        .collect();
    texts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // each row, and the stretches of text it begins and ends in
    let mut rows: Vec<(NoteLine, usize, usize)> = Vec::new();
    for (y, x, text, family, size, para) in texts {
        let h = font::line_advance(size);
        match rows.last_mut() {
            Some((row, _, last)) if (row.y - y).abs() < 0.5 => {
                // a table's cells stand apart by a tab, as a copied table's
                // do; the space the layout leaves between two runs of one
                // stretch, which no piece carries, is text a copy keeps
                let end = row.x_of(row.text.len());
                if para != *last {
                    row.text.push('\t');
                } else if x - end > 0.3 * font::width_in(family, " ", size) {
                    row.text.push(' ');
                }
                *last = para;
                row.runs.push(NoteRun {
                    from: row.text.len(),
                    x,
                    family,
                    size,
                });
                row.text.push_str(text);
                row.h = row.h.max(h);
            }
            _ => rows.push((
                NoteLine {
                    text: text.to_string(),
                    ends: true,
                    runs: vec![NoteRun {
                        from: 0,
                        x,
                        family,
                        size,
                    }],
                    y,
                    h,
                },
                para,
                para,
            )),
        }
    }
    // a row ends its stretch of text where the next row is another's
    let starts: Vec<usize> = rows.iter().map(|r| r.1).collect();
    let tops: Vec<f32> = rows.iter().map(|r| r.0.y).collect();
    rows.into_iter()
        .enumerate()
        .map(|(i, (mut row, _, last))| {
            row.ends = starts.get(i + 1).is_none_or(|&next| next != last);
            // **A row is its line's box**, as a selection fills it: as tall
            // as the lines stand apart -- to the next row, or from the one
            // before, where they stand a line apart -- else a page's
            // leading, about the middle of its letters. The pieces are laid
            // at the top of their letters, which a band from there would
            // hang below.
            let size = row.runs.iter().map(|r| r.size).fold(0.0f32, f32::max);
            let pitch = crate::host::graphics::docs::layout::line_pitch(size);
            let near = |d: f32| (d > 0.0 && d <= pitch * 1.05).then_some(d);
            let h = tops
                .get(i + 1)
                .and_then(|next| near(next - row.y))
                .or_else(|| i.checked_sub(1).and_then(|j| near(row.y - tops[j])))
                .unwrap_or(pitch);
            let middle = row.y + font::height(size) * 0.5;
            row.y = middle - h * 0.5;
            row.h = h;
            row
        })
        .collect()
}

/// **Where a note goes, and the lines of its documentation**: over its
/// anchor where it fits, under it otherwise, held inside the work area. Its
/// anatomy is the hover card's: an inset of [`Metrics::card_x`] by
/// [`Metrics::card_y`], the head, [`Metrics::card_gap`] and the
/// documentation, at most [`Metrics::card_w`] wide. A card's head wraps as
/// its documentation does; a note's that is no card is one line.
pub fn place_note(note: &Note, work: Rect, m: &Metrics) -> NotePlace {
    let aside_w = if note.aside.is_empty() {
        0.0
    } else {
        m.card_x + font::width(&note.aside, note.text_size)
    };
    // the widest it is, its insets included
    let most = if note.card {
        m.card_w.min(work.w * CARD_SHARE)
    } else {
        NOTE_CHARS * font::width("0", note.text_size) + 2.0 * m.card_x
    }
    .min(work.w)
    .max(1.0);
    let wrap_w = (most - 2.0 * m.card_x).max(1.0);
    let head_line = font::line_advance(note.code_size);
    // **the head wraps to the note's width**, a card's and a call's alike,
    // the word beside it kept on its last line
    let heads = head_lines(note, (wrap_w - aside_w).max(1.0), head_line);
    let head_w = heads
        .iter()
        .map(|l| l.x_of(l.text.len()))
        .fold(0.0f32, f32::max)
        + aside_w;
    let line_h = font::line_advance(note.text_size);
    // **A card's documentation is Markdown**, laid out as the documentation
    // panel lays a page out; a note's that is no card, a few plain lines
    let (lines, laid, doc_full) = if note.card && !note.doc.trim().is_empty() {
        use crate::host::graphics::docs::{layout, markdown};
        let laid = layout::lay(&markdown::parse(note.doc.trim()), wrap_w, note.text_size);
        let full = laid.height;
        (rows_of(&laid), Some(laid), full)
    } else {
        let mut lines: Vec<NoteLine> = Vec::new();
        for line in note.doc.trim().lines() {
            let y = lines.len() as f32 * line_h;
            if line.trim().is_empty() {
                lines.push(NoteLine::plain(
                    String::new(),
                    true,
                    font::Family::Ui,
                    note.text_size,
                    y,
                    line_h,
                ));
            } else {
                lines.extend(wrapped(
                    font::Family::Ui,
                    line,
                    wrap_w,
                    note.text_size,
                    y,
                    line_h,
                ));
            }
            if !note.card && lines.len() >= note.doc_lines {
                break;
            }
        }
        if !note.card {
            lines.truncate(note.doc_lines);
        }
        let full = lines.len() as f32 * line_h;
        (lines, None, full)
    };
    let doc_w = lines
        .iter()
        .map(|l| l.x_of(l.text.len()))
        .fold(0.0f32, f32::max);
    let w = (head_w.max(doc_w) + 2.0 * m.card_x).min(most);
    let head_h = heads.len().max(1) as f32 * head_line;
    let doc_h = if note.card {
        doc_full.min(m.card_doc_h)
    } else {
        doc_full
    };
    let h = head_h
        + if lines.is_empty() {
            0.0
        } else {
            m.card_gap + doc_h
        }
        + 2.0 * m.card_y;
    let (w, h) = (w.min(work.w), h.min(work.h));
    let a = note.anchor;
    let y = if a.y - h >= work.y {
        a.y - h
    } else {
        a.y + a.h
    };
    let rect = Rect::new(hold_x(a.x, w, work), hold_y(y, h, work), w, h);
    let head = Rect::new(
        rect.x + m.card_x,
        rect.y + m.card_y,
        w - 2.0 * m.card_x,
        head_h,
    );
    let top = head.y + head_h + m.card_gap;
    let doc = Rect::new(
        head.x,
        top,
        head.w,
        doc_h.min((rect.y + rect.h - m.card_y - top).max(0.0)),
    );
    NotePlace {
        rect,
        head,
        heads,
        head_line,
        doc,
        lines,
        line_h,
        laid,
        doc_full,
    }
}

impl Note {
    /// The head as one line, the runs joined ([`Self::head_flow`]).
    pub fn head_text(&self) -> String {
        self.head_flow().0
    }

    /// **The head as one flowing line**, and where in it the marked runs
    /// are: every stretch of white space -- a line break and the indentation
    /// after it included, as an owner prints a long signature -- is one
    /// space, none at either end. So the head wraps to the note's width and
    /// draws no control character, whoever wrote it.
    pub fn head_flow(&self) -> (String, Vec<(usize, usize)>) {
        let mut text = String::new();
        let mut marks = Vec::new();
        let mut space = false;
        for (run, marked) in &self.head {
            let mut first = None;
            for c in run.chars() {
                if c.is_whitespace() {
                    space = !text.is_empty();
                    continue;
                }
                if space {
                    text.push(' ');
                    space = false;
                }
                first.get_or_insert(text.len());
                text.push(c);
            }
            if *marked && let Some(from) = first {
                marks.push((from, text.len()));
            }
        }
        (text, marks)
    }

    /// **The place in the text at `(x, y)`**, with the documentation scrolled
    /// by `scroll`: above the documentation is the head, past either end of a
    /// line is its end.
    /// A line in a code block scrolled across (`pans`, [`Card::pans`]) is
    /// read where it is drawn.
    pub fn pos_at(&self, place: &NotePlace, scroll: f32, pans: &[f32], x: f32, y: f32) -> NotePos {
        let heads = place.heads.len();
        let line = if y < place.doc.y || place.lines.is_empty() {
            let row = ((y - place.head.y) / place.head_line.max(1.0))
                .floor()
                .max(0.0) as usize;
            row.min(heads.saturating_sub(1))
        } else {
            // the row it is on, or the last above it
            let at = y - place.doc.y + scroll;
            let row = place.lines.iter().rposition(|l| l.y <= at).unwrap_or(0);
            heads + row
        };
        let dx = x - place.head.x + place.line_pan(line, pans);
        let at = place.line(line).map_or(0, |(l, _)| l.byte_at(dx));
        (line, at)
    }

    /// The x of byte `at` of line `i`, from the text's left, its code block
    /// scrolled across by `pans`.
    pub fn x_of(&self, place: &NotePlace, pans: &[f32], i: usize, at: usize) -> f32 {
        place.line(i).map_or(0.0, |(l, _)| l.x_of(at)) - place.line_pan(i, pans)
    }

    /// **The text from `a` to `b`**, in either order: the head and the
    /// documentation on lines of their own, the lines either was wrapped
    /// into joined back with a space.
    pub fn text_between(&self, place: &NotePlace, a: NotePos, b: NotePos) -> String {
        let flow = Flow::of(place.heads.iter().chain(&place.lines).cloned().collect());
        let (a, b) = (flow.offset(a), flow.offset(b));
        flow.text()[a.min(b)..a.max(b)].to_string()
    }
}

/// What is open over one window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Popups {
    pub stack: Option<Stack>,
    pub tip: Option<Tip>,
    /// The hover card over the window: its scroll and its selection.
    pub card: Option<Card>,
    /// The title of the menu bar the pointer is over, with no list open.
    pub bar_hover: Option<Title>,
    /// The widget the pointer is over, when it is one a hand can act on -- what
    /// a control draws its hover from.
    pub hover: Option<i32>,
    /// The dialog whose close button the pointer is over, which draws it
    /// lit.
    pub close_hover: Option<i32>,
    /// The section whose bar the pointer is over, which draws its fold mark
    /// lit: a press anywhere on the bar folds it.
    pub fold_hover: Option<i32>,
}

/// **The rectangle a stack hangs off, as it stands now**: its owner's
/// placement, or -- for a menu bar's -- the title on the bar.
///
/// One function for the frame and for the hit test, like [`layout`] itself:
/// both hand it the placements they just made.
pub fn owner_rect(
    stack: &Stack,
    tree: &Widget,
    placed: &[super::layout::Placed],
    window: Rect,
    m: &Metrics,
) -> Option<Rect> {
    match stack.owner {
        Owner::Bar(title) => {
            let band = menubar::bar(tree, window, m)?;
            menubar::titles(menubar::entries(tree)?, band, m)
                .into_iter()
                .find(|(t, _)| *t == title)
                .map(|(_, r)| r)
        }
        Owner::Element(id)
        | Owner::Context(id)
        | Owner::Button(id)
        | Owner::Edit(id)
        | Owner::Own(id) => placed
            .iter()
            .find(|p| p.widget.id == Some(id))
            .map(|p| p.rect),
        Owner::Keys | Owner::Dock => None,
    }
}

/// What the key sheet's title strip says.
pub const KEYS_TITLE: &str = "Keys";

/// **The size a list's entries are drawn at**: the interface's size of the
/// type scale, the menus' and every list's.
pub fn list_size(m: &Metrics) -> f32 {
    font::em_scale(font::Family::Ui, m.type_sm)
}

/// **The size a tip is drawn at**: the smallest of the type scale.
pub fn tip_size(m: &Metrics) -> f32 {
    font::em_scale(font::Family::Ui, m.type_xs)
}

/// **The size an entry's key and aside are drawn at**: the smallest of the
/// type scale, in step with the list's own size.
pub fn small_size(text_size: f32, m: &Metrics) -> f32 {
    text_size * m.type_xs / m.type_sm.max(1.0)
}

/// The height of one row of a list: a line of the interface's text and an
/// entry's inset over and under it, and never less than a control's.
pub fn row_h(text_size: f32, m: &Metrics) -> f32 {
    (font::height(text_size) + 2.0 * m.pad)
        .max(m.control_h)
        .max(m.line_sm + 2.0 * m.menu_item_y)
}

/// The height a separator takes: a hairline with the list's inset over and
/// under it.
pub fn separator_h(m: &Metrics) -> f32 {
    2.0 * m.menu_inset + m.divider_w
}

/// The gutter a list reserves at its left for a mark or an icon: what puts
/// a checkable entry's text at the specification's indent ([`Metrics::menu_mark`])
/// from the row's edge.
pub fn gutter(_text_size: f32, m: &Metrics) -> f32 {
    (m.menu_mark - m.menu_item_x).max(0.0)
}

/// The gutter a list reserves at its right for a submenu's chevron: the
/// icon and the gap before it.
pub fn chevron_gutter(text_size: f32, m: &Metrics) -> f32 {
    font::advance(text_size) + m.menu_item_x
}

/// **The letter spacing of a key**: a tenth of its em, as the specification
/// sets a shortcut (`tracking-widest`).
pub fn key_tracking(size: f32) -> f32 {
    0.1 * font::height(size) / 0.727
}

/// The width of a key written at `size` with its letter spacing.
pub fn key_width(key: &str, size: f32) -> f32 {
    let n = key.chars().count() as f32;
    font::width(key, size) + (n - 1.0).max(0.0) * key_tracking(size)
}

fn entry_h(e: &Entry, text_size: f32, m: &Metrics) -> f32 {
    if e.is_separator() {
        separator_h(m)
    } else {
        row_h(text_size, m)
    }
}

/// **The gutter an entry keeps at the left of its text**: a check or a
/// radio entry's, or one with an icon, puts the text at the specification's
/// indent; any other entry's text stands at the row's own inset.
pub fn indent(e: &Entry, text_size: f32, m: &Metrics) -> f32 {
    if e.icon.is_some() || e.can_mark() {
        gutter(text_size, m)
    } else {
        0.0
    }
}

/// The gutter a list keeps at the right of its entries' text for a
/// submenu's chevron, when any entry opens one.
pub fn chevrons(entries: &[Entry], text_size: f32, m: &Metrics) -> f32 {
    if entries.iter().any(|e| e.submenu().is_some()) {
        chevron_gutter(text_size, m)
    } else {
        0.0
    }
}

/// **How big a list wants to be**: as wide as its widest entry with the
/// gutters and insets it needs, and as tall as its rows and its inset. The
/// width is held between the list's least and the widest a list grows
/// ([`Metrics::menu_max_w`]), past which an entry is cut with an ellipsis,
/// its aside first.
///
/// The entries are props, so this is a size read out of props -- nothing a
/// stream of values can move.
pub fn natural(entries: &[Entry], text_size: f32, m: &Metrics) -> (f32, f32) {
    let right = chevrons(entries, text_size, m);
    let small = small_size(text_size, m);
    let text = entries
        .iter()
        .map(|e| {
            indent(e, text_size, m)
                + font::width(&e.label, text_size)
                + e.aside
                    .as_deref()
                    .map_or(0.0, |a| m.menu_aside + font::width(a, small))
        })
        .fold(0.0f32, f32::max);
    let h = entries
        .iter()
        .map(|e| entry_h(e, text_size, m))
        .sum::<f32>()
        + 2.0 * m.menu_inset;
    let w = 2.0 * (m.menu_inset + m.menu_item_x) + text + key_column(entries, text_size, m) + right;
    (w.min(m.menu_max_w), h)
}

/// The column a list keeps at the right of its labels for the chords its
/// entries are bound to: the widest chord and a gap before it, or nothing when
/// no entry has one.
pub fn key_column(entries: &[Entry], text_size: f32, m: &Metrics) -> f32 {
    let small = small_size(text_size, m);
    entries
        .iter()
        .filter_map(|e| e.key.as_deref())
        .map(|k| key_width(k, small) + 2.0 * m.menu_item_x)
        .fold(0.0f32, f32::max)
}

/// One list as it stands on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    /// The list's rectangle, inside the work area.
    pub rect: Rect,
    /// Each row's rectangle in window pixels, already scrolled -- so a row
    /// scrolled out of the list lies outside [`rect`](Self::rect).
    pub rows: Vec<Rect>,
    /// The scroll actually applied (the stored one, clamped, or the one that
    /// reveals the hovered row).
    pub scroll: f32,
    /// How far the list could scroll.
    pub max_scroll: f32,
    /// **The title strip over the rows**, for a list that has one -- the key
    /// sheet's, which ends in its close mark ([`close`](Self::close)).
    pub strip: Option<Rect>,
}

impl Placed {
    /// Where the rows are seen: the list under its title strip.
    pub fn body(&self) -> Rect {
        match self.strip {
            Some(strip) => Rect::new(
                self.rect.x,
                self.rect.y + strip.h,
                self.rect.w,
                (self.rect.h - strip.h).max(0.0),
            ),
            None => self.rect,
        }
    }

    /// The **close mark** at the right end of the title strip.
    pub fn close(&self) -> Option<Rect> {
        self.strip.map(|strip| {
            let side = strip.h.min(strip.w);
            Rect::new(strip.x + strip.w - side, strip.y, side, side)
        })
    }

    /// The row under `(x, y)`, when the point is on this list and on a row a
    /// hand can land on.
    pub fn row_at(&self, entries: &[Entry], x: f64, y: f64) -> Option<usize> {
        if !self.body().contains(x, y) {
            return None;
        }
        self.rows
            .iter()
            .position(|r| r.contains(x, y))
            .filter(|&i| entries[i].pickable())
    }
}

/// **Where a rectangle of `size` goes** under (or over) `anchor`, inside
/// `work`: below when it fits, above when only that does, and otherwise on the
/// side with more room, cut to it -- the caller scrolls what was cut.
pub fn place_below(anchor: Rect, size: (f32, f32), work: Rect) -> Rect {
    let w = size.0.min(work.w);
    let below = (work.y + work.h - (anchor.y + anchor.h)).max(0.0);
    let above = (anchor.y - work.y).max(0.0);
    let (y, h) = if size.1 <= below {
        (anchor.y + anchor.h, size.1)
    } else if size.1 <= above {
        (anchor.y - size.1, size.1)
    } else if below >= above {
        (anchor.y + anchor.h, below)
    } else {
        (work.y, above)
    };
    Rect::new(hold_x(anchor.x, w, work), y, w, h)
}

/// The same for a list opening **beside** a row -- a submenu: to its right,
/// or to its left when the right has no room, level with the row and held
/// inside the work area.
pub fn place_beside(row: Rect, size: (f32, f32), work: Rect) -> Rect {
    let w = size.0.min(work.w);
    let h = size.1.min(work.h);
    let right = row.x + row.w;
    let x = if right + w <= work.x + work.w || row.x - w < work.x {
        hold_x(right, w, work)
    } else {
        row.x - w
    };
    Rect::new(x, hold_y(row.y, h, work), w, h)
}

/// The same for a list opening **at a point** -- a context menu: its corner on
/// the pointer, turned to the other side of it where it would leave the area.
pub fn place_at(at: (f32, f32), size: (f32, f32), work: Rect) -> Rect {
    let w = size.0.min(work.w);
    let h = size.1.min(work.h);
    let x = if at.0 + w <= work.x + work.w {
        at.0
    } else {
        at.0 - w
    };
    let y = if at.1 + h <= work.y + work.h {
        at.1
    } else {
        at.1 - h
    };
    Rect::new(hold_x(x, w, work), hold_y(y, h, work), w, h)
}

/// How much of the area's height a list standing in the middle of it may
/// take: a sheet taller than that scrolls, and the window shows around it.
pub const CENTRE_FILL: f32 = 0.8;

/// The same for a list standing **in the middle** of the area -- the key
/// sheet: centred both ways, and no taller than [`CENTRE_FILL`] of the area,
/// so it reads as a sheet over the window rather than as the window.
pub fn place_centre(size: (f32, f32), work: Rect) -> Rect {
    let w = size.0.min(work.w);
    let h = size.1.min((work.h * CENTRE_FILL).round());
    Rect::new(
        work.x + (work.w - w) * 0.5,
        work.y + (work.h - h) * 0.5,
        w,
        h,
    )
}

fn hold_x(x: f32, w: f32, work: Rect) -> f32 {
    x.min(work.x + work.w - w).max(work.x)
}

fn hold_y(y: f32, h: f32, work: Rect) -> f32 {
    y.min(work.y + work.h - h).max(work.y)
}

/// **Places every list of a stack**, first to last.
///
/// `owner` is the rectangle the owner stands in now (its widget's placement),
/// which is what moves the anchor with it; `work` is the area a popup may
/// cover. One function for the frame and for the hit test, so a press lands on
/// the row that was drawn.
pub fn layout(stack: &Stack, owner: Option<Rect>, work: Rect, m: &Metrics) -> Vec<Placed> {
    let size = stack.text_size;
    let shift = match (stack.origin, owner) {
        (Some((ox, oy)), Some(now)) => (now.x - ox, now.y - oy),
        _ => (0.0, 0.0),
    };
    // A bar's list hangs off its title, wherever the bar puts it now.
    let anchor = match (&stack.owner, owner) {
        (Owner::Bar(_), Some(title)) => Anchor::Below(title),
        _ => stack.anchor,
    };
    let mut out: Vec<Placed> = Vec::with_capacity(stack.levels.len());
    for (i, level) in stack.levels.iter().enumerate() {
        let (w, h) = natural(&level.entries, size, m);
        // A list standing on its own carries a title strip, and is as wide
        // as its title and close mark need.
        let strip_h = if i == 0 && stack.anchor == Anchor::Centre {
            row_h(size, m)
        } else {
            0.0
        };
        let w = if strip_h > 0.0 {
            w.max(font::width(KEYS_TITLE, size) + 2.0 * m.pad + strip_h)
        } else {
            w
        };
        let shown = match stack.max_rows {
            Some(rows) if i == 0 => h.min(rows as f32 * row_h(size, m) + 2.0 * m.menu_inset),
            _ => h,
        };
        // a menu is at least the specification's width, a submenu a little
        // less; a chooser's list is at least its field
        let least = match (i, &stack.owner) {
            (0, Owner::Element(_) | Owner::Keys) => stack.min_w,
            (0, _) => stack.min_w.max(m.menu_w),
            _ => m.submenu_w,
        };
        let want = (w.max(least), shown + strip_h);
        let rect = match (i, anchor) {
            (0, Anchor::Below(a)) => place_below(
                Rect::new(a.x + shift.0, a.y + shift.1, a.w, a.h),
                want,
                work,
            ),
            (0, Anchor::At(x, y)) => place_at((x, y), want, work),
            (0, Anchor::Centre) => place_centre(want, work),
            _ => {
                // A submenu hangs off the row that opened it, in the list
                // before it -- which is already placed.
                let parent = &out[i - 1];
                let row = level
                    .path
                    .last()
                    .and_then(|r| parent.rows.get(*r))
                    .copied()
                    .unwrap_or(parent.rect);
                // Level with the row while the row is on screen.
                let row = Rect::new(parent.rect.x, row.y - m.menu_inset, parent.rect.w, row.h);
                place_beside(row, want, work)
            }
        };
        let strip = (strip_h > 0.0).then(|| Rect::new(rect.x, rect.y, rect.w, strip_h.min(rect.h)));
        let seen = (rect.h - strip_h).max(0.0);
        let max_scroll = (h - seen).max(0.0);
        let mut scroll = level.scroll.clamp(0.0, max_scroll);
        if level.reveal
            && let Some(row) = level.hover
        {
            let top: f32 = m.menu_inset
                + level.entries[..row]
                    .iter()
                    .map(|e| entry_h(e, size, m))
                    .sum::<f32>();
            let bottom = top + entry_h(&level.entries[row], size, m);
            if top < scroll {
                scroll = top;
            } else if bottom > scroll + seen {
                scroll = bottom - seen;
            }
            scroll = scroll.clamp(0.0, max_scroll);
        }
        // the rows inside the list's inset
        let mut y = rect.y + strip_h + m.menu_inset - scroll;
        let (x, w) = (
            rect.x + m.menu_inset,
            (rect.w - 2.0 * m.menu_inset).max(0.0),
        );
        let rows = level
            .entries
            .iter()
            .map(|e| {
                let eh = entry_h(e, size, m);
                let r = Rect::new(x, y, w, eh);
                y += eh;
                r
            })
            .collect();
        out.push(Placed {
            rect,
            rows,
            scroll,
            max_scroll,
            strip,
        });
    }
    out
}

/// The list and the row under `(x, y)`: the deepest list first, since a
/// submenu lies over the list that opened it. `None` off every list; the row
/// is `None` on a list's separator or on a row that cannot be picked.
pub fn hit(stack: &Stack, placed: &[Placed], x: f64, y: f64) -> Option<(usize, Option<usize>)> {
    placed
        .iter()
        .enumerate()
        .rev()
        .find(|(_, p)| p.rect.contains(x, y))
        .map(|(i, p)| (i, p.row_at(&stack.levels[i].entries, x, y)))
}

/// **A tooltip, placed**: its card, the point its arrow reaches to, and
/// where its key cap stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TipPlace {
    pub rect: Rect,
    /// The arrow's tip, on the edge of what the tooltip is the tip of.
    pub arrow: (f32, f32),
    /// Whether it stands over what it is the tip of (its arrow under it).
    pub above: bool,
    pub key: Option<Rect>,
}

/// **Where a tooltip goes**: centered over what it is the tip of, a little
/// off it, and under it where the work area has no room over it, held
/// inside the area across, its arrow pointing at the middle of the anchor.
/// Its text is the smallest of the type scale; its key, after it, a key
/// cap ([`Metrics::kbd_h`]).
pub fn place_tip(tip: &Tip, text_size: f32, work: Rect, m: &Metrics) -> TipPlace {
    let text_w = font::width(&tip.text, text_size);
    let kbd_w = tip
        .key
        .as_deref()
        .map(|k| font::width_in(font::Family::UiMedium, k, text_size) + 2.0 * m.kbd_x);
    let w =
        m.tip_x + text_w + kbd_w.map_or(m.tip_x, |k| m.tip_key_gap + k.max(m.kbd_h) + m.tip_end);
    let inner = m.line_xs.max(if kbd_w.is_some() { m.kbd_h } else { 0.0 });
    let h = inner + 2.0 * m.tip_y;
    let (w, h) = (w.min(work.w), h.min(work.h));
    let a = tip.anchor;
    let mid = a.x + a.w * 0.5;
    let above = a.y - m.tip_offset - h >= work.y || a.y + a.h + m.tip_offset + h > work.y + work.h;
    let y = if above {
        a.y - m.tip_offset - h
    } else {
        a.y + a.h + m.tip_offset
    };
    let rect = Rect::new(hold_x(mid - w * 0.5, w, work), hold_y(y, h, work), w, h);
    // the arrow stays clear of the card's rounded corners
    let corner = m.radius_md + m.tip_arrow;
    let ax = mid.clamp(
        rect.x + corner,
        (rect.x + rect.w - corner).max(rect.x + corner),
    );
    let arrow = if above {
        (ax, rect.y + rect.h + m.tip_arrow)
    } else {
        (ax, rect.y - m.tip_arrow)
    };
    let key = kbd_w.map(|k| {
        let k = k.max(m.kbd_h);
        Rect::new(
            rect.x + rect.w - m.tip_end - k,
            rect.y + (rect.h - m.kbd_h) * 0.5,
            k,
            m.kbd_h,
        )
    });
    TipPlace {
        rect,
        arrow,
        above,
        key,
    }
}

impl super::Host {
    /// What is open over window `def_id`, for a front about to draw it.
    pub(crate) fn popups(&self, def_id: i32) -> Option<&Popups> {
        self.popups.get(&def_id)
    }

    /// The open stack of window `def_id`.
    pub(crate) fn popup(&self, def_id: i32) -> Option<&Stack> {
        self.popups.get(&def_id)?.stack.as_ref()
    }

    pub(crate) fn popup_mut(&mut self, def_id: i32) -> Option<&mut Stack> {
        self.popups.get_mut(&def_id)?.stack.as_mut()
    }

    /// Opens `stack` over window `def_id`, in place of whatever was open: one
    /// stack per window, since a list is modal.
    pub(crate) fn open_popup(&mut self, def_id: i32, mut stack: Stack) {
        // **A list shows the keys** of the verbs its entries report, read from
        // the table as it opens -- a submenu is opened out of these same
        // entries, so it carries them too.
        let keys = &self.keys;
        for level in &mut stack.levels {
            menu::show_keys(&mut level.entries, &|verb| keys.label(verb));
        }
        let popups = self.popups.entry(def_id).or_default();
        popups.tip = None;
        popups.stack = Some(stack);
    }

    /// Closes the stack over window `def_id`, answering whether one was open.
    pub(crate) fn close_popup(&mut self, def_id: i32) -> bool {
        self.popups
            .get_mut(&def_id)
            .is_some_and(|p| p.stack.take().is_some())
    }

    /// The title of window `def_id`'s menu bar the pointer is over, written by
    /// pointer motion. Answers whether it changed.
    pub(crate) fn set_bar_hover(&mut self, def_id: i32, title: Option<Title>) -> bool {
        let popups = self.popups.entry(def_id).or_default();
        if popups.bar_hover == title {
            return false;
        }
        popups.bar_hover = title;
        true
    }

    /// The widget the pointer is over in window `def_id`, written by pointer
    /// motion. Answers whether it changed.
    pub(crate) fn set_hover(&mut self, def_id: i32, widget: Option<i32>) -> bool {
        let popups = self.popups.entry(def_id).or_default();
        if popups.hover == widget {
            return false;
        }
        popups.hover = widget;
        true
    }

    /// The dialog of window `def_id` whose close button the pointer is
    /// over, written by pointer motion. Answers whether it changed.
    pub(crate) fn set_close_hover(&mut self, def_id: i32, dialog: Option<i32>) -> bool {
        let popups = self.popups.entry(def_id).or_default();
        if popups.close_hover == dialog {
            return false;
        }
        popups.close_hover = dialog;
        true
    }

    /// The section of window `def_id` whose bar the pointer is over,
    /// written by pointer motion. Answers whether it changed.
    pub(crate) fn set_fold_hover(&mut self, def_id: i32, section: Option<i32>) -> bool {
        let popups = self.popups.entry(def_id).or_default();
        if popups.fold_hover == section {
            return false;
        }
        popups.fold_hover = section;
        true
    }

    /// **Whose tip shows at `(x, y)`**: the widget under the point that
    /// carries one, or the nearest ancestor that does -- the rule a context
    /// menu is found by ([`Self::context_at`]).
    pub(crate) fn tip_at(&self, def_id: i32, fb_w: u32, fb_h: u32, x: f64, y: f64) -> Option<Tip> {
        let placed = self.layout_window(def_id, fb_w, fb_h)?;
        let from = super::chrome::modal_start(&placed).unwrap_or(0);
        placed[from..]
            .iter()
            .filter(|p| p.rect.contains(x, y) && p.clip.is_none_or(|c| c.contains(x, y)))
            .rev()
            .find_map(|p| {
                let (id, text) = (p.widget.id?, p.widget.tip.clone()?);
                // **A tool's tip shows its key**: a button that performs a
                // verb is pressed by the chord the key table binds to it, so
                // the tip names that chord as a menu entry does -- read from
                // the table now, and so the one a rebinding left. A widget a
                // key presses that is no tool -- a latch, which reports its
                // state -- names the verb itself (`shortcut`).
                let key = p
                    .widget
                    .kind
                    .as_element()
                    .and_then(|el| el.tool_verb())
                    .or(p.widget.shortcut.as_deref())
                    .and_then(|verb| self.keys.label(verb));
                Some(Tip {
                    widget: id,
                    text,
                    key,
                    anchor: p.clip.map_or(p.rect, |c| c.intersect(p.rect)),
                })
            })
    }

    /// The menu bar's band in window `def_id`'s framebuffer, when it has one.
    pub(crate) fn menu_bar_rect(&self, def_id: i32, fb_w: u32, fb_h: u32) -> Option<Rect> {
        let window = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        // under the window's header, when it has one
        let header = self.header_place(def_id, fb_w, fb_h);
        let window = super::header::content(header.as_ref(), window);
        menubar::bar(self.window_def(def_id)?, window, self.metrics_for(def_id))
    }

    /// **Who answers a context request at `(x, y)`**: the widget under the
    /// point that carries a `context`, or the nearest ancestor that does --
    /// with its id and its menu. `None` where nothing under the point has one.
    ///
    /// The placements are in the order the layout made them, a container
    /// before what it holds, so walked from the end the first one over the
    /// point that carries a menu is the nearest.
    pub(crate) fn context_at(
        &self,
        def_id: i32,
        fb_w: u32,
        fb_h: u32,
        x: f64,
        y: f64,
    ) -> Option<(Owner, Vec<Entry>)> {
        let placed = self.layout_window(def_id, fb_w, fb_h)?;
        let from = super::chrome::modal_start(&placed).unwrap_or(0);
        placed[from..]
            .iter()
            .filter(|p| p.rect.contains(x, y) && p.clip.is_none_or(|c| c.contains(x, y)))
            .filter(|p| p.widget.live)
            .rev()
            .find_map(|p| {
                let id = p.widget.id?;
                if let Some(menu) = p.widget.context.as_ref().filter(|m| !m.is_empty()) {
                    return Some((Owner::Context(id), menu.clone()));
                }
                // **A field's own edit menu**, where it carries no `context`:
                // the innermost answer, so it is the field's and not the page's
                // around it -- the standard menu of every text field.
                let el = p.widget.kind.as_element().filter(|el| el.takes_text())?;
                let has_text = matches!(el.value(), Some(clausters_core::osc::OscType::String(ref s)) if !s.is_empty());
                Some((
                    Owner::Edit(id),
                    menu::edit_entries(el.selected_text().is_some(), has_text),
                ))
            })
    }

    /// Shows `tip` over window `def_id` (or hides the one showing), answering
    /// whether the picture changed.
    pub(crate) fn set_tip(&mut self, def_id: i32, tip: Option<Tip>) -> bool {
        let popups = self.popups.entry(def_id).or_default();
        if popups.tip == tip {
            return false;
        }
        popups.tip = tip;
        true
    }

    /// **The hover card over window `def_id`**, as it stands on screen: the
    /// element whose note it is, the note, and where it is placed, by the
    /// function the frame draws it with.
    pub(crate) fn card_place(
        &self,
        def_id: i32,
        fb_w: u32,
        fb_h: u32,
    ) -> Option<(i32, Note, NotePlace)> {
        let work = self.popup_area(def_id, fb_w, fb_h);
        let m = *self.metrics_for(def_id);
        let placed = self.layout_window(def_id, fb_w, fb_h)?;
        placed.iter().find_map(|p| {
            let id = p.widget.id?;
            let note = p
                .widget
                .kind
                .as_element()?
                .own_popups(p.rect, p.scale, &p.metrics)?
                .note
                .filter(|n| n.card)?;
            let place = place_note(&note, work, &m);
            Some((id, note, place))
        })
    }

    /// The hover card's state over window `def_id`, for `owner`'s note: a
    /// note of another element's starts with none.
    pub(crate) fn card_mut(&mut self, def_id: i32, owner: i32) -> &mut Card {
        let popups = self.popups.entry(def_id).or_default();
        let card = popups.card.get_or_insert_with(Card::default);
        if card.owner != owner {
            *card = Card {
                owner,
                ..Card::default()
            };
        }
        card
    }

    /// The hover card's state goes with its note. Answers whether there was
    /// one.
    pub(crate) fn drop_card(&mut self, def_id: i32) -> bool {
        self.popups
            .get_mut(&def_id)
            .is_some_and(|p| p.card.take().is_some())
    }

    /// The open stack of window `def_id` as it stands on screen: every list
    /// placed, by the function the frame draws them with.
    pub(crate) fn popup_placed(&self, def_id: i32, fb_w: u32, fb_h: u32) -> Option<Vec<Placed>> {
        let stack = self.popup(def_id)?;
        let tree = self.window_def(def_id)?;
        let m = self.metrics_for(def_id);
        let work = self.popup_area(def_id, fb_w, fb_h);
        let window = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let placed = self.layout_window(def_id, fb_w, fb_h)?;
        let owner = owner_rect(stack, tree, &placed, window, m);
        Some(layout(stack, owner, work, m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::menu;
    use serde_json::json;

    fn work() -> Rect {
        Rect::new(0.0, 0.0, 400.0, 300.0)
    }

    fn options(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| Entry::option(&format!("option {i}"), i == 0))
            .collect()
    }

    /// A label longer than the list's most is cut, not followed: the list
    /// stops at the widest a list grows however long the entry.
    #[test]
    fn a_long_label_does_not_widen_the_list_past_its_most() {
        let m = Metrics::default();
        let long = "x".repeat(300);
        let (w, _) = natural(&[Entry::option(&long, false)], 1.0, &m);
        assert_eq!(w, m.menu_max_w);
        let (short, _) = natural(&options(2), 1.0, &m);
        assert!(short < m.menu_max_w, "a short list keeps its own width");
    }

    /// **A menu as the specification lays it out**: its rows inside the
    /// list's inset, each a line and an entry's inset high, a separator the
    /// inset over and under a hairline, a menu at least its least width and
    /// a submenu its own, level with the row that opened it.
    #[test]
    fn a_menu_is_laid_out_as_the_specification_says() {
        let m = Metrics::default();
        let size = list_size(&m);
        let entries = crate::host::menu::parse(&json!([
            "New", "-", {"label": "Recent", "menu": ["one", "two"]}
        ]));
        let mut stack = Stack::new(
            Owner::Bar(crate::host::menubar::Title::Entry(0)),
            entries,
            Anchor::Below(Rect::new(10.0, 0.0, 40.0, 30.0)),
            size,
        );
        let placed = layout(&stack, None, work(), &m);
        let list = &placed[0];
        assert!(list.rect.w >= m.menu_w, "{}", list.rect.w);
        let rows = &list.rows;
        assert_eq!(rows[0].x, list.rect.x + m.menu_inset);
        assert_eq!(rows[0].y, list.rect.y + m.menu_inset);
        assert_eq!(rows[0].h, m.line_sm + 2.0 * m.menu_item_y);
        assert_eq!(rows[1].h, 2.0 * m.menu_inset + m.divider_w);
        assert_eq!(
            list.rect.h,
            rows.iter().map(|r| r.h).sum::<f32>() + 2.0 * m.menu_inset
        );
        assert!(stack.open_child(0, 2));
        let placed = layout(&stack, None, work(), &m);
        let sub = &placed[1];
        assert!(sub.rect.w >= m.submenu_w);
        assert_eq!(
            sub.rows[0].y, placed[0].rows[2].y,
            "its first row level with the opener"
        );
    }

    /// **A letter finds an entry**: the first whose label starts with it,
    /// the same letter again the next one, a word typed within a second
    /// the entry it spells, and a pause starts over.
    #[test]
    fn typing_finds_an_entry_by_its_name() {
        let entries = crate::host::menu::parse(&json!(["Save", "Save As", "Select All", "Close"]));
        let mut stack = Stack::new(Owner::Context(1), entries, Anchor::At(0.0, 0.0), 1.0);
        let hover = |s: &Stack| s.levels[0].hover;
        assert!(stack.type_ahead('s', 0.0));
        assert_eq!(hover(&stack), Some(0));
        assert!(stack.type_ahead('s', 100.0));
        assert_eq!(hover(&stack), Some(1), "the same letter steps on");
        assert!(stack.type_ahead('S', 2000.0));
        assert_eq!(hover(&stack), Some(2));
        assert!(stack.type_ahead('e', 2100.0) && stack.type_ahead('l', 2200.0));
        assert_eq!(hover(&stack), Some(2), "a word stays on the row it found");
        assert!(stack.type_ahead('c', 5000.0));
        assert_eq!(hover(&stack), Some(3), "a pause starts over");
        assert!(!stack.type_ahead('z', 6000.0));
        assert_eq!(hover(&stack), Some(3));
    }

    /// **On its way to a submenu, the pointer crosses rows without taking
    /// them**: a move from the opener toward the open submenu is aiming at
    /// it, a move away is not, and the opener itself never is.
    #[test]
    fn a_pointer_on_its_way_to_a_submenu_is_aiming() {
        let m = Metrics::default();
        let entries = crate::host::menu::parse(&json!([
            {"label": "Recent", "menu": ["one", "two", "three"]}, "Save", "Close"
        ]));
        let mut stack = Stack::new(Owner::Context(1), entries, Anchor::At(10.0, 10.0), 1.0);
        stack.open_child(0, 0);
        let placed = layout(&stack, None, work(), &m);
        let (opener, below) = (placed[0].rows[0], placed[0].rows[1]);
        let from = (
            f64::from(opener.x + opener.w - 4.0),
            f64::from(opener.y + opener.h * 0.5),
        );
        let toward = (from.0 + 3.0, f64::from(below.y + 2.0));
        let away = (from.0 - 30.0, f64::from(below.y + below.h * 0.5));
        assert!(aiming(&stack, &placed, 0, Some(1), Some(from), toward));
        assert!(!aiming(&stack, &placed, 0, Some(1), Some(from), away));
        assert!(!aiming(&stack, &placed, 0, Some(0), Some(from), toward));
        assert!(!aiming(&stack, &placed, 0, Some(1), None, toward));
    }

    #[test]
    fn a_list_hangs_below_its_anchor_when_it_fits() {
        let r = place_below(Rect::new(10.0, 20.0, 120.0, 24.0), (120.0, 96.0), work());
        assert_eq!(r, Rect::new(10.0, 44.0, 120.0, 96.0));
    }

    #[test]
    fn a_list_with_no_room_below_opens_above() {
        let r = place_below(Rect::new(10.0, 260.0, 120.0, 24.0), (120.0, 96.0), work());
        assert_eq!(r.y + r.h, 260.0, "its foot is on the field's head");
    }

    /// The work area is the window **minus the host's bands**, so a list never
    /// runs under the status bar -- which is what it did when it was placed
    /// against the framebuffer.
    #[test]
    fn a_list_stays_clear_of_what_the_work_area_leaves_out() {
        let above_the_bar = Rect::new(0.0, 0.0, 400.0, 280.0);
        let r = place_below(
            Rect::new(10.0, 200.0, 120.0, 24.0),
            (120.0, 96.0),
            above_the_bar,
        );
        assert!(r.y + r.h <= 280.0, "inside the area: {r:?}");
    }

    #[test]
    fn a_list_taller_than_either_side_takes_the_roomier_one_and_scrolls() {
        let r = place_below(Rect::new(10.0, 100.0, 120.0, 24.0), (120.0, 900.0), work());
        assert_eq!(r.y, 124.0, "below has more room");
        assert_eq!(r.y + r.h, 300.0, "and it stops at the area's edge");
    }

    #[test]
    fn a_list_is_held_inside_the_right_edge() {
        let r = place_below(Rect::new(350.0, 20.0, 40.0, 24.0), (160.0, 48.0), work());
        assert_eq!(r.x + r.w, 400.0);
    }

    #[test]
    fn a_submenu_opens_to_the_left_when_the_right_has_no_room() {
        let row = Rect::new(300.0, 50.0, 90.0, 24.0);
        let r = place_beside(row, (120.0, 72.0), work());
        assert_eq!(r.x + r.w, 300.0);
        let roomy = place_beside(Rect::new(10.0, 50.0, 90.0, 24.0), (120.0, 72.0), work());
        assert_eq!(roomy.x, 100.0);
    }

    #[test]
    fn a_context_menu_turns_away_from_the_corner() {
        let r = place_at((390.0, 290.0), (100.0, 60.0), work());
        assert_eq!((r.x + r.w, r.y + r.h), (390.0, 290.0));
    }

    #[test]
    fn a_list_is_as_wide_as_its_widest_entry_and_no_narrower_than_its_field() {
        let m = Metrics::default();
        let entries = vec![
            Entry::option("a", true),
            Entry::option("a much longer option", false),
        ];
        let mut stack = Stack::new(
            Owner::Element(1),
            entries.clone(),
            Anchor::Below(Rect::new(10.0, 10.0, 40.0, 24.0)),
            2.0,
        );
        let narrow = layout(&stack, None, work(), &m);
        assert!(
            narrow[0].rect.w >= font::width("a much longer option", 2.0),
            "wider than the field it hangs off"
        );
        stack.min_w = 380.0;
        assert_eq!(layout(&stack, None, work(), &m)[0].rect.w, 380.0);
    }

    #[test]
    fn a_list_follows_the_widget_it_hangs_off() {
        let m = Metrics::default();
        let mut stack = Stack::new(
            Owner::Element(1),
            options(2),
            Anchor::Below(Rect::new(10.0, 10.0, 120.0, 24.0)),
            2.0,
        );
        stack.origin = Some((10.0, 10.0));
        let moved = layout(&stack, Some(Rect::new(60.0, 40.0, 120.0, 24.0)), work(), &m);
        assert_eq!((moved[0].rect.x, moved[0].rect.y), (60.0, 64.0));
    }

    #[test]
    fn a_long_list_scrolls_and_a_key_keeps_its_row_on_screen() {
        let m = Metrics::default();
        let mut stack = Stack::new(
            Owner::Element(1),
            options(40),
            Anchor::Below(Rect::new(10.0, 10.0, 120.0, 24.0)),
            2.0,
        );
        let placed = layout(&stack, None, work(), &m);
        assert!(placed[0].max_scroll > 0.0);
        assert!(placed[0].rect.y + placed[0].rect.h <= 300.0);
        // End walks to the last row, and the list scrolls to show it.
        assert_eq!(stack.key(Walk::End), Keyed::Moved);
        assert_eq!(stack.levels[0].hover, Some(39));
        let placed = layout(&stack, None, work(), &m);
        let last = placed[0].rows[39];
        assert!(
            last.y >= placed[0].rect.y && last.y + last.h <= placed[0].rect.y + placed[0].rect.h,
            "the hovered row is inside the list: {last:?} in {:?}",
            placed[0].rect
        );
    }

    #[test]
    fn a_row_is_hit_where_it_is_drawn_and_a_separator_is_not() {
        let m = Metrics::default();
        let entries = menu::parse(&json!(["Open", "-", "Quit"]));
        let stack = Stack::new(Owner::Context(1), entries, Anchor::At(20.0, 20.0), 2.0);
        let placed = layout(&stack, None, work(), &m);
        let mid = |r: Rect| ((r.x + r.w * 0.5) as f64, (r.y + r.h * 0.5) as f64);
        let (x, y) = mid(placed[0].rows[2]);
        assert_eq!(hit(&stack, &placed, x, y), Some((0, Some(2))));
        let (x, y) = mid(placed[0].rows[1]);
        assert_eq!(hit(&stack, &placed, x, y), Some((0, None)));
        assert_eq!(hit(&stack, &placed, 390.0, 290.0), None);
    }

    fn tree() -> Vec<Entry> {
        menu::parse(&json!([
            {"label": "New", "verb": "new"},
            {"label": "Recent", "menu": ["a.wav", "b.wav"]},
            "-",
            {"label": "Export", "enabled": false},
            {"label": "Quit", "verb": "quit"},
        ]))
    }

    #[test]
    fn the_keys_walk_skip_what_cannot_be_picked_and_open_a_submenu() {
        let mut stack = Stack::new(Owner::Context(1), tree(), Anchor::At(0.0, 0.0), 2.0);
        assert_eq!(stack.key(Walk::Down), Keyed::Moved);
        assert_eq!(stack.levels[0].hover, Some(0));
        stack.key(Walk::Down);
        assert_eq!(stack.levels[0].hover, Some(1));
        // Down again skips the separator and the disabled entry.
        stack.key(Walk::Down);
        assert_eq!(stack.levels[0].hover, Some(4));
        stack.key(Walk::Up);
        assert_eq!(stack.levels[0].hover, Some(1));
        // Right opens the submenu and lands on its first row.
        assert_eq!(stack.key(Walk::Right), Keyed::Moved);
        assert_eq!(stack.levels.len(), 2);
        assert_eq!(stack.levels[1].hover, Some(0));
        stack.key(Walk::Down);
        assert_eq!(stack.key(Walk::Enter), Keyed::Pick(vec![1, 1]));
        // Left closes the submenu; Escape then closes the stack.
        assert_eq!(stack.key(Walk::Left), Keyed::Moved);
        assert_eq!(stack.levels.len(), 1);
        assert_eq!(stack.key(Walk::Escape), Keyed::Closed);
    }

    #[test]
    fn pointing_at_a_submenu_row_opens_it_and_at_a_plain_one_closes_it() {
        let mut stack = Stack::new(Owner::Context(1), tree(), Anchor::At(0.0, 0.0), 2.0);
        assert!(stack.point(0, Some(1)));
        assert_eq!(stack.levels.len(), 2);
        assert!(
            !stack.point(0, Some(1)),
            "already there: nothing to repaint"
        );
        assert!(stack.point(0, Some(0)));
        assert_eq!(stack.levels.len(), 1);
    }

    #[test]
    fn left_and_right_at_the_first_list_of_a_bar_step_to_the_next_title() {
        let mut stack = Stack::new(
            Owner::Bar(Title::Entry(0)),
            tree(),
            Anchor::At(0.0, 0.0),
            2.0,
        );
        assert_eq!(stack.key(Walk::Right), Keyed::Step(1));
        assert_eq!(stack.key(Walk::Left), Keyed::Step(-1));
    }

    /// **A tooltip stands over what it is the tip of**, centered, its arrow
    /// reaching toward it; under it where there is no room over it; its key
    /// a cap at its end.
    #[test]
    fn a_tooltip_stands_over_its_tool_and_turns_under_it_at_the_top() {
        let m = Metrics::default();
        let size = tip_size(&m);
        let mut tip = Tip {
            widget: 1,
            text: "Evaluate".into(),
            key: None,
            anchor: Rect::new(180.0, 200.0, 32.0, 32.0),
        };
        let p = place_tip(&tip, size, work(), &m);
        let mid = 196.0;
        assert!(
            (p.rect.x + p.rect.w * 0.5 - mid).abs() < 0.5,
            "centered: {p:?}"
        );
        assert_eq!(p.rect.y + p.rect.h + m.tip_offset, 200.0, "over it");
        assert!(p.above);
        assert_eq!(p.arrow, (mid, 200.0 - m.tip_offset + m.tip_arrow));
        assert_eq!(p.rect.h, m.line_xs + 2.0 * m.tip_y);
        // at the top of the area: under it
        tip.anchor.y = 4.0;
        let p = place_tip(&tip, size, work(), &m);
        assert!(!p.above);
        assert_eq!(p.rect.y, 4.0 + 32.0 + m.tip_offset);
        // a key: a cap at the end, the inset after it the narrower one
        tip.key = Some("Ctrl+Enter".into());
        let with_key = place_tip(&tip, size, work(), &m);
        let k = with_key.key.expect("a key cap");
        assert_eq!(k.h, m.kbd_h);
        assert_eq!(k.x + k.w + m.tip_end, with_key.rect.x + with_key.rect.w);
        assert_eq!(with_key.rect.h, m.kbd_h + 2.0 * m.tip_y);
        assert!(with_key.rect.w > p.rect.w);
    }

    fn card(doc: &str) -> Note {
        Note {
            anchor: Rect::new(40.0, 200.0, 30.0, 20.0),
            head: vec![
                ("print(".into(), false),
                ("value".into(), true),
                (")".into(), false),
            ],
            aside: "function".into(),
            doc: doc.into(),
            doc_lines: 0,
            code_size: 2.0,
            text_size: 2.0,
            card: true,
        }
    }

    /// **A head printed over several lines is one flowing line, wrapped to
    /// the note**, a card's and a call's alike: no line break or indentation
    /// reaches a line drawn, every line fits the note, and the marked
    /// parameter stays marked where it lands.
    #[test]
    fn a_head_over_several_lines_flows_and_wraps_inside_the_note() {
        let m = Metrics::default();
        let area = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let mut note = card("Short.");
        note.head = vec![
            ("engine(".into(), false),
            ("timeout?: number".into(), true),
            (
                "): Promise<{\n    shm: string;\n    share: [number, number];\n}>".into(),
                false,
            ),
        ];
        assert_eq!(
            note.head_text(),
            "engine(timeout?: number): Promise<{ shm: string; share: [number, number]; }>"
        );
        for call in [true, false] {
            note.card = call;
            let p = place_note(&note, area, &m);
            let inner = p.rect.w - 2.0 * m.card_x;
            for l in &p.heads {
                assert!(!l.text.contains(char::is_control), "{:?}", l.text);
                assert!(l.x_of(l.text.len()) <= inner + 0.5, "{:?} fits", l.text);
            }
            let marked: String = p
                .heads
                .iter()
                .flat_map(|l| {
                    l.runs
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.family == font::Family::MonoBold)
                        .map(|(k, r)| {
                            l.text[r.from..l.runs.get(k + 1).map_or(l.text.len(), |n| n.from)]
                                .to_string()
                        })
                })
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(marked, "timeout?: number");
        }
        // a narrow note wraps it onto lines of its own
        let narrow = Rect::new(0.0, 0.0, 40.0 * font::width("0", 2.0), 800.0);
        note.card = true;
        let p = place_note(&note, narrow, &m);
        assert!(p.heads.len() > 1, "it wraps");
        assert!(p.rect.w <= narrow.w);
    }

    /// **A code block wider than the card scrolls across on its own**, as on
    /// the documentation's page: the card stays its width, the block is
    /// found under the pointer, and a line in it is read where it is drawn.
    #[test]
    fn a_code_block_wider_than_the_card_scrolls_across() {
        let m = Metrics::default();
        let area = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let wide = format!("Before.\n\n```\n{}\n```\n", "x".repeat(400));
        let note = card(&wide);
        let p = place_note(&note, area, &m);
        assert!(p.rect.w <= m.card_w);
        let laid = p.laid.as_ref().expect("a card lays Markdown out");
        assert_eq!(laid.pans.len(), 1, "the block scrolls across");
        let block = laid.pans[0];
        let (x, y) = (p.doc.x + block.rect.x + 1.0, p.doc.y + block.rect.y + 1.0);
        let (i, most) = p.pan_at(0.0, x, y).expect("the block under the pointer");
        assert_eq!(i, 0);
        assert!(most > 0.0);
        assert!(
            p.pan_at(0.0, p.doc.x + 1.0, p.doc.y + 1.0).is_none(),
            "the paragraph does not"
        );
        // a line of the block, scrolled across by ten, is read ten further on
        let row = (0..p.count())
            .find(|&r| {
                p.line(r)
                    .is_some_and(|(l, head)| !head && l.text.starts_with("xxx"))
            })
            .expect("the block's line");
        let (top, h) = p.line_top(row, 0.0);
        let at = |pans: &[f32]| note.pos_at(&p, 0.0, pans, x, top + h * 0.5).1;
        assert!(at(&[10.0 * font::width_in(font::Family::Mono, "x", 2.0)]) > at(&[]));
        assert_eq!(p.line_pan(row, &[1.0e9]), most, "held to its most");
    }

    /// **A hover card's documentation is all there, and scrolls past its
    /// most**: no line is dropped, the window it shows through stops at
    /// `card_doc_h`, and the card is held inside the work area.
    #[test]
    fn a_hover_card_keeps_every_line_and_scrolls_past_its_height() {
        let m = Metrics::default();
        let long: String = (0..60)
            .map(|i| format!("Line {i} of the help.\n\n"))
            .collect();
        let note = card(&long);
        let area = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let p = place_note(&note, area, &m);
        assert_eq!(p.lines.len(), 60, "every paragraph is kept");
        assert_eq!(p.doc.h, m.card_doc_h);
        assert!(p.max_scroll() > 0.0);
        assert!(p.rect.w <= m.card_w);
        assert_eq!(p.head.x, p.rect.x + m.card_x);
        assert_eq!(p.head.y, p.rect.y + m.card_y);
        assert_eq!(p.doc.y, p.head.y + p.head.h + m.card_gap);
        // a long signature wraps inside the card, and copies whole
        let mut long_head = card("Short.");
        long_head.head = vec![("f(".to_string() + &"argument, ".repeat(40) + ")", false)];
        let p = place_note(&long_head, area, &m);
        assert!(p.heads.len() > 1, "the head wraps");
        assert!(p.rect.w <= m.card_w);
        let whole = long_head.text_between(&p, (0, 0), (p.heads.len() - 1, usize::MAX));
        assert_eq!(whole, long_head.head_text().trim_end());
        // a short one does not scroll
        let p = place_note(&card("Short."), area, &m);
        assert_eq!(p.max_scroll(), 0.0);
        // no wider than nine tenths of a narrow area
        let narrow = Rect::new(0.0, 0.0, 300.0, 800.0);
        let wide: String = "word ".repeat(200);
        assert!(place_note(&card(&wide), narrow, &m).rect.w <= 270.0 + 0.01);
    }

    /// **What is selected in a card is copied as its text**: the head on a
    /// line of its own, and the lines the documentation was wrapped into
    /// joined back with a space.
    #[test]
    fn a_card_s_selection_is_its_text_unwrapped() {
        let m = Metrics::default();
        let words: String = "alpha beta gamma delta ".repeat(20);
        let note = card(&format!("{}\n\nNext.", words.trim()));
        let area = Rect::new(0.0, 0.0, 400.0, 800.0);
        let p = place_note(&note, area, &m);
        assert!(p.lines.len() > 2, "it wraps: {:?}", p.lines);
        let last = p.count() - 1;
        let all = note.text_between(&p, (0, 0), (last, p.lines[p.lines.len() - 1].text.len()));
        assert_eq!(all, format!("print(value)\n{}\nNext.", words.trim()));
        // backwards is the same
        let back = note.text_between(&p, (last, 5), (0, 6));
        assert_eq!(back, note.text_between(&p, (0, 6), (last, 5)));
        assert!(back.starts_with("value)\n"));
        // a point finds its place: the head's first character, a line's end
        let head = note.pos_at(&p, 0.0, &[], p.head.x + 0.1, p.head.y + 1.0);
        assert_eq!(head, (0, 0));
        let end = note.pos_at(&p, 0.0, &[], p.rect.x + p.rect.w + 50.0, p.doc.y + 1.0);
        assert_eq!(end, (1, p.lines[0].text.len()));
        // the scroll moves which line a point is on
        let scrolled = note.pos_at(&p, p.lines[1].y, &[], p.head.x, p.doc.y + 1.0);
        assert_eq!(scrolled.0, 2);
    }

    /// **A card's documentation is Markdown**: a strong word in the bold
    /// face, code in the monospaced one, a heading and a code block laid out
    /// as the documentation panel lays them out; and a selection over it
    /// still copies its text, from run to run.
    #[test]
    fn a_card_draws_its_documentation_as_markdown() {
        let m = Metrics::default();
        let note = card("Returns the **square root** of `x`.\n\n    sqrt(2)\n");
        let p = place_note(&note, Rect::new(0.0, 0.0, 800.0, 600.0), &m);
        assert!(p.laid.is_some(), "laid out as a page");
        let first = &p.lines[0];
        let families: Vec<font::Family> = first.runs.iter().map(|r| r.family).collect();
        assert!(families.len() >= 3, "{:?}", first.runs);
        assert!(families.contains(&font::Family::Mono), "code: {families:?}");
        assert!(
            families
                .iter()
                .any(|f| *f != font::Family::Mono && *f != families[0]),
            "a strong word in another face: {families:?}"
        );
        assert_eq!(first.text, "Returns the square root of x.");
        // the code block is a row of its own, in the monospaced face
        let block = p
            .lines
            .iter()
            .find(|l| l.text.contains("sqrt(2)"))
            .expect("the block");
        assert_eq!(block.runs[0].family, font::Family::Mono);
        // a selection across the runs copies the words
        let at = |i: usize| first.text.find("square").map(|b| b + i).unwrap_or(0);
        let copied = note.text_between(&p, (1, at(0)), (1, at(11)));
        assert_eq!(copied, "square root");
        // a point over a run lands in it
        let x = p.head.x + first.x_of(at(2));
        let pos = note.pos_at(&p, 0.0, &[], x + 0.1, p.doc.y + first.y + 1.0);
        assert_eq!(pos, (1, at(2)));
    }
}
