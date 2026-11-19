//! **The dock's pages**: one arrangement per kind of work. A page is a name
//! and a layout of its own ([`Node`]); the dock shows one at a time, and a
//! strip of page tabs along its bottom switches them. A pane may be on several
//! pages, at a place of each's, or on none; switching a page moves panes and
//! hides those it does not name, and frees nothing -- a pane on no page shown
//! keeps its def and whatever it plays, as a pane behind a tab does.
//!
//! **An application is a page.** A window that names a page (its
//! `dock_page`) is an application's, and opens on a new page of its own,
//! named after it, which holds it alone: no panel goes there, no panel's bar
//! is drawn over it (its menu is the dock's bar), and it goes on no other
//! page. Closing that page closes the application, and the application
//! closing takes its page. Panels -- every other window -- stay on the pages
//! that are not an application's.
//!
//! A page's tab is shown by a click and closed by its cross; the first page
//! has none, since it is the dock's own.

use clausters_gui_draw::font;
use serde_json::{Value, json};

use super::{Dock, Edge, FIRST_PAGE, Landing, Node, Rect};

/// One thing a hand can land on in the page strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagePart {
    /// The tab of the page at this index.
    Tab(usize),
    /// That tab's cross.
    Close(usize),
    /// The tabs from this index on, which did not fit: a press shows them
    /// one after another.
    More(usize),
}

impl Dock {
    /// The pages' names, in the order of their tabs.
    pub fn page_names(&self) -> Vec<String> {
        self.pages.iter().map(|(name, _)| name.clone()).collect()
    }

    /// The name of the page shown.
    pub fn page_name(&self) -> &str {
        &self.pages[self.page].0
    }

    /// **Shows page `name`**, made with nothing on it if there is none.
    /// Answers whether the page shown changed.
    pub fn show_page(&mut self, name: &str) -> bool {
        let at = match self.pages.iter().position(|(n, _)| n == name) {
            Some(at) => at,
            None => {
                self.pages.push((name.to_string(), Node::default()));
                self.pages.len() - 1
            }
        };
        self.switch_to(at)
    }

    /// **A new page**, named `name` or the first free `Page n`, holding a
    /// copy of the page shown's layout when `copy`, and shown. Answers its
    /// name.
    pub fn new_page(&mut self, name: Option<&str>, copy: bool) -> Result<String, String> {
        let name = match name {
            Some(name) if self.pages.iter().any(|(n, _)| n == name) => {
                return Err(format!("there is a page {name:?} already"));
            }
            Some("") => return Err("a page has a name".into()),
            Some(name) => name.to_string(),
            None => (2..)
                .map(|n| format!("Page {n}"))
                .find(|n| !self.pages.iter().any(|(p, _)| p == n))
                .unwrap_or_default(),
        };
        let layout = if copy && self.page_app(self.page).is_none() {
            self.tree.clone()
        } else {
            Node::default()
        };
        self.pages.push((name.clone(), layout));
        self.switch_to(self.pages.len() - 1);
        Ok(name)
    }

    /// Names page `name` `to`.
    pub fn rename_page(&mut self, name: &str, to: &str) -> Result<(), String> {
        if to.is_empty() {
            return Err("a page has a name".into());
        }
        if name != to && self.pages.iter().any(|(n, _)| n == to) {
            return Err(format!("there is a page {to:?} already"));
        }
        let page = self
            .pages
            .iter_mut()
            .find(|(n, _)| n == name)
            .ok_or_else(|| format!("no page {name:?}"))?;
        page.0 = to.to_string();
        self.dirty.chrome = true;
        Ok(())
    }

    /// **Closes page `name`**: its arrangement goes, its panes stay (on the
    /// other pages that name them, or on none). The last page is not closed.
    /// An application's page closes by closing the application, which its
    /// front asks it first ([`super::Dirty::close`]): its page goes with it.
    pub fn close_page(&mut self, name: &str) -> Result<(), String> {
        let at = self
            .pages
            .iter()
            .position(|(n, _)| n == name)
            .ok_or_else(|| format!("no page {name:?}"))?;
        if self.pages.len() == 1 {
            return Err("the dock keeps one page".into());
        }
        if let Some(app) = self.page_app(at) {
            self.dirty.close.push(app);
            return Ok(());
        }
        self.drop_page(at);
        Ok(())
    }

