//! **What a container shows of itself**: a group's title strip and frame, the
//! dividers of a split strip, a plane's scroll bars, and where a dialog begins
//! in a window's placements.
//!
//! None of these is a widget. A title belongs to the group it names, a divider
//! to the two children it stands between, a scroll bar to the plane it
//! measures -- so they are props of the containers there already are, and this
//! module is their **geometry**: one function per part, read by the frame to
//! draw it and by the gesture machine to hit it, so a divider is grabbed where
//! it is drawn.

use super::layout::{self, Placed, Rect};
use super::metrics::Metrics;
use super::scroll;
use super::widget::{Axis, Layout, WidgetKind};

/// The title strip of a group: the top of its rectangle, one line of control
/// tall. `None` for a container with no title.
pub fn title_strip(p: &Placed) -> Option<Rect> {
    let WidgetKind::Panel { group, .. } = &p.widget.kind else {
        return None;
    };
    let h = group.strip_h(&p.metrics).min(p.rect.h);
    (h > 0.0).then(|| Rect::new(p.rect.x, p.rect.y, p.rect.w, h))
}

/// The group whose title strip is under `(x, y)` and **folds**: its id, and
/// whether it is folded now. A plain titled group has a strip and no fold.
pub fn fold_at(placed: &[Placed], x: f64, y: f64) -> Option<(i32, bool)> {
    placed.iter().rev().find_map(|p| {
        let strip = title_strip(p)?;
        let WidgetKind::Panel { group, .. } = &p.widget.kind else {
            return None;
        };
        let inside = strip.contains(x, y) && p.clip.is_none_or(|c| c.contains(x, y));
        (inside && p.widget.live).then_some((p.widget.id?, group.collapsed?))
    })
}

/// A divider of a split strip: the gap between two neighbouring children, which
/// a drag moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divider {
    /// The container, as an index into the placements.
    pub container: usize,
    /// The child before the gap, counted among the container's flow children.
    pub before: usize,
    /// The gap itself.
    pub rect: Rect,
    /// Whether the strip is a row -- the divider then stands upright and is
    /// dragged sideways.
    pub row: bool,
}

/// The flow children of placement `container`, in order, as indices into
/// `placed`: every placement that hangs off it, its dialogs left out.
pub fn flow_children(placed: &[Placed], container: usize) -> Vec<usize> {
    placed
        .iter()
        .enumerate()
        .filter(|(_, p)| p.parent == Some(container) && !p.widget.kind.is_modal())
        .map(|(i, _)| i)
        .collect()
}

/// Every divider of every split strip in the window.
pub fn dividers(placed: &[Placed]) -> Vec<Divider> {
    let mut out = Vec::new();
    for (i, p) in placed.iter().enumerate() {
        let (layout, flow) = match &p.widget.kind {
            WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow, .. } => {
                (*layout, *flow)
            }
            _ => continue,
        };
        let row = match layout {
            Layout::Row => true,
            Layout::Col => false,
            _ => continue,
        };
        if !flow.split {
            continue;
        }
        let kids = flow_children(placed, i);
        for (before, pair) in kids.windows(2).enumerate() {
            let (a, b) = (placed[pair[0]].rect, placed[pair[1]].rect);
            let rect = if row {
                Rect::new(a.x + a.w, a.y, (b.x - a.x - a.w).max(0.0), a.h)
            } else {
                Rect::new(a.x, a.y + a.h, a.w, (b.y - a.y - a.h).max(0.0))
            };
            out.push(Divider {
                container: i,
                before,
                rect,
                row,
            });
        }
    }
    out
}

/// The divider under `(x, y)`, with `slop` of air along the axis it is dragged
/// on -- a gap is a few pixels wide, and a hand does not hunt for it.
pub fn divider_at(placed: &[Placed], x: f64, y: f64, slop: f32) -> Option<Divider> {
    dividers(placed).into_iter().rev().find(|d| {
        let grown = if d.row {
            Rect::new(d.rect.x - slop, d.rect.y, d.rect.w + 2.0 * slop, d.rect.h)
        } else {
            Rect::new(d.rect.x, d.rect.y - slop, d.rect.w, d.rect.h + 2.0 * slop)
        };
        grown.contains(x, y) && placed[d.container].widget.live
    })
}

