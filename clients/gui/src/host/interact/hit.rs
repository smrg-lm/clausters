//! **What is under a point**: the one layout pass that answers a pointer
//! question, and the per-element hit-tests that read the answer finer.
//!
//! [`hit`] lays the window out once and hands back the deepest interactive
//! widget plus the [`Frame`] chain of containers over it — the containment the
//! layout already resolved ([`chain_of`]), each container's coordinate system
//! resolved with it ([`time_axis`], [`view_of`]). Everything else here is the
//! second question a gesture asks once it knows *which* element it hit: which
//! part of a clip ([`clip_hit`]), which header control ([`header_hit`]), which
//!
//! The rule that keeps these honest is that they reconstruct **the geometry the
//! renderer drew through**, never a parallel derivation of it: a note is grabbed
//! by the pixels it was drawn on.

use super::super::Host;
use super::super::layout::{self, Rect};
use super::super::widget::WidgetKind;
use super::coords::{Coords, Frame, Hit, TimeAxis, YAxis, clip_part};
use super::{ClipPart, HeaderPart};
use crate::host::graphics::track;
use crate::viewport::View;

/// The [`Hit`] under `(x, y)` in window `def_id`. Containers (`window`/`panel`)
/// are not hit targets — except `scroll`, whose empty area is the pan gesture's
/// surface (its children, laid out through its view transform, still win over
/// it). A widget scrolled out of its container's window (outside its clip) is
/// not hit. `fb_w`/`fb_h` is the window's framebuffer size in device pixels.
///
/// `lanes` answers how many channel lanes a stacked heavy view draws — the one
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
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
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
        chain: chain_of(host, def_id, &placed, i, lanes),
    })
}

/// The window's **one** navigation group, when it has exactly one: a member's
/// id and axis (they share the window and the gutter, so any member answers for
/// all of them), plus every **lane** on it.
///
/// It is what a gesture falls back to when the pointer is not over a timeline
/// at all — the gap between two lanes, the slack under the last one, a
/// container's margin. In a window built around one axis those pixels are not a
/// third thing the user meant: they are the axis with nothing drawn on them.
/// With two groups there is no such answer, so there is no fallback either.
pub(crate) struct SoleAxis {
    pub id: i32,
    pub axis: TimeAxis,
    pub lanes: Vec<i32>,
}

pub(crate) fn sole_time_axis(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<SoleAxis> {
    let placed = host.layout_window(def_id, fb_w, fb_h)?;
    let mut key = None;
    let mut found: Option<(i32, TimeAxis)> = None;
    let mut lane_ids = Vec::new();
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
        if matches!(p.widget.kind, WidgetKind::Track { .. }) {
            lane_ids.push(id);
        }
        if found.is_none() {
            found = time_axis(host, def_id, p, p.indent, lanes).map(|axis| (id, axis));
        }
    }
    let (id, axis) = found?;
    Some(SoleAxis {
        id,
        axis,
        lanes: lane_ids,
    })
}

/// The containers from the window down to `i`, `i` itself included when it is
/// one — walked back through [`super::super::layout::Placed::parent`], which is the containment the
/// layout pass already resolved.
fn chain_of(
    host: &Host,
    def_id: i32,
    placed: &[layout::Placed],
    i: usize,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
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
            // A clip carries the axis the layout gave it, which is the whole
            // point of the layout placing clips: the rectangle and the window
            // are one fact, resolved once, read by the renderer and by this.
            WidgetKind::Clip { .. } => p.time.map(|nav| {
                Coords::Local(TimeAxis {
                    body: p.rect,
                    nav,
                    y: None,
                })
            }),
            // Where the body begins is the **group's** call, not this widget's:
            // every member of one axis starts it at the same x, and the layout
            // already resolved it (`Placed::indent`).
            _ if p.widget.is_timeline() => {
                time_axis(host, def_id, &p, p.indent, lanes).map(Coords::Time)
            }
            _ => None,
        };
        if let Some(coords) = coords {
            chain.push(Frame {
                id: p.widget.id,
                rect: p.rect,
                map: p.widget.gesture_map(),
                coords,
            });
        }
        at = p.parent;
    }
    chain.reverse();
    chain
}

/// The [`TimeAxis`] of a placed timeline view — the geometry the renderer drew
/// through, resolved once here rather than by each gesture from the kind it
/// happens to have hit. The body is the strip samples map onto; the vertical
/// axis is the band left of it, when the view has one.
fn time_axis(
    host: &Host,
    def_id: i32,
    p: &layout::Placed,
    indent: f32,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<TimeAxis> {
    let metrics = host.metrics_for(def_id);
    let ruler_on = p.widget.kind.editor()?.ruler != super::super::widget::Ruler::Off;
    // The body samples map onto, and whether the axis has a vertical gesture
    // surface beside it.
    let (body, y_surface) = match &p.widget.kind {
        WidgetKind::Track { .. } => (track::lane_body(p.rect, ruler_on, indent, metrics), false),
        WidgetKind::TimeRuler { .. } => {
            (super::super::frame::ruler_strip_body(p.rect, indent), false)
        }
        // Every other member answers for itself: where the axis lies inside
        // its rect and whether it offers a vertical surface beside it. Only a
        // leaf whose picture is not "the rect minus its chrome" overrides the
        // generic body — a roll's grid, with its strips stacked under it.
        kind => kind.axis_body(p.rect, indent, metrics).unwrap_or((
            super::super::frame::timeline_body(p.rect, kind.editor()?, indent, metrics),
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
            lane_h: (body.h as f64
                / p.widget.id.map_or(1, |id| lanes(id, &p.widget.kind)).max(1) as f64)
                .max(1.0),
        }),
    })
}

