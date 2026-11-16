//! **The dock**: several windows drawn into one surface, each in a
//! rectangle of it -- one window of the platform holding every window the
//! host opens, natively and in a page alike.
//!
//! A window in a dock is a **pane**. It keeps everything a window of its
//! own has -- its def, its instance, its history, its gesture machine, its
//! replies -- and loses only the surface: its frames are drawn into a texture
//! of its own on the dock's device ([`Gpu::pane`](crate::gpu::Gpu)),
//! through the very passes a window's frame takes, and the dock copies
//! each texture into its place in the surface it presents ([`Composer`]).
//! So a pane's picture is the picture the same window would show on its own,
//! at the same size, by construction.
//!
//! What is shared by the fronts:
//!
//! - [`tree`] is the layout as data: splits whose leaves are groups of
//!   tabs, the moves over it, and the JSON it is saved as.
//! - [`Dock`] measures that layout and holds the hand: where each pane
//!   and each tab strip is, which pane the pointer is over, what a press
//!   holds until its release (a pane, a divider, a tab being dragged) and
//!   which pane the keyboard goes to. Pure logic, with no device and no
//!   window; it also draws its own chrome (`chrome`).
//! - [`call`] is the wire's door to it (`/gui_dock`): the layout read and
//!   restored, the moves a hand makes, by name.
//! - [`Composer`] is the copy of every shown pane into the surface with the
//!   chrome over it, and a picture of the whole read back (the dock's
//!   capture).
//!
//! A pane behind another's tab is not drawn and is told it is not seen, as a
//! covered window is; it keeps its def, its state and whatever it plays.

use std::collections::HashMap;

use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::Theme;

use crate::gpu::Gpu;
use crate::host::frame::Copier;
use crate::host::frame::{Readback, read_back, read_target};
use crate::host::graphics::choice::{self, Part};
use crate::host::widget::size::control_box;
use crate::painter::Painter;
use crate::view::Target;

pub mod call;
mod chrome;
mod pages;
pub mod tree;

pub use tree::{Group, Node, Path, Side};

/// **The color between two panes**: the theme's `separator` role, what the
/// dock clears its surface to before it copies the panes in.
pub(crate) fn divider_color(theme: &Theme) -> wgpu::Color {
    let [r, g, b, a] = theme.separator;
    wgpu::Color {
        r: f64::from(r),
        g: f64::from(g),
        b: f64::from(b),
        a: f64::from(a),
    }
}

/// A pane's rectangle in the dock's surface, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PaneRect {
    /// Whether the point `(x, y)` of the surface is inside.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && y >= f64::from(self.y)
            && x < f64::from(self.x + self.w)
            && y < f64::from(self.y + self.h)
    }

    /// The same rectangle, as the drawing base measures one.
    pub fn rect(&self) -> Rect {
        Rect::new(self.x as f32, self.y as f32, self.w as f32, self.h as f32)
    }
}

/// Where a pointer's motion goes: the pane it reaches, with the point in the
/// pane's own pixels, and the pane it left on the way, if it left one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub to: Option<(i32, (f64, f64))>,
    pub left: Option<i32>,
}

/// What changed in the dock since a front last asked ([`Dock::take_dirty`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dirty {
    /// Where the panes are, or which of them are shown: every pane is put at
    /// its place again, and the dock is drawn.
    pub layout: bool,
    /// Only the dock's own chrome (a tab lit under the pointer, where a
    /// dragged tab would land): the dock is drawn, and no pane.
    pub chrome: bool,
    /// A pane whose tab was dragged out of the dock and let go there: the
    /// front gives it a window of its own ([`Landing::Out`]).
    pub out: Option<i32>,
}

/// A group of tabs as measured: its tab strip and the body its shown pane
/// fills.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupPlace {
    pub path: Path,
    pub strip: Rect,
    pub body: PaneRect,
    pub panes: Vec<i32>,
    pub active: usize,
}

/// A divider as measured: the gap between part `before` and the next of the
/// split at `split`, and what the parts of that split measure along it.
#[derive(Clone, Debug, PartialEq)]
pub struct DividerPlace {
    pub split: Path,
    pub before: usize,
    pub rect: Rect,
    /// Whether the split is a row: the divider stands upright and is dragged
    /// sideways.
    pub row: bool,
    /// Every part's length along the split, in device pixels.
    pub lengths: Vec<f32>,
}

