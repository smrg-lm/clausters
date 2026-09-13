//! winit-side input adapters: translate pointer/wheel/keyboard events into
//! calls on the shared gesture machine ([`crate::host::gestures`]) and carry
//! out its effects over this front's sinks — the OSC transports (`/gui_event`)
//! and winit redraw requests. All gesture *logic* lives in the machine; this file only snapshots the per-call context (frame
//! buffer size, modifiers, the GPU slots' lane counts) and applies effects.

use crate::host::gestures::{ClipVerb, GestureCtx, GestureEffect};
use crate::host::widget::element::Key as HostKey;
use crate::host::widget::element::SlotKey;

use super::app::App;

impl App {
    /// Snapshots the gesture context for one window: framebuffer size,
    /// modifier keys, and the heavy views' lane counts (channel/lane splits
    /// live in this front's GPU slots, so they are copied out here).
    fn gesture_ctx(&self, def_id: i32) -> GestureCtx {
        let (fb_w, fb_h) = self.fb(def_id);
        let mut ctx = GestureCtx::new(def_id, fb_w, fb_h);
        // The same rate the frame draws with, so a gesture over a measured
        // axis resolves the same hertz the reader is looking at.
        ctx.sample_rate = self.shm.as_ref().map_or(0.0, |s| s.sample_rate());
        // ...and the clock the frame sweeps the playhead with, for the same
        // reason: a click that locates while the transport runs re-anchors the
        // sweep, and it must land where the line is drawn.
        ctx.sample_clock = self.host.playhead_clock(self.shm.as_deref());
        // ...and this front's own wall clock, which is what tells one press
        // from the second of a double click. The rule is the machine's; the
        // clock is the platform's, because there is none in the shared core.
        ctx.now_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        if let Some(ws) = self.windows.get(&def_id) {
            ctx.shift = ws.shift;
            ctx.ctrl = ws.ctrl;
            ctx.alt = ws.alt;
            // **The widget's own picture, not a body's**: the row count a
            // gesture divides by is the view's, and a box inside a view has
            // rows of its own that mean nothing to the axis around it.
            for ((id, key), slot) in &ws.waveforms {
                if *key == SlotKey::SELF {
                    ctx.slot_channels.insert(*id, slot.view.num_channels());
                }
            }
            for ((id, key), slot) in &ws.spectrograms {
                if *key == SlotKey::SELF {
                    ctx.slot_channels.insert(*id, slot.views.len());
                }
            }
        }
        ctx
    }

    /// Whether window `def_id` holds a clip drag pinned against a lane's edge,
    /// so the frame tick must keep running: the view scrolls under a standing
    /// cursor, which sends no events of its own.
    pub(super) fn window_is_edge_scrolling(&self, def_id: i32) -> bool {
        self.windows.get(&def_id).is_some_and(|ws| {
            // No cursor is not a cursor at the far left: a window the
            // pointer is not over edge-scrolls nothing.
            ws.cursor
                .is_some_and(|(cx, _)| ws.gestures.edge_scrolling(cx))
        })
    }

