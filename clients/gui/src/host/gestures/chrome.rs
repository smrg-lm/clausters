//! The gestures of what a container shows of itself ([`crate::host::chrome`]):
//! folding a section by its title strip, moving a divider of a split strip,
//! dragging a plane's scroll bar -- and the one rule a dialog adds, that nothing
//! behind it can be reached.
//!
//! They run before the tree's own hit test for the reason the status bar does:
//! a title strip, a gap and a scroll bar are not widgets, so no widget is under
//! the pointer there and the press would otherwise fall through to whatever
//! the container's plan does with empty space.

use clausters_core::osc::OscType;

use super::super::Host;
use super::super::chrome;
use super::super::layout::Rect;
use super::effects::{emit, redraws};
use super::nav::{scroll_view, set_scroll_view};
use super::{Drag, GestureCtx, GestureEffect, Gestures};

/// The tag a folded or unfolded section reports: `"collapsed" <1|0>`.
pub const COLLAPSED: &str = "collapsed";

/// The tag a moved divider reports: `"split"` and the size of each child of
/// the strip along it, in logical pixels.
pub const SPLIT: &str = "split";

/// What a divider drag holds from its press.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Split {
    /// The container whose strip it is.
    pub container: i32,
    /// The child before the divider, counted among the strip's flow children.
    pub before: usize,
    pub row: bool,
    /// Where the pointer was at the press, along the strip.
    pub origin: f64,
    /// Every flow child's extent along the strip at the press, in pixels.
    pub sizes: Vec<f32>,
    /// Pixels per logical unit in the strip, so a size can be written back as
    /// the prop it is.
    pub unit: f32,
    /// The least a neighbour is left with.
    pub least: f32,
}

/// What a scroll-bar drag holds from its press.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct BarDrag {
    pub id: i32,
    pub area: Rect,
    pub vertical: bool,
    /// Where the pointer was at the press, along the bar.
    pub origin: f64,
    /// Where the view stood at the press, on the bar's axis.
    pub start: f64,
    /// Content units one pixel of travel is worth.
    pub per_px: f64,
}

/// The sizes a split strip's children have after the divider travelled
/// `travel` pixels from where it was pressed.
fn moved(split: &Split, travel: f32) -> Vec<f32> {
    let mut sizes = split.sizes.clone();
    let (a, b) = chrome::split(
        sizes[split.before],
        sizes[split.before + 1],
        travel,
        split.least,
    );
    sizes[split.before] = a;
    sizes[split.before + 1] = b;
    sizes
}

/// Writes `sizes` into the strip's children as the props they are: a child
/// with a fixed size along the strip keeps a fixed size, and every other one
/// takes its extent as its `weight` -- so the shares the layout hands out are
/// exactly these, and they stay proportional when the window is resized.
///
/// Only the two neighbours of the divider, and the children that were already
/// sharing the leftover, are written: a child sized by its own nature (a
/// button, a label) beside a divider nobody dragged keeps its nature.
fn write(host: &mut Host, def_id: i32, split: &Split, sizes: &[f32]) {
    let metrics = *host.metrics_for(def_id);
    let Some(container) = host
        .window_def_mut(def_id)
        .and_then(|tree| tree.find_mut(split.container))
    else {
        return;
    };
    let unit = split.unit.max(0.01);
    let kids = container.children.iter_mut().filter(|c| !c.kind.is_modal());
    for (i, (child, size)) in kids.zip(sizes).enumerate() {
        let neighbour = i == split.before || i == split.before + 1;
        let fixed = if split.row {
            &mut child.place.w
        } else {
            &mut child.place.h
        };
        if fixed.is_some() {
            if neighbour {
                *fixed = Some(size / unit);
                child.place.split_moved = true;
            }
            continue;
        }
        let natural = child.natural_size(&metrics, unit);
        let elastic =
            child.place.weight.is_some() || if split.row { natural.0 } else { natural.1 }.is_none();
        if neighbour || elastic {
            child.place.weight = Some(*size);
            child.place.split_moved |= neighbour;
        }
    }
}

