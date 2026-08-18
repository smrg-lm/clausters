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
use super::super::layers::Layer;
use super::super::layout::Rect;
use super::super::widget::WidgetKind;
use super::super::widget::element::{Claim, Element, Events, Input, Mods, TimeSpace};
use super::effects::{deliver, deliver_args};
use super::{GestureCtx, GestureEffect};

/// **Where an element is**, for the machine: which widget — or which *body* of
/// which container — plus the placement the press was measured against and the
/// coordinate system it was placed on.
///
/// The body half is what a container's routing costs. A clip's bodies carry no
/// id (a script addresses the clip), so a body's address is its container's id
/// plus the **layer** it is ([`Layer::Content`]) — the same address the
/// selection, the drawing gate and the wire name all use, rather than a second
/// way to name the same child.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct At {
    /// The widget the press was addressed to — a clip, for a body.
    pub id: i32,
    /// Which **layer** of that widget, or `None` for the widget itself.
    pub layer: Option<Layer>,
    pub rect: Rect,
    /// Where the shared axis begins inside `rect` (see [`Input::indent`]).
    pub indent: f32,
    pub scale: f32,
    /// The **container's** axis, for a body — a clip's own span, resolved by
    /// the container that offered the press.
    ///
    /// A widget addressed directly carries none: its axis is its *navigation
    /// group's*, which is looked up per call ([`with`]) rather than
    /// snapshotted, because the group's window moves under a drag — a note
    /// held past the edge of a lane is dragged against an axis that is
    /// scrolling.
    pub time: Option<TimeSpace>,
}

impl At {
    /// A widget addressed directly, on no *container's* axis — every element
    /// outside a clip. `indent` is where its navigation group starts its body
    /// inside `rect`, `0.0` for an element on no shared axis.
    pub(super) fn widget(id: i32, rect: Rect, scale: f32, indent: f32) -> Self {
        Self {
            id,
            layer: None,
            rect,
            indent,
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
    time: Option<TimeSpace>,
) -> Input<'a> {
    Input {
        metrics,
        rect: at.rect,
        indent: at.indent,
        scale: at.scale,
        mods: Mods {
            shift: ctx.shift,
            ctrl: ctx.ctrl,
            alt: ctx.alt,
        },
        viewport: (ctx.fb_w as f32, ctx.fb_h as f32),
        time,
    }
}

/// The coordinate system this address stands on: a container's axis for a body
/// (the press resolved it), else the widget's own **navigation group** looked
/// up now.
///
/// The playhead is deliberately absent — the engine clock is the front's, and
/// no gesture is decided by where the line is (see [`TimeSpace::head`]).
fn time_of(host: &Host, ctx: &GestureCtx, at: At) -> Option<TimeSpace> {
    if at.layer.is_some() {
        return at.time;
    }
    let link = host.widget_kind(ctx.def_id, at.id)?.editor()?.link;
    host.timelines().space_of(at.id, link, None)
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
    let time = time_of(host, ctx, at);
    let input = input(&metrics, ctx, at, time);
    let widget = host.window_def_mut(ctx.def_id)?.find_mut(at.id)?;
    let kind = match at.layer {
        Some(layer) => widget.layer_body_mut(layer)?,
        None => &mut widget.kind,
    };
    match kind {
        WidgetKind::Custom(el) => Some(f(&mut **el, &input)),
        _ => None,
    }
}

/// **Which layer of the container `at` addresses the pointer is on** — the
/// rule in [`crate::host::layers`], asked with the [`Input`] the container's
/// own placement implies, so a layer answers about the pixels it was drawn on.
///
/// `None` when the widget is gone or layers nothing; the placement layer
/// otherwise, which is what empty material means.
pub(super) fn layer_under_pointer(
    host: &Host,
    ctx: &GestureCtx,
    at: At,
    cx: f64,
    cy: f64,
) -> Option<Layer> {
    let metrics = host.metrics_for(ctx.def_id).at(at.scale);
    let input = input(&metrics, ctx, at, at.time);
    let widget = host.window_def(ctx.def_id)?.find(at.id)?;
    Some(crate::host::layers::under_pointer(widget, (cx, cy), &input))
}

/// **The rectangle and the axis the layer `at` names stands on** — the
/// container's when the layer fills it, its own stretch when it names one
/// ([`crate::host::layers::layer_input`]). The press and the drag both address
/// a layer through this, so a note dragged inside a clip's second segment is
/// grabbed on the pixels it was drawn on.
pub(super) fn layer_frame(host: &Host, ctx: &GestureCtx, at: At) -> At {
    let Some(Layer::Content(n)) = at.layer else {
        return at;
    };
    let metrics = host.metrics_for(ctx.def_id).at(at.scale);
    let Some(container) = host.window_def(ctx.def_id).and_then(|t| t.find(at.id)) else {
        return at;
    };
    let Some(child) = container
        .children
        .iter()
        .filter(|c| c.kind.body_role().is_some())
        .nth(n)
    else {
        return at;
    };
    let derived = crate::host::layers::layer_input(&input(&metrics, ctx, at, at.time), child);
    At {
        rect: derived.rect,
        time: derived.time,
        ..at
    }
}

/// **Offers a press to the element `at` addresses, if the point is on it.**
///
/// The one place an element's declared shape ([`Element::hit_area`]) is
/// applied, with the metrics' hit slop around it. A placement is a rectangle
/// and the layout hands out whole cells, but plenty of elements are drawn
/// smaller or rounder than the cell they were given — a knob's dial, a
/// slider's groove, a checkbox with a word beside it in a stretched row — and
/// the air around them belongs to the window, not to the control. Filtering
/// here rather than in each `press` is what makes that general: an element
/// states its shape once and never writes the guard, and one that states
/// nothing keeps the whole rectangle it always had.
///
/// A point off the shape reads exactly as a [`Claim::Decline`], so the press
/// goes back to the chain the way any declined press does.
pub(super) fn press(host: &mut Host, ctx: &GestureCtx, at: At, cx: f64, cy: f64) -> Option<Claim> {
    with(host, ctx, at, |el, input| {
        if !el.hit_area(input).hit(cx, cy, input.metrics.hit_slop) {
            return Claim::Decline;
        }
        el.press((cx, cy), input)
    })
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
    // The voices first: a note that both sounds and reports must sound in the
    // order the element asked, and the host is the only one with a leg to the
    // server.
    for voice in events.voices() {
        if voice.on {
            host.voice_on(ctx.def_id, id, voice.pitch, voice.velocity);
        } else {
            host.voice_off(id, voice.pitch);
        }
    }
    // ...then the container's selection, if the element asked for it: a
    // marquee sweeping a roll moves the axis' shared selection, which every
    // linked view follows and no element can reach.
    let selected = events.selection().inspect(|&((a, b), values)| {
        super::nav::set_selection(host, out, ctx.def_id, id, a, b, values);
    });
    let voiced = !events.voices().is_empty() || selected.is_some();
    let messages = events.into_messages();
    if messages.is_empty() {
        if voiced {
            out.push(GestureEffect::Redraw(ctx.def_id));
        }
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