    /// **Page `at` goes**, the one before it shown if it was: what its
    /// tree holds stays a pane, on no page.
    pub(super) fn drop_page(&mut self, at: usize) {
        if self.pages.len() == 1 || at >= self.pages.len() {
            return;
        }
        if at == self.page {
            self.switch_to(if at == 0 { 1 } else { at - 1 });
        }
        self.pages.remove(at);
        if at < self.page {
            self.page -= 1;
        }
        // one page left: the strip goes, and the page takes its room
        self.arrange();
        self.dirty.layout = true;
        self.dirty.chrome = true;
    }

    /// The layout of page `at`: the shown one's as it stands.
    fn page_tree(&self, at: usize) -> Option<&Node> {
        if at == self.page {
            Some(&self.tree)
        } else {
            self.pages.get(at).map(|(_, tree)| tree)
        }
    }

    /// **The application page `at` holds**, if it is an application's.
    pub fn page_app(&self, at: usize) -> Option<i32> {
        let tree = self.page_tree(at)?;
        tree.panes().into_iter().find(|p| self.apps.contains(p))
    }

    /// The page pane `pane` is on (the first, for a panel on several).
    pub(super) fn page_of(&self, pane: i32) -> Option<usize> {
        (0..self.pages.len()).find(|&i| {
            self.page_tree(i)
                .is_some_and(|tree| tree.group_of(pane).is_some())
        })
    }

    /// Whether pane `pane` is an application ([`Dock::add_on`]).
    pub fn is_app(&self, pane: i32) -> bool {
        self.apps.contains(&pane)
    }

    /// **A new page for an application, named `name`** (or `name 2`, and
    /// on), shown: the dock's one page instead, renamed, while it holds
    /// nothing.
    pub(super) fn open_app_page(&mut self, name: &str) {
        let free = std::iter::once(name.to_string())
            .chain((2..).map(|n| format!("{name} {n}")))
            .find(|n| !self.pages.iter().any(|(p, _)| p == n))
            .unwrap_or_default();
        if self.pages.len() == 1 && self.tree.panes().is_empty() {
            self.pages[0].0 = free;
            self.dirty.chrome = true;
            return;
        }
        self.pages.push((free, Node::default()));
        self.switch_to(self.pages.len() - 1);
    }

    /// **A panel never opens on an application's page**: with one shown,
    /// the first page that is not an application's is shown instead, made
    /// when there is none.
    pub(super) fn leave_app_page(&mut self) {
        if self.page_app(self.page).is_none() {
            return;
        }
        match (0..self.pages.len()).find(|&i| self.page_app(i).is_none()) {
            Some(at) => {
                self.switch_to(at);
            }
            None => {
                let _ = self.new_page(Some(FIRST_PAGE), false);
            }
        }
    }

    /// **Puts pane `pane` on the page shown**, as a column of its own, if
    /// it is not on it.
    pub fn place(&mut self, pane: i32) -> Result<(), String> {
        if !self.holds(pane) {
            return Err(format!("no pane {pane}"));
        }
        if self.tree.group_of(pane).is_none()
            && (self.apps.contains(&pane) || self.page_app(self.page).is_some())
        {
            return Err("an application is alone on its page".into());
        }
        if self.tree.group_of(pane).is_none() {
            self.tree.add(pane);
        }
        self.focus = Some(pane);
        self.arrange();
        Ok(())
    }

    /// **Takes pane `pane` off the page shown**: it stays a pane, on the
    /// other pages that name it, or on none.
    pub fn leave(&mut self, pane: i32) -> Result<(), String> {
        if self.tree.group_of(pane).is_none() {
            return Err(format!("pane {pane} is not on the page shown"));
        }
        self.tree.remove(pane);
        self.refocus();
        self.arrange();
        Ok(())
    }