/// Where a dragged tab would land ([`Dock`]'s hand), and what it would do
/// there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Landing {
    /// Among the tabs of the group holding `of`, before the tab at `at`.
    Join { of: i32, at: Option<usize> },
    /// On `side` of the group holding `of`, a group of its own.
    Split { of: i32, side: Side },
    /// Out of the dock: a window of its own, which only a front can open, so
    /// the dock says so ([`Dirty::out`]) rather than moving anything.
    Out,
    /// Onto the page at this index, leaving the page shown: a column of its
    /// own there. One past the last page is a new page holding it alone.
    Page(usize),
}

/// What a press is holding.
#[derive(Clone, Debug, PartialEq)]
enum Grip {
    /// A pane: every motion and the release go to it.
    Pane(i32),
    /// A divider, pressed at `from` (along its axis) with the parts it
    /// stands between that long.
    Divider { place: DividerPlace, from: f64 },
    /// A tab, pressed at `from`; dragged once it has moved past a few
    /// pixels, with where it would land.
    Tab {
        pane: i32,
        from: (f64, f64),
        dragging: bool,
        landing: Option<(Landing, Rect)>,
    },
}

/// What the pointer is over that the chrome lights.
#[derive(Clone, Debug, PartialEq)]
enum Hot {
    Tab(Path, Part),
    Divider(usize),
    /// A part of the page strip.
    Page(Part),
}

/// How far a pressed tab moves before it is dragged rather than chosen.
const DRAG_PX: f64 = 6.0;

/// The share of a group's body, from each edge, where a dragged tab splits it
/// rather than joining it.
const EDGE: f32 = 0.25;

/// **The arrangement of the panes, and where the hand is among them.**
#[derive(Debug)]
pub struct Dock {
    /// The shown page's layout: splits and groups of tabs ([`tree`]).
    tree: Node,
    /// **The pages**: each a name and its layout. The shown one's layout is
    /// `tree`, and its entry here holds an empty one meanwhile.
    pages: Vec<(String, Node)>,
    /// Which page is shown.
    page: usize,
    /// Every pane the dock holds, in the order they came: on the shown page,
    /// on another, or on none -- a pane no page names is kept, and shown
    /// again by putting it on one.
    members: Vec<i32>,
    /// What the layout measures to at this size.
    groups: Vec<GroupPlace>,
    dividers: Vec<DividerPlace>,
    /// The surface's size, in device pixels.
    size: (u32, u32),
    /// The size roles at the dock's density: the divider, the tab strip, the
    /// text of the tabs.
    metrics: Metrics,
    /// Each pane's tab title.
    titles: HashMap<i32, String>,
    /// Where the pointer is, in the surface's pixels.
    at: Option<(f64, f64)>,
    /// The pane the pointer is over, with no button held.
    pointer: Option<i32>,
    /// What a press holds until its release -- a drag owns the pointer, in a
    /// pane as in a window, and on the dock's chrome too.
    grip: Option<Grip>,
    /// What the chrome lights under the pointer.
    hot: Option<Hot>,
    /// The pane the keyboard goes to: the one last pressed in.
    focus: Option<i32>,
    dirty: Dirty,
}

impl Dock {
    /// An empty dock of `size`, laid out with `metrics` (already resolved at
    /// the dock's density).
    pub fn new(size: (u32, u32), metrics: Metrics) -> Self {
        Self {
            tree: Node::default(),
            pages: vec![(FIRST_PAGE.to_string(), Node::default())],
            page: 0,
            members: Vec::new(),
            groups: Vec::new(),
            dividers: Vec::new(),
            size,
            metrics,
            titles: HashMap::new(),
            at: None,
            pointer: None,
            grip: None,
            hot: None,
            focus: None,
            dirty: Dirty::default(),
        }
    }

    /// Every pane the dock holds, in the order they came: shown, behind a
    /// tab, or on another page.
    pub fn panes(&self) -> Vec<i32> {
        self.members.clone()
    }

    /// Whether `def_id` is a pane here.
    pub fn holds(&self, def_id: i32) -> bool {
        self.members.contains(&def_id)
    }

