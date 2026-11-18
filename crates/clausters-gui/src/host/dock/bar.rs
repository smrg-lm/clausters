//! **A panel's bar, and the dock's menus.**
//!
//! Every group of the dock is a **panel** with a bar along its top: a grip to
//! drag the panel by, what it shows, a button that opens its menu and -- for a
//! panel showing no tabs -- a close mark. A panel of one window shows that
//! window's title, as a tool's panel does; tabs appear when the panel holds
//! several, each with its own close mark, or when its one window is a
//! **document** (`dock_tab`), as an editor's group of files shows a single
//! file as its tab. So a tab means a document, or a choice between the
//! windows of one panel.
//!
//! A panel's **menu** (its button, or the secondary button on its bar or on a
//! tab) and a page's (the secondary button on its tab) are lists of the popup
//! layer ([`popup::Stack`]), placed by its one [`popup::layout`] and drawn by
//! the frame's own list painter, over the dock's surface rather than over a
//! window's. A pick is a move the dock makes, or -- closing a window, taking
//! it out, putting it back -- one it hands its front ([`super::Dirty`]).
//!
//! A window taken out of the dock keeps its bar, as a **floating panel**
//! ([`Dock::floating`]): its title, its menu and its close mark, with no
//! grip, and a menu whose first entry puts it back into the dock.

use clausters_gui_draw::font;
use clausters_gui_draw::rect::Rect;

use super::{Dock, Grip, GroupPlace, Landing, Side};
use crate::host::menu::{Entry, EntryKind};
use crate::host::popup::{self, Anchor, Owner, Stack};

/// One thing a hand can land on in a panel's bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarPart {
    /// The handle the panel is dragged by.
    Grip,
    /// What a panel showing no tabs shows: its window's title. Dragged as
    /// the grip is.
    Title,
    /// The tab of the panel's window at this index.
    Tab(usize),
    /// That tab's close mark.
    TabClose(usize),
    /// The button that opens the panel's menu.
    Menu,
    /// The close mark of a panel showing no tabs.
    Close,
}

/// Where a menu opens: under a button, or at the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuAt {
    Below(Rect),
    Point(f32, f32),
}

/// What a menu is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A pane: its panel's menu, about that pane.
    Pane(i32),
    /// The page at this index.
    Page(usize),
}

/// The menu open over the dock.
#[derive(Clone, Debug, PartialEq)]
pub struct DockMenu {
    pub stack: Stack,
    pub target: Target,
}

