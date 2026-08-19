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

/// The two edit verbs a clip's **placement** answers to: cutting one in two,
/// and reading two as one. Named here rather than taken as a bool because the
/// front spells them out and a bool at a call site says nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipEdit {
    Split,
    Join,
}

/// The clips on `lane` that **touch** the clip `id` — the ones whose spans meet
/// its own, in the order they sit on the axis, `id` among them.
///
/// `None` when nothing touches it, which is what makes a join over a lone clip
/// fall through instead of reporting an operation with one operand.
fn touching_clips(host: &Host, def_id: i32, lane: i32, id: i32) -> Option<Vec<i32>> {
    use crate::host::widget::WidgetKind;
    let tree = host.window_def(def_id)?;
    let span = |w: &crate::host::widget::Widget| match w.kind {
        WidgetKind::Clip { offset, dur, .. } => Some((offset, offset + dur)),
        _ => None,
    };
    let lane_widget = tree.find(lane)?;
    let mut clips: Vec<(f64, f64, i32)> = lane_widget
        .children
        .iter()
        .filter_map(|c| {
            let (from, to) = span(c)?;
            Some((from, to, c.id?))
        })
        .collect();
    clips.sort_by(|a, b| a.0.total_cmp(&b.0));
    let me = clips.iter().position(|(_, _, cid)| *cid == id)?;
    // A pixel of slack has no meaning on a sample axis: touching is touching,
    // and two clips a sample apart are two clips.
    let touches = |a: &(f64, f64, i32), b: &(f64, f64, i32)| a.1 >= b.0 && b.1 >= a.0;
    let mut run = vec![clips[me]];
    for c in clips[..me].iter().rev() {
        match touches(c, run.first()?) {
            true => run.insert(0, *c),
            false => break,
        }
    }
    for c in &clips[me + 1..] {
        match touches(run.last()?, c) {
            true => run.push(*c),
            false => break,
        }
    }
    (run.len() > 1).then(|| run.into_iter().map(|(_, _, cid)| cid).collect())
}

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

    /// **Plays the contents under the cursor, or stops what is playing** — the
    /// editor's monitor, on the space bar.
    ///
    /// Addressed by the pointer for the same reason a copy is: a window may
    /// hold several takes, and what the hand is over is the one it means. It is
    /// the host's own action and not an intent — sounding a take changes
    /// nothing, so there is nobody to report it to (a host driven by a script
    /// plays through that script's own transport).
    ///
    /// Returns `None` when there is nothing under the cursor to play, so the
    /// key falls through to whatever else the window does with it.
    ///
    /// **Where it starts and whether it repeats are read off the view**, not
    /// asked for: a selection plays as a loop over exactly the span it covers,
    /// and with no selection the take plays from its start. The transport is
    /// what carries both — a locate and a loop span — so nothing here computes
    /// a time or keeps one in step.
    pub fn play_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        // **Space is play/pause, and a loaded monitor is never reloaded.** A
        // monitor already sounding is paused where it stands; a paused one
        // resumes and *continues*, because the governed group froze with its
        // readers' state intact and the position froze with it. Neither needs a
        // start: where to begin was decided when the take was loaded, and moved
        // since by whatever swept over it.
        //
        // The one thing still addressed by the pointer is *which* take to load:
        // space over a take the monitor is not holding plays that one instead,
        // so a window of several takes is driven by pointing at them. Over
        // nothing at all it is the transport that is meant, and there is one.
        let over = hit(host, ctx, cx, cy).map(|Hit { id, .. }| id);
        if let Some(loaded) = host.monitor()
            && over.is_none_or(|id| id == loaded.widget)
        {
            host.pause_playback();
            return Some(Vec::new());
        }
        let id = over?;
        let state = host
            .timeline_key(id)
            .and_then(|key| host.timelines().state(key))
            .copied();
        // **The cursor is where it starts, span or no span.** A click leaves a
        // selection of zero length, and its start is the cursor — reading only
        // the *spans*, as this did, sent every play back to frame 0 and made
        // the click that placed the head look like it had done nothing.
        let start = state.map_or(0.0, |s| s.sel_start).max(0.0) as u64;
        let span = state
            .and_then(|s| s.selection())
            .map(|(from, len)| (from.max(0.0) as u64, (from + len).max(0.0) as u64));
        host.play_buffer(ctx.def_id, id, start, span).then(Vec::new)
    }

    /// **Cutting a clip in two, and joining two into one** — the placement
    /// layer's own edit verbs, addressed by the pointer like every other verb
    /// over a view.
    ///
    /// Both are **intents**, because both change what the composition holds and
    /// the host holds nothing: `"split" <t>` names the clip-local time to cut
    /// at, and `"join" <id…>` names the clips to read as one. What the owner
    /// makes of them is its own — two windows over one source for a split, one
    /// element reading several segments for a join — and it answers with the
    /// tree that now stands, exactly as it answers a drag.
    ///
    /// **Where the cut falls is the time cursor**, which is what a multitrack
    /// means by splitting a clip: the axis' selection start when it lies inside
    /// the clip, and the pointer when it does not — so the gesture works with
    /// or without a placed cursor, and the cut is always somewhere the reader
    /// can see. A join takes the clip under the pointer and the clips that
    /// **touch** it on the same lane, which is the "two juxtaposed clips" the
    /// operation is described in terms of, without a second selection model to
    /// keep in step.
    ///
    /// Returns `None` when the pointer is not over a clip, so the key falls
    /// through to whatever else the window does with it.
    pub fn clip_verb(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        verb: ClipEdit,
        cx: f64,
        cy: f64,
    ) -> Option<Vec<GestureEffect>> {
        let h = hit(host, ctx, cx, cy)?;
        let lane = crate::host::interact::time_of(&h.chain)?;
        let local = crate::host::interact::local_time_of(&h.chain)?;
        let clip = crate::host::interact::clip_hit(host, ctx.def_id, lane, local, cx)?;
        let mut out = Vec::new();
        let args = match verb {
            ClipEdit::Split => {
                // The cursor's own timeline position, turned into the clip's
                // time: a cut is stated in the clip's terms, because that is
                // what the owner cuts.
                let head = host
                    .timeline_key(lane.0)
                    .and_then(|key| host.timelines().state(key))
                    .map(|s| s.sel_start)
                    .filter(|t| {
                        *t > clip.placement.offset && *t < clip.placement.offset + clip.dur
                    });
                let at = head.unwrap_or_else(|| {
                    crate::host::interact::sample_at(
                        clip.nav.start,
                        clip.nav.len,
                        clip.body.x as f64,
                        clip.body.w as f64,
                        cx,
                    )
                }) - clip.placement.offset;
                // A cut at either edge is not a cut: it would leave a clip of
                // nothing beside the one that was already there.
                if at <= 0.0 || at >= clip.dur {
                    return None;
                }
                vec![OscType::String("split".into()), OscType::Float(at as f32)]
            }
            ClipEdit::Join => {
                let others = touching_clips(host, ctx.def_id, lane.0, clip.id)?;
                std::iter::once(OscType::String("join".into()))
                    .chain(others.into_iter().map(OscType::Int))
                    .collect()
            }
        };
        emit(host, &mut out, ctx.def_id, clip.id, args);
        Some(out)
    }

    /// **Copy, cut and paste over the selection**, addressed to the view under
    /// the cursor — the window's own shortcuts, reached only by a key nothing
    /// focused and nothing under the cursor answered first (a field's Ctrl+C is
    /// still the field's).
    ///
    /// The three verbs split exactly where the host's authority does. A **copy**
    /// is a read, and the host may honestly do it: it takes the selected span
    /// out of the contents it has *mapped* and puts it on the clipboard. A
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
        // The pointer names the addressee whenever it is over a view; when it
        // is over the window's margin — or off the window, which is where a
        // sweep to the first or last sample leaves it — the window's most
        // recent selection does (`Host::selection_addressee`).
        let id = match hit(host, ctx, cx, cy).filter(|h| host.timeline_key(h.id).is_some()) {
            Some(Hit { id, .. }) => id,
            None => host.selection_addressee(ctx.def_id)?,
        };
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
                // contents on it.
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

/// The contents behind widget `id` over `frames` of its own frames from
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