    /// Whether pane `def_id` is shown: the chosen tab of its group.
    pub fn shown(&self, def_id: i32) -> bool {
        self.groups
            .iter()
            .any(|g| g.panes.get(g.active) == Some(&def_id))
    }

    /// The surface's size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The size roles the dock is laid out with.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// The layout.
    pub fn tree(&self) -> &Node {
        &self.tree
    }

    /// The groups as measured.
    pub fn groups(&self) -> &[GroupPlace] {
        &self.groups
    }

    /// The dividers as measured.
    pub fn dividers(&self) -> &[DividerPlace] {
        &self.dividers
    }

    /// Pane `def_id`'s tab title: what it was given, or its id.
    pub fn title(&self, def_id: i32) -> String {
        self.titles
            .get(&def_id)
            .cloned()
            .unwrap_or_else(|| def_id.to_string())
    }

    /// Names pane `def_id`'s tab.
    pub fn set_title(&mut self, def_id: i32, title: String) {
        if self.titles.get(&def_id) != Some(&title) {
            self.titles.insert(def_id, title);
            self.dirty.chrome = true;
        }
    }

    /// **What changed since the last time**, and nothing from then on.
    pub fn take_dirty(&mut self) -> Dirty {
        std::mem::take(&mut self.dirty)
    }

    /// Adds `def_id` as a column of its own at the right of the page shown,
    /// and gives it the keyboard. Answers whether it was not one already.
    pub fn add(&mut self, def_id: i32) -> bool {
        self.add_on(def_id, None)
    }

    /// **Adds `def_id` on page `page`** -- made if there is none of that
    /// name, and shown -- or on the page shown with none, as a column of its
    /// own at the right, and gives it the keyboard. Answers whether it was
    /// not a pane already.
    pub fn add_on(&mut self, def_id: i32, page: Option<&str>) -> bool {
        if self.holds(def_id) {
            return false;
        }
        self.members.push(def_id);
        if let Some(name) = page {
            self.show_page(name);
        }
        self.tree.add(def_id);
        self.focus = Some(def_id);
        self.arrange();
        true
    }

    /// Takes `def_id` out, off every page; the panes left share its room.
    /// Answers whether it was one.
    pub fn remove(&mut self, def_id: i32) -> bool {
        if !self.holds(def_id) {
            return false;
        }
        self.members.retain(|&p| p != def_id);
        self.tree.remove(def_id);
        for (_, tree) in &mut self.pages {
            tree.remove(def_id);
        }
        self.titles.remove(&def_id);
        if self.pointer == Some(def_id) {
            self.pointer = None;
        }
        if matches!(&self.grip, Some(Grip::Pane(p) | Grip::Tab { pane: p, .. }) if *p == def_id) {
            self.grip = None;
        }
        if self.focus == Some(def_id) || self.focus.is_none() {
            self.focus = self.panes().into_iter().rfind(|&p| self.tree_shows(p));
        }
        self.hot = None;
        self.arrange();
        true
    }

    /// Whether the layout shows `pane`, measured or not.
    fn tree_shows(&self, pane: i32) -> bool {
        self.tree
            .group_of(pane)
            .and_then(|path| self.tree.group(&path).and_then(Group::shown))
            == Some(pane)
    }

    /// The surface's size changed. Answers whether it did.
    pub fn set_size(&mut self, size: (u32, u32)) -> bool {
        if self.size == size {
            return false;
        }
        self.size = size;
        self.arrange();
        true
    }

    /// The size roles changed (a display of another density). Answers
    /// whether they did.
    pub fn set_metrics(&mut self, metrics: Metrics) -> bool {
        if self.metrics == metrics {
            return false;
        }
        self.metrics = metrics;
        self.arrange();
        true
    }

    /// Pane `def_id`'s rectangle: its group's body, shown or not.
    pub fn rect(&self, def_id: i32) -> Option<PaneRect> {
        self.groups
            .iter()
            .find(|g| g.panes.contains(&def_id))
            .map(|g| g.body)
    }