/// The line a divider is drawn as: a hairline down the middle of its gap.
pub fn divider_line(d: &Divider, m: &Metrics) -> Rect {
    if d.row {
        Rect::new(
            d.rect.x + (d.rect.w - m.divider_w) * 0.5,
            d.rect.y,
            m.divider_w,
            d.rect.h,
        )
    } else {
        Rect::new(
            d.rect.x,
            d.rect.y + (d.rect.h - m.divider_w) * 0.5,
            d.rect.w,
            m.divider_w,
        )
    }
}

/// **Where a moved divider leaves its two neighbours**: `a` and `b` are their
/// extents along the strip at the press, `travel` how far the pointer has gone
/// since, and neither is taken under `least`. The pair keeps its sum, so what
/// one gains the other gives.
pub fn split(a: f32, b: f32, travel: f32, least: f32) -> (f32, f32) {
    let total = a + b;
    let least = least.min(total * 0.5).max(0.0);
    let to = (a + travel).clamp(least, total - least);
    (to, total - to)
}

/// One scroll bar of a plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// Whether it runs down the plane's right edge (and scrolls `view_y`).
    pub vertical: bool,
    /// The groove the thumb travels in.
    pub track: Rect,
    /// The thumb: where the view stands in the content, and how much of it the
    /// view shows.
    pub thumb: Rect,
    /// Content units one pixel of thumb travel is worth.
    pub per_px: f64,
}

/// The scroll bars of plane `p`: one along each axis it pans on which the
/// content is larger than the view. Empty for a plane without `bars`.
pub fn bars(p: &Placed, metrics: &Metrics) -> Vec<Bar> {
    let WidgetKind::Scroll { view, .. } = &p.widget.kind else {
        return Vec::new();
    };
    if !view.bars {
        return Vec::new();
    }
    let area = p.rect;
    let zoom = view.zoom(metrics);
    let (content_w, content_h) = layout::scroll_content(p.widget, area, metrics);
    let slack = view.axis.slack();
    let vx = scroll::clamp_pan(view.view_x, area.w, zoom, content_w, slack);
    let vy = scroll::clamp_pan(view.view_y, area.h, zoom, content_h, slack);
    let thick = metrics.handle_thick.min(area.w).min(area.h);
    let (seen_w, seen_h) = (area.w as f64 / zoom, area.h as f64 / zoom);
    let wants_x = !matches!(view.axis, Axis::Y) && content_w as f64 > seen_w + 0.5;
    let wants_y = !matches!(view.axis, Axis::X) && content_h as f64 > seen_h + 0.5;
    let mut out = Vec::new();
    // The thumb's place along a track: its start and its length, as fractions
    // of the content, never shorter than a grip a hand can take.
    let thumb = |start: f64, seen: f64, content: f64, len: f32| {
        let size = ((seen / content) as f32 * len)
            .max(metrics.handle_grip.min(len))
            .min(len);
        let travel = (len - size).max(0.0);
        let span = (content - seen).max(f64::MIN_POSITIVE);
        let at = (start / span).clamp(0.0, 1.0) as f32 * travel;
        (at, size, span / travel.max(1.0) as f64)
    };
    if wants_y {
        // The corner both bars would meet in is left to neither.
        let len = area.h - if wants_x { thick } else { 0.0 };
        let track = Rect::new(area.x + area.w - thick, area.y, thick, len);
        let (at, size, per_px) = thumb(vy, seen_h, content_h as f64, len);
        out.push(Bar {
            vertical: true,
            track,
            thumb: Rect::new(track.x, track.y + at, thick, size),
            per_px,
        });
    }
    if wants_x {
        let len = area.w - if wants_y { thick } else { 0.0 };
        let track = Rect::new(area.x, area.y + area.h - thick, len, thick);
        let (at, size, per_px) = thumb(vx, seen_w, content_w as f64, len);
        out.push(Bar {
            vertical: false,
            track,
            thumb: Rect::new(track.x + at, track.y, size, thick),
            per_px,
        });
    }
    out
}

