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
use crate::host::gestures::{Wheel, WheelDelta};
use crate::host::widget::element::SlotKey;
use crate::host::winit_keys::to_key;

impl WebApp {
    /// **Hands the loop what the host did on its own** -- a window it opened
    /// in answer to a gesture -- since opening a canvas needs the event loop,
    /// which a gesture's handler does not hold.
    fn post_host_effects(&self) {
        if !self.host.has_effects() {
            return;
        }
        if let Some(proxy) = super::web_proxy() {
            let _ = proxy.send_event(super::HostEvent::To(self.id, super::WebEvent::HostEffects));
        }
    }

    /// Snapshots the gesture context for one canvas: its framebuffer size, its
    /// modifier keys, and the heavy views' row counts (channel splits
    /// live in this front's GPU slots, so they are copied out here) -- the
    /// browser twin of the native front's snapshot.
    pub(super) fn gesture_ctx(&self, def: i32) -> Option<(GestureCtx, (f64, f64))> {
        let ctx = self.window_ctx(def)?;
        Some((ctx, self.canvases.get(&def)?.cursor?))
    }

    /// The gesture context of canvas `def` whether or not a pointer has been
    /// over it -- what a window key that needs no pointer is handed, with the
    /// pointer standing off the canvas.
    pub(super) fn window_ctx(&self, def: i32) -> Option<GestureCtx> {
        let slot = self.canvases.get(&def)?;
        let (fb_w, fb_h) = slot.fb();
        let mut ctx = GestureCtx::new(def, fb_w, fb_h);
        // The same rate the frame draws with, so a gesture over a measured
        // axis resolves the same hertz the reader is looking at.
        ctx.sample_rate = self.server_rate;
        // ...and the clock the canvas sweeps the playhead with, for the same
        // reason: a click that locates while the transport runs re-anchors the
        // sweep, and it must land where the line is drawn.
        ctx.clocks = self.host.head_clocks(def, Some(self.buses.as_ref()));
        // ...and this front's own wall clock, which is what tells one press
        // from the second of a double click. The rule is the machine's; the
        // clock is the platform's, because there is none in the shared core --
        // the same seam that made the browser's frame tick a `setInterval`.
        ctx.now_ms = js_sys::Date::now();
        (ctx.shift, ctx.ctrl, ctx.alt) = slot.modifiers();
        // A finger owns the pointer while it is down: its held press is the
        // context request a finger has no second button for.
        ctx.touch = slot.touch.is_some();
        if let Some(render) = slot.render.as_ref() {
            // **The widget's own picture, not a body's**: the row count a
            // gesture divides by is the view's, and a box inside a view has
            // rows of its own that mean nothing to the axis around it.
            for ((id, key), view) in &render.waveforms {
                if *key == SlotKey::SELF {
                    ctx.slot_channels.insert(*id, view.view.num_channels());
                }
            }
            for ((id, key), view) in &render.spectrograms {
                if *key == SlotKey::SELF {
                    ctx.slot_channels.insert(*id, view.views.len());
                }
            }
        }
        Some(ctx)
    }