    /// Every shown pane with its rectangle, in layout order.
    pub fn placed(&self) -> impl Iterator<Item = (i32, PaneRect)> + '_ {
        self.groups
            .iter()
            .filter_map(|g| g.panes.get(g.active).map(|&p| (p, g.body)))
    }

    /// The shown pane under the point `(x, y)` of the surface, and the point
    /// in its own pixels. A divider and a tab strip are in no pane.
    pub fn at(&self, x: f64, y: f64) -> Option<(i32, (f64, f64))> {
        self.placed()
            .find(|(_, r)| r.contains(x, y))
            .map(|(id, r)| (id, (x - f64::from(r.x), y - f64::from(r.y))))
    }

    /// The point `(x, y)` of the surface in pane `def_id`'s own pixels,
    /// wherever it is -- a drag that left the pane still reads it there.
    pub fn local(&self, def_id: i32, x: f64, y: f64) -> Option<(f64, f64)> {
        self.rect(def_id)
            .map(|r| (x - f64::from(r.x), y - f64::from(r.y)))
    }

    /// The pane the keyboard goes to.
    pub fn focus(&self) -> Option<i32> {
        self.focus
    }

    /// The pane the pointer is over, or the one a press holds.
    pub fn pointer(&self) -> Option<i32> {
        match &self.grip {
            Some(Grip::Pane(p)) => Some(*p),
            Some(_) => None,
            None => self.pointer,
        }
    }

    /// The pane a press holds.
    pub fn held(&self) -> Option<i32> {
        match &self.grip {
            Some(Grip::Pane(p)) => Some(*p),
            _ => None,
        }
    }

    /// Where a dragged tab would land, and the rectangle that shows it.
    pub fn landing(&self) -> Option<(Landing, Rect)> {
        match &self.grip {
            Some(Grip::Tab {
                landing, dragging, ..
            }) if *dragging => *landing,
            _ => None,
        }
    }

    /// The divider lit: under the pointer, or dragged.
    pub fn lit_divider(&self) -> Option<&DividerPlace> {
        match (&self.grip, &self.hot) {
            (Some(Grip::Divider { place, .. }), _) => Some(place),
            (None, Some(Hot::Divider(i))) => self.dividers.get(*i),
            _ => None,
        }
    }

    /// The part of group `path`'s strip lit under the pointer.
    pub fn lit_tab(&self, path: &[usize]) -> Option<Part> {
        match (&self.grip, &self.hot) {
            (None, Some(Hot::Tab(at, part))) if at == path => Some(*part),
            _ => None,
        }
    }

    /// The text size of the tabs.
    pub fn text_size(&self) -> f32 {
        self.metrics.text_scale
    }

    /// The tabs of a group, as the strip's parts lay them out.
    pub fn tab_parts(&self, group: &GroupPlace) -> Vec<(Part, Rect)> {
        let titles: Vec<String> = group.panes.iter().map(|&p| self.title(p)).collect();
        choice::parts(
            choice::View::Tabs,
            &titles,
            group.strip,
            self.text_size(),
            &self.metrics,
        )
    }

    /// **The pointer moved to `(x, y)`**: the pane a press holds, if one
    /// does, else the pane under it -- and the pane it left, which hears it
    /// left, as a window hears the pointer leave it. A divider or a tab the
    /// press holds moves instead, and no pane hears it.
    pub fn moved(&mut self, x: f64, y: f64) -> Motion {
        self.at = Some((x, y));
        match self.grip.clone() {
            Some(Grip::Pane(held)) => {
                return Motion {
                    to: self.local(held, x, y).map(|p| (held, p)),
                    left: None,
                };
            }
            Some(Grip::Divider { place, from }) => {
                self.drag_divider(&place, from, x, y);
                return Motion {
                    to: None,
                    left: None,
                };
            }
            Some(Grip::Tab {
                pane,
                from,
                dragging,
                landing,
            }) => {
                let dragging = dragging || (x - from.0).hypot(y - from.1) > DRAG_PX;
                let now = if dragging {
                    self.landing_at(pane, x, y)
                } else {
                    None
                };
                if now != landing {
                    self.dirty.chrome = true;
                }
                self.grip = Some(Grip::Tab {
                    pane,
                    from,
                    dragging,
                    landing: now,
                });
                return Motion {
                    to: None,
                    left: None,
                };
            }
            None => {}
        }
        let hot = self.hot_at(x, y);
        if hot != self.hot {
            self.hot = hot;
            self.dirty.chrome = true;
        }
        let to = self.at(x, y);
        let now = to.map(|(id, _)| id);
        let left = self.pointer.filter(|&was| Some(was) != now);
        self.pointer = now;
        Motion { to, left }
    }

    /// **A button went down** where the pointer is: a pane under it takes it
    /// and the keyboard with it, and the answer is that pane; a tab shows its
    /// pane and may be dragged, a divider is dragged, and no pane hears it.
    pub fn pressed(&mut self) -> Option<i32> {
        if let Some(Grip::Pane(p)) = self.grip {
            return Some(p);
        }
        let (x, y) = self.at?;
        if let Some(part) = self.page_at(x, y) {
            self.press_page(part);
            return None;
        }
        if let Some(i) = self.divider_at(x, y) {
            let place = self.dividers[i].clone();
            let from = if place.row { x } else { y };
            self.grip = Some(Grip::Divider { place, from });
            self.dirty.chrome = true;
            return None;
        }
        if let Some((group, part)) = self.tab_at(x, y) {
            let panes = &self.groups[group].panes;
            let active = self.groups[group].active;
            let pane = match part {
                Part::Option(i) => panes.get(i).copied(),
                // the tabs that did not fit are shown one after another
                Part::More(from) => panes
                    .get(if active >= from { active + 1 } else { from })
                    .or_else(|| panes.get(from))
                    .copied(),
                _ => None,
            }?;
            if self.tree.select(pane) {
                self.arrange();
            }
            self.focus = Some(pane);
            if matches!(part, Part::Option(_)) {
                self.grip = Some(Grip::Tab {
                    pane,
                    from: (x, y),
                    dragging: false,
                    landing: None,
                });
            }
            self.dirty.chrome = true;
            return None;
        }
        let pane = self.pointer?;
        self.grip = Some(Grip::Pane(pane));
        self.focus = Some(pane);
        Some(pane)
    }

    /// **The button came up**: answers the pane the press held, which hears
    /// the release wherever the pointer is. A tab dragged somewhere lands
    /// there; a divider stays where it was left.
    pub fn released(&mut self) -> Option<i32> {
        match self.grip.take() {
            Some(Grip::Pane(p)) => Some(p),
            Some(Grip::Tab {
                pane,
                landing: Some((landing, _)),
                dragging: true,
                ..
            }) => {
                if landing == Landing::Out {
                    self.dirty.out = Some(pane);
                    self.dirty.chrome = true;
                } else if let Landing::Page(to) = landing {
                    self.move_to_page(pane, to);
                } else {
                    let _ = self.land(pane, landing);
                }
                None
            }
            Some(_) => {
                self.dirty.chrome = true;
                None
            }
            None => self.pointer,
        }
    }

    /// **The pointer left the surface**: answers the pane it was over, unless
    /// a press holds something -- motion keeps reaching that until the
    /// release.
    pub fn left(&mut self) -> Option<i32> {
        if self.grip.is_some() {
            return None;
        }
        self.at = None;
        if self.hot.take().is_some() {
            self.dirty.chrome = true;
        }
        self.pointer.take()
    }

    /// **Shows pane `def_id`** in its group, and gives it the keyboard.
    pub fn select(&mut self, def_id: i32) -> Result<(), String> {
        if !self.holds(def_id) {
            return Err(format!("no pane {def_id}"));
        }
        if self.tree.select(def_id) {
            self.arrange();
        }
        self.focus = Some(def_id);
        Ok(())
    }

    /// **Moves pane `pane`** where a dragged tab would land.
    pub fn land(&mut self, pane: i32, landing: Landing) -> Result<(), String> {
        match landing {
            Landing::Join { of, at } => self.tree.join(pane, of, at)?,
            Landing::Split { of, side } => self.tree.split(pane, side, of)?,
            Landing::Out => return Err("a pane is taken out by the front that holds it".into()),
            Landing::Page(to) => {
                self.move_to_page(pane, to);
                return Ok(());
            }
        }
        self.focus = Some(pane);
        self.arrange();
        Ok(())
    }

    /// **A layout put in place of this one**: every page, from
    /// `{"pages": [{"name", "layout"}...], "page": name}`, or the page shown
    /// from a layout alone ([`Node::read`]). A pane no page names after it
    /// goes on the page shown, as a column of its own, so no pane is lost.
    pub fn restore(&mut self, layout: &serde_json::Value) -> Result<(), String> {
        self.restore_pages(layout)?;
        self.grip = None;
        self.hot = None;
        if self.focus.is_none_or(|f| !self.tree_shows(f)) {
            self.focus = self.panes().into_iter().find(|&p| self.tree_shows(p));
        }
        self.arrange();
        Ok(())
    }

    /// The divider whose gap, with the hit slop around it, holds `(x, y)`.
    fn divider_at(&self, x: f64, y: f64) -> Option<usize> {
        let slop = self.metrics.hit_slop;
        self.dividers.iter().position(|d| {
            let grown = if d.row {
                Rect::new(d.rect.x - slop, d.rect.y, d.rect.w + 2.0 * slop, d.rect.h)
            } else {
                Rect::new(d.rect.x, d.rect.y - slop, d.rect.w, d.rect.h + 2.0 * slop)
            };
            grown.contains(x, y)
        })
    }

    /// The group whose strip holds `(x, y)`, and the part of it there.
    fn tab_at(&self, x: f64, y: f64) -> Option<(usize, Part)> {
        let at = self.groups.iter().position(|g| g.strip.contains(x, y))?;
        let parts = self.tab_parts(&self.groups[at]);
        choice::part_at(&parts, x, y).map(|part| (at, part))
    }

    /// What the chrome lights at `(x, y)`.
    fn hot_at(&self, x: f64, y: f64) -> Option<Hot> {
        if let Some(part) = self.page_at(x, y) {
            return Some(Hot::Page(part));
        }
        if let Some(i) = self.divider_at(x, y) {
            return Some(Hot::Divider(i));
        }
        self.tab_at(x, y)
            .map(|(g, part)| Hot::Tab(self.groups[g].path.clone(), part))
    }

    /// **The divider pressed at `from` moved to `(x, y)`**: the two parts
    /// beside it trade the distance, as a divider of a split strip in a
    /// window does ([`crate::host::chrome::split`]), neither taken under a
    /// tab strip and a little.
    fn drag_divider(&mut self, place: &DividerPlace, from: f64, x: f64, y: f64) {
        let travel = (if place.row { x } else { y }) - from;
        let i = place.before;
        let (a, b) = (place.lengths[i], place.lengths[i + 1]);
        let least = 2.0 * self.strip_h() as f32;
        let (a, b) = crate::host::chrome::split(a, b, travel as f32, least);
        let mut lengths = place.lengths.clone();
        lengths[i] = a;
        lengths[i + 1] = b;
        if self.tree.shares(&place.split).is_some() {
            self.tree.set_shares(&place.split, &lengths);
            self.arrange();
        }
    }

    /// **Where a tab dragged to `(x, y)` would land**: among the tabs of the
    /// strip under it, before the tab whose middle is past it; in a group's
    /// body, on the side whose edge is near (a new group there) or among its
    /// tabs from the middle; past the dock's edge, out of it, the pane it
    /// leaves lit. Nowhere when the move would change nothing.
    fn landing_at(&self, pane: i32, x: f64, y: f64) -> Option<(Landing, Rect)> {
        let source = self.groups.iter().find(|g| g.panes.contains(&pane))?;
        let (w, h) = self.size;
        if x < 0.0 || y < 0.0 || x >= f64::from(w) || y >= f64::from(h) {
            return Some((Landing::Out, source.body.rect()));
        }
        if self.page_strip().contains(x, y) {
            return self.page_landing(x, y);
        }
        let alone = source.panes.len() == 1;
        let group = self
            .groups
            .iter()
            .find(|g| g.strip.contains(x, y) || g.body.contains(x, y))?;
        let of = group.panes[group.active];
        let same = group.path == source.path;
        if group.strip.contains(x, y) {
            let at = self
                .tab_parts(group)
                .iter()
                .position(|(_, r)| x < f64::from(r.x + r.w * 0.5));
            if same && alone {
                return None;
            }
            return Some((Landing::Join { of, at }, group.body.rect()));
        }
        let body = group.body.rect();
        let fx = ((x as f32 - body.x) / body.w.max(1.0)).clamp(0.0, 1.0);
        let fy = ((y as f32 - body.y) / body.h.max(1.0)).clamp(0.0, 1.0);
        let edges = [
            (fx, Side::Left),
            (1.0 - fx, Side::Right),
            (fy, Side::Top),
            (1.0 - fy, Side::Bottom),
        ];
        let (near, side) = edges
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((1.0, Side::Left));
        if near < EDGE {
            if same && alone {
                return None;
            }
            let half = match side {
                Side::Left => Rect::new(body.x, body.y, body.w * 0.5, body.h),
                Side::Right => Rect::new(body.x + body.w * 0.5, body.y, body.w * 0.5, body.h),
                Side::Top => Rect::new(body.x, body.y, body.w, body.h * 0.5),
                Side::Bottom => Rect::new(body.x, body.y + body.h * 0.5, body.w, body.h * 0.5),
            };
            return Some((Landing::Split { of, side }, half));
        }
        if same {
            return None;
        }
        Some((Landing::Join { of, at: None }, body))
    }

    /// The height of a group's tab strip, and of the page strip.
    fn strip_h(&self) -> u32 {
        control_box(self.text_size(), &self.metrics).round() as u32
    }

    /// **The layout measured at this size**: every group's strip and body,
    /// every divider; and what that changed marked for the fronts.
    fn arrange(&mut self) {
        let mut groups = Vec::new();
        let mut dividers = Vec::new();
        let (w, h) = self.size;
        // the page strip along the top, the page's layout under it
        let top = self.strip_h().min(h.saturating_sub(1));
        let whole = PaneRect {
            x: 0,
            y: top,
            w,
            h: h - top,
        };
        let divider = (self.metrics.divider_w.round() as u32).max(1);
        measure(
            &self.tree,
            whole,
            divider,
            self.strip_h(),
            &mut Vec::new(),
            &mut groups,
            &mut dividers,
        );
        if groups != self.groups || dividers != self.dividers {
            self.dirty.layout = true;
        }
        self.groups = groups;
        self.dividers = dividers;
        if let Some(Hot::Divider(i)) = self.hot
            && i >= self.dividers.len()
        {
            self.hot = None;
        }
    }
}

