//! winit-side input adapters: translate pointer/wheel/keyboard events into
//! calls on the shared gesture machine ([`crate::host::gestures`]) and carry
//! out its effects over this front's sinks — the OSC transports (`/gui_event`),
//! winit redraw requests, and the native pointer grab. All gesture *logic*
//! lives in the machine; this file only snapshots the per-call context (frame
//! buffer size, modifiers, the GPU slots' lane counts) and applies effects.

use tracing::debug;
use winit::window::{CursorGrabMode, Window};

use crate::host::gestures::{ClipVerb, GestureCtx, GestureEffect};
use crate::host::widget::element::Key as HostKey;

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
        if let Some(ws) = self.windows.get(&def_id) {
            ctx.shift = ws.shift;
            ctx.ctrl = ws.ctrl;
            ctx.alt = ws.alt;
            for (id, slot) in &ws.waveforms {
                ctx.slot_channels.insert(*id, slot.view.num_channels());
            }
            for (id, slot) in &ws.spectrograms {
                ctx.slot_channels.insert(*id, slot.views.len());
            }
        }
        ctx
    }

    /// Whether window `def_id` holds a clip drag pinned against a lane's edge,
    /// so the frame tick must keep running: the view scrolls under a standing
    /// cursor, which sends no events of its own.
    pub(super) fn window_is_edge_scrolling(&self, def_id: i32) -> bool {
        self.windows
            .get(&def_id)
            .is_some_and(|ws| ws.gestures.edge_scrolling(ws.cursor.0))
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
            let (cx, cy) = ws.cursor;
            let effects = ws.gestures.tick(&mut self.host, &ctx, cx, cy, dt);
            self.apply_gesture_effects(effects);
        }
    }

    /// Carries out a gesture's effects: events over the window's transport,
    /// repaints, releasing the pointer grab.
    fn apply_gesture_effects(&mut self, effects: Vec<GestureEffect>) {
        for effect in effects {
            match effect {
                GestureEffect::Emit {
                    def_id,
                    widget_id,
                    seq,
                    args,
                } => {
                    // A host that owns what it draws answers itself; every
                    // other one emits and waits, as it always has.
                    if !self.host.answer_own(def_id, widget_id, seq, &args) {
                        self.emit(def_id, widget_id, seq, args);
                    } else {
                        self.redraw(def_id);
                    }
                }
                GestureEffect::Redraw(def_id) => self.redraw(def_id),
                GestureEffect::ReleasePointer(def_id) => self.release_pointer(def_id),
                // A desktop window is not inside a document: there is nothing
                // to hand the focus back to, so the ring simply runs out and
                // the next Tab enters it again. (In a page this is what keeps a
                // mounted GuiDef from trapping the keyboard — `web::input`.)
                GestureEffect::FocusOut(_) => {}
            }
        }
    }

    /// Press on a widget: the machine acts by kind and possibly starts a drag.
    /// The grab callback is this front's pointer lock for knob/number drags.
    pub(super) fn on_press(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let ctx = self.gesture_ctx(def_id);
        // The window handle is cloned out so the grab closure borrows nothing
        // of `self` while the machine holds the host and the gesture state.
        let win = self.windows.get(&def_id).map(|w| w.gpu.window.clone());
        let mut grab = || win.as_deref().is_some_and(|w| grab_pointer(w, def_id));
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.press(&mut self.host, &ctx, cx, cy, &mut grab);
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

    /// Release: the machine finishes the drag (button up, wire landing) and
    /// may ask for the pointer grab to be dropped.
    pub(super) fn on_release(&mut self, def_id: i32) {
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
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
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
            return;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.wheel(&mut self.host, &ctx, cx, cy, steps);
        self.apply_gesture_effects(effects);
    }

    /// Relative motion while some window's knob/number drag holds the pointer
    /// lock (raw `DeviceEvent::MouseMotion` deltas; only one pointer drag runs
    /// at a time, so the first locked window is the target).
    pub(super) fn on_relative_motion(&mut self, dy: f64) {
        let Some(def_id) = self
            .windows
            .iter()
            .find_map(|(d, ws)| ws.gestures.locked().then_some(*d))
        else {
            return;
        };
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.relative_motion(&mut self.host, &ctx, dy);
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
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
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
        let Some((cx, cy)) = self.windows.get(&def_id).map(|w| w.cursor) else {
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
        if self.host.answer_own(def_id, def_id, seq, &args) {
            self.redraw(def_id);
        } else {
            self.emit(def_id, def_id, seq, args);
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

    pub(super) fn reset_timelines(&mut self, def_id: i32) {
        let ctx = self.gesture_ctx(def_id);
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        let effects = ws.gestures.reset_timelines(&mut self.host, &ctx);
        self.apply_gesture_effects(effects);
    }

    /// Releases the pointer grab a knob/number drag took and restores the cursor.
    fn release_pointer(&self, def_id: i32) {
        if let Some(ws) = self.windows.get(&def_id) {
            let _ = ws.gpu.window.set_cursor_grab(CursorGrabMode::None);
            ws.gpu.window.set_cursor_visible(true);
        }
    }
}

/// Grabs the pointer for a knob/number drag so motion keeps arriving even
/// over the window decorations or past its edges, where `CursorMoved`
/// otherwise stops (the title-bar/out-of-surface gap). Tries `Locked` first —
/// the cursor stays put and motion comes as relative `DeviceEvent::MouseMotion`
/// (the canonical knob feel, unbounded range) — and falls back to `Confined`,
/// which keeps the cursor inside the client area (so it cannot reach the title
/// bar) and is still driven by `CursorMoved`. Returns whether the pointer was
/// *locked* (which motion source the drag should read).
fn grab_pointer(window: &Window, def_id: i32) -> bool {
    if window.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
        window.set_cursor_visible(false);
        return true;
    }
    if let Err(e) = window.set_cursor_grab(CursorGrabMode::Confined) {
        debug!("gui_def {def_id}: no pointer grab for the drag ({e})");
    }
    false
}
