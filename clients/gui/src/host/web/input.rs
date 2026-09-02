//! The page's input, adapted onto the shared gesture machine.
//!
//! The browser twin of the native front's `gui::input` (native-only, so it is
//! named rather than linked): winit's web events in, a
//! [`GestureCtx`] built, the one
//! [`Gestures`] machine driven, its
//! [`GestureEffect`]s applied. Every editing behaviour lives in that machine
//! and none of it here -- this module is the *source* and the *sink*, which is
//! the whole reason a drag behaves identically on a desktop and in a tab.

use super::*;
use crate::host::gestures::{ClipEdit, Wheel, WheelDelta};

/// Translates a winit key into the platform-neutral [`HostKey`] the focus reads
/// (the browser front's twin of the native `to_key`), or `None` for a key
/// nothing focusable answers.
fn to_key(key: &Key) -> Option<HostKey> {
    match key {
        Key::Named(NamedKey::Backspace) => Some(HostKey::Backspace),
        Key::Named(NamedKey::Delete) => Some(HostKey::Delete),
        Key::Named(NamedKey::ArrowLeft) => Some(HostKey::Left),
        Key::Named(NamedKey::ArrowRight) => Some(HostKey::Right),
        Key::Named(NamedKey::ArrowUp) => Some(HostKey::Up),
        Key::Named(NamedKey::ArrowDown) => Some(HostKey::Down),
        Key::Named(NamedKey::Home) => Some(HostKey::Home),
        Key::Named(NamedKey::End) => Some(HostKey::End),
        Key::Named(NamedKey::Enter) => Some(HostKey::Enter),
        Key::Named(NamedKey::Space) => Some(HostKey::Char(' ')),
        Key::Named(NamedKey::Tab) => Some(HostKey::Tab),
        Key::Character(s) => s
            .chars()
            .next()
            .filter(|c| !c.is_control())
            .map(HostKey::Char),
        _ => None,
    }
}

impl WebApp {
    /// Snapshots the gesture context for one canvas: its framebuffer size, its
    /// modifier keys, and the heavy views' lane counts (channel/lane splits
    /// live in this front's GPU slots, so they are copied out here) — the
    /// browser twin of the native front's snapshot.
    pub(super) fn gesture_ctx(&self, def: i32) -> Option<(GestureCtx, (f64, f64))> {
        let slot = self.canvases.get(&def)?;
        let (fb_w, fb_h) = slot.fb();
        let mut ctx = GestureCtx::new(def, fb_w, fb_h);
        // The same rate the frame draws with, so a gesture over a measured
        // axis resolves the same hertz the reader is looking at.
        ctx.sample_rate = self.server_rate;
        (ctx.shift, ctx.ctrl, ctx.alt) = slot.modifiers();
        if let Some(render) = slot.render.as_ref() {
            for (id, view) in &render.waveforms {
                ctx.slot_channels.insert(*id, view.view.num_channels());
            }
            for (id, view) in &render.spectrograms {
                ctx.slot_channels.insert(*id, view.views.len());
            }
        }
        Some((ctx, slot.cursor))
    }