/// The name of the page a dock opens with.
pub const FIRST_PAGE: &str = "Main";

/// Measures `node` into `rect`: a group's strip and body, a split's parts
/// along its axis with a divider between each two, the last part taking what
/// rounding left.
fn measure(
    node: &Node,
    rect: PaneRect,
    divider: u32,
    strip_h: u32,
    path: &mut Path,
    groups: &mut Vec<GroupPlace>,
    dividers: &mut Vec<DividerPlace>,
) {
    match node {
        Node::Tabs(g) => {
            let strip = strip_h.min(rect.h.saturating_sub(1));
            groups.push(GroupPlace {
                path: path.clone(),
                strip: Rect::new(rect.x as f32, rect.y as f32, rect.w as f32, strip as f32),
                body: PaneRect {
                    x: rect.x,
                    y: rect.y + strip,
                    w: rect.w.max(1),
                    h: (rect.h - strip).max(1),
                },
                panes: g.panes.clone(),
                active: g.active,
            });
        }
        Node::Split { row, parts } => {
            let n = parts.len() as u32;
            let along = if *row { rect.w } else { rect.h };
            let room = along.saturating_sub(divider * n.saturating_sub(1));
            let mut sum = 0.0f32;
            let mut starts = Vec::with_capacity(parts.len() + 1);
            for (share, _) in parts {
                starts.push((room as f32 * sum).round() as u32);
                sum += share;
            }
            starts.push(room);
            let lengths: Vec<f32> = starts.windows(2).map(|s| (s[1] - s[0]) as f32).collect();
            for (i, (_, part)) in parts.iter().enumerate() {
                let offset = starts[i] + divider * i as u32;
                let len = (starts[i + 1] - starts[i]).max(1);
                let inner = if *row {
                    PaneRect {
                        x: rect.x + offset,
                        y: rect.y,
                        w: len,
                        h: rect.h,
                    }
                } else {
                    PaneRect {
                        x: rect.x,
                        y: rect.y + offset,
                        w: rect.w,
                        h: len,
                    }
                };
                path.push(i);
                measure(part, inner, divider, strip_h, path, groups, dividers);
                path.pop();
                if i + 1 < parts.len() {
                    let end = offset + (starts[i + 1] - starts[i]);
                    let gap = if *row {
                        Rect::new(
                            (rect.x + end) as f32,
                            rect.y as f32,
                            divider as f32,
                            rect.h as f32,
                        )
                    } else {
                        Rect::new(
                            rect.x as f32,
                            (rect.y + end) as f32,
                            rect.w as f32,
                            divider as f32,
                        )
                    };
                    dividers.push(DividerPlace {
                        split: path.clone(),
                        before: i,
                        rect: gap,
                        row: *row,
                        lengths: lengths.clone(),
                    });
                }
            }
        }
    }
}