impl Dock {
    /// **The parts of a panel's bar**, each with the rectangle it is drawn
    /// in: the grip, the title or the tabs (each tab's close mark before the
    /// tab, so it is hit first), the menu button and, with no tabs, the
    /// close mark. A floating panel shows its title. Tabs that do not fit are cut to an ellipsis down to a
    /// least, and those past the bar's end are left out -- the menu lists
    /// them all.
    pub fn bar_parts(&self, group: &GroupPlace) -> Vec<(BarPart, Rect)> {
        let r = group.strip;
        let m = self.metrics();
        let h = r.h;
        let size = self.text_size();
        // a floating panel has nowhere to be dragged to, so no grip
        let grip_w = if self.floating { 0.0 } else { m.panel_grip };
        let mut out = Vec::new();
        if !self.floating {
            out.push((BarPart::Grip, Rect::new(r.x, r.y, grip_w, h)));
        }
        // a floating panel is a window of its own: its title, no tabs
        let single = self.floating || !self.shows_tabs(&group.panes);
        // a pane that fills the header has the middle: no title over it
        let headed = group
            .panes
            .get(group.active)
            .is_some_and(|&p| self.is_headed(p));
        let buttons = if single { 2.0 } else { 1.0 };
        let end = (r.x + r.w - buttons * h).max(r.x + grip_w);
        if single {
            if !headed {
                out.push((
                    BarPart::Title,
                    Rect::new(r.x + grip_w, r.y, end - r.x - grip_w, h),
                ));
            }
            let shown = group.panes.get(group.active).copied();
            if shown.is_some_and(|p| self.closable(p)) {
                out.push((BarPart::Menu, Rect::new(end, r.y, h, h)));
                out.push((BarPart::Close, Rect::new(end + h, r.y, h, h)));
            } else {
                // no close mark: the menu button at the bar's end
                out.push((BarPart::Menu, Rect::new(end + h, r.y, h, h)));
            }
            return out;
        }
        // a tab: its inset, its name, a gap, its close mark and a gap -- a
        // long name is cut, not followed, at the widest a tab grows
        let close = m.tab_close;
        let around = m.tab_inset + 2.0 * m.tab_end + close;
        let natural: Vec<f32> = group
            .panes
            .iter()
            .map(|&p| (font::width(&self.title(p), size) + around).min(m.tab_w))
            .collect();
        let room = (end - r.x - grip_w).max(0.0);
        // a pane filling the header keeps half the bar for its tools
        let room = if headed { room * 0.5 } else { room };
        let least = around + font::width("\u{2026}", size);
        let total: f32 = natural.iter().sum();
        let each = (room / natural.len() as f32).max(least);
        let mut x = r.x + grip_w;
        for (i, w) in natural.iter().enumerate() {
            let w = if total <= room { *w } else { w.min(each) };
            if x + w > r.x + grip_w + room + 0.5 {
                break;
            }
            let tab = Rect::new(x, r.y, w, h);
            if self.closable(group.panes[i]) {
                out.push((
                    BarPart::TabClose(i),
                    Rect::new(
                        x + w - close - m.tab_end,
                        r.y + (h - close) * 0.5,
                        close,
                        close,
                    ),
                ));
            }
            out.push((BarPart::Tab(i), tab));
            x += w;
        }
        out.push((BarPart::Menu, Rect::new(end, r.y, h, h)));
        out
    }

    /// Where pane `pane`'s tab is, when its panel shows tabs.
    pub(super) fn tab_rect(&self, pane: i32) -> Option<Rect> {
        let group = self.groups.iter().find(|g| g.panes.contains(&pane))?;
        let i = group.panes.iter().position(|&p| p == pane)?;
        self.bar_parts(group)
            .into_iter()
            .find(|(p, _)| *p == BarPart::Tab(i))
            .map(|(_, r)| r)
    }

    /// **The middle of `group`'s bar**, from where its start's parts end (the
    /// grip, the tabs) to where its end's begin (the buttons), as x: what a
    /// pane filling the header draws its tools in.
    pub(super) fn slot(&self, group: &GroupPlace) -> (f32, f32) {
        let parts = self.bar_parts(group);
        let to = parts
            .iter()
            .filter(|(p, _)| matches!(p, BarPart::Menu | BarPart::Close))
            .map(|(_, r)| r.x)
            .fold(group.strip.x + group.strip.w, f32::min);
        let from = parts
            .iter()
            .filter(|(p, _)| matches!(p, BarPart::Grip | BarPart::Tab(_)))
            .map(|(_, r)| r.x + r.w)
            .fold(group.strip.x, f32::max);
        (from, to.max(from))
    }

