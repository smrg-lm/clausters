//! The keyboard half of the machine: the focus ring, the key that goes to the
//! focused element, and the block operations a timeline view answers to
//! (quantize, cut/copy/paste over the multi-note selection, resetting every view
//! to its full extent).
//!
//! Split from the pointer machine because it shares nothing with it but the
//! `Gestures` state: no hit-test, no drag, no cursor — a key arrives already
//! addressed to whatever the window has focused or selected.
//!
//! **Two addressees, in this order.** Tab is the window's, always
//! ([`super::focus`]). Everything else is the focused element's, and only
//! what the element declines falls through to the front's own shortcuts, which
//! are addressed to what is under the *cursor* rather than to what holds the
//! focus. That order is what lets a field swallow `q` while a piano-roll behind
//! it still quantizes on the same key when nothing is focused.

use super::super::Host;
use super::super::interact::Hit;
use super::super::widget::element::{Key, KeyInput, Mods};
use super::effects::{emit_view, redraw_all};
use super::nav::{freq_nav_ids, hit, set_x_view, set_y_view, timeline_ids};
use super::{GestureCtx, GestureEffect, Gestures, element, focus};

impl Gestures {
    /// A key arriving at this window: Tab walks the focus ring, anything else
    /// goes to the focused element's
    /// [`Element::key`](crate::host::widget::Element::key) — which delivers
    /// whatever it reports exactly as a drag would, bound → straight to the
    /// audio server, else a `/gui_event`.
    ///
    /// `clipboard` is the host-wide text clipboard a cut/copy/paste reads and
    /// writes (the native front's internal one; the browser front swaps in the
    /// page's around this call).
    ///
    /// Returns `Some(effects)` when the key was consumed — the front then skips
    /// its own shortcuts — and `None` when nothing here answered it.
    pub fn key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: Key,
        clipboard: &mut String,
    ) -> Option<Vec<GestureEffect>> {
        if key == Key::Tab {
            return Some(focus::step(host, ctx, ctx.shift));
        }
        // Only an element focused in *this* window: a key is delivered by the
        // window it was typed into.
        let (fdef, id) = host.focused()?;
        if fdef != ctx.def_id {
            return None;
        }
        let placed = host.layout_window(ctx.def_id, ctx.fb_w, ctx.fb_h)?;
        let (rect, scale, indent) = placed
            .iter()
            .find(|p| p.widget.id == Some(id))
            .map(|p| (p.rect, p.scale, p.indent))?;
        let mut input = KeyInput {
            mods: Mods {
                shift: ctx.shift,
                ctrl: ctx.ctrl,
                alt: ctx.alt,
            },
            clipboard,
        };
        let at = element::At::widget(id, rect, scale, indent);
        let events = element::with(host, ctx, at, |el, _| el.key(&key, &mut input)).flatten()?;
        let mut out = Vec::new();
        // The element consumed it, so the window repaints whether or not
        // anything was reported: a caret that moved is a picture that changed.
        element::report(host, &mut out, ctx, id, events);
        out.push(GestureEffect::Redraw(ctx.def_id));
        Some(out)
    }

    /// A key the focus did not answer, offered to the **element under the
    /// cursor** — the other addressee, and the reason a field can swallow `q`
    /// while a roll behind it keeps quantizing on the same key.
    ///
    /// It is the same call [`key`](Self::key) makes, at a different address:
    /// what an element does with a key is the element's, and the machine only
    /// decides *who* is asked. Returns `Some` when it was consumed, so the
    /// front runs its own shortcuts only on what nothing wanted.
    pub fn key_at_cursor(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: Key,
        cx: f64,
        cy: f64,
        clipboard: &mut String,
    ) -> Option<Vec<GestureEffect>> {
        let Hit {
            id,
            rect,
            scale,
            indent,
            ..
        } = hit(host, ctx, cx, cy)?;
        let mut input = KeyInput {
            mods: Mods {
                shift: ctx.shift,
                ctrl: ctx.ctrl,
                alt: ctx.alt,
            },
            clipboard,
        };
        let at = element::At::widget(id, rect, scale, indent);
        let events = element::with(host, ctx, at, |el, _| el.key(&key, &mut input)).flatten()?;
        let mut out = Vec::new();
        element::report(host, &mut out, ctx, id, events);
        // A content edit moves the extent the shared axis spans, and the window
        // repaints whether or not anything was reported.
        host.sync_track_totals_keeping_view();
        out.push(GestureEffect::Redraw(ctx.def_id));
        Some(out)
    }

    /// `R` over a window: reset every navigable view's axes — a timeline's
    /// navigation (the whole group, linked members in other windows too) and
    /// its vertical window, and a navigable spectrum's frequency window. The
    /// views are found by walking the window's tree, so no front slot list is
    /// needed.
    pub fn reset_timelines(&mut self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let ids = host
            .window_def(def_id)
            .map(timeline_ids)
            .unwrap_or_default();
        for id in ids {
            // The whole group resets (linked members in other windows too).
            let roots = host.reset_timeline(id);
            redraw_all(&mut out, &roots);
            emit_view(host, &mut out, def_id, id);
            // The reset also restores the full vertical axis (and reports it).
            set_y_view(host, &mut out, def_id, id, 0.0, 1.0);
        }
        // A spectrum is in no group, so its frequency window resets on its own
        // — the same key, since to a reader it is the same "show me all of it".
        let spectra = host
            .window_def(def_id)
            .map(freq_nav_ids)
            .unwrap_or_default();
        for id in spectra {
            set_x_view(host, &mut out, def_id, id, 0.0, 1.0, ctx.sample_rate);
        }
        out.push(GestureEffect::Redraw(def_id));
        out
    }
}
