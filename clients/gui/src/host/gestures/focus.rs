//! **Where the keyboard points**: the window's tab ring, and the one door that
//! moves the focus.
//!
//! Focus is the host's state and not a widget's — there is one keyboard, so
//! there is one focus per host — but *which* widgets it can point at is the
//! window's, and it is asked rather than remembered: the ring is the layout's
//! own order over the widgets that answer
//! [`accepts_focus`](crate::host::widget::Element::accepts_focus), recomputed on
//! each step. Nothing to keep in step with a `/gui_def` that replaced the tree,
//! a widget that was freed under it, or a `stack` that turned the page.
//!
//! **Tab past the last stop leaves the tree**, and that is a decision rather
//! than an oversight. A window in a page is *inside a document*: if the ring
//! wrapped, a mounted GuiDef would be a keyboard trap and everything around it
//! unreachable. So the ring runs out, the focus clears, and the front is told
//! ([`GestureEffect::FocusOut`]) — a page blurs its canvas and the browser's own
//! tab order carries on from there, while a desktop window simply has nothing
//! focused and the next Tab starts the ring again.

use clausters_core::osc::OscType;

use super::super::Host;
use super::super::widget::WidgetKind;
use super::effects::emit;
use super::{GestureCtx, GestureEffect};

/// The `/gui_event` a focus change reports: `<widget> "focus" <1|0>`.
///
/// It is a **notification**, not a value, so it goes out as an event even from a
/// bound widget: a binding says where this widget's *value* goes, and where the
/// keyboard is pointing is not it.
fn report(out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32, gained: bool) {
    emit(
        out,
        def_id,
        widget_id,
        vec![
            OscType::String("focus".into()),
            OscType::Int(i32::from(gained)),
        ],
    );
}

/// The window's **tab ring**: every widget that takes the focus, in the order
/// the layout placed it — which is the order a reader's eye takes them in, and
/// the only order the host has that means anything.
pub(super) fn ring(host: &Host, ctx: &GestureCtx) -> Vec<i32> {
    let Some(placed) = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h) else {
        return Vec::new();
    };
    placed
        .iter()
        .filter(|p| p.widget.kind.accepts_focus())
        .filter_map(|p| p.widget.id)
        .collect()
}

/// Moves the focus to `to` — or clears it entirely (`None`) — reporting both
/// ends of the move and repainting whatever changed.
///
/// The one door: a press, a Tab and a `/gui_set focus` all come through here, so
/// the events and the repaints cannot depend on which of them moved it.
pub(super) fn set(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    ctx: &GestureCtx,
    to: Option<i32>,
) {
    let from = host.focused();
    if from.map(|(_, id)| id) == to {
        return; // already there, or already nowhere
    }
    if let Some((def, id)) = from {
        host.clear_focus();
        report(out, def, id, false);
        out.push(GestureEffect::Redraw(def));
    }
    if let Some(id) = to {
        host.focus(ctx.def_id, id);
        report(out, ctx.def_id, id, true);
        out.push(GestureEffect::Redraw(ctx.def_id));
    }
}

/// A press landed on `hit` (or on nothing, `None`): the focus follows it to a
/// widget that takes one, and leaves whatever held it otherwise — a click away
/// from a field is how a caret disappears.
pub(super) fn on_press(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    ctx: &GestureCtx,
    hit: Option<(i32, &WidgetKind)>,
) {
    let to = hit
        .filter(|(_, kind)| kind.accepts_focus())
        .map(|(id, _)| id);
    match to {
        // A press on a widget that takes the focus moves it there.
        Some(id) => set(host, out, ctx, Some(id)),
        // A press anywhere else drops it — but only if it was *this* window's:
        // clicking in one window must not take the focus out of another.
        None if host.focused().is_some_and(|(d, _)| d == ctx.def_id) => set(host, out, ctx, None),
        None => {}
    }
}

/// Tab (`back` for Shift+Tab): the next stop on the ring, or **out of the
/// tree** when there is none.
///
/// With nothing focused the ring is entered at its end — its first stop
/// forwards, its last backwards — which is what makes a Tab into a page's canvas
/// land on the first field rather than on the second.
pub(super) fn step(host: &mut Host, ctx: &GestureCtx, back: bool) -> Vec<GestureEffect> {
    let mut out = Vec::new();
    let ring = ring(host, ctx);
    if ring.is_empty() {
        // Nothing here reads the keyboard, so Tab was never ours.
        out.push(GestureEffect::FocusOut(ctx.def_id));
        return out;
    }
    let current = host
        .focused()
        .filter(|(d, _)| *d == ctx.def_id)
        .and_then(|(_, id)| ring.iter().position(|r| *r == id));
    let next = match (current, back) {
        (None, false) => Some(0),
        (None, true) => Some(ring.len() - 1),
        (Some(0), true) => None,
        (Some(i), true) => Some(i - 1),
        (Some(i), false) if i + 1 < ring.len() => Some(i + 1),
        (Some(_), false) => None,
    };
    match next {
        Some(i) => set(host, &mut out, ctx, Some(ring[i])),
        // Off the end of the ring: the focus leaves the tree, and the front is
        // told so the document around it can take over.
        None => {
            set(host, &mut out, ctx, None);
            out.push(GestureEffect::FocusOut(ctx.def_id));
        }
    }
    out
}