    /// Carries out a gesture's effects over this front's sinks: `/gui_event`s
    /// to the page outbox (a bound widget already forwarded inside the
    /// machine), and a repaint of the canvas the effect names — a linked-view
    /// mutation can name a *different* def than the one gestured on, and with a
    /// canvas each that now lands where it belongs.
    pub(super) fn apply_gesture_effects(&mut self, effects: Vec<GestureEffect>) {
        for effect in effects {
            match effect {
                GestureEffect::Emit {
                    def_id,
                    widget_id,
                    seq,
                    args,
                } => {
                    // A host that owns what it draws answers itself; every
                    // other one emits and waits, as it always has. A page
                    // rarely owns one — but the seam is the same on both
                    // fronts, and a gesture is implemented once.
                    if self.host.answer_own(def_id, widget_id, seq, &args) {
                        self.request_redraw(def_id);
                        continue;
                    }
                    // The stamp and the version are the second and third
                    // arguments on both fronts, before any tag, so one rule
                    // reads every event whatever its payload. The version says
                    // what state the edit was made against, and it is read here
                    // rather than carried in the effect because it belongs to
                    // the conversation and not to the gesture.
                    let mut msg_args = vec![
                        OscType::Int(widget_id),
                        OscType::Int(seq),
                        OscType::Long(self.host.outbox.borrow().version()),
                    ];
                    msg_args.extend(args);
                    self.queue(OscMessage {
                        addr: GUI_EVENT.into(),
                        args: msg_args,
                    });
                }
                GestureEffect::Redraw(def_id) => self.request_redraw(def_id),
                // The focus stepped past the ring: **blur the canvas**, so the
                // browser's own tab order carries on to whatever the document
                // holds after this GuiDef. Without it a mounted def is a
                // keyboard trap — winit prevents the default on every key it
                // sees, so the page around it would become unreachable, which
                // is a worse regression than having no keyboard at all.
                GestureEffect::FocusOut(def_id) => self.blur(def_id),
            }
        }
    }