    /// Carries out a gesture's effects over this front's sinks: `/gui_event`s
    /// to the page outbox (a bound widget already forwarded inside the
    /// machine), and a repaint of the canvas the effect names -- a linked-view
    /// mutation can name a *different* def than the one gestured on, and with a
    /// canvas each that now lands where it belongs.
    pub(super) fn apply_gesture_effects(&mut self, effects: Vec<GestureEffect>) {
        // A gesture may have sent a chooser into another directory.
        self.start_listings();
        for effect in effects {
            match effect {
                GestureEffect::Emit {
                    def_id,
                    widget_id,
                    seq,
                    args,
                } => {
                    // **One message, whoever it goes to**, built where the
                    // native front builds it: a host that owns what it draws
                    // is delivered it in memory, and every other one queues it
                    // for the page. A page rarely owns one -- but the seam is
                    // the same on both fronts, and a gesture is implemented
                    // once.
                    let message = self.host.event_message(widget_id, seq, args);
                    if self.host.deliver(def_id, &message) {
                        self.request_redraw(def_id);
                        self.post_host_effects();
                        continue;
                    }
                    self.queue(message);
                }
                GestureEffect::Redraw(def_id) => self.request_redraw(def_id),
                // The focus stepped past the ring: **blur the canvas**, so the
                // browser's own tab order carries on to whatever the document
                // holds after this GuiDef. Without it a mounted def is a
                // keyboard trap -- winit prevents the default on every key it
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

    /// Pointer move with no button held: the popup layer's hover, the hovered
    /// control and a tip's wait.
    fn on_motion(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.motion(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
        self.ensure_timers(def);
    }

    /// The pointer left the canvas: nothing is hovered and no tip waits.
    fn on_leave(&mut self, def: i32) {
        let Some(ctx) = self.window_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        slot.cursor = None;
        let effects = slot.gestures.hover(&mut self.host, &ctx, None);
        self.apply_gesture_effects(effects);
        self.request_redraw(def);
    }

    /// The secondary button: the request for a context menu.
    fn on_context(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        if let Some(effects) = slot.gestures.context(&mut self.host, &ctx, cx, cy) {
            self.apply_gesture_effects(effects);
        }
    }

    /// The middle button: the primary selection pasted at the pointer, as on
    /// the desktop (`Gestures::middle`).
    fn on_middle(&mut self, def: i32) {
        let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
            return;
        };
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        let effects = slot.gestures.middle(&mut self.host, &ctx, cx, cy);
        self.apply_gesture_effects(effects);
        self.aim_keyboard(def);
    }

    /// Keeps the tick running while this canvas' gesture machine has a timer
    /// armed -- a tip waiting, a finger held. The tick itself turns back off
    /// when the timers have run out ([`Self::advance_timers`]).
    fn ensure_timers(&mut self, def: i32) {
        if self
            .canvases
            .get(&def)
            .is_some_and(|s| s.gestures.pending())
        {
            self.ensure_tick(true);
        }
    }

    /// The tick's step of the machine's timers, as on the desktop: a rest that
    /// lasted shows its tip, a held finger asks for its context menu.
    pub(super) fn advance_timers(&mut self) {
        let waiting: Vec<i32> = self
            .canvases
            .iter()
            .filter(|(_, slot)| slot.gestures.pending())
            .map(|(def, _)| *def)
            .collect();
        if waiting.is_empty() {
            return;
        }
        for def in waiting {
            let Some(mut ctx) = self.window_ctx(def) else {
                continue;
            };
            let Some(slot) = self.canvases.get_mut(&def) else {
                continue;
            };
            ctx.touch = slot.touch.is_some();
            let effects = slot.gestures.elapsed(&mut self.host, &ctx);
            self.apply_gesture_effects(effects);
        }
        // The last timer ran out: the tick goes back to what the trees ask for.
        if !self.canvases.values().any(|s| s.gestures.pending()) {
            self.on_tree_changed();
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

    /// Keyboard: the **one dispatch** the desktop front calls too
    /// ([`Gestures::press_key`](crate::host::gestures::Gestures::press_key)) --
    /// the focus, the key table and the verb it names. Escape with nothing open
    /// is the one key that does less here: it closes an OS window there and has
    /// no window to close in a page.
    pub(super) fn on_key(&mut self, def: i32, key: &Key) {
        let Some(k) = to_key(key) else {
            return;
        };
        // With no pointer over the canvas yet the context still holds: the
        // pointer is unknown, and a verb addressed by it means the window.
        let Some(ctx) = self.window_ctx(def) else {
            return;
        };
        // A chord with the modifier the host has no name for is none of the
        // host's: Control+E on a Mac is not a bare `e`.
        if self.canvases.get(&def).is_some_and(|slot| slot.unnamed()) {
            return;
        }
        let pointer = self.canvases.get(&def).and_then(|slot| slot.cursor);
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        // A key takes a tip down, whoever ends up answering it.
        let effects = slot.gestures.key_began(&mut self.host, &ctx);
        self.apply_gesture_effects(effects);
        let Some(slot) = self.canvases.get_mut(&def) else {
            return;
        };
        if let Some(effects) = slot.gestures.press_key(&mut self.host, &ctx, k, pointer) {
            self.apply_gesture_effects(effects);
            // Tab walks the ring and Escape leaves it, so a key moves the focus
            // as readily as a press does (`compose`).
            self.aim_keyboard(def);
        }
    }

    /// Whether this instance is the one holding `id`'s canvas -- how
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
            // event to carry it -- a Ctrl+Z over a focused canvas. The pointer
            // events are the other writer of the same three flags, and the
            // authoritative one for a gesture (see `CanvasSlot::mods`).
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(slot) = self.canvases.get_mut(&def) {
                    let state = mods.state();
                    slot.mods.set(super::canvas::mod_bits(
                        state.shift_key(),
                        state.control_key(),
                        state.alt_key(),
                        state.super_key(),
                    ));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let fitted = self.host.ui_scale(def);
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
                slot.cursor = Some(at_fit(&slot.window, fitted, position.x, position.y));
                if slot.gestures.dragging() {
                    self.on_move(def);
                } else {
                    // Motion with no button held is the machine's too (the
                    // native rule): an open list's row, the bar's title, the
                    // control under the pointer, a tip's wait.
                    self.on_motion(def);
                    if self
                        .host
                        .window_def(def)
                        .is_some_and(Widget::has_hover_readout)
                    {
                        // The hover readout follows the pointer.
                        self.request_redraw(def);
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                if !slot.gestures.dragging() {
                    self.on_leave(def);
                }
            }
            // The secondary button asks for a context menu (the native rule).
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => self.on_context(def),
            // The middle button pastes the last selection made in this host: a
            // page has no primary selection of the platform's to read.
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Middle,
                ..
            } => self.on_middle(def),
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
            // press -> drag -> release, with the touch's own position. winit
            // reports touch separately from the pointer events, so without this
            // arm a phone reaches every DOM control on the page and nothing at
            // all inside a canvas.
            WindowEvent::Touch(touch) => {
                let fitted = self.host.ui_scale(def);
                let Some(slot) = self.canvases.get_mut(&def) else {
                    return;
                };
                let owned = slot.touch == Some(touch.id);
                let at = at_fit(&slot.window, fitted, touch.location.x, touch.location.y);
                match touch.phase {
                    TouchPhase::Started if slot.touch.is_none() => {
                        slot.touch = Some(touch.id);
                        slot.cursor = Some(at);
                        self.on_press(def);
                        // A finger held still is its context request, which is
                        // a timer: the tick has to run to see it expire.
                        self.ensure_timers(def);
                    }
                    TouchPhase::Moved if owned => {
                        slot.cursor = Some(at);
                        if slot.gestures.dragging() {
                            self.on_move(def);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled if owned => {
                        slot.touch = None;
                        slot.cursor = Some(at);
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

/// **A pointer's place in the pixels the surface was fitted at.** The shell
/// hands a position in the page's density *of the moment*, read at each
/// event, and the surface is as dense as its last fit made it
/// (`WebEvent::Resize`). The two agree but for the time between a change of
/// density and the fit that follows it -- and for good where the page is
/// never told of the change, as under a browser's own emulation of another
/// screen: every press then landed off its mark by the ratio of the two. So
/// a position is read back into CSS pixels by the density it came in, and
/// out by the surface's own.
fn at_fit(window: &winit::window::Window, fitted: f32, x: f64, y: f64) -> (f64, f64) {
    let live = window.scale_factor();
    if live > 0.0 && fitted > 0.0 {
        let to_fit = f64::from(fitted) / live;
        (x * to_fit, y * to_fit)
    } else {
        (x, y)
    }
}
