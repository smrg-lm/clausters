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

use std::collections::{HashMap, HashSet};

use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::Theme;

use crate::gpu::Gpu;
use crate::host::frame::Copier;
use crate::host::frame::{Readback, read_back, read_target};
use crate::host::graphics::choice::Part;
use crate::host::widget::size::control_box;
use crate::painter::Painter;
use crate::view::Target;

mod bar;
pub mod call;
mod chrome;
mod edges;
mod pages;
pub mod tree;

pub use bar::BarPart;
pub use edges::{Edge, Overlay, PaneFloat};
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dirty {
    /// Where the panes are, or which of them are shown: every pane is put at
    /// its place again, and the dock is drawn.
    pub layout: bool,
    /// Only the dock's own chrome (a tab lit under the pointer, where a
    /// dragged tab would land): the dock is drawn, and no pane.
    pub chrome: bool,
    /// Panes dragged out of the dock and let go there: the front gives each
    /// a window of its own ([`Landing::Out`]), or picked out of it in a
    /// panel's menu.
    pub out: Vec<i32>,
    /// Panes a hand closed (a tab's close mark, the panel's, its menu): the
    /// front closes each window as its own close mark would.
    pub close: Vec<i32>,
    /// Panes a floating panel's menu asked back into the dock
    /// ([`Dock::floating`]): the front moves each back.
    pub back: Vec<i32>,
    /// Panes whose open list a press elsewhere took down: the front closes
    /// each one's popups, as a press beside a list does in a window.
    pub dismiss: Vec<i32>,
}

/// **Which shell** a front means: the dock's, or the floating panel of a
/// window taken out of it ([`Dock::floating`]), named by that window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Main,
    Float(i32),
}

/// What a hand asked of the windows of the shells, gathered from each one's
/// [`Dirty`] for the front to carry out: the panes taken out, those closed,
/// those put back.
#[derive(Debug, Default)]
pub struct Handed {
    pub out: Vec<i32>,
    pub close: Vec<i32>,
    pub back: Vec<i32>,
    pub dismiss: Vec<i32>,
}

impl Handed {
    /// Adds what one shell's hand asked.
    pub fn take(&mut self, dirty: Dirty) {
        self.out.extend(dirty.out);
        self.close.extend(dirty.close);
        self.back.extend(dirty.back);
        self.dismiss.extend(dirty.dismiss);
    }
}

/// A group of tabs as measured: its tab strip and the body its shown pane
/// fills.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupPlace {
    pub path: Path,
    pub strip: Rect,
    pub body: PaneRect,
    /// The strip and the body together: where a pane that fills its
    /// panel's header with its own tools ([`Dock::set_header`]) is drawn.
    pub whole: PaneRect,
    pub panes: Vec<i32>,
    pub active: usize,
}

/// **What a dock shows of a window on its panel**: its title, whether it is
/// a document (a tab even alone), whether it fills its panel's header with
/// tools of its own, whether it may be closed, whether its work has changes
/// not kept, and its tab's tip. A front reads it of the window
/// ([`crate::host::Host::pane_look`]) as the pane opens and whenever the
/// window says any of it changed, and hands it over whole
/// ([`Dock::describe`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaneLook {
    pub title: Option<String>,
    pub tab: bool,
    pub header: bool,
    pub closable: bool,
    pub modified: bool,
    pub tip: Option<String>,
}

/// **A pane's header band** ([`Dock::band`]): its height, and the room the
/// panel's bar keeps at each end of it, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub h: f32,
    pub left: f32,
    pub right: f32,
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
    /// pixels, with where it would land. `group` when the panel's grip (or
    /// its title) was pressed: the whole group moves, `pane` its shown one.
    Tab {
        pane: i32,
        group: bool,
        from: (f64, f64),
        dragging: bool,
        landing: Option<(Landing, Rect)>,
    },
}

/// What the pointer is over that the chrome lights.
#[derive(Clone, Debug, PartialEq)]
enum Hot {
    /// A part of a panel's bar.
    Bar(Path, bar::BarPart),
    Divider(usize),
    /// A part of the page strip.
    Page(Part),
}

/// How far a pressed tab moves before it is dragged rather than chosen.
const DRAG_PX: f64 = 6.0;

