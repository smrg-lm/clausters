//! **What the pointer says before a press, and while one is held.**
//!
//! The shape is the press's own question asked with no button down: what
//! lies under the pointer, and what a press there would start
//! ([`crate::host::pointer`]). So it is answered here, in the press's order
//! -- what is modal first, then the window's chrome, then the containers'
//! plans innermost first and the element under the pointer where a plan
//! hands it the press -- and nothing is pressed to find it out.
//!
//! **A press keeps the shape it began with until the release**, as a drag
//! owns the pointer: a selection swept out of a text keeps its I-beam over
//! the line numbers it crosses. An element whose drag changes what it is
//! doing says so ([`Element::held_pointer`]), and a pan holds the view under
//! a closed hand.
//!
//! [`Element::held_pointer`]: crate::host::widget::element::Element::held_pointer

use super::super::Host;
use super::super::chrome;
use super::super::interact;
use super::super::pointer::Pointer;
use super::super::popup::Tip;
use super::super::widget::{GestureStep, WidgetKind};
use super::nav::{freq_axis, hit, value_axis};
use super::{Drag, GestureCtx, Gestures, card, element};

impl Gestures {
    /// **The pointer's shape this window asks for**: what a press under the
    /// pointer would do, or what the press in hand is doing. The front sets
    /// it on its window when it changes.
    pub fn pointer(&self) -> Pointer {
        self.pointer
    }

    /// The press at `(cx, cy)` was handled, and `before` is what the pointer
    /// said over that point: the shape the press holds until its release.
    pub(super) fn pressed_pointer(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        before: Pointer,
        cx: f64,
        cy: f64,
    ) {
        self.pointer = match &self.drag {
            None => self.pointer_at(host, ctx, cx, cy),
            Some(Drag::Element { at, .. }) => {
                element::with(host, ctx, *at, |el, input| el.held_pointer(input))
                    .flatten()
                    .unwrap_or(before)
            }
            Some(drag) if is_pan(drag) => Pointer::Grabbing,
            Some(_) => before,
        };
    }

    /// A drag step: an element holding the press may say its drag now does
    /// something else.
    pub(super) fn held_moved(&mut self, host: &mut Host, ctx: &GestureCtx) {
        if let Some(Drag::Element { at, .. }) = &self.drag
            && let Some(Some(p)) = element::with(host, ctx, *at, |el, input| el.held_pointer(input))
        {
            self.pointer = p;
        }
    }

    /// **What a press at `(x, y)` would do**, as a shape. The same order the
    /// press walks ([`Gestures::press`]), each step asking whether it would
    /// take the press rather than taking it.
    pub(super) fn pointer_at(&self, host: &mut Host, ctx: &GestureCtx, x: f64, y: f64) -> Pointer {
        let def_id = ctx.def_id;
        // a hover card's text is selected by a press on it; its bars and
        // buttons are pressed
        if let Some(pointer) = card::pointer_on(host, ctx, x, y) {
            return pointer;
        }
        // an open list, or a dialog's scrim: a press there picks or dismisses
        if host.popup(def_id).is_some() || self.behind_dialog(host, ctx, x, y) {
            return Pointer::Default;
        }
        // the work panel's button and the status bar are buttons
        let on_band =
            |r: Option<clausters_gui_draw::rect::Rect>| r.is_some_and(|r| r.contains(x, y));
        if on_band(host.progress_cancel_rect(def_id, ctx.fb_w, ctx.fb_h))
            || on_band(host.status_bar_rect(def_id, ctx.fb_w, ctx.fb_h))
        {
            return Pointer::Default;
        }
        if let Some(p) = chrome_pointer(host, ctx, x, y) {
            return p;
        }
        let Some(found) = hit(host, ctx, x, y) else {
            return self.sole_axis_pointer(host, ctx);
        };
        // a y-ruler or a roll's keyboard: the vertical axis, panned
        if let Some((_, axis)) = interact::time_of(&found.chain)
            && let Some(y_axis) = axis.y
            && y_axis.strip.contains(x, y)
        {
            return Pointer::Grab;
        }
        if freq_axis(host, ctx, &found).is_some_and(|a| a.surface.contains(x, y)) {
            return Pointer::Grab;
        }
        let mut element_asked = false;
        for frame in found.chain.iter().rev() {
            for step in frame.map.plan(ctx.shift, ctx.ctrl, ctx.alt).steps() {
                let said = match (step, frame.coords) {
                    (GestureStep::Element, _) if element_asked => None,
                    (GestureStep::Element, _) => {
                        element_asked = true;
                        element_pointer(host, ctx, &found, x, y)
                    }
                    (_, interact::Coords::Time(axis)) if !axis.spans(x) => None,
                    (GestureStep::Pan, interact::Coords::Time(_)) => Some(Pointer::Grab),
                    (GestureStep::Pan, interact::Coords::Plane(view)) => frame
                        .id
                        .filter(|&id| interact::plane_can_pan(host, def_id, id, frame.rect, view))
                        .map(|_| Pointer::Grab),
                    // a stretch of the axis swept, as text is
                    (GestureStep::Select | GestureStep::Range, interact::Coords::Time(_)) => {
                        Some(Pointer::Text)
                    }
                    // a rectangle of time and value, and the pencil: placed
                    // precisely
                    (
                        GestureStep::SelectBox | GestureStep::Sample | GestureStep::Draw,
                        interact::Coords::Time(_),
                    ) => value_axis(host, ctx, frame, &found)
                        .filter(|v| v.body.contains(x, y))
                        .map(|_| Pointer::Crosshair),
                    (GestureStep::Marquee | GestureStep::Locate, interact::Coords::Time(_)) => {
                        Some(Pointer::Default)
                    }
                    (GestureStep::Marker, interact::Coords::Time(axis)) => {
                        let strip = crate::host::frame::ruler_strip(frame.rect, axis.body);
                        (strip.h > 0.0 && strip.contains(x, y)).then_some(Pointer::Default)
                    }
                    _ => None,
                };
                if let Some(p) = said {
                    return p;
                }
            }
        }
        self.sole_axis_pointer(host, ctx)
    }