/// **The copy of every pane into the dock's surface**: each pane's
/// texture into its rectangle, the dividers left in the clear color.
pub(crate) struct Composer {
    copier: Copier,
    /// One shift buffer a pane, reused frame to frame.
    shifts: Vec<wgpu::Buffer>,
    /// The dock's own chrome, drawn over the panes.
    chrome: Painter,
}

impl Composer {
    /// A composer drawing into a single-sampled target of `format`.
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let target = Target::new(format);
        Self {
            copier: Copier::new(device, target),
            shifts: Vec::new(),
            chrome: Painter::new(device, target),
        }
    }

    /// **Encodes the copy of `panes` into `target`**, the dock's
    /// surface or a texture of its size, cleared to `clear` first: each
    /// pane's texture into its rectangle, one texel to one pixel, and
    /// `chrome` over them. `dock` is the dock's GPU, whose configuration
    /// is the surface's size.
    pub(crate) fn compose(
        &mut self,
        dock: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clear: wgpu::Color,
        panes: &[(PaneRect, &wgpu::TextureView)],
        chrome: &Mesh,
    ) {
        let (device, queue) = (&dock.device, &dock.queue);
        let size = (dock.config.width, dock.config.height);
        self.chrome
            .upload(device, queue, chrome, size.0.max(1), size.1.max(1));
        while self.shifts.len() < panes.len() {
            self.shifts
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pane shift"),
                    size: 16,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        let groups: Vec<_> = panes
            .iter()
            .zip(&self.shifts)
            .map(|((rect, view), shift)| {
                let offset = [-(rect.x as f32), -(rect.y as f32), 0.0, 0.0];
                queue.write_buffer(shift, 0, bytemuck::cast_slice(&offset));
                self.copier.bind(device, view, shift)
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("dock pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.copier.single);
        let (sw, sh) = (size.0.max(1), size.1.max(1));
        for ((rect, _), group) in panes.iter().zip(&groups) {
            // a pane is never drawn past the surface, whatever it was sized
            // to before the surface shrank
            let x = rect.x.min(sw);
            let y = rect.y.min(sh);
            let w = rect.w.min(sw - x);
            let h = rect.h.min(sh - y);
            if w == 0 || h == 0 {
                continue;
            }
            pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            pass.set_scissor_rect(x, y, w, h);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        // Left over all of the target: a blit WebGL presents through is
        // clipped by the scissor still set. The chrome is drawn across it.
        pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        pass.set_scissor_rect(0, 0, sw, sh);
        self.chrome.draw(&mut pass);
    }

    /// **The dock's picture, read back**: the panes composed into a
    /// texture of the surface's size instead of the surface, as a window's
    /// capture is its frame drawn into a texture -- the same copy, so the
    /// picture is the screen's.
    pub(crate) fn capture(
        &mut self,
        dock: &Gpu,
        clear: wgpu::Color,
        panes: &[(PaneRect, &wgpu::TextureView)],
        chrome: &Mesh,
    ) -> Result<Readback, String> {
        let size = (dock.config.width, dock.config.height);
        let texture = read_target(&dock.device, dock.config.format, size)?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = dock
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("dock capture"),
            });
        self.compose(dock, &mut encoder, &view, clear, panes, chrome);
        Ok(read_back(&dock.device, &dock.queue, encoder, &texture))
    }
}

#[cfg(test)]
mod tests;