/// The navigation window a placed timeline view is seen through: its group's,
/// or — while it is in none — the fallback its own contents imply, so a gesture
/// on an ungrouped view still measures against something the renderer agrees
/// with.
fn view_of(host: &Host, def_id: i32, p: &layout::Placed, body: Rect) -> View {
    if let Some((nav, _total)) = p.widget.id.and_then(|id| host.timeline_nav(id)) {
        return nav;
    }
    match &p.widget.kind {
        WidgetKind::Track { .. } => host
            .window_def(def_id)
            .map_or(View::full(1), track::window_nav),
        // A surface that is *authored* rather than loaded spans its own
        // content until it joins a group.
        kind if kind.content_span().is_some() => {
            View::full(kind.content_span().unwrap_or(0.0).ceil().max(1.0) as usize)
        }
        _ => View::full(body.w.max(1.0) as usize),
    }
}

/// A press on a lane's header: which control it landed on, and — for the fader
/// — the rectangle the drag maps its value through.
pub(crate) struct HeaderHit {
    pub part: HeaderPart,
    pub fader: Option<Rect>,
}

/// The header control under `(cx, cy)` on the placed lane `rect`, whose axis
/// begins at `body_x` (so the band beside it is the header). `None` when the
/// press is on the axis, or on the band's empty space — which names no sample
/// and no control, and so means nothing.
pub(crate) fn header_hit(
    host: &Host,
    def_id: i32,
    lane_id: i32,
    rect: Rect,
    body_x: f32,
    cx: f64,
    cy: f64,
) -> Option<HeaderHit> {
    let WidgetKind::Track { header, .. } = host.widget_kind(def_id, lane_id)? else {
        return None;
    };
    let band = super::super::timeline::gutter_band(rect, body_x - rect.x);
    let m = host.metrics_for(def_id);
    let part = track::header_hit(band, header, m, cx, cy)?;
    Some(HeaderHit {
        part,
        fader: track::header_parts(band, header, m).fader,
    })
}

/// A clip press: the clip id, its current placement (`offset`/`dur`), the lane
/// body and the shared navigation window the drag maps through (so the front
/// turns cursor pixels into timeline samples), and which part was hit.
pub(crate) struct ClipHit {
    pub id: i32,
    /// The lane the clip sits on. A clip is not itself a navigation-group
    /// member — the *lane* is — so anything that has to reach the shared axis
    /// (the drag's cursor mapping, the edge scroll) asks through this id.
    pub lane: i32,
    pub dur: f64,
    pub body: Rect,
    /// The clip's own rectangle — the box its bodies fill, so a body's edits
    /// map onto the pixels that were drawn.
    pub rect: Rect,
    pub nav: View,
    /// The clip's **own** axis: the window of its `[0, dur]` span that `rect`
    /// shows. Every edit inside the clip maps through `(rect, local)` — it is
    /// the coordinate system the clip hands its body elements; `body`/`nav`
    /// above are only what the clip's *placement* on the lane is dragged
    /// through.
    pub local: View,
    pub part: ClipPart,
    /// The placement the press found, as one value — the snapshot a drag is
    /// measured against.
    pub placement: super::ClipPlacement,
    /// What the material behind the clip allows its edges to do (how many
    /// frames it has, whether the window loops off them).
    pub material: super::Material,
}

/// The [`ClipHit`] of the `clip` the pointer landed on: the clip the layout
/// **placed** (its id and its rectangle, straight off the hit) read against the
/// lane's time axis, which is what the placement was computed from.
///
/// Nothing is re-derived here any more. The clip used to be found by walking
/// the lane's children and re-running `clip_x_range` on each, because a clip
/// was not a placed widget and there was nothing else to ask; now it is one, so
/// the topmost-wins rule is the layout's (later children are placed later, and
/// the hit takes the last match) and the rectangle is the one that was drawn.
/// What is *inside* the clip is not here: a body element is asked for the
/// press directly, on `(rect, local)`, and answers for its own parts — the
/// hit-test says which widget, never which part of what it holds.
/// Native-only, like the other edit-back gestures.
pub(crate) fn clip_hit(
    host: &Host,
    def_id: i32,
    lane: (i32, TimeAxis),
    clip: (i32, TimeAxis),
    x: f64,
) -> Option<ClipHit> {
    let (lane_id, TimeAxis { body, nav, .. }) = lane;
    let (id, local) = clip;
    let rect = local.body;
    let widget = host.window_def(def_id)?.find(id)?;
    let WidgetKind::Clip {
        offset,
        dur,
        window,
        ..
    } = widget.kind
    else {
        return None;
    };
    // What the clip is a window **onto**: the take's own length, asked of the
    // body that holds it. A clip with no material — a roll, a bare automation —
    // has no window to run off, and its edges are bounded by nothing but the
    // clip's own floor.
    let total = widget
        .clip_body(crate::host::widget::element::BodyRole::Take)
        .and_then(|k| k.as_element())
        .and_then(|el| el.material_shape())
        .map(|(_, frames)| frames as f64)
        .filter(|f| *f > 0.0);
    Some(ClipHit {
        id,
        lane: lane_id,
        dur,
        body,
        rect,
        nav,
        local: local.nav,
        placement: super::ClipPlacement {
            offset,
            dur,
            start: window.start,
        },
        material: super::Material {
            total,
            looping: window.looping,
        },
        // The grips the renderer drew: the same rectangle, the same ends, the
        // same size table.
        part: clip_part(
            rect,
            crate::host::graphics::track::clip_ends_on_screen(&local.nav, dur),
            host.metrics_for(def_id),
            x as f32,
        ),
    })
}