    /// Pane `pane` leaves the page shown for page `to` (a new one past the
    /// last), as a column of its own there.
    pub(super) fn move_to_page(&mut self, pane: i32, to: usize) {
        if to == self.page || self.apps.contains(&pane) || self.page_app(to).is_some() {
            return;
        }
        self.tree.remove(pane);
        if to >= self.pages.len() {
            let _ = self.new_page(None, false);
            self.tree.add(pane);
            self.focus = Some(pane);
        } else {
            let tree = &mut self.pages[to].1;
            if tree.group_of(pane).is_none() {
                tree.add(pane);
            }
            self.refocus();
        }
        self.arrange();
        self.dirty.layout = true;
        self.dirty.chrome = true;
    }

    /// **Page `to` shown in place of the one shown**: its layout measured,
    /// the keyboard on a pane it shows.
    pub(super) fn switch_to(&mut self, to: usize) -> bool {
        if to == self.page || to >= self.pages.len() {
            return false;
        }
        let shown = std::mem::take(&mut self.tree);
        self.pages[self.page].1 = shown;
        self.tree = std::mem::take(&mut self.pages[to].1);
        self.page = to;
        self.grip = None;
        self.hot = None;
        self.pointer = None;
        self.refocus();
        self.arrange();
        // a page whose layout measures as the last one's still shows other
        // tabs and the other page's tab lit
        self.dirty.layout = true;
        self.dirty.chrome = true;
        true
    }

    /// The keyboard on a pane the page shows, if it was on one it does not.
    fn refocus(&mut self) {
        if self.focus.is_none_or(|f| !self.tree_shows(f)) {
            self.focus = self.tree.panes().into_iter().find(|&p| self.tree_shows(p));
        }
    }

    /// The strip of page tabs, along the dock's bottom -- apart from the
    /// panels' bars, which run along each panel's top.
    pub fn page_strip(&self) -> Rect {
        let bottom = self.edges_h(Edge::Bottom);
        let h = self
            .page_strip_h()
            .min(self.size.1.saturating_sub(1 + bottom));
        let y = self.size.1.saturating_sub(bottom + h);
        Rect::new(0.0, y as f32, self.size.0 as f32, h as f32)
    }

    /// Whether page `at`'s tab has a cross: every page's but the first,
    /// which is the dock's own.
    pub(super) fn page_closable(&self, at: usize) -> bool {
        at > 0
    }

