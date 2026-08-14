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
use clausters_core::osc::OscType;

use super::super::clipboard::Clip;
use super::super::widget::element::{Key, KeyInput, Mods, SampleBlock};
use super::effects::{emit, emit_view, redraw_all};
use super::nav::{freq_nav_ids, hit, set_x_view, set_y_view, timeline_ids};
use super::{GestureCtx, GestureEffect, Gestures, element, focus};

impl Gestures {
    /// A key arriving at this window: Tab walks the focus ring, anything else
    /// goes to the focused element's
    /// [`Element::key`](crate::host::widget::Element::key) — which delivers
    /// whatever it reports exactly as a drag would, bound → straight to the
    /// audio server, else a `/gui_event`.
    ///
    /// `clipboard` is the host-wide clipboard a cut/copy/paste reads and writes
    /// (the native front's internal one; the browser front swaps the page's
    /// string in and out around this call).
    ///
    /// Returns `Some(effects)` when the key was consumed — the front then skips
    /// its own shortcuts — and `None` when nothing here answered it.
    pub fn key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: Key,
        clipboard: &mut super::super::clipboard::Clip,
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
        clipboard: &mut super::super::clipboard::Clip,
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

    /// Undo or redo over a window: report it to whoever owns the document.
    ///
    /// **The host holds no history** — the log lives with the document, in
    /// `clausters-document`, because a log a view keeps sees only the gestures
    /// *it* made. So this is a route and not an action: it emits
    /// `/gui_event <window_id> <seq> <version> "undo"|"redo"` and the owner
    /// answers with the state that now holds, exactly as it answers a drag.
    ///
    /// It is addressed to the **window** rather than to a widget because that
    /// is what it is scoped to: undo is not addressed to a place under the
    /// cursor, which is why it is not a step in the gesture plan — a
    /// `GesturePlan`'s steps each consume a press *somewhere*. `/gui_closed`
    /// already names a window the same way.
    pub fn history(&self, host: &mut Host, ctx: &GestureCtx, redo: bool) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let tag = if redo { "redo" } else { "undo" };
        emit(
            host,
            &mut out,
            ctx.def_id,
            ctx.def_id,
            vec![OscType::String(tag.into())],
        );
        out
    }

    /// **Copy, cut and paste over the selection**, addressed to the view under
    /// the cursor — the window's own shortcuts, reached only by a key nothing
    /// focused and nothing under the cursor answered first (a field's Ctrl+C is
    /// still the field's).
    ///
    /// The three verbs split exactly where the host's authority does. A **copy**
    /// is a read, and the host may honestly do it: it takes the selected span
    /// out of the material it has *mapped* and puts it on the clipboard. A
    /// source it cannot read — a mapped pyramid is an overview, a live view has
    /// no addressable past — **declines, visibly**, because putting silence on
    /// the clipboard is the one answer worse than saying no. A **cut** and a
    /// **paste** change data, which the host does not own, so they leave as
    /// intents and the owner answers with what the composition now is.
    ///
    /// A paste carries the clipboard **with** it (`"paste" position kind json
    /// [blob…]`), rather than the owner keeping a clipboard of its own: the
    /// clipboard is the host's precisely so that a block copied in one window
    /// pastes in another, against a different owner or none.
    pub fn clipboard_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: ClipVerb,
        cx: f64,
        cy: f64,
        clip: &mut Clip,
    ) -> Option<Vec<GestureEffect>> {
        let Hit { id, .. } = hit(host, ctx, cx, cy)?;
        let key = host.timeline_key(id)?;
        let (start, len) = host.timelines().state(key)?.selection().unzip();
        let mut out = Vec::new();
        match verb {
            ClipVerb::Copy => {
                let (start, len) = (start?, len?);
                let offset = host.widget_kind(ctx.def_id, id)?.editor()?.offset;
                // The selection is in **timeline** samples and an element reads
                // its own frames: a clip placed late holds sample 0 at its
                // offset, which is the one conversion between the axis and the
                // material on it.
                let from = (start - offset).max(0.0) as u64;
                let block = element_block(host, ctx, id, from, len as u64);
                match block {
                    Some(block) => {
                        clip.put_samples(block.samples.into(), block.channels, block.sample_rate);
                        // A copy changed nothing, so it reports nothing.
                    }
                    // Said out loud, in the one direction the host has: the
                    // owner learns the reader could not read, which is what a
                    // refusal is for.
                    None => emit(
                        host,
                        &mut out,
                        ctx.def_id,
                        id,
                        vec![
                            OscType::String("refused".into()),
                            OscType::String("copy".into()),
                            OscType::String("this source has no samples the host can read".into()),
                        ],
                    ),
                }
            }
            ClipVerb::Cut => {
                let (start, len) = (start?, len?);
                emit(
                    host,
                    &mut out,
                    ctx.def_id,
                    id,
                    vec![
                        OscType::String("cut".into()),
                        OscType::Float(start as f32),
                        OscType::Float(len as f32),
                    ],
                );
            }
            ClipVerb::Paste => {
                let doc = clip.doc()?;
                if !clip.is_whole() {
                    // A header whose payload did not travel: declining is the
                    // whole reason `blobs()` is on the clipboard at all.
                    emit(
                        host,
                        &mut out,
                        ctx.def_id,
                        id,
                        vec![
                            OscType::String("refused".into()),
                            OscType::String("paste".into()),
                            OscType::String(
                                "the clipboard's payload did not travel with it".into(),
                            ),
                        ],
                    );
                    return Some(out);
                }
                let mut args = vec![
                    OscType::String("paste".into()),
                    // Where: the selection's start, which is where a locate or a
                    // sweep last put the axis -- a paste has no pointer of its
                    // own, and the cursor is what the reader was looking at.
                    OscType::Float(start.unwrap_or(0.0) as f32),
                    OscType::String(doc.kind().into()),
                    OscType::String(doc.to_json()),
                ];
                for i in 0..doc.blobs() {
                    if let Some(bytes) = clip.blob_bytes(i) {
                        args.push(OscType::Blob(bytes));
                    }
                }
                emit(host, &mut out, ctx.def_id, id, args);
            }
        }
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

/// Which of the three clipboard verbs a key asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipVerb {
    Copy,
    Cut,
    Paste,
}

/// The material behind widget `id` over `frames` of its own frames from
/// `start` — the element's own answer ([`crate::host::widget::element::Element::sample_block`]), since only
/// it knows what it holds and whether it may be read.
fn element_block(
    host: &mut Host,
    ctx: &GestureCtx,
    id: i32,
    start: u64,
    frames: u64,
) -> Option<SampleBlock> {
    host.widget_kind(ctx.def_id, id)?
        .as_element()?
        .sample_block(start, frames, ctx.sample_rate)
}
