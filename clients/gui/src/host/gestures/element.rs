//! **How the machine talks to an element**: the one place the four phases of a
//! gesture reach a [`Element`](crate::host::widget::Element), and the one place
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
use super::super::widget::element::{Element, Events, Input, Mods};
use super::effects::{deliver, deliver_args};
use super::{GestureCtx, GestureEffect};

/// The gesture context for an element placed at `rect`/`scale` in this window.
///
/// The size table is resolved **at the placement's scale**, the same one the
/// renderer drew with, so a grab lands on the groove that was painted rather
/// than on the one an unzoomed table would put there.
pub(super) fn input<'a>(
    metrics: &'a super::super::metrics::Metrics,
    ctx: &GestureCtx,
    rect: Rect,
    scale: f32,
) -> Input<'a> {
    Input {
        metrics,
        rect,
        scale,
        mods: Mods {
            shift: ctx.shift,
            ctrl: ctx.ctrl,
            alt: ctx.alt,
        },
        viewport: (ctx.fb_w as f32, ctx.fb_h as f32),
        time: None,
    }
}

/// Runs `f` on the element addressed by `id`, with the [`Input`] its placement
/// implies. `None` when the widget is gone or was never an element — a drag
/// whose widget was freed under it, which is an ordinary thing to survive.
pub(super) fn with<R>(
    host: &mut Host,
    ctx: &GestureCtx,
    id: i32,
    rect: Rect,
    scale: f32,
    f: impl FnOnce(&mut dyn Element, &Input) -> R,
) -> Option<R> {
    let metrics = host.metrics_for(ctx.def_id).at(scale);
    let input = input(&metrics, ctx, rect, scale);
    match host.widget_kind_mut(ctx.def_id, id) {
        Some(WidgetKind::Custom(el)) => Some(f(&mut **el, &input)),
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
/// ([`Element::overlay_rect`](crate::host::widget::Element::overlay_rect)), so
/// there is no machine state to keep in step with an element that opened or
/// closed one, and a def that replaced the tree takes its overlays with it.
pub(super) fn overlay_owner(host: &Host, ctx: &GestureCtx) -> Option<(i32, Rect, f32)> {
    let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?;
    placed
        .iter()
        .find(|p| p.widget.kind.overlay_rect().is_some())
        .and_then(|p| Some((p.widget.id?, p.rect, p.scale)))
}