    /// Pointer press: the shared gesture machine acts by widget kind.
    fn on_press(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.press(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
        // A press is how a field is entered and how one is left, so it is where
        // the keyboard is re-aimed (`compose`).
        self.aim_keyboard(def);
        // A clip drag needs the frame tick even on an otherwise still window:
        // held against a lane's edge it scrolls the view, and a standing cursor
        // sends no events of its own.
        if self
            .canvases
            .get(&def)
            .is_some_and(|s| s.gestures.dragging())
        {
            self.ensure_tick(true);
        }
    }

    /// Pointer move while dragging: the machine drives the dragged target.
    fn on_move(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.drag_to(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
    }

    /// Pointer release: the machine finishes the drag (button up, wire landing).
    fn on_release(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.release(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
        // The drag is over: the tick goes back to what the tree actually asks
        // for (it stays on only if a live widget wants it).
        self.on_tree_changed();
    }

    /// Wheel: the machine zooms the time axis or the vertical display window.
    fn on_wheel(&mut self, def: i32, steps: f64) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.wheel(&mut self.host, &ctx, cx, cy, steps);
        self.apply_gesture_effects(effects);
    }

    /// Keyboard: the same two addressees the desktop front has — the window's
    /// focus, then the element under the cursor — and the same window shortcuts
    /// after them (`r` resets every axis, `e` and `j` split and join the clip
    /// under the cursor). Escape is missing on purpose: it closes an OS window
    /// there and has no window to close here.
    pub(super) fn on_key(&mut self, def: i32, key: &Key) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        // The focus consumes the key first — Tab walks the ring, a focused
        // element edits — and only what nothing there answered runs the global
        // shortcuts, which are addressed to what is under the cursor.
        if let Some(k) = to_key(key) {
            let Some(slot) = self.canvases.get_mut(&def) else {
                return;
            };
            if let Some(effects) =
                slot.gestures
                    .key(&mut self.host, &ctx, k, &mut self.text_clipboard)
            {
                self.apply_gesture_effects(effects);
                // Tab walks the ring and Escape leaves it, so a key moves the
                // focus as readily as a press does (`compose`).
                self.aim_keyboard(def);
                return;
            }
        }
        // ...then the element under the cursor, which is where a block
        // operation is addressed.
        if let Some(k) = to_key(key) {
            let Some(slot) = self.canvases.get_mut(&def) else {
                return;
            };
            if let Some(effects) = slot.gestures.key_at_cursor(
                &mut self.host,
                &ctx,
                k,
                cx,
                cy,
                &mut self.text_clipboard,
            ) {
                self.apply_gesture_effects(effects);
                return;
            }
        }
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = match key {
            // The window's own shortcuts, addressed to the document behind it
            // rather than to whatever is under the cursor.
            Key::Character(c) if c.eq_ignore_ascii_case("z") && ctx.ctrl => {
                slot.gestures.history(&mut self.host, &ctx, ctx.shift)
            }
            Key::Character(c) if c.eq_ignore_ascii_case("y") && ctx.ctrl => {
                slot.gestures.history(&mut self.host, &ctx, true)
            }
            Key::Character(c) if c.eq_ignore_ascii_case("r") => {
                slot.gestures.reset_timelines(&mut self.host, &ctx)
            }
            // A clip's own edit verbs, over the clip under the cursor: cut it at
            // the time cursor, or read it and what touches it as one. They were
            // the desktop front's alone, which made the same host answer a key
            // in a window and not in a tab -- and a page had no way to split a
            // clip at all, since neither is a menu or an affordance anywhere.
            Key::Character(c) if c.eq_ignore_ascii_case("e") => {
                match slot
                    .gestures
                    .clip_verb(&mut self.host, &ctx, ClipEdit::Split, cx, cy)
                {
                    Some(effects) => effects,
                    None => return,
                }
            }
            Key::Character(c) if c.eq_ignore_ascii_case("j") => {
                match slot
                    .gestures
                    .clip_verb(&mut self.host, &ctx, ClipEdit::Join, cx, cy)
                {
                    Some(effects) => effects,
                    None => return,
                }
            }
            // The clipboard verbs over the view under the cursor, last, so a
            // focused field and a roll's own block keys answer first.
            Key::Character(c) if c.eq_ignore_ascii_case("c") && ctx.ctrl => {
                match slot.gestures.clipboard_key(
                    &mut self.host,
                    &ctx,
                    ClipVerb::Copy,
                    cx,
                    cy,
                    &mut self.text_clipboard,
                ) {
                    Some(effects) => effects,
                    None => return,
                }
            }
            Key::Character(c) if c.eq_ignore_ascii_case("x") && ctx.ctrl => {
                match slot.gestures.clipboard_key(
                    &mut self.host,
                    &ctx,
                    ClipVerb::Cut,
                    cx,
                    cy,
                    &mut self.text_clipboard,
                ) {
                    Some(effects) => effects,
                    None => return,
                }
            }
            Key::Character(c) if c.eq_ignore_ascii_case("v") && ctx.ctrl => {
                match slot.gestures.clipboard_key(
                    &mut self.host,
                    &ctx,
                    ClipVerb::Paste,
                    cx,
                    cy,
                    &mut self.text_clipboard,
                ) {
                    Some(effects) => effects,
                    None => return,
                }
            }
            _ => return,
        };
        self.apply_gesture_effects(effects);
    }

    /// Whether this instance is the one holding `id`'s canvas — how
    /// [`WebHosts`] finds an event's owner without a second index to keep in
    /// step with every attach and detach.
    pub(super) fn owns(&self, id: WindowId) -> bool {
        self.by_winit.contains_key(&id)
    }

    /// Every per-canvas event routes by winit's window id: a document's
    /// canvases each get their own pointer, modifiers and repaints.
    pub(super) fn on_window_event(&mut self, id: WindowId, event: WindowEvent) {
        let Some(def) = self.by_winit.get(&id).copied() else {
            return;
        };
        match event {
            WindowEvent::Resized(size) => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                match slot.render.as_mut() {
                    Some(render) => render.gpu.resize(size.width, size.height),
                    // The GPU is still coming up; remember the size so `GpuReady`
                    // can configure the surface to it instead of a stale 1x1.
                    None => slot.pending_size = Some((size.width, size.height)),
                }
                slot.request_redraw();
            }
            // The keyboard's own path, for a modifier held with no pointer
            // event to carry it — a Ctrl+Z over a focused canvas. The pointer
            // events are the other writer of the same three flags, and the
            // authoritative one for a gesture (see `CanvasSlot::mods`).
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(slot) = self.canvases.get_mut(&def) {
                    let state = mods.state();
                    slot.mods.set(
                        u8::from(state.shift_key())
                            | u8::from(state.control_key()) << 1
                            | u8::from(state.alt_key()) << 2,
                    );
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                // **A release the page never saw.** The primary button can come
                // up outside the browser window -- over another application,
                // after an alt-tab -- and no event reaches the document; winit
                // synthesizes a button event only from a move that *reports* a
                // change, which that move does not. So the drag is still held,
                // and this move looks exactly like a drag step: whatever is in
                // hand teleports to wherever the pointer came back in. A
                // desktop window cannot lose a release, so this is the browser
                // shell's job -- ending the gesture **where it was last seen**,
                // not where the pointer now is, which is what makes the two
                // fronts deliver the same press -> drag -> release.
                if slot.gestures.dragging() && slot.buttons.get() & 1 == 0 {
                    self.on_release(def);
                    return;
                }
                slot.cursor = (position.x, position.y);
                if slot.gestures.dragging() {
                    self.on_move(def);
                } else if self
                    .host
                    .window_def(def)
                    .is_some_and(Widget::has_hover_readout)
                {
                    // The hover readout follows the pointer (the native rule).
                    self.request_redraw(def);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                // **One press per gesture.** The browser's event stream can
                // repeat it, and the desktop's cannot: winit turns any
                // `pointermove` carrying a button (`PointerEvent.button != -1`)
                // into a synthesized `MouseInput` whose state is *pressed*
                // while that button is still down -- so a drag delivers a fresh
                // press on **every frame**. Chrome reports `-1` on a move and
                // never triggers it; Firefox reports `0` and triggers it
                // throughout, which is how a bend anchored at the press came to
                // re-anchor every frame and drift exactly as the relative form
                // it replaced did.
                //
                // The machine is single-pointer by design -- one press, one
                // drag, one release, the rule the touch slot already states --
                // so a press arriving mid-drag is never a new gesture, whatever
                // produced it: a repeat, or a second button chorded onto the
                // first. Dropping it here is what makes the two fronts hand the
                // host the same stream.
                // A press repeated mid-drag is dropped by the machine itself
                // (`Gestures::press`), which is where the single-pointer rule
                // belongs: both fronts hand it the same stream.
                ElementState::Pressed => self.on_press(def),
                ElementState::Released => self.on_release(def),
            },
            // A finger drives the same machine a pointer does: the desktop's
            // press → drag → release, with the touch's own position. winit
            // reports touch separately from the pointer events, so without this
            // arm a phone reaches every DOM control on the page and nothing at
            // all inside a canvas.
            WindowEvent::Touch(touch) => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                let owned = slot.touch == Some(touch.id);
                match touch.phase {
                    TouchPhase::Started if slot.touch.is_none() => {
                        slot.touch = Some(touch.id);
                        slot.cursor = (touch.location.x, touch.location.y);
                        self.on_press(def);
                    }
                    TouchPhase::Moved if owned => {
                        slot.cursor = (touch.location.x, touch.location.y);
                        if slot.gestures.dragging() {
                            self.on_move(def);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled if owned => {
                        slot.touch = None;
                        slot.cursor = (touch.location.x, touch.location.y);
                        self.on_release(def);
                    }
                    // Another finger while one is already down, or a stray
                    // phase for a finger this canvas never claimed.
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // The shell translates its own event; how many steps that is
                // belongs to the wheel and is written once (BROWSER).
                let delta = match delta {
                    MouseScrollDelta::LineDelta(_, y) => WheelDelta::Lines(y as f64),
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels(p.y),
                };
                let steps = Wheel::BROWSER.steps(delta, self.host.ui_scale(def) as f64);
                self.on_wheel(def, steps);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = event.logical_key.clone();
                self.on_key(def, &key);
            }
            // A canvas out of the viewport is skipped: the browser would not
            // composite it anyway, and *we* would still have computed the frame.
            WindowEvent::RedrawRequested if self.canvases.get(&def).is_some_and(|s| s.visible) => {
                self.draw(def)
            }
            _ => {}
        }
    }
}