impl Gestures {
    /// Whether a dialog is up and `(cx, cy)` is **behind** it: a press there
    /// reaches nothing, since nothing behind a dialog can be reached.
    pub(super) fn behind_dialog(&self, host: &Host, ctx: &GestureCtx, cx: f64, cy: f64) -> bool {
        host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)
            .and_then(|placed| chrome::modal_rect(&placed))
            .is_some_and(|dialog| !dialog.contains(cx, cy))
    }

    /// A press on a container's own chrome: a title strip that folds, a scroll
    /// bar, a divider. Returns whether it was one -- the tree then never sees
    /// the press.
    pub(super) fn chrome_press(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        out: &mut Vec<GestureEffect>,
    ) -> bool {
        let def_id = ctx.def_id;
        let metrics = *host.metrics_for(def_id);
        // Read what is under the pointer while the placements are in hand, and
        // act once they are let go of: a gesture mutates the tree they borrow.
        enum Found {
            Fold(i32, bool),
            Bar(i32, Rect, chrome::Bar, f64),
            Split(Split),
        }
        let found = {
            let Some(placed) = host.layout_window(def_id, ctx.fb_w, ctx.fb_h) else {
                return false;
            };
            // With a dialog up, only the dialog's own chrome is in reach.
            let from = chrome::modal_start(&placed).unwrap_or(0);
            if let Some((id, folded)) = chrome::fold_at(&placed[from..], cx, cy) {
                Some(Found::Fold(id, folded))
            } else if let Some((id, area, bar)) = chrome::bar_at(&placed[from..], &metrics, cx, cy)
            {
                let start = scroll_view(host, def_id, id)
                    .map_or(0.0, |v| if bar.vertical { v.view_y } else { v.view_x });
                Some(Found::Bar(id, area, bar, start))
            } else {
                chrome::divider_at(&placed, cx, cy, metrics.hit_slop)
                    .filter(|d| d.container >= from)
                    .and_then(|d| {
                        let kids = chrome::flow_children(&placed, d.container);
                        let sizes = kids
                            .iter()
                            .map(|&k| {
                                let r = placed[k].rect;
                                if d.row { r.w } else { r.h }
                            })
                            .collect();
                        Some(Found::Split(Split {
                            container: placed[d.container].widget.id?,
                            before: d.before,
                            row: d.row,
                            origin: if d.row { cx } else { cy },
                            sizes,
                            // The children's own scale: what a declared length
                            // is multiplied by where they stand.
                            unit: placed[*kids.first()?].scale,
                            least: metrics.control_h,
                        }))
                    })
            }
        };
        match found {
            None => false,
            Some(Found::Fold(id, folded)) => {
                // The state is a prop, written where a `/gui_set` writes it, and
                // reported so whoever keeps the session knows it moved.
                let mut effects = Vec::new();
                host.set_props(
                    id,
                    vec![(COLLAPSED.into(), serde_json::Value::from(!folded))],
                    &mut effects,
                );
                redraws(out, effects);
                emit(
                    host,
                    out,
                    def_id,
                    id,
                    vec![
                        OscType::String(COLLAPSED.into()),
                        OscType::Int(i32::from(!folded)),
                    ],
                );
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            Some(Found::Bar(id, area, bar, start)) => {
                let at = if bar.vertical { cy } else { cx };
                let (thumb_at, thumb_len) = if bar.vertical {
                    (bar.thumb.y as f64, bar.thumb.h as f64)
                } else {
                    (bar.thumb.x as f64, bar.thumb.w as f64)
                };
                // On the groove, off the thumb: the thumb comes to the pointer,
                // and the drag carries on from there.
                let start = if at < thumb_at || at > thumb_at + thumb_len {
                    start + (at - (thumb_at + thumb_len * 0.5)) * bar.per_px
                } else {
                    start
                };
                let held = BarDrag {
                    id,
                    area,
                    vertical: bar.vertical,
                    origin: at,
                    start,
                    per_px: bar.per_px,
                };
                self.bar_to(host, ctx, &held, at, out);
                self.drag = Some(Drag::Bar(held));
                out.push(GestureEffect::Redraw(def_id));
                true
            }
            Some(Found::Split(split)) => {
                self.drag = Some(Drag::Split(split));
                true
            }
        }
    }

    /// One step of a scroll-bar drag: the view follows the pointer absolutely
    /// from the press, on the bar's axis alone.
    pub(super) fn bar_to(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        bar: &BarDrag,
        at: f64,
        out: &mut Vec<GestureEffect>,
    ) {
        let Some(view) = scroll_view(host, ctx.def_id, bar.id) else {
            return;
        };
        let to = bar.start + (at - bar.origin) * bar.per_px;
        let zoom = view.zoom(host.metrics_for(ctx.def_id));
        let next = if bar.vertical {
            (view.view_x, to, zoom)
        } else {
            (to, view.view_y, zoom)
        };
        set_scroll_view(host, out, ctx.def_id, bar.id, bar.area, next);
    }

    /// One step of a divider drag: the two neighbours trade room, live.
    pub(super) fn split_to(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        split: &Split,
        at: f64,
        out: &mut Vec<GestureEffect>,
    ) {
        let sizes = moved(split, (at - split.origin) as f32);
        write(host, ctx.def_id, split, &sizes);
        out.push(GestureEffect::Redraw(ctx.def_id));
    }

    /// The divider was let go of: the strip reports where it left its
    /// children, each one's extent along it in logical pixels.
    pub(super) fn split_done(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        split: &Split,
        at: f64,
        out: &mut Vec<GestureEffect>,
    ) {
        let sizes = moved(split, (at - split.origin) as f32);
        write(host, ctx.def_id, split, &sizes);
        let unit = split.unit.max(0.01);
        let mut args = vec![OscType::String(SPLIT.into())];
        args.extend(sizes.iter().map(|s| OscType::Float(s / unit)));
        emit(host, out, ctx.def_id, split.container, args);
        out.push(GestureEffect::Redraw(ctx.def_id));
    }
}
