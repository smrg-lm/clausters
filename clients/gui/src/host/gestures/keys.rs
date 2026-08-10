//! The keyboard half of the machine: editing a focused `text` field, and the
//! block operations a timeline view answers to (quantize, cut/copy/paste over
//! the multi-note selection, resetting every view to its full extent).
//!
//! Split from the pointer machine because it shares nothing with it but the
//! `Gestures` state: no hit-test, no drag, no cursor — a key arrives already
//! addressed to whatever the window has focused or selected.

use crate::viewport::View;

use super::super::interact::{self, Hit};
use super::super::widget::WidgetKind;
use super::super::{Host, pianoroll, textedit};
use super::effects::{emit_notes, emit_value, emit_view, redraw_all};
use super::nav::{freq_nav_ids, hit, set_x_view, set_y_view, timeline_ids};
use super::{GestureCtx, GestureEffect, Gestures, TextKey};

impl Gestures {
    /// A key while a `text` field is focused: edit it and, on any content
    /// change, deliver its new string exactly as a numeric control delivers on a
    /// drag — bound → straight to the audio server, else a `/gui_event`, on
    /// **every** keystroke (never gated on Enter). Modifiers ride in `ctx`
    /// (`shift` extends a selection, `ctrl` word-jumps and drives
    /// cut/copy/paste/select-all). Clipboard cut/copy/paste use the host-wide
    /// `clipboard` (the native internal clipboard; the browser front swaps in the
    /// OS clipboard around this call).
    ///
    /// Returns `Some(effects)` when the key was consumed by the focused field
    /// (the front then skips its global editor shortcuts), or `None` when no
    /// text field is focused in this window (the front runs its shortcuts).
    pub fn text_key(
        &self,
        host: &mut Host,
        ctx: &GestureCtx,
        key: TextKey,
        clipboard: &mut String,
    ) -> Option<Vec<GestureEffect>> {
        let def_id = ctx.def_id;
        // Only when a field in *this* window holds the focus.
        let (fdef, id) = host.focused_text()?;
        if fdef != def_id {
            return None;
        }
        let mut out = Vec::new();
        let mut changed = false;
        let edit =
            |host: &mut Host,
             f: &mut dyn FnMut(&mut String, &mut textedit::Caret, bool) -> bool| {
                interact::text_edit(host, def_id, id, |v, c, ml| f(v, c, ml)).unwrap_or(false)
            };

        match key {
            TextKey::Char(c) if ctx.ctrl => match c.to_ascii_lowercase() {
                'c' => {
                    if let Some(Some(s)) = interact::text_edit(host, def_id, id, |v, c, _| {
                        textedit::selected(v, c).map(str::to_string)
                    }) {
                        *clipboard = s;
                    }
                }
                'x' => {
                    let cut = &mut *clipboard;
                    changed = edit(host, &mut |v, c, _| {
                        if let Some(s) = textedit::selected(v, c) {
                            *cut = s.to_string();
                            textedit::delete_selection(v, c)
                        } else {
                            false
                        }
                    });
                }
                'v' => {
                    let paste = clipboard.clone();
                    if !paste.is_empty() {
                        changed = edit(host, &mut |v, c, ml| {
                            let text = if ml {
                                paste.clone()
                            } else {
                                paste.replace('\n', " ")
                            };
                            textedit::insert(v, c, &text)
                        });
                    }
                }
                'a' => {
                    edit(host, &mut |v, c, _| {
                        textedit::select_all(v, c);
                        false
                    });
                }
                _ => {} // another Ctrl combo: consumed but inert
            },
            // A plain (or Alt-less) printable char inserts; Alt combos are inert.
            TextKey::Char(c) if !ctx.alt => {
                changed = edit(host, &mut |v, cc, _| {
                    textedit::insert(v, cc, c.encode_utf8(&mut [0; 4]))
                });
            }
            TextKey::Char(_) => {}
            TextKey::Backspace => changed = edit(host, &mut |v, c, _| textedit::backspace(v, c)),
            TextKey::Delete => changed = edit(host, &mut |v, c, _| textedit::delete(v, c)),
            TextKey::Left => {
                let word = ctx.ctrl;
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    if word {
                        textedit::move_word_left(v, c, sel);
                    } else {
                        textedit::move_left(v, c, sel);
                    }
                    false
                });
            }
            TextKey::Right => {
                let word = ctx.ctrl;
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    if word {
                        textedit::move_word_right(v, c, sel);
                    } else {
                        textedit::move_right(v, c, sel);
                    }
                    false
                });
            }
            TextKey::Up => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    textedit::move_up(v, c, sel);
                    false
                });
            }
            TextKey::Down => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    textedit::move_down(v, c, sel);
                    false
                });
            }
            TextKey::Home => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    textedit::move_home(v, c, sel);
                    false
                });
            }
            TextKey::End => {
                let sel = ctx.shift;
                edit(host, &mut |v, c, _| {
                    textedit::move_end(v, c, sel);
                    false
                });
            }
            TextKey::Enter => {
                changed = edit(host, &mut |v, c, ml| {
                    if ml {
                        textedit::insert(v, c, "\n")
                    } else {
                        false // a single-line field ignores Enter (no send-on-Enter)
                    }
                });
            }
        }

        // The focused field always repaints (the caret/selection moved); a
        // content change also delivers the new value, ungated.
        out.push(GestureEffect::Redraw(def_id));
        if changed {
            emit_value(host, &mut out, def_id, id);
        }
        Some(out)
    }

    /// `q` over a piano-roll: quantize the selected notes' onsets (all of them
    /// when nothing is selected) to the widget's `snap` grid — the same grid a
    /// drag snaps to. Durations are kept; a roll with no grid is left alone.
    /// (The client-side counterpart, in beats over the model, is the Python
    /// `Timeline.quantize` — the standalone host cannot reach it, hence both.)
    pub fn quantize(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            kind: WidgetKind::PianoRoll { snap, .. },
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let moved = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            pianoroll::quantize_notes(notes, sel, snap)
        })
        .unwrap_or(false);
        if moved {
            host.sync_track_totals();
            emit_notes(host, &mut out, def_id, id);
            out.push(GestureEffect::Redraw(def_id));
        }
        out
    }

    /// Ctrl+C / Ctrl+X over a piano-roll: copy the selected notes to the
    /// host-wide `clipboard`, normalized to the block's first onset (a cut also
    /// removes them) — host-wide so a block travels between rolls and windows.
    /// A no-op when the cursor is elsewhere or nothing is selected.
    pub fn copy_selected(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        cut: bool,
        clipboard: &mut Vec<pianoroll::Note>,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            kind: WidgetKind::PianoRoll { .. },
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let copied = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            let clip = pianoroll::copy_notes(notes, sel);
            if cut && !clip.is_empty() {
                pianoroll::remove_notes(notes, sel);
                sel.clear();
            }
            clip
        })
        .unwrap_or_default();
        if copied.is_empty() {
            return out;
        }
        *clipboard = copied;
        if cut {
            host.sync_track_totals();
            emit_notes(host, &mut out, def_id, id);
            out.push(GestureEffect::Redraw(def_id));
        }
        out
    }

    /// Ctrl+V over a piano-roll: paste the clipboard with its first onset at
    /// the cursor's time (snapped to the note grid), original pitches and
    /// spread kept. The pasted block becomes the new selection, ready to drag
    /// into place.
    pub fn paste_at_cursor(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
        clipboard: &[pianoroll::Note],
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        if clipboard.is_empty() {
            return out;
        }
        let Some(Hit {
            id,
            rect,
            kind: WidgetKind::PianoRoll { .. },
            chain,
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let Some((_, axis)) = interact::time_of(&chain) else {
            return out;
        };
        let Some(h) = interact::pianoroll_hit(host, def_id, (id, rect, axis), cx, cy) else {
            return out;
        };
        let nav = View {
            start: h.nav.start,
            len: h.nav.len,
        };
        let at = interact::snap(pianoroll::time_at(h.grid, &nav, 0.0, cx as f32), h.snap);
        interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            *sel = pianoroll::paste_notes(notes, clipboard, at);
        });
        host.sync_track_totals();
        emit_notes(host, &mut out, def_id, id);
        out.push(GestureEffect::Redraw(def_id));
        out
    }

    /// Delete/Backspace: remove every selected note of the piano-roll under the
    /// cursor — the block delete (Ctrl+click removes one). A no-op when the
    /// cursor is elsewhere or nothing is selected.
    pub fn delete_selected(
        &mut self,
        host: &mut Host,
        ctx: &GestureCtx,
        cx: f64,
        cy: f64,
    ) -> Vec<GestureEffect> {
        let mut out = Vec::new();
        let def_id = ctx.def_id;
        let Some(Hit {
            id,
            kind: WidgetKind::PianoRoll { .. },
            ..
        }) = hit(host, ctx, cx, cy)
        else {
            return out;
        };
        let removed = interact::pianoroll_state_edit(host, def_id, id, |notes, sel| {
            if sel.is_empty() {
                return false;
            }
            pianoroll::remove_notes(notes, sel);
            sel.clear();
            true
        })
        .unwrap_or(false);
        if removed {
            host.sync_track_totals();
            emit_notes(host, &mut out, def_id, id);
            out.push(GestureEffect::Redraw(def_id));
        }
        out
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
            set_x_view(host, &mut out, def_id, id, 0.0, 1.0);
        }
        out.push(GestureEffect::Redraw(def_id));
        out
    }
}
