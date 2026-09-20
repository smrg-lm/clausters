//! **What is under a point**: the one layout pass that answers a pointer
//! question, and the per-element hit-tests that read the answer finer.
//!
//! [`hit`] lays the window out once and hands back the deepest interactive
//! widget plus the [`Frame`] chain of containers over it -- the containment the
//! layout already resolved ([`chain_of`]), each container's coordinate system
//! resolved with it ([`time_axis`], [`view_of`]). The second question -- *where
//! on* the element the point landed -- is the element's own, asked through the
//! trait.
//!
//! The rule that keeps these honest is that they reconstruct **the geometry the
//! renderer drew through**, never a parallel derivation of it: a note is grabbed
//! by the pixels it was drawn on.

use super::super::Host;
use super::super::layout::{self, Rect};
use super::super::widget::{GestureMap, WidgetKind};
use super::coords::{Coords, Frame, Hit, TimeAxis, YAxis};
use crate::viewport::View;

/// The [`Hit`] under `(x, y)` in window `def_id`. Containers (`window`/`panel`)
/// are not hit targets -- except `scroll`, whose empty area is the pan gesture's
/// surface (its children, laid out through its view transform, still win over
/// it). A widget scrolled out of its container's window (outside its clip) is
/// not hit. `fb_w`/`fb_h` is the window's framebuffer size in device pixels.
///
/// `lanes` answers how many channel lanes a stacked heavy view draws -- the one
/// datum the host tree does not hold (it lives in the front's GPU slots), and
/// the divisor a vertical axis is panned through.
///
/// [`Placed::scale`]: super::super::layout::Placed::scale
pub(crate) fn hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
    rows: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<Hit> {
    let placed = host.layout_window(def_id, fb_w, fb_h)?;
    let mut found = None;
    for (i, p) in placed.iter().enumerate() {
        if p.rect.contains(x, y)
            && p.clip.is_none_or(|c| c.contains(x, y))
            && p.widget.id.is_some()
            && !matches!(
                p.widget.kind,
                WidgetKind::Window { .. } | WidgetKind::Panel { .. } | WidgetKind::Stack { .. }
            )
        {
            found = Some(i);
        }
    }
    let i = found?;
    let p = placed[i];
    Some(Hit {
        id: p.widget.id?,
        rect: p.rect,
        scale: p.scale,
        indent: p.indent,
        kind: p.widget.kind.clone(),
        chain: chain_of(host, def_id, &placed, i, y, rows),
    })
}

/// The window's **one** navigation group, when it has exactly one: a member's
/// id and axis (they share the window and the gutter, so any member answers for
/// all of them), plus every **lane** on it.
///
/// It is what a gesture falls back to when the pointer is not over a timeline
/// at all -- the gap between two lanes, the slack under the last one, a
/// container's margin. In a window built around one axis those pixels are not a
/// third thing the user meant: they are the axis with nothing drawn on them.
/// With two groups there is no such answer, so there is no fallback either.
pub(crate) struct SoleAxis {
    pub id: i32,
    pub axis: TimeAxis,
}

