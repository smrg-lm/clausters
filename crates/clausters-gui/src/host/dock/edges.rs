//! **The dock's edges, and what floats over it.**
//!
//! An application's bar and its footer are not panels: no application puts
//! its menu in one of its panes, and a status line belongs to the window, not
//! to whatever panel happens to be under it. A window that names an edge (its
//! `dock_edge`) is placed **along that edge of the dock**, across its whole
//! width, on every page, outside the panels: no bar of its own, no tab, never
//! dragged, at the height the window declares. A press on it leaves the
//! keyboard where the work is.
//!
//! A pane's popups are not held inside its rectangle either. The lists a
//! pane opens (a menu bar's, a chooser's, a context menu's), its tips and its
//! notes are placed against the **whole dock** ([`Dock::popup_area`]), drawn
//! by the pane's frame into a batch of their own rather than into its texture,
//! and joined to the dock's picture over every pane ([`Dock::picture`]). What
//! they cover is the pane's for the hand: the pointer there reaches it, in its
//! own pixels, and a press anywhere else takes its open list down, as a press
//! beside a list does in a window of its own.

use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::paint::Mesh;
use clausters_gui_draw::rect::Rect;
use clausters_gui_draw::theme::Theme;

use super::{Dock, PaneRect};

/// An edge of the dock a pane may stand along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// Along the top: an application's bar.
    Top,
    /// Along the bottom, under the page strip: an application's footer.
    Bottom,
}

impl Edge {
    /// The edge a `dock_edge` prop names.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    /// Its name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }
}

/// **What a shell draws over its panes**: its chrome, and over that what
/// the panes float over it, as two batches.
pub struct Overlay {
    pub chrome: Mesh,
    /// What the panes float over the dock, one batch a layer
    /// ([`FLOAT_LAYERS`](crate::host::frame::FLOAT_LAYERS)), every pane's
    /// in each.
    pub floats: Vec<Mesh>,
}

/// **What one pane floats over the dock**: its layers in its own pixels,
/// whether a list the keyboard walks is open in it, and whether a dialog is
/// up in it.
pub struct PaneFloat<'a> {
    pub pane: i32,
    pub layers: &'a [Mesh],
    pub keys: bool,
    pub dialog: bool,
}

/// What one pane draws over the dock, as the last picture placed it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Float {
    pub pane: i32,
    /// What it covers, in the surface's pixels.
    pub rect: Rect,
    /// Whether it is a list the keyboard walks (a menu, a chooser), which
    /// takes the keys and is taken down by a press elsewhere -- a tip or a
    /// note does neither.
    pub keys: bool,
    /// Whether it is a dialog, which takes the keys and every press over
    /// the dock while it is up, and which a press does not take down.
    pub dialog: bool,
}

impl Dock {
    /// **Puts `def_id` along `edge`**, `height` device pixels high, or
    /// changes the height of one already there. Answers whether it was not a
    /// pane already.
    pub fn add_edge(&mut self, def_id: i32, edge: Edge, height: u32) -> bool {
        if let Some(at) = self.edges.iter_mut().find(|(id, ..)| *id == def_id) {
            if (at.1, at.2) != (edge, height) {
                *at = (def_id, edge, height);
                self.arrange();
            }
            return false;
        }
        if self.holds(def_id) {
            return false;
        }
        self.edges.push((def_id, edge, height));
        self.arrange();
        true
    }

    /// The edge pane `def_id` stands along, if it is one.
    pub fn edge_of(&self, def_id: i32) -> Option<Edge> {
        self.edges
            .iter()
            .find(|(id, ..)| *id == def_id)
            .map(|&(_, edge, _)| edge)
    }

    /// Where the edge pane `def_id` is.
    pub(super) fn edge_rect(&self, def_id: i32) -> Option<PaneRect> {
        self.edge_places
            .iter()
            .find(|(id, _)| *id == def_id)
            .map(|&(_, r)| r)
    }