    /// **The page strip's parts**, each with its rectangle: every page's tab
    /// (its cross before it, so it is hit first), as a panel's tabs are laid
    /// out -- its inset, its name, and for a tab with a cross a gap, the
    /// cross and a gap -- and, past the strip's end, one tab holding the
    /// rest.
    pub fn page_parts(&self) -> Vec<(PagePart, Rect)> {
        let r = self.page_strip();
        let m = self.metrics();
        let size = self.text_size();
        let names = self.page_names();
        let close = m.tab_close;
        let widths: Vec<f32> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let around = if self.page_closable(i) {
                    m.tab_inset + 2.0 * m.tab_end + close
                } else {
                    2.0 * m.tab_inset
                };
                (font::width(name, size) + around).min(m.tab_w)
            })
            .collect();
        let more = font::width(&font::symbol::MENU.to_string(), size) + 2.0 * m.tab_inset;
        let total: f32 = widths.iter().sum();
        let fit = if total <= r.w {
            names.len()
        } else {
            let mut used = 0.0;
            widths
                .iter()
                .take_while(|w| {
                    used += **w;
                    used + more <= r.w
                })
                .count()
        };
        let mut out = Vec::new();
        let mut x = r.x;
        for (i, w) in widths[..fit].iter().enumerate() {
            if self.page_closable(i) {
                out.push((
                    PagePart::Close(i),
                    Rect::new(
                        x + w - close - m.tab_end,
                        r.y + (r.h - close) * 0.5,
                        close,
                        close,
                    ),
                ));
            }
            out.push((PagePart::Tab(i), Rect::new(x, r.y, *w, r.h)));
            x += w;
        }
        if fit < names.len() {
            let w = more.min((r.x + r.w - x).max(0.0));
            out.push((PagePart::More(fit), Rect::new(x, r.y, w, r.h)));
        }
        out
    }

    /// Where page `at`'s tab is, when the strip shows it.
    pub fn page_tab(&self, at: usize) -> Option<Rect> {
        self.page_parts()
            .into_iter()
            .find(|(part, _)| *part == PagePart::Tab(at))
            .map(|(_, r)| r)
    }

    /// The page shown's index.
    pub fn page_index(&self) -> usize {
        self.page
    }

    /// The part of the page strip lit under the pointer.
    pub fn lit_page(&self) -> Option<PagePart> {
        match (&self.grip, &self.hot) {
            (None, Some(super::Hot::Page(part))) => Some(*part),
            _ => None,
        }
    }

    /// The part of the page strip at `(x, y)`.
    pub(super) fn page_at(&self, x: f64, y: f64) -> Option<PagePart> {
        if !self.page_strip().contains(x, y) {
            return None;
        }
        self.page_parts()
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(part, _)| part)
    }

    /// **A press on the page strip**: a page's tab shows it, its cross
    /// closes it ([`Dock::close_page`]), and the tab the others gather
    /// behind shows them one after another.
    pub(super) fn press_page(&mut self, part: PagePart) {
        let n = self.pages.len();
        match part {
            PagePart::Tab(i) => {
                self.switch_to(i);
            }
            PagePart::Close(i) => {
                if let Some(name) = self.page_names().get(i).cloned() {
                    let _ = self.close_page(&name);
                }
            }
            PagePart::More(from) => {
                let next = if self.page >= from {
                    self.page + 1
                } else {
                    from
                };
                self.switch_to(if next < n { next } else { from });
            }
        }
        self.dirty.chrome = true;
    }

    /// **Where a tab dragged onto the page strip lands**: on another page's
    /// tab, that page -- never an application's, which holds it alone.
    pub(super) fn page_landing(&self, x: f64, y: f64) -> Option<(Landing, Rect)> {
        match self.page_at(x, y)? {
            PagePart::Tab(i) | PagePart::Close(i)
                if i != self.page && self.page_app(i).is_none() =>
            {
                Some((Landing::Page(i), self.page_tab(i)?))
            }
            _ => None,
        }
    }

    /// Every page with its layout, the one shown's as it stands.
    pub fn pages_json(&self) -> Value {
        let pages: Vec<Value> = self
            .pages
            .iter()
            .enumerate()
            .map(|(i, (name, tree))| {
                let tree = if i == self.page { &self.tree } else { tree };
                json!({"name": name, "layout": tree.to_json()})
            })
            .collect();
        json!(pages)
    }

    /// **The pages put back** from `{"pages": [{"name", "layout"}...],
    /// "page": name}`, or the page shown's layout from a layout alone. A pane
    /// no page names after it goes on the page shown.
    pub(super) fn restore_pages(&mut self, value: &Value) -> Result<(), String> {
        let members = self.members.clone();
        if let Some(pages) = value.get("pages") {
            let pages = pages.as_array().ok_or("`pages` is a list")?;
            let mut read = Vec::with_capacity(pages.len());
            for page in pages {
                let name = page
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .ok_or("a page has a `name`")?;
                if read.iter().any(|(n, _): &(String, Node)| n == name) {
                    return Err(format!("two pages are named {name:?}"));
                }
                let layout = match page.get("layout") {
                    Some(layout) => Node::read(layout, &members)?,
                    None => Node::default(),
                };
                read.push((name.to_string(), layout));
            }
            if read.is_empty() {
                read.push((FIRST_PAGE.to_string(), Node::default()));
            }
            let shown = value
                .get("page")
                .and_then(Value::as_str)
                .and_then(|name| read.iter().position(|(n, _)| n == name))
                .unwrap_or(0);
            self.tree = std::mem::take(&mut read[shown].1);
            self.pages = read;
            self.page = shown;
        } else {
            self.tree = Node::read(value, &members)?;
        }
        for pane in members {
            let anywhere = self.tree.group_of(pane).is_some()
                || self.pages.iter().any(|(_, t)| t.group_of(pane).is_some());
            if !anywhere {
                self.tree.add(pane);
            }
        }
        self.dirty.layout = true;
        self.dirty.chrome = true;
        Ok(())
    }
}
