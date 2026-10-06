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

use super::font;
use super::layout::Rect;
use super::menu::{self, Entry};
use super::menubar::{self, Title};
use super::metrics::Metrics;
use super::widget::Widget;

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
    pub text_size: f32,
    pub levels: Vec<Level>,
}

impl Stack {
    pub fn new(owner: Owner, entries: Vec<Entry>, anchor: Anchor, text_size: f32) -> Self {
        Self {
            owner,
            anchor,
            origin: None,
            min_w: 0.0,
            text_size,
            levels: vec![Level::new(Vec::new(), entries)],
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
            Owner::Bar(_) | Owner::Keys => None,
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

/// A tip: a short text beside the pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct Tip {
    /// The widget whose tip this is.
    pub widget: i32,
    pub text: String,
    /// Where the pointer rested.
    pub at: (f32, f32),
}

/// What is open over one window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Popups {
    pub stack: Option<Stack>,
    pub tip: Option<Tip>,
    /// The title of the menu bar the pointer is over, with no list open.
    pub bar_hover: Option<Title>,
    /// The widget the pointer is over, when it is one a hand can act on -- what
    /// a control draws its hover from.
    pub hover: Option<i32>,
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
        Owner::Keys => None,
    }
}

/// What the key sheet's title strip says.
pub const KEYS_TITLE: &str = "Keys";

/// The height of one row of a list.
pub fn row_h(text_size: f32, m: &Metrics) -> f32 {
    (font::height(text_size) + 2.0 * m.pad).max(m.control_h)
}

/// The height a separator takes: a hairline with a pad over and under it.
pub fn separator_h(m: &Metrics) -> f32 {
    2.0 * m.pad + m.divider_w
}

/// The gutter a list reserves at its left for a mark or an icon, and at its
/// right for a submenu's arrow: one glyph and a pad.
pub fn gutter(text_size: f32, m: &Metrics) -> f32 {
    font::height(text_size) + m.pad
}

fn entry_h(e: &Entry, text_size: f32, m: &Metrics) -> f32 {
    if e.is_separator() {
        separator_h(m)
    } else {
        row_h(text_size, m)
    }
}

/// How big a list wants to be: as wide as its widest entry with the gutters it
/// needs, and as tall as its rows.
///
/// The entries are props, so this is a size read out of props -- nothing a
/// stream of values can move.
pub fn natural(entries: &[Entry], text_size: f32, m: &Metrics) -> (f32, f32) {
    let left = if entries.iter().any(|e| e.icon.is_some() || e.can_mark()) {
        gutter(text_size, m)
    } else {
        0.0
    };
    let right = if entries.iter().any(|e| e.submenu().is_some()) {
        gutter(text_size, m)
    } else {
        0.0
    };
    let label = entries
        .iter()
        .map(|e| font::width(&e.label, text_size))
        .fold(0.0f32, f32::max);
    let h = entries.iter().map(|e| entry_h(e, text_size, m)).sum();
    (
        left + label + key_column(entries, text_size, m) + right + 2.0 * m.pad,
        h,
    )
}

/// The column a list keeps at the right of its labels for the chords its
/// entries are bound to: the widest chord and a gap before it, or nothing when
/// no entry has one.
pub fn key_column(entries: &[Entry], text_size: f32, m: &Metrics) -> f32 {
    entries
        .iter()
        .filter_map(|e| e.key.as_deref())
        .map(|k| font::width(k, text_size) + 4.0 * m.pad)
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
        let want = (if i == 0 { w.max(stack.min_w) } else { w }, h + strip_h);
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
                let row = Rect::new(parent.rect.x, row.y, parent.rect.w, row.h);
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
            let top: f32 = level.entries[..row]
                .iter()
                .map(|e| entry_h(e, size, m))
                .sum();
            let bottom = top + entry_h(&level.entries[row], size, m);
            if top < scroll {
                scroll = top;
            } else if bottom > scroll + seen {
                scroll = bottom - seen;
            }
            scroll = scroll.clamp(0.0, max_scroll);
        }
        let mut y = rect.y + strip_h - scroll;
        let rows = level
            .entries
            .iter()
            .map(|e| {
                let eh = entry_h(e, size, m);
                let r = Rect::new(rect.x, y, rect.w, eh);
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

/// **Where a tip goes**: under the pointer and a little to its right, turned
/// to the other side where it would leave the work area.
pub fn place_tip(tip: &Tip, text_size: f32, work: Rect, m: &Metrics) -> Rect {
    let w = font::width(&tip.text, text_size) + 2.0 * m.pad;
    let h = font::height(text_size) + font::descent(text_size) + 2.0 * m.pad;
    // Clear of the pointer itself, which a tip under it would hide behind:
    // one line below it, or -- where the area ends first -- just over it.
    let offset = m.pad + font::height(text_size);
    let y = if tip.at.1 + offset + h <= work.y + work.h {
        tip.at.1 + offset
    } else {
        tip.at.1 - m.pad - h
    };
    Rect::new(
        hold_x(tip.at.0 + m.pad, w.min(work.w), work),
        hold_y(y, h.min(work.h), work),
        w.min(work.w),
        h.min(work.h),
    )
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

    /// **Whose tip shows at `(x, y)`**: the widget under the point that
    /// carries one, or the nearest ancestor that does -- the rule a context
    /// menu is found by ([`Self::context_at`]).
    pub(crate) fn tip_at(
        &self,
        def_id: i32,
        fb_w: u32,
        fb_h: u32,
        x: f64,
        y: f64,
    ) -> Option<(i32, String)> {
        let placed = self.layout_window(def_id, fb_w, fb_h)?;
        let from = super::chrome::modal_start(&placed).unwrap_or(0);
        placed[from..]
            .iter()
            .filter(|p| p.rect.contains(x, y) && p.clip.is_none_or(|c| c.contains(x, y)))
            .rev()
            .find_map(|p| Some((p.widget.id?, p.widget.tip.clone()?)))
    }

    /// The menu bar's band in window `def_id`'s framebuffer, when it has one.
    pub(crate) fn menu_bar_rect(&self, def_id: i32, fb_w: u32, fb_h: u32) -> Option<Rect> {
        let window = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
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

    /// The open stack of window `def_id` as it stands on screen: every list
    /// placed, by the function the frame draws them with.
    pub(crate) fn popup_placed(&self, def_id: i32, fb_w: u32, fb_h: u32) -> Option<Vec<Placed>> {
        let stack = self.popup(def_id)?;
        let tree = self.window_def(def_id)?;
        let m = self.metrics_for(def_id);
        let work = self.content_area(def_id, fb_w, fb_h);
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

    #[test]
    fn a_tip_turns_over_the_pointer_where_there_is_no_room_under_it() {
        let m = Metrics::default();
        let tip = Tip {
            widget: 1,
            text: "what it is".into(),
            at: (100.0, 295.0),
        };
        let r = place_tip(&tip, 2.0, work(), &m);
        assert!(r.y + r.h <= 295.0, "over the pointer: {r:?}");
        assert!(!r.contains(100.0, 295.0));
    }
}