    /// Off everything with a plan: Shift pans the window's one axis, if it
    /// has exactly one ([`Gestures::press`]).
    fn sole_axis_pointer(&self, host: &Host, ctx: &GestureCtx) -> Pointer {
        let sole = ctx.shift
            && interact::sole_time_axis(host, ctx.def_id, ctx.fb_w, ctx.fb_h, &|id, kind| {
                ctx.rows(id, kind)
            })
            .is_some();
        if sole {
            Pointer::Grab
        } else {
            Pointer::Default
        }
    }
}

/// Whether a drag is a view panned under the hand.
fn is_pan(drag: &Drag) -> bool {
    matches!(
        drag,
        Drag::Pan { .. } | Drag::ScrollPan { .. } | Drag::PanX { .. } | Drag::PanY { .. }
    )
}

/// **A container's own chrome under the pointer**: a close mark, a title
/// strip, a scroll bar (the arrow, as buttons keep it) and a divider, which
/// a drag pulls across the flow.
fn chrome_pointer(host: &Host, ctx: &GestureCtx, x: f64, y: f64) -> Option<Pointer> {
    let metrics = *host.metrics_for(ctx.def_id);
    let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?;
    let from = chrome::modal_start(&placed).unwrap_or(0);
    let mine = &placed[from..];
    if chrome::close_at(mine, x, y).is_some()
        || chrome::fold_at(mine, x, y).is_some()
        || chrome::bar_at(mine, &metrics, x, y).is_some()
    {
        return Some(Pointer::Default);
    }
    chrome::divider_at(&placed, x, y, metrics.hit_slop)
        .filter(|d| d.container >= from)
        .map(|d| {
            if d.row {
                Pointer::ColResize
            } else {
                Pointer::RowResize
            }
        })
}

/// The element under the pointer, asked on the shape its press filters with
/// ([`element::press`]): its answer, or `None` where it would decline.
fn element_pointer(
    host: &mut Host,
    ctx: &GestureCtx,
    found: &interact::Hit,
    x: f64,
    y: f64,
) -> Option<Pointer> {
    if !matches!(found.kind, WidgetKind::Custom(_)) {
        return None;
    }
    let at = element::At::widget(found.id, found.rect, found.scale, found.indent);
    element::with(host, ctx, at, |el, input| {
        if !el.hit_area(input).hit(x, y, input.metrics.hit_slop) {
            return None;
        }
        el.pointer((x, y), input)
    })
    .flatten()
}

/// **The tip of the place under the pointer**, where the element there names
/// one ([`Element::tip_at`]); else the widget's own `tip`.
///
/// [`Element::tip_at`]: crate::host::widget::element::Element::tip_at
pub(super) fn tip_at(host: &mut Host, ctx: &GestureCtx, x: f64, y: f64) -> Option<Tip> {
    let point = hit(host, ctx, x, y)
        .filter(|found| matches!(found.kind, WidgetKind::Custom(_)))
        .and_then(|found| {
            let at = element::At::widget(found.id, found.rect, found.scale, found.indent);
            element::with(host, ctx, at, |el, input| el.tip_at((x, y), input))
                .flatten()
                .map(|t| Tip {
                    widget: found.id,
                    text: t.text,
                    key: None,
                    anchor: t.anchor,
                })
        });
    point.or_else(|| host.tip_at(ctx.def_id, ctx.fb_w, ctx.fb_h, x, y))
}