    /// The panel whose bar holds `(x, y)`, and the part of it there.
    pub(super) fn bar_at(&self, x: f64, y: f64) -> Option<(usize, BarPart)> {
        let at = self.groups.iter().position(|g| g.strip.contains(x, y))?;
        self.bar_parts(&self.groups[at])
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(part, _)| (at, part))
    }

    /// **A press on a panel's bar**: the grip or the title takes the panel
    /// in hand, a tab shows its window and takes it in hand, a close mark
    /// closes its window, the menu button opens the panel's menu.
    pub(super) fn press_bar(&mut self, group: usize, part: BarPart, at: (f64, f64)) {
        let g = &self.groups[group];
        let shown = g.panes[g.active];
        match part {
            BarPart::Grip | BarPart::Title if self.floating => self.focus = Some(shown),
            BarPart::Grip | BarPart::Title => {
                self.focus = Some(shown);
                self.grip = Some(Grip::Tab {
                    pane: shown,
                    group: true,
                    from: at,
                    dragging: false,
                    landing: None,
                });
            }
            BarPart::Tab(i) => {
                let pane = g.panes[i];
                if self.tree.select(pane) {
                    self.arrange();
                }
                self.focus = Some(pane);
                self.grip = Some(Grip::Tab {
                    pane,
                    group: false,
                    from: at,
                    dragging: false,
                    landing: None,
                });
            }
            BarPart::TabClose(i) => self.dirty.close.push(g.panes[i]),
            BarPart::Close => self.dirty.close.push(shown),
            BarPart::Menu => {
                let button = self
                    .bar_parts(g)
                    .into_iter()
                    .find(|(p, _)| *p == BarPart::Menu)
                    .map(|(_, r)| r)
                    .unwrap_or(g.strip);
                self.open_pane_menu(shown, MenuAt::Below(button));
            }
        }
    }

    /// The menu open over the dock, if one is.
    pub fn menu(&self) -> Option<&DockMenu> {
        self.menu.as_ref()
    }

    /// **The menu's lists, placed** over the whole surface: what draws them
    /// and what a press hits, one call.
    pub fn menu_placed(&self) -> Vec<popup::Placed> {
        let Some(menu) = &self.menu else {
            return Vec::new();
        };
        let (w, h) = self.size;
        popup::layout(
            &menu.stack,
            None,
            Rect::new(0.0, 0.0, w as f32, h as f32),
            self.metrics(),
        )
    }

    /// **Opens the menu of pane `pane`'s panel**, about that pane.
    pub fn open_pane_menu(&mut self, pane: i32, at: MenuAt) {
        let Some(g) = self.groups.iter().find(|g| g.panes.contains(&pane)) else {
            return;
        };
        if self.floating {
            let mut entries = vec![Entry::action("Back into the dock", "dock")];
            if self.closable(pane) {
                entries.push(Entry::separator());
                entries.push(Entry::action("Close", "close"));
            }
            return self.open_menu(entries, at, Target::Pane(pane));
        }
        let several = g.panes.len() > 1;
        let mut entries = Vec::new();
        if several {
            let tabs = g
                .panes
                .iter()
                .map(|&p| {
                    Entry::check(&self.title(p), &format!("show:{p}"), p == g.panes[g.active])
                })
                .collect();
            entries.push(submenu("Tabs", tabs));
            entries.push(Entry::separator());
        }
        let split = |label: &str, verb: &str| {
            let e = Entry::action(label, verb);
            if several { e } else { e.disabled() }
        };
        entries.push(split("Split right", "split:right"));
        entries.push(split("Split down", "split:bottom"));
        let mut pages: Vec<Entry> = self
            .page_names()
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.page_index())
            .map(|(i, name)| Entry::action(name, &format!("page:{i}")))
            .collect();
        if !pages.is_empty() {
            pages.push(Entry::separator());
        }
        pages.push(Entry::action("New page", "page:new"));
        entries.push(submenu("Move to page", pages));
        entries.push(Entry::action("Move to its own window", "undock"));
        entries.push(Entry::separator());
        let others = Entry::action("Close the other tabs", "close_others");
        let any = g.panes.iter().any(|&p| p != pane && self.closable(p));
        entries.push(if any { others } else { others.disabled() });
        if self.closable(pane) {
            entries.push(Entry::action("Close", "close"));
        }
        self.open_menu(entries, at, Target::Pane(pane));
    }

    /// **Opens the menu of the page at `page`**, at `point`.
    pub fn open_page_menu(&mut self, page: usize, (x, y): (f32, f32)) {
        let last = self.page_names().len() == 1;
        let close = Entry::action("Close page", "page_close");
        let entries = vec![
            Entry::action("Show", "page_show"),
            Entry::action("Copy page", "page_copy"),
            Entry::action("New page", "page_new"),
            Entry::separator(),
            if last { close.disabled() } else { close },
        ];
        self.open_menu(entries, MenuAt::Point(x, y), Target::Page(page));
    }

    fn open_menu(&mut self, entries: Vec<Entry>, at: MenuAt, target: Target) {
        let anchor = match at {
            MenuAt::Below(r) => Anchor::Below(r),
            MenuAt::Point(x, y) => Anchor::At(x, y),
        };
        self.grip = None;
        self.hot = None;
        self.menu = Some(DockMenu {
            stack: Stack::new(Owner::Dock, entries, anchor, self.text_size()),
            target,
        });
        self.dirty.chrome = true;
    }

    /// Closes the menu, answering whether one was open.
    pub fn close_menu(&mut self) -> bool {
        let open = self.menu.take().is_some();
        if open {
            self.dirty.chrome = true;
        }
        open
    }

    /// The pointer over the menu at `(x, y)`: the row under it lit, a
    /// submenu's opened. Answers whether the picture changed.
    pub(super) fn menu_moved(&mut self, x: f64, y: f64) -> bool {
        let placed = self.menu_placed();
        let Some(menu) = self.menu.as_mut() else {
            return false;
        };
        match popup::hit(&menu.stack, &placed, x, y) {
            Some((level, row)) => menu.stack.point(level, row),
            None => false,
        }
    }

    /// **A press with the menu open**: a row is picked and the menu closes;
    /// a submenu's row opens it; a press off every list closes the menu.
    pub(super) fn menu_pressed(&mut self, x: f64, y: f64) {
        let placed = self.menu_placed();
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let target = menu.target;
        match popup::hit(&menu.stack, &placed, x, y) {
            Some((level, Some(row))) => {
                let entry = menu.stack.levels[level].entries[row].clone();
                if entry.submenu().is_some() {
                    menu.stack.open_child(level, row);
                    self.dirty.chrome = true;
                    return;
                }
                if !entry.pickable() {
                    return;
                }
                self.close_menu();
                if let Some(verb) = entry.verb {
                    self.picked(target, &verb);
                }
            }
            Some((_, None)) => {}
            None => {
                self.close_menu();
            }
        }
    }

    /// **What a menu's pick does**: a move of the layout or a page, made
    /// here, or a window closed or taken out, handed to the front.
    pub fn picked(&mut self, target: Target, verb: &str) {
        match target {
            Target::Pane(pane) => {
                let group = self
                    .groups
                    .iter()
                    .find(|g| g.panes.contains(&pane))
                    .map(|g| g.panes.clone())
                    .unwrap_or_default();
                let other = group.iter().copied().find(|&p| p != pane);
                match verb {
                    "close" if self.closable(pane) => self.dirty.close.push(pane),
                    "close_others" => {
                        let others: Vec<i32> = group
                            .iter()
                            .copied()
                            .filter(|&p| p != pane && self.closable(p))
                            .collect();
                        self.dirty.close.extend(others);
                    }
                    "undock" => self.dirty.out.push(pane),
                    "dock" => self.dirty.back.push(pane),
                    "split:right" | "split:bottom" => {
                        let side = if verb == "split:right" {
                            Side::Right
                        } else {
                            Side::Bottom
                        };
                        if let Some(of) = other {
                            let _ = self.land(pane, Landing::Split { of, side });
                        }
                    }
                    "page:new" => {
                        let to = self.page_names().len();
                        self.move_to_page(pane, to);
                    }
                    _ => {
                        if let Some(to) = verb.strip_prefix("page:").and_then(|i| i.parse().ok()) {
                            self.move_to_page(pane, to);
                        } else if let Some(p) =
                            verb.strip_prefix("show:").and_then(|i| i.parse().ok())
                        {
                            let _ = self.select(p);
                        }
                    }
                }
            }
            Target::Page(page) => match verb {
                "page_show" => {
                    self.switch_to(page);
                }
                "page_copy" => {
                    self.switch_to(page);
                    let _ = self.new_page(None, true);
                }
                "page_new" => {
                    let _ = self.new_page(None, false);
                }
                "page_close" => {
                    if let Some(name) = self.page_names().get(page).cloned() {
                        let _ = self.close_page(&name);
                    }
                }
                _ => {}
            },
        }
        self.dirty.chrome = true;
    }
}

/// An entry that opens `entries` as a list of its own.
fn submenu(label: &str, entries: Vec<Entry>) -> Entry {
    Entry {
        label: label.to_string(),
        verb: None,
        kind: EntryKind::Submenu(entries),
        enabled: true,
        icon: None,
        key: None,
    }
}