/// The scroll bar under `(x, y)`: the plane's id, its area, and the bar.
pub fn bar_at(placed: &[Placed], metrics: &Metrics, x: f64, y: f64) -> Option<(i32, Rect, Bar)> {
    placed.iter().rev().find_map(|p| {
        if !p.widget.live || !p.clip.is_none_or(|c| c.contains(x, y)) {
            return None;
        }
        let bar = bars(p, metrics)
            .into_iter()
            .find(|b| b.track.contains(x, y))?;
        Some((p.widget.id?, p.rect, bar))
    })
}

/// Where the **dialog** begins in a window's placements: the index of the
/// first placement of a `layout` that carries `modal`. Everything from there
/// on is the dialog, since the layout places its dialogs last.
pub fn modal_start(placed: &[Placed]) -> Option<usize> {
    placed.iter().position(|p| p.widget.kind.is_modal())
}

/// The rectangle of the window's dialog, when one is up.
pub fn modal_rect(placed: &[Placed]) -> Option<Rect> {
    modal_start(placed).map(|i| placed[i].rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;
    use crate::host::widget::Widget;

    fn tree(json: &str) -> Widget {
        let node: GuiNode = serde_json::from_str(json).unwrap();
        Widget::from_node(1, &node, &[]).unwrap()
    }

    fn area() -> Rect {
        Rect::new(0.0, 0.0, 400.0, 300.0)
    }

    #[test]
    fn a_split_row_has_a_divider_in_each_gap_and_a_plain_row_has_none() {
        let m = Metrics::default();
        let split = tree(
            r#"{"type":"window","margin":0,"gap":6,"flow":"row","split":true,"children":[
                {"id":2,"type":"label","text":"a"},
                {"id":3,"type":"label","text":"b"},
                {"id":4,"type":"label","text":"c"}]}"#,
        );
        let placed = layout::layout(area(), &split, &m);
        let d = dividers(&placed);
        assert_eq!(d.len(), 2);
        assert!(d[0].row && d[0].rect.w == 6.0 && d[0].rect.h == 300.0);
        assert_eq!(d[0].before, 0);
        let mid = (d[1].rect.x + 3.0) as f64;
        assert_eq!(divider_at(&placed, mid, 100.0, 0.0), Some(d[1]));
        // A hand a little off the gap still takes it, along the drag's axis.
        assert_eq!(
            divider_at(&placed, (d[1].rect.x - 3.0) as f64, 100.0, m.hit_slop),
            Some(d[1])
        );
        let plain = tree(
            r#"{"type":"window","flow":"row","children":[
                {"id":2,"type":"label","text":"a"},{"id":3,"type":"label","text":"b"}]}"#,
        );
        assert!(dividers(&layout::layout(area(), &plain, &m)).is_empty());
    }

    #[test]
    fn a_moved_divider_trades_room_and_takes_neither_side_under_the_least() {
        assert_eq!(split(100.0, 100.0, 30.0, 20.0), (130.0, 70.0));
        assert_eq!(split(100.0, 100.0, -500.0, 20.0), (20.0, 180.0));
        assert_eq!(split(100.0, 100.0, 500.0, 20.0), (180.0, 20.0));
        // Two cells too small for the least still split what they have.
        assert_eq!(split(10.0, 10.0, 0.0, 20.0), (10.0, 10.0));
    }

    #[test]
    fn a_titled_group_reserves_its_strip_and_a_folded_one_places_nothing() {
        let m = Metrics::default();
        let open = tree(
            r#"{"type":"window","margin":0,"flow":"col","children":[
                {"id":2,"type":"layout","title":"Filter","collapsed":false,"margin":0,
                 "children":[{"id":3,"type":"label","text":"inside"}]}]}"#,
        );
        let placed = layout::layout(area(), &open, &m);
        let group = placed.iter().find(|p| p.widget.id == Some(2)).unwrap();
        let strip = title_strip(group).expect("it has a title");
        assert_eq!(strip.h, m.control_h);
        let child = placed.iter().find(|p| p.widget.id == Some(3)).unwrap();
        assert_eq!(
            child.rect.y,
            strip.y + strip.h,
            "the content starts under it"
        );
        assert_eq!(fold_at(&placed, 10.0, 4.0), Some((2, false)));
        assert_eq!(fold_at(&placed, 10.0, 200.0), None, "under the strip");

        let folded = tree(
            r#"{"type":"window","margin":0,"flow":"col","children":[
                {"id":2,"type":"layout","title":"Filter","collapsed":true,
                 "children":[{"id":3,"type":"label","text":"inside"}]},
                {"id":4,"type":"label","text":"after"}]}"#,
        );
        let placed = layout::layout(area(), &folded, &m);
        assert!(placed.iter().all(|p| p.widget.id != Some(3)));
        let group = placed.iter().find(|p| p.widget.id == Some(2)).unwrap();
        assert_eq!(
            group.rect.h, m.control_h,
            "it is its strip and nothing else"
        );
    }

    #[test]
    fn a_plain_title_does_not_fold() {
        let m = Metrics::default();
        let t = tree(
            r#"{"type":"window","margin":0,"children":[
                {"id":2,"type":"layout","title":"Group","children":[]}]}"#,
        );
        assert_eq!(fold_at(&layout::layout(area(), &t, &m), 10.0, 4.0), None);
    }

    #[test]
    fn a_plane_shows_a_bar_only_where_its_content_is_larger_than_the_view() {
        let m = Metrics::default();
        let t = tree(
            r#"{"type":"window","margin":0,"children":[
                {"id":2,"type":"plane","bars":true,"zoom":false,
                 "content_w":300,"content_h":1200,"children":[]}]}"#,
        );
        let placed = layout::layout(area(), &t, &m);
        let plane = placed.iter().find(|p| p.widget.id == Some(2)).unwrap();
        let b = bars(plane, &m);
        assert_eq!(b.len(), 1, "only the axis that overflows");
        assert!(b[0].vertical);
        assert_eq!(b[0].thumb.y, b[0].track.y, "the view starts at the top");
        assert!((b[0].thumb.h - 300.0 * 300.0 / 1200.0).abs() < 1.0);
        // The thumb's whole travel is the content the view cannot see.
        let travel = (b[0].track.h - b[0].thumb.h) as f64;
        assert!((b[0].per_px * travel - 900.0).abs() < 1.0);
        assert!(bar_at(&placed, &m, 398.0, 100.0).is_some());
        assert!(bar_at(&placed, &m, 200.0, 100.0).is_none());
    }

    #[test]
    fn a_dialog_is_placed_last_centred_and_out_of_its_parents_flow() {
        let m = Metrics::default();
        let t = tree(
            r#"{"type":"window","margin":0,"flow":"col","children":[
                {"id":2,"type":"label","text":"work","weight":1},
                {"id":3,"type":"layout","modal":true,"w":200,"h":100,"children":[
                    {"id":4,"type":"button","label":"OK"}]},
                {"id":5,"type":"label","text":"more work","weight":1}]}"#,
        );
        let placed = layout::layout(area(), &t, &m);
        let start = modal_start(&placed).expect("there is a dialog");
        assert_eq!(placed[start].widget.id, Some(3));
        assert!(
            placed[start..]
                .iter()
                .all(|p| matches!(p.widget.id, Some(3 | 4))),
            "everything after it is the dialog"
        );
        assert_eq!(
            modal_rect(&placed),
            Some(Rect::new(100.0, 100.0, 200.0, 100.0))
        );
        // The two labels share the window as if the dialog were not there.
        let work = placed.iter().find(|p| p.widget.id == Some(2)).unwrap();
        let more = placed.iter().find(|p| p.widget.id == Some(5)).unwrap();
        assert_eq!(work.rect.h + more.rect.h + m.gap, 300.0);
    }
}