/// How long the pointer rests on a tab before its tip shows: a pane's own
/// tips' wait.
const TIP_MS: f64 = 600.0;

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
    /// The panes that are documents, shown as a tab even alone
    /// ([`Dock::set_tab`]).
    tabbed: HashSet<i32>,
    /// The panes that fill their panel's header with tools of their own
    /// ([`Dock::set_header`]).
    headed: HashSet<i32>,
    /// The panes their owner says may not be closed ([`Dock::set_closable`]).
    fixed: HashSet<i32>,
    /// The panes whose work has changes not kept: a dot on the tab.
    modified: HashSet<i32>,
    /// Each pane's tab tip.
    tips: HashMap<i32, String>,
    /// The tab the pointer rests on, and since when (the front's clock,
    /// unread until its first tick): its tip shows once it has rested.
    rest: Option<(i32, Option<f64>)>,
    /// The tab whose tip shows.
    tip: Option<i32>,
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
    /// **The menu open over the dock**: a panel's or a page's.
    menu: Option<bar::DockMenu>,
    /// A floating panel's: one window taken out of the dock, with its bar
    /// and no pages ([`Dock::floating`]).
    floating: bool,
    /// **The panes along the dock's edges** ([`Edge`]), in the order they
    /// came, each with its height in device pixels: on every page, outside
    /// the panels, no bar of their own.
    edges: Vec<(i32, Edge, u32)>,
    /// Where the edge panes are, as measured.
    edge_places: Vec<(i32, PaneRect)>,
    /// **What the panes draw over the dock** past their own rectangles --
    /// their open lists, their tips -- as the last picture placed it
    /// ([`edges::Float`]).
    overlays: Vec<edges::Float>,
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
            tabbed: HashSet::new(),
            headed: HashSet::new(),
            fixed: HashSet::new(),
            modified: HashSet::new(),
            tips: HashMap::new(),
            rest: None,
            tip: None,
            at: None,
            pointer: None,
            grip: None,
            hot: None,
            focus: None,
            menu: None,
            floating: false,
            edges: Vec::new(),
            edge_places: Vec::new(),
            overlays: Vec::new(),
            dirty: Dirty::default(),
        }
    }

    /// **A floating panel**: the window of a pane taken out of the dock,
    /// which keeps the bar it had as a panel -- its title, its menu, its
    /// close mark -- so it still reads as one, and its menu is the way back
    /// into the dock. It has no pages and no grip: there is nothing in it
    /// to drag a panel to.
    pub fn floating(size: (u32, u32), metrics: Metrics) -> Self {
        Self {
            floating: true,
            ..Self::new(size, metrics)
        }
    }

    /// Whether this is a floating panel's ([`Dock::floating`]).
    pub fn is_floating(&self) -> bool {
        self.floating
    }

    /// **The height of a panel's bar**, in device pixels: what a floating
    /// panel's window adds to its window's own height.
    pub fn bar_height(&self) -> u32 {
        self.strip_h()
    }

    /// Every pane the dock holds, in the order they came: shown, behind a
    /// tab, or on another page.
    pub fn panes(&self) -> Vec<i32> {
        let edges = self.edges.iter().map(|&(id, ..)| id);
        self.members.iter().copied().chain(edges).collect()
    }

    /// Whether `def_id` is a pane here.
    pub fn holds(&self, def_id: i32) -> bool {
        self.members.contains(&def_id) || self.edge_of(def_id).is_some()
    }

    /// Whether pane `def_id` is shown: the chosen tab of its group, or a
    /// pane along an edge.
    pub fn shown(&self, def_id: i32) -> bool {
        self.edge_of(def_id).is_some()
            || self
                .groups
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

    /// **Says whether pane `def_id` is a document** (`tab`): its panel shows
    /// it as a tab even when it is the panel's only window, where a panel
    /// of one window that is not shows a bar with its title.
    pub fn set_tab(&mut self, def_id: i32, tab: bool) {
        let changed = if tab {
            self.tabbed.insert(def_id)
        } else {
            self.tabbed.remove(&def_id)
        };
        if changed {
            self.dirty.chrome = true;
        }
    }

    /// **Says whether pane `def_id` fills its panel's header with tools of
    /// its own** (`header`): the pane is then drawn over its panel's whole
    /// rectangle, its header band under the bar, and the bar leaves it the
    /// middle -- the dock draws the grip, the tabs if any and its buttons,
    /// and the pane its tools between them ([`Dock::band`]).
    pub fn set_header(&mut self, def_id: i32, header: bool) {
        let changed = if header {
            self.headed.insert(def_id)
        } else {
            self.headed.remove(&def_id)
        };
        if changed {
            self.dirty.layout = true;
        }
    }

    /// **Hands over what the dock shows of pane `def_id`** ([`PaneLook`]).
    pub fn describe(&mut self, def_id: i32, look: PaneLook) {
        if let Some(title) = look.title {
            self.set_title(def_id, title);
        }
        self.set_tab(def_id, look.tab);
        self.set_header(def_id, look.header);
        self.set_closable(def_id, look.closable);
        let modified = if look.modified {
            self.modified.insert(def_id)
        } else {
            self.modified.remove(&def_id)
        };
        let tip = match look.tip {
            Some(t) => self.tips.insert(def_id, t.clone()).as_ref() != Some(&t),
            None => self.tips.remove(&def_id).is_some(),
        };
        if modified || tip {
            self.dirty.chrome = true;
        }
    }

    /// Whether pane `def_id`'s work has changes not kept.
    pub(super) fn is_modified(&self, def_id: i32) -> bool {
        self.modified.contains(&def_id)
    }

    /// **Says whether pane `def_id` may be closed** (`closable`): its owner's
    /// say. One that may not has no close mark on its tab or its bar, no
    /// Close in its panel's menu, and is left open by Close the other tabs
    /// -- the post window of an editor, which nothing could open again.
    pub fn set_closable(&mut self, def_id: i32, closable: bool) {
        let changed = if closable {
            self.fixed.remove(&def_id)
        } else {
            self.fixed.insert(def_id)
        };
        if changed {
            self.dirty.chrome = true;
        }
    }

    /// Whether pane `def_id` may be closed ([`Dock::set_closable`]).
    pub fn closable(&self, def_id: i32) -> bool {
        !self.fixed.contains(&def_id)
    }

    /// Whether pane `def_id` fills its panel's header ([`Dock::set_header`]).
    pub(super) fn is_headed(&self, def_id: i32) -> bool {
        self.headed.contains(&def_id)
    }

    /// **Where pane `def_id`'s header band is, and what of it is its own**:
    /// the band's height and, in the pane's pixels, the room the bar keeps
    /// at its start (the grip and the tabs) and at its end (the buttons).
    /// `None` for a pane that fills no header, or that no panel shows.
    pub fn band(&self, def_id: i32) -> Option<Band> {
        if !self.is_headed(def_id) {
            return None;
        }
        let group = self
            .groups
            .iter()
            .find(|g| g.panes.get(g.active) == Some(&def_id))?;
        let (from, to) = self.slot(group);
        Some(Band {
            h: group.strip.h,
            left: from - group.strip.x,
            right: group.strip.x + group.strip.w - to,
        })
    }

    /// Whether a panel holding `panes` shows them as tabs: several, or one
    /// that is a document.
    pub(super) fn shows_tabs(&self, panes: &[i32]) -> bool {
        panes.len() > 1 || panes.iter().any(|p| self.tabbed.contains(p))
    }

    /// The pane whose tab is under `(x, y)`, its close mark included.
    fn tab_at(&self, x: f64, y: f64) -> Option<i32> {
        match self.bar_at(x, y)? {
            (g, BarPart::Tab(i) | BarPart::TabClose(i)) => self.groups[g].panes.get(i).copied(),
            _ => None,
        }
    }

    /// **The pointer rests on `pane`'s tab, or on none**: a tab moved onto
    /// starts its wait, and the tip shown goes as the pointer leaves it.
    fn rest_on(&mut self, pane: Option<i32>) {
        if self.rest.map(|(p, _)| p) == pane {
            return;
        }
        self.rest = pane.map(|p| (p, None));
        if self.tip.take().is_some() {
            self.dirty.chrome = true;
        }
    }

    /// **Whether a tab's tip is waiting for its time** -- what keeps a
    /// front's clock coming back ([`Dock::elapsed`]).
    pub fn waiting(&self) -> bool {
        self.tip.is_none() && self.rest.is_some_and(|(p, _)| self.tips.contains_key(&p))
    }

    /// **The front's clock, at `now_ms`**: a tab rested on long enough shows
    /// its tip -- the wait a pane's own tips have. Answers whether it showed
    /// one now, for the front to draw.
    pub fn elapsed(&mut self, now_ms: f64) -> bool {
        if !self.waiting() {
            return false;
        }
        let Some((pane, since)) = self.rest else {
            return false;
        };
        match since {
            None => {
                self.rest = Some((pane, Some(now_ms)));
                false
            }
            Some(t) if now_ms - t >= TIP_MS => {
                self.tip = Some(pane);
                true
            }
            Some(_) => false,
        }
    }

    /// The tab whose tip shows, and what it says.
    pub(super) fn shown_tip(&self) -> Option<(i32, &str)> {
        let pane = self.tip?;
        self.tips.get(&pane).map(|t| (pane, t.as_str()))
    }

    /// **The middle button went down**: on a tab it closes that tab's
    /// window, as its close mark would, and answers `true`; anywhere else
    /// the press is a pane's.
    pub fn middle_pressed(&mut self) -> bool {
        let Some((x, y)) = self.at else {
            return false;
        };
        if self.menu.is_some() || self.grip.is_some() {
            return false;
        }
        match self.tab_at(x, y) {
            Some(pane) => {
                if self.closable(pane) {
                    self.dirty.close.push(pane);
                }
                true
            }
            None => false,
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
        self.edges.retain(|&(p, ..)| p != def_id);
        self.overlays.retain(|o| o.pane != def_id);
        self.tree.remove(def_id);
        for (_, tree) in &mut self.pages {
            tree.remove(def_id);
        }
        self.titles.remove(&def_id);
        self.tabbed.remove(&def_id);
        self.headed.remove(&def_id);
        self.fixed.remove(&def_id);
        self.modified.remove(&def_id);
        self.tips.remove(&def_id);
        if self.tip == Some(def_id) {
            self.tip = None;
        }
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
            .map(|g| self.pane_rect(g, def_id))
            .or_else(|| self.edge_rect(def_id))
    }

    /// Where pane `def_id` of `group` is drawn: its panel's body, or the
    /// whole panel for one that fills its header.
    fn pane_rect(&self, group: &GroupPlace, def_id: i32) -> PaneRect {
        if self.is_headed(def_id) {
            group.whole
        } else {
            group.body
        }
    }

    /// Every shown pane with its rectangle: the panels' in layout order,
    /// then those along the edges.
    pub fn placed(&self) -> impl Iterator<Item = (i32, PaneRect)> + '_ {
        self.groups
            .iter()
            .filter_map(|g| g.panes.get(g.active).map(|&p| (p, self.pane_rect(g, p))))
            .chain(self.edge_places.iter().copied())
    }

    /// The shown pane under the point `(x, y)` of the surface, and the point
    /// in its own pixels. A divider and a tab strip are in no pane; what a
    /// pane draws over the dock past its rectangle (an open list) is that
    /// pane's, wherever it lies.
    pub fn at(&self, x: f64, y: f64) -> Option<(i32, (f64, f64))> {
        let over = self.overlay_at(x, y);
        over.and_then(|id| self.rect(id).map(|r| (id, r)))
            .or_else(|| {
                // the bar's own parts are the dock's, over a pane drawn
                // under its panel's header
                if self.bar_at(x, y).is_some() {
                    return None;
                }
                self.placed().find(|(_, r)| r.contains(x, y))
            })
            .map(|(id, r)| (id, (x - f64::from(r.x), y - f64::from(r.y))))
    }

    /// The point `(x, y)` of the surface in pane `def_id`'s own pixels,
    /// wherever it is -- a drag that left the pane still reads it there.
    pub fn local(&self, def_id: i32, x: f64, y: f64) -> Option<(f64, f64)> {
        self.rect(def_id)
            .map(|r| (x - f64::from(r.x), y - f64::from(r.y)))
    }

    /// The pane the keyboard goes to: one whose list is open over the dock,
    /// which its keys walk, else the one last pressed in.
    pub fn focus(&self) -> Option<i32> {
        self.overlays
            .iter()
            .find(|o| o.keys)
            .or_else(|| self.overlays.iter().find(|o| o.dialog))
            .map(|o| o.pane)
            .or(self.focus)
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

    /// The divider lit: under the pointer, or dragged -- where it is now,
    /// which a drag moves.
    pub fn lit_divider(&self) -> Option<&DividerPlace> {
        match (&self.grip, &self.hot) {
            (Some(Grip::Divider { place, .. }), _) => self
                .dividers
                .iter()
                .find(|d| d.split == place.split && d.before == place.before),
            (None, Some(Hot::Divider(i))) => self.dividers.get(*i),
            _ => None,
        }
    }

    /// **The resize cursor the pointer wants**, over a divider or dragging
    /// one: `Some(true)` for a divider standing upright (dragged sideways),
    /// `Some(false)` for one lying across; `None` anywhere else, where the
    /// pointer is the platform's arrow.
    pub fn resize_cursor(&self) -> Option<bool> {
        if self.menu.is_some() {
            return None;
        }
        self.lit_divider().map(|d| d.row)
    }

    /// The part of group `path`'s bar lit under the pointer.
    pub fn lit_bar(&self, path: &[usize]) -> Option<BarPart> {
        match (&self.grip, &self.hot, &self.menu) {
            (None, Some(Hot::Bar(at, part)), None) if at == path => Some(*part),
            _ => None,
        }
    }

    /// The text size of the bars and the tabs: the interface's size of the
    /// type scale.
    pub fn text_size(&self) -> f32 {
        clausters_gui_draw::font::em_scale(
            clausters_gui_draw::font::Family::Ui,
            self.metrics.type_sm,
        )
    }

    /// **The pointer moved to `(x, y)`**: the pane a press holds, if one
    /// does, else the pane under it -- and the pane it left, which hears it
    /// left, as a window hears the pointer leave it. A divider or a tab the
    /// press holds moves instead, and an open menu lights its rows; no pane
    /// hears either.
    pub fn moved(&mut self, x: f64, y: f64) -> Motion {
        self.at = Some((x, y));
        if self.menu.is_some() {
            if self.menu_moved(x, y) {
                self.dirty.chrome = true;
            }
            return Motion {
                to: None,
                left: self.pointer.take(),
            };
        }
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
                group,
                from,
                dragging,
                landing,
            }) => {
                let dragging = dragging || (x - from.0).hypot(y - from.1) > DRAG_PX;
                let now = if dragging {
                    self.landing_at(pane, group, x, y)
                } else {
                    None
                };
                if now != landing {
                    self.dirty.chrome = true;
                }
                self.grip = Some(Grip::Tab {
                    pane,
                    group,
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
        self.rest_on(self.tab_at(x, y));
        let to = self.at(x, y);
        let now = to.map(|(id, _)| id);
        let left = self.pointer.filter(|&was| Some(was) != now);
        self.pointer = now;
        Motion { to, left }
    }

    /// **A button went down** where the pointer is: a pane under it takes it
    /// and the keyboard with it, and the answer is that pane. Everything
    /// else is the dock's and no pane hears it: an open menu takes the press
    /// (a row is picked, a press off it closes the menu), a page's tab shows
    /// its page, a divider is dragged, and a panel's bar is pressed -- its
    /// grip or its title to drag the panel, a tab to show its pane and drag
    /// it, a close mark, the menu button.
    pub fn pressed(&mut self) -> Option<i32> {
        if let Some(Grip::Pane(p)) = self.grip {
            return Some(p);
        }
        // a press is not a rest: a tab's tip goes, and waits again
        self.rest = None;
        if self.tip.take().is_some() {
            self.dirty.chrome = true;
        }
        let (x, y) = self.at?;
        if self.menu.is_some() {
            self.menu_pressed(x, y);
            return None;
        }
        if let Some(part) = self.page_at(x, y) {
            self.dismiss_lists(None);
            self.press_page(part);
            return None;
        }
        if let Some(i) = self.divider_at(x, y) {
            self.dismiss_lists(None);
            let place = self.dividers[i].clone();
            let from = if place.row { x } else { y };
            self.grip = Some(Grip::Divider { place, from });
            self.dirty.chrome = true;
            return None;
        }
        if let Some((group, part)) = self.bar_at(x, y) {
            self.dismiss_lists(None);
            self.press_bar(group, part, (x, y));
            self.dirty.chrome = true;
            return None;
        }
        let pane = self.pointer;
        self.dismiss_lists(pane);
        let pane = pane?;
        self.grip = Some(Grip::Pane(pane));
        // a pane along an edge is the application's bar or footer: a press
        // on it leaves the keyboard where the work is
        if self.edge_of(pane).is_none() {
            self.focus = Some(pane);
        }
        Some(pane)
    }

    /// **The secondary button went down**: on a panel's bar or a page's tab
    /// it opens that one's menu at the pointer, and answers `true` -- the
    /// dock took it; anywhere else it is a pane's, and the answer is `false`.
    pub fn context_pressed(&mut self) -> bool {
        let Some((x, y)) = self.at else {
            return false;
        };
        if self.menu.is_some() {
            self.close_menu();
            return true;
        }
        if self.grip.is_some() {
            return false;
        }
        if let Some(part) = self.page_at(x, y) {
            if let Part::Option(i) = part
                && i < self.page_names().len()
            {
                self.open_page_menu(i, (x as f32, y as f32));
            }
            return true;
        }
        if let Some((group, part)) = self.bar_at(x, y) {
            let pane = match part {
                BarPart::Tab(i) | BarPart::TabClose(i) => self.groups[group].panes[i],
                _ => self.groups[group].panes[self.groups[group].active],
            };
            self.open_pane_menu(pane, bar::MenuAt::Point(x as f32, y as f32));
            return true;
        }
        false
    }

    /// **Escape**: an open menu closes, and the answer says the dock took
    /// the key; otherwise it is the focused pane's.
    pub fn escape(&mut self) -> bool {
        self.close_menu()
    }

    /// **The button came up**: answers the pane the press held, which hears
    /// the release wherever the pointer is. A tab or a panel dragged
    /// somewhere lands there; a divider stays where it was left.
    pub fn released(&mut self) -> Option<i32> {
        match self.grip.take() {
            Some(Grip::Pane(p)) => Some(p),
            Some(Grip::Tab {
                pane,
                group,
                landing: Some((landing, _)),
                dragging: true,
                ..
            }) => {
                let panes = if group {
                    self.groups
                        .iter()
                        .find(|g| g.panes.contains(&pane))
                        .map(|g| g.panes.clone())
                        .unwrap_or_else(|| vec![pane])
                } else {
                    vec![pane]
                };
                if landing == Landing::Out {
                    self.dirty.out.extend(panes);
                } else {
                    let _ = self.land_all(&panes, pane, landing);
                }
                self.dirty.chrome = true;
                None
            }
            Some(_) => {
                self.dirty.chrome = true;
                None
            }
            None if self.menu.is_some() => None,
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

    /// **Moves `panes` -- a panel's, `shown` the one it shows -- where its
    /// grip would land**: the first goes there and the others join it, in
    /// their order, `shown` shown.
    pub fn land_all(&mut self, panes: &[i32], shown: i32, landing: Landing) -> Result<(), String> {
        let Some((&first, rest)) = panes.split_first() else {
            return Ok(());
        };
        if let Landing::Page(to) = landing {
            for &p in panes {
                self.move_to_page(p, to);
            }
            return Ok(());
        }
        self.land(first, landing)?;
        for &p in rest {
            match landing {
                Landing::Join { of, .. } => self.tree.join(p, of, None)?,
                _ => self.tree.join(p, first, None)?,
            }
        }
        self.tree.select(shown);
        self.focus = Some(shown);
        self.arrange();
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

    /// What the chrome lights at `(x, y)`.
    fn hot_at(&self, x: f64, y: f64) -> Option<Hot> {
        if let Some(part) = self.page_at(x, y) {
            return Some(Hot::Page(part));
        }
        if let Some(i) = self.divider_at(x, y) {
            return Some(Hot::Divider(i));
        }
        self.bar_at(x, y)
            .map(|(g, part)| Hot::Bar(self.groups[g].path.clone(), part))
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
    fn landing_at(&self, pane: i32, group: bool, x: f64, y: f64) -> Option<(Landing, Rect)> {
        let source = self.groups.iter().find(|g| g.panes.contains(&pane))?;
        let (w, h) = self.size;
        if x < 0.0 || y < 0.0 || x >= f64::from(w) || y >= f64::from(h) {
            return Some((Landing::Out, source.body.rect()));
        }
        if self.page_strip().contains(x, y) {
            return self.page_landing(x, y);
        }
        // a whole panel dragged by its grip leaves nothing behind
        let alone = group || source.panes.len() == 1;
        let group = self
            .groups
            .iter()
            .find(|g| g.strip.contains(x, y) || g.body.contains(x, y))?;
        let of = group.panes[group.active];
        let same = group.path == source.path;
        if group.strip.contains(x, y) {
            let at = self
                .bar_parts(group)
                .iter()
                .filter(|(part, _)| matches!(part, BarPart::Tab(_)))
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

    /// The height of a panel's bar: the panel header's role, never less
    /// than a line of control.
    fn strip_h(&self) -> u32 {
        self.metrics
            .panel_h
            .max(control_box(self.text_size(), &self.metrics))
            .round() as u32
    }

    /// **The height of the page strip**: a line of control, and none while
    /// there is one page to show -- a strip that offers no choice is a
    /// band of dead room -- or in a floating panel. A second page is made
    /// from a panel's menu, and the strip appears with it.
    pub(super) fn page_strip_h(&self) -> u32 {
        if self.floating || self.pages.len() <= 1 {
            0
        } else {
            control_box(self.text_size(), &self.metrics).round() as u32
        }
    }

    /// **The layout measured at this size**: every group's strip and body,
    /// every divider; and what that changed marked for the fronts.
    fn arrange(&mut self) {
        let mut groups = Vec::new();
        let mut dividers = Vec::new();
        let (w, h) = self.size;
        let divider = (self.metrics.divider_w.round() as u32).max(1);
        // the panes along the edges first, then the page's layout between
        // them, over the page strip
        let (top, bottom) = self.place_edges(divider);
        let strip = self.page_strip_h().min(h.saturating_sub(1 + top + bottom));
        let whole = PaneRect {
            x: 0,
            y: top,
            w,
            h: h.saturating_sub(top + bottom + strip),
        };
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
        // an empty dock (or page) has no panel, and so no bar to draw
        Node::Tabs(g) if g.panes.is_empty() => {}
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
                whole: PaneRect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w.max(1),
                    h: rect.h.max(1),
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
    /// What the panes draw over the dock, drawn over the chrome: a batch a
    /// layer, since a batch draws its glyphs after all of its shapes, and
    /// the chrome's letters must not show through a list, nor a dialog's
    /// through a list opened from it.
    floats: Vec<Painter>,
    target: Target,
}

impl Composer {
    /// A composer drawing into a single-sampled target of `format`.
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let target = Target::new(format);
        let chrome = Painter::new(device, target);
        Self {
            copier: Copier::new(device, target),
            shifts: Vec::new(),
            floats: Vec::new(),
            chrome,
            target,
        }
    }

    /// **Encodes the copy of `panes` into `target`**, the dock's
    /// surface or a texture of its size, cleared to `clear` first: each
    /// pane's texture into its rectangle, one texel to one pixel, and the
    /// shell's picture over them ([`Overlay`]). `dock` is the dock's GPU,
    /// whose configuration is the surface's size.
    pub(crate) fn compose(
        &mut self,
        dock: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clear: wgpu::Color,
        panes: &[(PaneRect, &wgpu::TextureView)],
        over: &Overlay,
    ) {
        let (device, queue) = (&dock.device, &dock.queue);
        let size = (dock.config.width, dock.config.height);
        self.chrome
            .upload(device, queue, &over.chrome, size.0.max(1), size.1.max(1));
        while self.floats.len() < over.floats.len() {
            let painter = Painter::sharing(device, self.target, &self.chrome);
            self.floats.push(painter);
        }
        for (painter, mesh) in self.floats.iter_mut().zip(&over.floats) {
            painter.upload(device, queue, mesh, size.0.max(1), size.1.max(1));
        }
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
        for painter in self.floats.iter().take(over.floats.len()) {
            painter.draw(&mut pass);
        }
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
        over: &Overlay,
    ) -> Result<Readback, String> {
        let size = (dock.config.width, dock.config.height);
        let texture = read_target(&dock.device, dock.config.format, size)?;
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = dock
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("dock capture"),
            });
        self.compose(dock, &mut encoder, &view, clear, panes, over);
        Ok(read_back(&dock.device, &dock.queue, encoder, &texture))
    }
}

#[cfg(test)]
mod tests;
