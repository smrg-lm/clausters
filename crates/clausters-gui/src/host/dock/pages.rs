//! **The dock's pages**: one arrangement per kind of work. A page is a name
//! and a layout of its own ([`Node`]); the dock shows one at a time, and a
//! strip of page tabs along its bottom switches them. A pane may be on several
//! pages, at a place of each's, or on none; switching a page moves panes and
//! hides those it does not name, and frees nothing -- a pane on no page shown
//! keeps its def and whatever it plays, as a pane behind a tab does.
//!
//! A window may name the page it opens on (its `dock_page`), which is how an
//! application gives its kind of work a page of its own: the page is made the
//! first time and shown as the window opens.

use serde_json::{Value, json};

use super::{Dock, FIRST_PAGE, Landing, Node, Part, Rect};
use crate::host::graphics::choice;

/// The page strip's last tab: a new page, a copy of the one shown.
const NEW_PAGE: &str = "+";

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
        let layout = if copy {
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
    pub fn close_page(&mut self, name: &str) -> Result<(), String> {
        let at = self
            .pages
            .iter()
            .position(|(n, _)| n == name)
            .ok_or_else(|| format!("no page {name:?}"))?;
        if self.pages.len() == 1 {
            return Err("the dock keeps one page".into());
        }
        if at == self.page {
            self.switch_to(if at == 0 { 1 } else { at - 1 });
        }
        self.pages.remove(at);
        if at < self.page {
            self.page -= 1;
        }
        self.dirty.chrome = true;
        Ok(())
    }

    /// **Puts pane `pane` on the page shown**, as a column of its own, if
    /// it is not on it.
    pub fn place(&mut self, pane: i32) -> Result<(), String> {
        if !self.holds(pane) {
            return Err(format!("no pane {pane}"));
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
        if to == self.page {
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
        let h = self.page_strip_h().min(self.size.1.saturating_sub(1));
        Rect::new(0.0, (self.size.1 - h) as f32, self.size.0 as f32, h as f32)
    }

    /// The page strip's tabs: every page, then the new page's.
    pub fn page_options(&self) -> Vec<String> {
        let mut options = self.page_names();
        options.push(NEW_PAGE.to_string());
        options
    }

    /// The page strip's tabs as laid out.
    pub fn page_parts(&self) -> Vec<(Part, Rect)> {
        choice::parts(
            choice::View::Tabs,
            &self.page_options(),
            self.page_strip(),
            self.text_size(),
            self.metrics(),
        )
    }

    /// The page shown's index.
    pub fn page_index(&self) -> usize {
        self.page
    }

    /// The part of the page strip lit under the pointer.
    pub fn lit_page(&self) -> Option<Part> {
        match (&self.grip, &self.hot) {
            (None, Some(super::Hot::Page(part))) => Some(*part),
            _ => None,
        }
    }

    /// The part of the page strip at `(x, y)`.
    pub(super) fn page_at(&self, x: f64, y: f64) -> Option<Part> {
        if !self.page_strip().contains(x, y) {
            return None;
        }
        choice::part_at(&self.page_parts(), x, y)
    }

    /// **A press on the page strip**: a page's tab shows it, the last tab
    /// makes a page copying the one shown, and the tab the others gather
    /// behind shows them one after another.
    pub(super) fn press_page(&mut self, part: Part) {
        let n = self.pages.len();
        match part {
            Part::Option(i) if i < n => {
                self.switch_to(i);
            }
            Part::Option(_) => {
                let _ = self.new_page(None, true);
            }
            Part::More(from) => {
                let next = if self.page >= from {
                    self.page + 1
                } else {
                    from
                };
                self.switch_to(if next < n { next } else { from });
            }
            _ => {}
        }
        self.dirty.chrome = true;
    }

    /// **Where a tab dragged onto the page strip lands**: on another page's
    /// tab, that page; on the last tab, a new page.
    pub(super) fn page_landing(&self, x: f64, y: f64) -> Option<(Landing, Rect)> {
        let parts = self.page_parts();
        let (part, rect) = parts.iter().find(|(_, r)| r.contains(x, y))?;
        match *part {
            Part::Option(i) if i != self.page => Some((Landing::Page(i), *rect)),
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
