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
        ctx.shift = slot.shift;
        ctx.ctrl = slot.ctrl;
        ctx.alt = slot.alt;
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
    /// canvas each that now lands where it belongs. There is no pointer grab in
    /// the browser (the grab callback returns `false`), so releases are no-ops.
    pub(super) fn apply_gesture_effects(&mut self, effects: Vec<GestureEffect>) {
        for effect in effects {
            match effect {
                GestureEffect::Emit {
                    widget_id,
                    seq,
                    args,
                    ..
                } => {
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
                GestureEffect::ReleasePointer(_) => {}
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
        let effects = slot
            .gestures
            .press(&mut self.host, &ctx, cx, cy, &mut || false);
        self.apply_gesture_effects(effects);
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
    /// focus, then the element under the cursor — and the same window shortcut
    /// after them (`r` resets every axis). Escape is missing on purpose: it
    /// closes an OS window there and has no window to close here.
    fn on_key(&mut self, def: i32, key: &Key) {
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
            Key::Character(c) if c.eq_ignore_ascii_case("r") => {
                slot.gestures.reset_timelines(&mut self.host, &ctx)
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
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(slot) = self.canvases.get_mut(&def) {
                    slot.shift = mods.state().shift_key();
                    slot.ctrl = mods.state().control_key();
                    slot.alt = mods.state().alt_key();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
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
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
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