    /// The edge panes as measured, with the edge each stands along.
    pub(super) fn edge_placed(&self) -> impl Iterator<Item = (i32, Edge, PaneRect)> + '_ {
        self.edge_places
            .iter()
            .filter_map(|&(id, r)| self.edge_of(id).map(|e| (id, e, r)))
    }

    /// What the panes along `edge` take of the dock's height, a divider
    /// after each.
    pub(super) fn edges_h(&self, edge: Edge) -> u32 {
        let divider = (self.metrics.divider_w.round() as u32).max(1);
        self.edges
            .iter()
            .filter(|(_, e, _)| *e == edge)
            .map(|&(_, _, h)| h + divider)
            .sum::<u32>()
            .min(self.size.1)
    }

    /// **Measures the edge panes**: the top ones stacked from the top, the
    /// bottom ones from the bottom, a divider between each and what it
    /// borders. Answers what the two edges take.
    pub(super) fn place_edges(&mut self, divider: u32) -> (u32, u32) {
        let (w, h) = self.size;
        let (mut top, mut bottom) = (0u32, 0u32);
        let mut places = Vec::with_capacity(self.edges.len());
        for &(id, edge, height) in &self.edges {
            let room = h.saturating_sub(top + bottom + divider);
            let height = height.min(room);
            let y = match edge {
                Edge::Top => top,
                Edge::Bottom => h.saturating_sub(bottom + height),
            };
            places.push((
                id,
                PaneRect {
                    x: 0,
                    y,
                    w,
                    h: height,
                },
            ));
            match edge {
                Edge::Top => top += height + divider,
                Edge::Bottom => bottom += height + divider,
            }
        }
        if places != self.edge_places {
            self.dirty.layout = true;
        }
        self.edge_places = places;
        (top.min(h), bottom.min(h))
    }

    /// **Where pane `def_id`'s popups may go**, in its own pixels: the whole
    /// dock, so a list may cross into the next pane and a bar's list hang
    /// below the bar.
    pub fn popup_area(&self, def_id: i32) -> Option<Rect> {
        let r = self.rect(def_id)?;
        let (w, h) = self.size;
        Some(Rect::new(-(r.x as f32), -(r.y as f32), w as f32, h as f32))
    }

    /// The pane whose popups cover the point `(x, y)` of the surface: the
    /// one a list the keyboard walks belongs to first.
    pub(super) fn overlay_at(&self, x: f64, y: f64) -> Option<i32> {
        let mut over = self.overlays.iter().filter(|o| o.rect.contains(x, y));
        let first = over.clone().find(|o| o.keys);
        let dialog = over.clone().find(|o| o.dialog);
        first.or(dialog).or_else(|| over.next()).map(|o| o.pane)
    }

    /// **A press that went to `to`** (a pane, or the dock's chrome with
    /// `None`): every other pane's open list goes down, as a press beside a
    /// list does in a window of its own. The pane pressed handles its own.
    pub(super) fn dismiss_lists(&mut self, to: Option<i32>) {
        let gone: Vec<i32> = self
            .overlays
            .iter()
            .filter(|o| o.keys && Some(o.pane) != to)
            .map(|o| o.pane)
            .collect();
        if gone.is_empty() {
            return;
        }
        self.overlays.retain(|o| !gone.contains(&o.pane));
        self.dirty.dismiss.extend(gone);
    }

    /// **The dock's picture over its panes**: its own chrome, then what each
    /// shown pane draws over the dock (`floats`: the pane, its batch in its
    /// own pixels, and whether a list the keyboard walks is open in it),
    /// moved to where the pane is. What each covers is kept, so the hand
    /// finds it where it was drawn.
    pub fn picture(&mut self, theme: &Theme, floats: &[PaneFloat<'_>]) -> Overlay {
        let mut chrome = Mesh::new();
        let metrics = *self.metrics();
        self.draw(&mut Draw::new(&mut chrome, &metrics, theme));
        let layers = floats.iter().map(|f| f.layers.len()).max().unwrap_or(0);
        let mut meshes: Vec<Mesh> = (0..layers).map(|_| Mesh::new()).collect();
        let mut overlays = Vec::new();
        for float in floats {
            if float.layers.iter().all(Mesh::is_empty) || !self.shown(float.pane) {
                continue;
            }
            let Some(r) = self.rect(float.pane) else {
                continue;
            };
            let (dx, dy) = (r.x as f32, r.y as f32);
            let bounds: Vec<[f32; 4]> = float
                .layers
                .iter()
                .flat_map(|l| l.triangle_bounds())
                .collect();
            let x0 = bounds.iter().map(|b| b[0]).fold(f32::MAX, f32::min);
            let y0 = bounds.iter().map(|b| b[1]).fold(f32::MAX, f32::min);
            let x1 = bounds.iter().map(|b| b[2]).fold(f32::MIN, f32::max);
            let y1 = bounds.iter().map(|b| b[3]).fold(f32::MIN, f32::max);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            for (into, layer) in meshes.iter_mut().zip(float.layers) {
                into.append_shifted(layer, dx, dy);
            }
            overlays.push(Float {
                pane: float.pane,
                rect: Rect::new(x0 + dx, y0 + dy, x1 - x0, y1 - y0),
                keys: float.keys,
                dialog: float.dialog,
            });
        }
        self.overlays = overlays;
        Overlay {
            chrome,
            floats: meshes,
        }
    }
}
