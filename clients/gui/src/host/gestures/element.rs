//! **How the machine talks to an element**: the one place the four phases of a
//! gesture reach an [`Element`], and the one place
//! what an element reports becomes what the front sends.
//!
//! It is a file of its own because the four phases must agree about it, and
//! because everything here is the *same* three steps: resolve the element by
//! id, build the [`Input`] for its placement, deliver whatever came back. A
//! phase that reached into the tree on its own would be a fifth answer to
//! questions that have one.

use super::super::Host;
use super::super::layout::Rect;
use super::super::widget::WidgetKind;
use super::super::widget::element::{BodyRole, Element, Events, Input, Mods, TimeSpace};
use super::effects::{deliver, deliver_args};
use super::{GestureCtx, GestureEffect};

/// **Where an element is**, for the machine: which widget — or which *body* of
/// which container — plus the placement the press was measured against and the
/// coordinate system it was placed on.
///
/// The body half is what a container's routing costs. A clip's bodies carry no
/// id (a script addresses the clip), so a body's address is its container's
/// id plus the [`BodyRole`] it fills: the same routing a `/gui_set` of a body
/// prop already takes, rather than a second way to name the same child.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct At {
    /// The widget the press was addressed to — a clip, for a body.
    pub id: i32,
    /// Which body of that widget, or `None` for the widget itself.
    pub body: Option<BodyRole>,
    pub rect: Rect,
    pub scale: f32,
    /// The container's axis, when it placed the element on one.
    pub time: Option<TimeSpace>,
}

impl At {
    /// A widget addressed directly, on no container's axis — every element
    /// outside a clip.
    pub(super) fn widget(id: i32, rect: Rect, scale: f32) -> Self {
        Self {
            id,
            body: None,
            rect,
            scale,
            time: None,
        }
    }
}

/// The gesture context for an element placed at `rect`/`scale` in this window.
///
/// The size table is resolved **at the placement's scale**, the same one the
/// renderer drew with, so a grab lands on the groove that was painted rather
/// than on the one an unzoomed table would put there.
pub(super) fn input<'a>(
    metrics: &'a super::super::metrics::Metrics,
    ctx: &GestureCtx,
    at: At,
) -> Input<'a> {
    Input {
        metrics,
        rect: at.rect,
        scale: at.scale,
        mods: Mods {
            shift: ctx.shift,
            ctrl: ctx.ctrl,
            alt: ctx.alt,
        },
        viewport: (ctx.fb_w as f32, ctx.fb_h as f32),
        time: at.time,
    }
}

/// Runs `f` on the element `at` addresses, with the [`Input`] its placement
/// implies. `None` when the widget is gone, was never an element, or no longer
/// holds the body — a drag whose widget was freed under it, which is an
/// ordinary thing to survive.
pub(super) fn with<R>(
    host: &mut Host,
    ctx: &GestureCtx,
    at: At,
    f: impl FnOnce(&mut dyn Element, &Input) -> R,
) -> Option<R> {
    let metrics = host.metrics_for(ctx.def_id).at(at.scale);
    let input = input(&metrics, ctx, at);
    let widget = host.window_def_mut(ctx.def_id)?.find_mut(at.id)?;
    let kind = match at.body {
        Some(role) => widget.clip_body_mut(role)?,
        None => &mut widget.kind,
    };
    match kind {
        WidgetKind::Custom(el) => Some(f(&mut **el, &input)),
        _ => None,
    }
}

/// Sends what an element reported, and repaints when it reported anything.
///
/// The rule per message is the one the host already had, stated once here: a
/// **single argument is a value** and takes the value road (a bound widget
/// forwards it straight to the audio server), and anything longer is a tagged
/// edit-back payload and takes that road. So an element reports in one
/// vocabulary and the bound-vs-event decision stays where it was.
pub(super) fn report(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    ctx: &GestureCtx,
    id: i32,
    events: Events,
) {
    let messages = events.into_messages();
    if messages.is_empty() {
        return;
    }
    for mut args in messages {
        match args.len() {
            1 => deliver(host, out, ctx.def_id, id, args.remove(0)),
            _ => deliver_args(host, out, ctx.def_id, id, Some(args)),
        }
    }
    out.push(GestureEffect::Redraw(ctx.def_id));
}

/// The element holding an **overlay** in this window — an open list, a popup —
/// with the placement it was drawn at.
///
/// Found by asking the tree rather than by remembering: an overlay is declared
/// ([`Element::overlay_rect`]), so
/// there is no machine state to keep in step with an element that opened or
/// closed one, and a def that replaced the tree takes its overlays with it.
pub(super) fn overlay_owner(host: &Host, ctx: &GestureCtx) -> Option<(i32, Rect, f32)> {
    let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?;
    placed
        .iter()
        .find(|p| p.widget.kind.overlay_rect().is_some())
        .and_then(|p| Some((p.widget.id?, p.rect, p.scale)))
}
