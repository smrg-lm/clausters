//! The keyboard half of the machine: the focus ring, the key that goes to the
//! focused element, and the block operations a timeline view answers to
//! (quantize, cut/copy/paste over the multi-note selection, resetting every view
//! to its full extent).
//!
//! Split from the pointer machine because it shares nothing with it but the
//! `Gestures` state: no hit-test, no drag, no cursor -- a key arrives already
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
use super::super::play::Pass;
use crate::host::diag;
use clausters_core::osc::OscType;

use super::super::clipboard::Clip;
use super::super::widget::element::{Key, KeyInput, Mods, SampleBlock, refusal};
use super::effects::{emit, emit_view, redraw_all};
use super::nav::{cursor_of, freq_nav_ids, hit, set_x_view, set_y_view, timeline_ids};
use super::{GestureCtx, GestureEffect, Gestures, element, focus};

impl Gestures {
    /// A key arriving at this window: Tab walks the focus ring, anything else
    /// goes to the focused element's
    /// [`Element::key`](crate::host::widget::Element::key) -- which delivers
    /// whatever it reports exactly as a drag would, bound -> straight to the
    /// audio server, else a `/gui_event`.
    ///
    /// `clipboard` is the host-wide clipboard a cut/copy/paste reads and writes
    /// (the native front's internal one; the browser front swaps the page's
    /// string in and out around this call).
    ///
    /// Returns `Some(effects)` when the key was consumed -- the front then skips
    /// its own shortcuts -- and `None` when nothing here answered it.
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
            cursor: cursor_of(host, ctx, id),
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
    /// cursor** -- the other addressee, and the reason a field can swallow `q`
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
            cursor: cursor_of(host, ctx, id),
        };
        let at = element::At::widget(id, rect, scale, indent);
        let Some(events) = element::with(host, ctx, at, |el, _| el.key(&key, &mut input)).flatten()
        else {
            // **A key nothing claimed is the quietest failure there is**, and
            // it is the shape of the defect reported twice on 2026-09-12: a
            // verb refused correctly by an element that had nothing to act on
            // and a letter no element answers to are indistinguishable at the
            // window. The element's own refusals are said out loud now; this is
            // the other case, and it is the machine's business rather than the
            // hand's -- so it is a note, and only a debug build carries it.
            diag::note!(
                host,
                ctx.def_id,
                "key",
                "key {key:?}: widget {id} did not take it"
            );
            return None;
        };
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
    /// **The host holds no history** -- the log lives with the document, in
    /// `clausters-document`, because a log a view keeps sees only the gestures
    /// *it* made. So this is a route and not an action: it emits
    /// `/gui_event <window_id> <seq> <version> "undo"|"redo"` and the owner
    /// answers with the state that now holds, exactly as it answers a drag.
    ///
    /// It is addressed to the **window** rather than to a widget because that
    /// is what it is scoped to: undo is not addressed to a place under the
    /// cursor, which is why it is not a step in the gesture plan -- a
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

    /// **Plays the contents under the cursor, or stops what is playing** -- the
    /// editor's monitor, on the space bar.
    ///
    /// Addressed by the pointer for the same reason a copy is: a window may
    /// hold several takes, and what the hand is over is the one it means. It is
    /// the host's own action and not an intent -- sounding a take changes
    /// nothing, so there is nobody to report it to (a host driven by a script
    /// plays through that script's own transport).
    ///
    /// Returns `None` when there is nothing under the cursor to play, so the
    /// key falls through to whatever else the window does with it.
    ///
    /// **Where it starts and whether it repeats are read off the view**, not
    /// asked for: a selection plays as a loop over exactly the span it covers,
    /// and with no selection the take plays from the position cursor. The transport is
    /// what carries both -- a locate and a loop span -- so nothing here computes
    /// a time or keeps one in step.
    pub fn play_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        // **Space is play/stop, and a stop goes back to the position cursor.**
        // A monitor already sounding is stopped and the transport is located
        // at the mark the reader put down -- not left wherever the pass ended
        // -- so the next press plays from the same place, as a multitrack's
        // does.
        //
        // The one thing still addressed by the pointer is *which* take to load:
        // space over a take the monitor is not holding plays that one instead,
        // so a window of several takes is driven by pointing at them. Over
        // nothing at all it is the transport that is meant, and there is one.
        let over = hit(host, ctx, cx, cy).map(|Hit { id, .. }| id);
        if let Some(loaded) = host.monitor()
            && over.is_none_or(|id| id == loaded.widget)
        {
            let mark =
                position_cursor(host, loaded.widget).unwrap_or(start_of(host, loaded.widget).0);
            host.stop_playback();
            host.locate(mark);
            return Some(vec![GestureEffect::Redraw(ctx.def_id)]);
        }
        let id = over?;
        let (start, span) = start_of(host, id);
        let frames = host.buffer_frames(ctx.def_id, id)?;
        // **How the pass ends is the loop switch's** (`L`). Looping, a
        // selection repeats and so does a take with none. Not looping, the
        // pass stops at the end of the selection or of the take, and the
        // transport goes back to the position cursor -- where a selection's
        // play started, when there is no cursor apart from it.
        let pass = if host.monitor_loops() {
            let (from, to) = span.unwrap_or((0, frames));
            Pass::Loop(from, to)
        } else {
            let end = span.map_or(frames, |(_, to)| to);
            let back = position_cursor(host, id).unwrap_or(start);
            Pass::Until { end, back }
        };
        host.play_buffer(ctx.def_id, id, start, pass)
            .then(|| vec![GestureEffect::Redraw(ctx.def_id)])
    }

    /// **`L` switches the monitor's loop**, and the status bar says which way
    /// it went -- the one place the state is shown, since the applications have
    /// no transport row. It takes effect on the next play.
    pub fn loop_key(&self, host: &mut Host, ctx: &GestureCtx) -> Vec<GestureEffect> {
        let looping = host.toggle_monitor_loop();
        host.say(
            ctx.def_id,
            crate::host::status::Line {
                kind: crate::host::status::Kind::Did,
                widget: None,
                verb: "loop".into(),
                text: if looping { "loop on" } else { "loop off" }.into(),
            },
        );
        vec![GestureEffect::Redraw(ctx.def_id)]
    }

    /// **Home and End: the position cursor to the start or the end of the
    /// samples under the pointer** -- the same placing a click on the body
    /// makes, so the owner is told where the mark went and a play that follows
    /// starts there.
    ///
    /// Returns `None` over anything that draws no samples, so the key falls
    /// through to whatever else the window does with it.
    pub fn ends_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        to_end: bool,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        let Hit { id, .. } = hit(host, ctx, cx, cy)?;
        let frames = host.buffer_frames(ctx.def_id, id)?;
        let pos = if to_end { frames as f64 } else { 0.0 };
        let mut out = Vec::new();
        super::nav::locate_at(host, &mut out, ctx, id, pos);
        Some(out)
    }

    /// **Copy, cut and paste over the selection**, addressed to the view under
    /// the cursor -- the window's own shortcuts, reached only by a key nothing
    /// focused and nothing under the cursor answered first (a field's Ctrl+C is
    /// still the field's).
    ///
    /// The three verbs split exactly where the host's authority does. A **copy**
    /// is a read, and the host may honestly do it: it takes the selected span
    /// out of the contents it has *mapped* and puts it on the clipboard. A
    /// source it cannot read -- a mapped pyramid is an overview, a live view has
    /// no addressable past -- **declines, visibly**, because putting silence on
    /// the clipboard is the one answer worse than saying no. A **cut** and a
    /// **paste** change data, which the host does not own, so they leave as
    /// intents and the owner answers with what the document now is.
    ///
    /// A paste carries the clipboard **with** it (`"paste" position kind json
    /// [blob...]`), rather than the owner keeping a clipboard of its own: the
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
        // The pointer names the addressee whenever it is over a view; when it
        // is over the window's margin -- or off the window, which is where a
        // sweep to the first or last sample leaves it -- the window's most
        // recent selection does (`Host::selection_addressee`).
        let id = match hit(host, ctx, cx, cy).filter(|h| host.timeline_key(h.id).is_some()) {
            Some(Hit { id, .. }) => id,
            None => host.selection_addressee(ctx.def_id)?,
        };
        let key = host.timeline_key(id)?;
        let state = *host.timelines().state(key)?;
        let (start, len) = state.selection().unzip();
        // **Where a paste lands is the cursor, span or no span**: a click
        // leaves a selection of zero length whose start is the cursor, which
        // `selection()` -- the spans only -- does not answer, and a paste read
        // that way always landed at frame 0. The same reading `play_key` makes.
        let cursor = state.sel_start.max(0.0);
        let mut out = Vec::new();
        match verb {
            ClipVerb::Copy => {
                let (start, len) = (start?, len?);
                copy_selection(host, ctx, id, start, len, clip, &mut out, "copy")?;
            }
            // **A cut is a copy and a removal**, and the copy half is the
            // host's to make, as a copy is: the block goes on the clipboard
            // first, and only a cut whose block is on it asks the owner to take
            // the span out. One the host cannot read declines, like a copy --
            // taking something out that nothing holds any more would be the
            // one cut worse than none.
            ClipVerb::Cut => {
                let (start, len) = (start?, len?);
                if !copy_selection(host, ctx, id, start, len, clip, &mut out, "cut")? {
                    out.push(GestureEffect::Redraw(ctx.def_id));
                    return Some(out);
                }
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
            ClipVerb::Paste | ClipVerb::Mix => {
                let tag = if verb == ClipVerb::Mix {
                    "mix"
                } else {
                    "paste"
                };
                let doc = clip.doc()?;
                if !clip.is_whole() {
                    // A header whose payload did not travel: declining is the
                    // whole reason `blobs()` is on the clipboard at all.
                    emit(
                        host,
                        &mut out,
                        ctx.def_id,
                        id,
                        refusal(tag, "the clipboard's payload did not travel with it"),
                    );
                    return Some(out);
                }
                let mut args = vec![
                    OscType::String(tag.into()),
                    // Where: the selection's start, which is where a locate or a
                    // sweep last put the axis -- a paste has no pointer of its
                    // own, and the cursor is what the reader was looking at.
                    OscType::Float(cursor as f32),
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

    /// `R` over a window: reset every navigable view's axes -- a timeline's
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
        // -- the same key, since to a reader it is the same "show me all of it".
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

/// Which of the clipboard verbs a key asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipVerb {
    Copy,
    Cut,
    Paste,
    /// A paste that **adds** the block onto what is there rather than putting
    /// it in -- the same payload, answered by the owner as a mix.
    Mix,
}

/// **The selected span onto the clipboard**, or the refusal said to the owner
/// under `verb` when the host cannot read it. Answers whether it copied.
#[allow(clippy::too_many_arguments)] // the gesture's own context, spelled out
fn copy_selection(
    host: &mut Host,
    ctx: &GestureCtx,
    id: i32,
    start: f64,
    len: f64,
    clip: &mut Clip,
    out: &mut Vec<GestureEffect>,
    verb: &str,
) -> Option<bool> {
    let offset = host.widget_kind(ctx.def_id, id)?.editor()?.offset;
    // The selection is in **timeline** samples and an element reads its own
    // frames: a clip placed late holds sample 0 at its offset, which is the
    // one conversion between the axis and the contents on it.
    let from = (start - offset).max(0.0) as u64;
    match element_block(host, ctx, id, from, len as u64) {
        Some(block) => {
            clip.put_samples(block.samples.into(), block.channels, block.sample_rate);
            Some(true)
        }
        // Said out loud, in the one direction the host has: the owner learns
        // the reader could not read, which is what a refusal is for.
        None => {
            emit(
                host,
                out,
                ctx.def_id,
                id,
                refusal(verb, "this source has no samples the host can read"),
            );
            Some(false)
        }
    }
}

/// The contents behind widget `id` over `frames` of its own frames from
/// `start` -- the element's own answer ([`crate::host::widget::element::Samples::sample_block`]), since only
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
        .samples()?
        .sample_block(start, frames, ctx.sample_rate)
}

/// The position cursor of view `id`, when one is placed.
fn position_cursor(host: &Host, id: i32) -> Option<u64> {
    let key = host.timeline_key(id)?;
    let cursor = host.timelines().state(key)?.cursor()?;
    Some(cursor.max(0.0) as u64)
}

/// **Where a play over view `id` starts, and the span it loops** -- read off
/// the view, never asked for. A selection plays as a loop over exactly the
/// span it covers, from its start; with none it plays from the position
/// cursor, and from frame 0 where no cursor has been placed.
fn start_of(host: &Host, id: i32) -> (u64, Option<(u64, u64)>) {
    let state = host
        .timeline_key(id)
        .and_then(|key| host.timelines().state(key))
        .copied();
    let span = state
        .and_then(|s| s.selection())
        .map(|(from, len)| (from.max(0.0) as u64, (from + len).max(0.0) as u64));
    let start = match span {
        Some((from, _)) => from,
        None => state.and_then(|s| s.cursor()).unwrap_or(0.0).max(0.0) as u64,
    };
    (start, span)
}