    /// The frame step of every edge-held clip drag: pans the view and carries
    /// the clip with it, so a clip travels further than one window's worth.
    pub(super) fn advance_edge_scroll(&mut self, dt: f64) {
        let dragging: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_is_edge_scrolling(*id))
            .collect();
        for def_id in dragging {
            let ctx = self.gesture_ctx(def_id);
            let Some(ws) = self.windows.get_mut(&def_id) else {
                continue;
            };
            let Some((cx, cy)) = ws.cursor else {
                continue;
            };
            let effects = ws.gestures.tick(&mut self.host, &ctx, cx, cy, dt);
            self.apply_gesture_effects(effects);
        }
    }

    /// Carries out a gesture's effects: events over the window's transport,
    /// repaints.
    fn apply_gesture_effects(&mut self, effects: Vec<GestureEffect>) {
        for effect in effects {
            match effect {
                GestureEffect::Emit {
                    def_id,
                    widget_id,
                    seq,
                    args,
                } => {
                    // **One message, whoever it goes to**: a host that owns
                    // what it draws is delivered it in memory, and every other
                    // one sends it and waits, as it always has.
                    let message = self.host.event_message(widget_id, seq, args);
                    if self.host.deliver(def_id, &message) {
                        self.redraw(def_id);
                    } else {
                        self.emit(def_id, message);
                    }
                }
                GestureEffect::Redraw(def_id) => self.redraw(def_id),
                // A desktop window is not inside a document: there is nothing
                // to hand the focus back to, so the ring simply runs out and
                // the next Tab enters it again. (In a page this is what keeps a
                // mounted GuiDef from trapping the keyboard — `web::input`.)
                GestureEffect::FocusOut(_) => {}
            }
        }
    }

    /// Press on a widget: the machine acts by kind and possibly starts a drag.
    pub(super) fn on_press(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).and_then(|w| w.cursor) else {
            return;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.press(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Pointer moved while a drag is active: the machine drives the target.
    pub(super) fn on_drag(&mut self, def_id: i32, cx: f64, cy: f64) {
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.drag_to(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Release: the machine finishes the drag (button up, wire landing).
    pub(super) fn on_release(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).and_then(|w| w.cursor) else {
            return;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.release(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Wheel: the machine zooms the time axis or the vertical display window.
    pub(super) fn on_wheel(&mut self, def_id: i32, steps: f64) {
        let Some((cx, cy)) = self.windows.get(&def_id).and_then(|w| w.cursor) else {
            return;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.wheel(&mut self.host, &ctx, cx, cy, steps);
        self.apply_gesture_effects(effects);
    }

    // ---- keyboard operations (dispatched from `window_event`) ----

    /// Routes a key to the window's focus — the ring for Tab, the focused
    /// element for everything else. Returns whether it was consumed (so the
    /// caller skips the global editor shortcuts).
    pub(super) fn key_input(&mut self, def_id: i32, key: HostKey) -> bool {
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return false;
        };
        let effects = match ws
            .gestures
            .key(&mut self.host, &ctx, key, &mut self.text_clipboard)
        {
            Some(effects) => effects,
            None => return false,
        };
        self.apply_gesture_effects(effects);
        true
    }

    /// Routes a key the focus did not answer to the element **under the
    /// cursor** — the block operations of a view, addressed where the pointer
    /// already is. Returns whether it was consumed.
    pub(super) fn key_at_cursor(&mut self, def_id: i32, key: HostKey) -> bool {
        let Some((cx, cy)) = self.windows.get(&def_id).and_then(|w| w.cursor) else {
            return false;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return false;
        };
        let effects = match ws.gestures.key_at_cursor(
            &mut self.host,
            &ctx,
            key,
            cx,
            cy,
            &mut self.text_clipboard,
        ) {
            Some(effects) => effects,
            None => return false,
        };
        self.apply_gesture_effects(effects);
        true
    }

    /// Copy, cut or paste over the view under the cursor — the window's own
    /// shortcut, reached only by a key the focus and the element under the
    /// cursor both declined. Returns whether it was consumed.
    pub(super) fn clipboard_key(&mut self, def_id: i32, verb: ClipVerb) -> bool {
        let Some((cx, cy)) = self.windows.get(&def_id).and_then(|w| w.cursor) else {
            return false;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return false;
        };
        let effects = match ws.gestures.clipboard_key(
            &mut self.host,
            &ctx,
            verb,
            cx,
            cy,
            &mut self.text_clipboard,
        ) {
            Some(effects) => effects,
            None => return false,
        };
        self.apply_gesture_effects(effects);
        true
    }

    /// The space bar: play the take the cursor is over and stop what is
    /// playing, or — over nothing a take answers for — the window's own
    /// `play`, which a multitrack editor reads as play/pause. Returns whether it
    /// was consumed.
    pub(super) fn play_key(&mut self, def_id: i32) -> bool {
        if let Some((cx, cy)) = self.windows.get(&def_id).and_then(|w| w.cursor) {
            let ctx = self.gesture_ctx(def_id);
            if let Some(ws) = self.windows.get_mut(&def_id)
                && let Some(effects) = ws.gestures.play_key(&mut self.host, &ctx, cx, cy)
            {
                self.apply_gesture_effects(effects);
                return true;
            }
        }
        // **A piece is the window's, not the pointer's.** Its readers are
        // resident and follow the transport, so there is nothing to point at --
        // and requiring a pointer is what made the first press after opening a
        // window do nothing at all, since the cursor is unknown until it moves.
        // So the window is told, the way `Ctrl`+`Z` and `Ctrl`+`S` tell it, and
        // whoever edits the piece answers: this host's own editor, or a
        // script's.
        self.window_verb(def_id, clausters_apps::multitrack::editor::PLAY_KEY);
        true
    }

    /// Undo or redo over a window: the route to whoever owns the document.
    /// The host keeps no history, so this only reports (see
    /// [`Gestures::history`](crate::host::gestures::Gestures::history)).
    pub(super) fn history(&mut self, def_id: i32, redo: bool) {
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.history(&mut self.host, &ctx, redo);
        self.apply_gesture_effects(effects);
    }

    /// A verb addressed to the **window** rather than to anything under the
    /// cursor — the shape undo and redo already take, and for the same reason:
    /// what a save saves is the document behind the window. A host that **owns**
    /// that document answers it here; every other one emits it, and a script
    /// may answer.
    pub(super) fn window_verb(&mut self, def_id: i32, verb: &str) {
        let seq = self.host.outbox.borrow_mut().stamp(def_id, def_id);
        let args = vec![clausters_core::osc::OscType::String(verb.into())];
        let message = self.host.event_message(def_id, seq, args);
        if self.host.deliver(def_id, &message) {
            self.redraw(def_id);
        } else {
            self.emit(def_id, message);
        }
    }

    /// Whether a modifier is held on window `def_id`, for a shortcut the
    /// element machinery never sees (it takes its modifiers from the context).
    pub(super) fn ctrl(&self, def_id: i32) -> bool {
        self.windows.get(&def_id).is_some_and(|ws| ws.ctrl)
    }

    /// As [`Self::ctrl`], for Shift.
    pub(super) fn shift(&self, def_id: i32) -> bool {
        self.windows.get(&def_id).is_some_and(|ws| ws.shift)
    }

    /// As [`Self::ctrl`], for Alt — the other modifier that makes a letter a
    /// command rather than a character (`key_pressed` reads the pair).
    pub(super) fn alt(&self, def_id: i32) -> bool {
        self.windows.get(&def_id).is_some_and(|ws| ws.alt)
    }

    pub(super) fn reset_timelines(&mut self, def_id: i32) {
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.reset_timelines(&mut self.host, &ctx);
        self.apply_gesture_effects(effects);
    }
}