pub(crate) fn sole_time_axis(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    rows: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<SoleAxis> {
    let placed = host.layout_window(def_id, fb_w, fb_h)?;
    let mut key = None;
    let mut found: Option<(i32, TimeAxis)> = None;
    for p in &placed {
        let (Some(id), true) = (p.widget.id, p.widget.is_timeline()) else {
            continue;
        };
        let Some(editor) = p.widget.kind.editor() else {
            continue;
        };
        let this = super::super::timeline::group_key(id, editor.link);
        match key {
            Some(k) if k != this => return None, // two axes: nothing to fall back to
            Some(_) => {}
            None => key = Some(this),
        }
        if found.is_none() {
            found = time_axis(host, def_id, p, p.indent, rows).map(|axis| (id, axis));
        }
    }
    let (id, axis) = found?;
    Some(SoleAxis { id, axis })
}

/// The containers from the window down to `i`, `i` itself included when it is
/// one -- walked back through [`super::super::layout::Placed::parent`], which is the containment the
/// layout pass already resolved.
fn chain_of(
    host: &Host,
    def_id: i32,
    placed: &[layout::Placed],
    i: usize,
    y: f64,
    rows: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Vec<Frame> {
    let mut chain = Vec::new();
    let mut at = Some(i);
    while let Some(j) = at {
        let p = placed[j];
        let coords = match &p.widget.kind {
            WidgetKind::Window { .. } | WidgetKind::Panel { .. } | WidgetKind::Stack { .. } => {
                Some(Coords::Layout)
            }
            WidgetKind::Scroll { view, .. } => Some(Coords::Plane(*view)),
            // Where the body begins is the **group's** call, not this widget's:
            // every member of one axis starts it at the same x, and the layout
            // already resolved it (`Placed::indent`).
            _ if p.widget.is_timeline() => {
                time_axis(host, def_id, &p, p.indent, rows).map(Coords::Time)
            }
            _ => None,
        };
        if let Some(coords) = coords {
            chain.push(Frame {
                id: p.widget.id,
                rect: p.rect,
                map: map_for(host, def_id, &p, y),
                ruler: on_ruler(host, def_id, &p, y),
                coords,
            });
        }
        at = p.parent;
    }
    chain.reverse();
    chain
}

/// **A ruler strip answers as a ruler, whoever drew it** -- the widget's own
/// table everywhere else.
///
/// A free-standing `timeruler` is a widget, so its table is its kind's and this
/// changes nothing for it. The ruler a *view* reserves out of its own height --
/// a lane's `ruler` prop, a roll's, a signal's -- is not a widget at all: the
/// press lands on the lane or on the element, and their table is the body's, so
/// without this a drag on the strip sweeps clips or notes and Alt never reaches
/// the time range. Two selections told apart by *where* the gesture began need
/// the where to be asked, and a [`GestureMap`] is asked by modifier alone.
///
/// It is one band, because every view reserves it the same way: the bottom
/// `Metrics::ruler_h` of the widget's rect, when its editor has a ruler on
/// (`lane_body`, `pianoroll::regions` and `timeline_body` all take it off the
/// bottom). Read from the same numbers the drawing reserves it with, so the
/// picture and the gesture cannot drift.
fn on_ruler(host: &Host, def_id: i32, p: &layout::Placed, y: f64) -> bool {
    if matches!(p.widget.kind, WidgetKind::TimeRuler { .. }) {
        return true;
    }
    let Some(editor) = p.widget.kind.editor() else {
        return false;
    };
    if editor.ruler == super::super::widget::Ruler::Off {
        return false;
    }
    let rh = host.metrics_for(def_id).ruler_h.min(p.rect.h);
    let top = (p.rect.y + p.rect.h - rh) as f64;
    y >= top && y <= (p.rect.y + p.rect.h) as f64
}

/// The table that press reads, which is the answer above plus the widget's own.
fn map_for(host: &Host, def_id: i32, p: &layout::Placed, y: f64) -> GestureMap {
    match p.widget.kind.editor() {
        Some(editor) if on_ruler(host, def_id, p, y) => {
            GestureMap::of_kind(&WidgetKind::TimeRuler {
                editor: editor.clone(),
            })
        }
        _ => p.widget.gesture_map(),
    }
}

/// The [`TimeAxis`] of a placed timeline view -- the geometry the renderer drew
/// through, resolved once here rather than by each gesture from the kind it
/// happens to have hit. The body is the strip samples map onto; the vertical
/// axis is the band left of it, when the view has one.
fn time_axis(
    host: &Host,
    def_id: i32,
    p: &layout::Placed,
    indent: f32,
    rows: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<TimeAxis> {
    let metrics = host.metrics_for(def_id);
    let _ruler_on = p.widget.kind.editor()?.ruler != super::super::widget::Ruler::Off;
    // The body samples map onto, and whether the axis has a vertical gesture
    // surface beside it.
    let (body, y_surface) = match &p.widget.kind {
        WidgetKind::TimeRuler { .. } => {
            (super::super::frame::ruler_strip_body(p.rect, indent), false)
        }
        // Every other member answers for itself: where the axis lies inside
        // its rect and whether it offers a vertical surface beside it. Only a
        // leaf whose picture is not "the rect minus its chrome" overrides the
        // generic body -- a roll's grid, with its strips stacked under it.
        kind => kind.axis_body(p.rect, indent, metrics).unwrap_or((
            super::super::frame::timeline_body(p.rect, kind.editor()?, false, indent, metrics),
            kind.editor()?.ruler_y != super::super::widget::RulerY::Off,
        )),
    };
    let (start, len) = p.widget.kind.editor()?.y_view();
    Some(TimeAxis {
        body,
        nav: view_of(host, def_id, p, body),
        y: y_surface.then(|| YAxis {
            // The whole band left of the body, full height: the strip is where
            // the axis is grabbed, and a press beside it at any height means the
            // same axis.
            strip: Rect::new(p.rect.x, p.rect.y, (body.x - p.rect.x).max(0.0), p.rect.h),
            start,
            len,
            rows: p.widget.id.map_or(1, |id| rows(id, &p.widget.kind)).max(1),
            row_h: (body.h as f64
                / p.widget.id.map_or(1, |id| rows(id, &p.widget.kind)).max(1) as f64)
                .max(1.0),
        }),
    })
}

/// The navigation window a placed timeline view is seen through: its group's,
/// or -- while it is in none -- the fallback its own contents imply, so a gesture
/// on an ungrouped view still measures against something the renderer agrees
/// with.
fn view_of(host: &Host, _def_id: i32, p: &layout::Placed, body: Rect) -> View {
    if let Some((nav, _total)) = p.widget.id.and_then(|id| host.timeline_nav(id)) {
        return nav;
    }
    match &p.widget.kind {
        // A surface that is *authored* rather than loaded spans its own
        // content until it joins a group.
        kind if kind.content_span().is_some() => {
            View::full(kind.content_span().unwrap_or(0.0).ceil().max(1.0) as usize)
        }
        _ => View::full(body.w.max(1.0) as usize),
    }
}
