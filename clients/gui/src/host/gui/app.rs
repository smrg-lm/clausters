//! The windowed host's state and winit handler: the [`App`] (one per process)
//! and the per-window [`WindowState`], plus the plumbing every other `gui`
//! submodule drives — sending replies/events over the right transport, the
//! bound-vs-event delivery door, the animation tick and the shared-frame render.

use std::collections::{HashMap, VecDeque};
use std::net::{TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
use tracing::warn;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::gpu::Gpu;
use crate::host::canvas::CanvasView;
use crate::host::fetch::BufferFetches;
use crate::host::frame::{self, SpectrogramSlot, WaveformSlot};
use crate::host::gestures::Gestures;
#[cfg(feature = "midi")]
use crate::host::interact;
use crate::host::live::{collect_scopes, push_sample, tree_has_canvas, tree_has_live_widget};
use crate::host::nodetree::NodeTree;
use crate::host::paint::Painter;
use crate::host::spectrum::SpectrumState;
use crate::host::widget::{Widget, WidgetKind};
use crate::host::{BusSource, ClientId, GUI_EVENT, Host, HostEffect};

use super::{FRAME, NODETREE_POLL, PLACEHOLDER_ORIGIN, UserEvent};

/// One open window: its GPU surface, the per-waveform slots, the painter, the
/// script address its events go to, the pointer/drag state, and the per-`scope`
/// rolling history. The widget tree itself lives in the [`Host`] (single source
/// of truth).
pub(super) struct WindowState {
    pub(super) gpu: Gpu,
    pub(super) waveforms: HashMap<i32, WaveformSlot>,
    /// Per-`spectrogram` GPU resources (one STFT view per channel lane).
    pub(super) spectrograms: HashMap<i32, SpectrogramSlot>,
    /// Per-`canvas` GPU resources (the compiled user shader + uniforms).
    pub(super) canvases: HashMap<i32, CanvasView>,
    pub(super) painter: Painter,
    /// The second mesh pass: editor chrome drawn over the heavy views
    /// (selection, playhead, rulers' overlay parts, cursor readout).
    pub(super) overlay: Painter,
    pub(super) origin: ClientId,
    pub(super) cursor: (f64, f64),
    /// Whether Shift is held (Shift+drag pans a timeline view; plain drag
    /// selects).
    pub(super) shift: bool,
    /// Whether Ctrl is held (Ctrl+click adds/removes a `bpf` breakpoint).
    pub(super) ctrl: bool,
    /// Whether Alt is held (Alt+click toggles a piano-roll note in/out of the
    /// multi-note selection).
    pub(super) alt: bool,
    /// This window's gesture state (the shared machine's in-progress drag).
    pub(super) gestures: Gestures,
    /// Recent control-bus samples per `scope` widget id (oldest .. newest).
    pub(super) scopes: HashMap<i32, VecDeque<f32>>,
    /// Triggered display window per audio-rate `scope` widget id, refreshed on
    /// the frame tick from the shared segment's tap rings. Also holds each
    /// `phasescope`'s interleaved L/R window (ids do not collide).
    pub(super) tap_windows: HashMap<i32, Vec<f32>>,
    /// Persistent FFT analysis state per `spectrum` widget id (the smoothed and
    /// peak-hold curves), advanced on the frame tick.
    pub(super) spectra: HashMap<i32, SpectrumState>,
}

pub(super) struct App {
    pub(super) host: Host,
    pub(super) socket: Arc<UdpSocket>,
    /// Live control-bus source (the shared segment) for meters/scopes, if mapped.
    pub(super) shm: Option<Arc<dyn BusSource>>,
    pub(super) windows: HashMap<i32, WindowState>,
    pub(super) by_winit: HashMap<WindowId, i32>,
    /// TCP write halves by connection id (the script front's stream carrier);
    /// registered on `TcpConnected`, pruned on `TcpDisconnected`.
    pub(super) tcp_conns: HashMap<u64, TcpStream>,
    /// Window opens requested before the first `resumed`, flushed on resume.
    pub(super) pending: Vec<(i32, ClientId)>,
    pub(super) resumed: bool,
    /// Next scheduled repaint for animated (meter/scope) windows.
    pub(super) next_frame: Instant,
    /// The server-buffer fetch machine (`/b_query` → chunked `/b_getn`),
    /// shared with the browser front.
    pub(super) fetches: BufferFetches,
    /// The node tree last read from the server, by group id, feeding `nodetree`
    /// widgets (filled by `/g_queryTree.reply`).
    pub(super) node_trees: HashMap<i32, NodeTree>,
    /// Whether the client leg has registered for node notifications
    /// (`/notify 1`), so it is sent once even with several node-tree windows.
    pub(super) notified: bool,
    /// Next scheduled re-query of the server's node tree (the `/n_set` poll).
    pub(super) next_query: Instant,
    /// Standalone mode: the host booted a pre-loaded GuiDef with no script front
    /// (`--standalone`). Closing the last window then quits the app, so the
    /// embedded audio server is dropped (and `/quit`ed) instead of left running.
    pub(super) standalone: bool,
    /// Live MIDI input: the virtual input port, held open while any open
    /// window has a `midi_in` piano-roll (dropping it closes the port).
    #[cfg(feature = "midi")]
    pub(super) midi_in: Option<clausters_midi::live::Input>,
    /// Whether the port-open failure was already reported (retrying is cheap,
    /// warning every frame is not).
    #[cfg(feature = "midi")]
    pub(super) midi_warned: bool,
    /// Held keys being painted: `(window, widget, channel, pitch)` → the index
    /// of the note the matching note-off will close.
    #[cfg(feature = "midi")]
    pub(super) held: HashMap<(i32, i32, u8, u8), usize>,
    /// Step-entry cursor per `(window, widget)` (timeline samples), used while
    /// the shared playhead is stopped; the last note-off advances it a grid.
    #[cfg(feature = "midi")]
    pub(super) step: HashMap<(i32, i32), f64>,
    /// The piano-roll note clipboard (Ctrl+C/X/V), normalized to the block's
    /// first onset — host-wide, so notes travel between rolls and windows.
    pub(super) clipboard: Vec<crate::host::pianoroll::Note>,
}

impl App {
    pub(super) fn new(host: Host, socket: Arc<UdpSocket>, shm: Option<Arc<dyn BusSource>>) -> Self {
        Self {
            host,
            socket,
            shm,
            windows: HashMap::new(),
            by_winit: HashMap::new(),
            tcp_conns: HashMap::new(),
            pending: Vec::new(),
            resumed: false,
            next_frame: Instant::now(),
            fetches: BufferFetches::default(),
            node_trees: HashMap::new(),
            notified: false,
            next_query: Instant::now(),
            standalone: false,
            #[cfg(feature = "midi")]
            midi_in: None,
            #[cfg(feature = "midi")]
            midi_warned: false,
            #[cfg(feature = "midi")]
            held: HashMap::new(),
            #[cfg(feature = "midi")]
            step: HashMap::new(),
            clipboard: Vec::new(),
        }
    }

    /// The current value of control bus `bus` from the shared segment (`0.0`
    /// without a segment or for a negative/out-of-range bus).
    pub(super) fn read_bus(&self, bus: i32) -> f32 {
        if bus < 0 {
            return 0.0;
        }
        self.shm.as_ref().map_or(0.0, |s| s.control(bus as usize))
    }

    /// Pushes one fresh sample into every `scope`'s rolling history, read from the
    /// shared segment. Called once per animation frame tick (not per `render`), so
    /// the scope scrolls at a steady, time-based rate regardless of how often the
    /// window happens to repaint.
    fn advance_scopes(&mut self) {
        // Collect (window, scope id, bus value) under immutable borrows, then push
        // — keeps the shared-segment read and the per-window history mutation
        // from overlapping borrows of `self`.
        let mut samples: Vec<(i32, i32, f32)> = Vec::new();
        for def_id in self.windows.keys() {
            if let Some(tree) = self.host.window_def(*def_id) {
                let mut scopes = Vec::new();
                collect_scopes(tree, &mut scopes);
                for (id, bus) in scopes {
                    samples.push((*def_id, id, self.read_bus(bus)));
                }
            }
        }
        for (def_id, id, value) in samples {
            if let Some(ws) = self.windows.get_mut(&def_id) {
                push_sample(ws.scopes.entry(id).or_default(), value);
            }
        }
    }

    /// Refreshes every audio-tap consumer from the shared segment's tap rings,
    /// once per animation frame tick (the same cadence as
    /// [`Self::advance_scopes`]): the audio-rate scopes' triggered windows, the
    /// phasescopes' interleaved L/R windows, and the spectra's FFT analysis.
    /// Without a segment the views stay empty and draw their framed field.
    fn advance_tap_windows(&mut self) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let sample_rate = shm.sample_rate();
        for (def_id, ws) in &mut self.windows {
            if let Some(tree) = self.host.window_def(*def_id) {
                crate::host::live::update_tap_windows(
                    tree,
                    sample_rate,
                    |tap, out| shm.read_tap(tap, out),
                    &mut ws.tap_windows,
                );
                crate::host::live::update_phase_windows(
                    tree,
                    sample_rate,
                    |tap, out| shm.read_tap(tap, out),
                    &mut ws.tap_windows,
                );
                crate::host::live::update_spectra(
                    tree,
                    |tap, out| shm.read_tap(tap, out),
                    &mut ws.spectra,
                );
            }
        }
    }

    /// Whether window `def_id` should repaint continuously: it has a `canvas`
    /// (time-driven, always), or a meter/scope with a shared segment to feed it.
    fn window_is_animated(&self, def_id: i32) -> bool {
        self.host.window_def(def_id).is_some_and(|tree| {
            tree_has_canvas(tree) || (self.shm.is_some() && tree_has_live_widget(tree))
        })
    }

    pub(super) fn apply(
        &mut self,
        event_loop: &ActiveEventLoop,
        from: ClientId,
        effects: Vec<HostEffect>,
    ) {
        for effect in effects {
            match effect {
                HostEffect::Reply(msg) => self.send(from, msg),
                HostEffect::OpenWindow(id) => {
                    if self.resumed {
                        self.open_window(event_loop, id, from);
                    } else {
                        self.pending.push((id, from));
                    }
                }
                HostEffect::CloseWindow(id) => self.drop_window(id),
                HostEffect::Redraw(id) => {
                    if let Some(ws) = self.windows.get(&id) {
                        ws.gpu.window.request_redraw();
                    }
                }
            }
        }
    }

    /// Encodes and sends one message to `to`, over the transport it belongs to.
    pub(super) fn send(&self, to: ClientId, msg: OscMessage) {
        let addr = msg.addr.clone();
        let bytes = match encode(&OscPacket::Message(msg)) {
            Ok(bytes) => bytes,
            Err(e) => return warn!("failed to encode {addr}: {e}"),
        };
        match to {
            ClientId::Udp(to) => {
                if let Err(e) = self.socket.send_to(&bytes, to) {
                    warn!("failed to send {addr} to {to}: {e}");
                }
            }
            ClientId::Tcp(id) => {
                // Length-prefixed on the originating connection; dropped if it
                // has since closed (TcpDisconnected prunes it).
                if let Some(stream) = self.tcp_conns.get(&id)
                    && let Err(e) = crate::host::tcp::write_frame(stream, &bytes)
                {
                    warn!("failed to send {addr} to tcp client {id}: {e}");
                }
            }
            // The wasm front never reaches the native event loop.
            ClientId::Web => warn!("reply {addr} to a web client on the native front"),
        }
    }

    /// Emits `/gui_event widget_id <args…>` to the window's script.
    pub(super) fn emit(&self, def_id: i32, widget_id: i32, mut args: Vec<OscType>) {
        let Some(ws) = self.windows.get(&def_id) else {
            return;
        };
        let mut msg_args = vec![OscType::Int(widget_id)];
        msg_args.append(&mut args);
        self.send(
            ws.origin,
            OscMessage {
                addr: GUI_EVENT.into(),
                args: msg_args,
            },
        );
    }

    /// Delivers a piano-roll's edited notes (the MIDI painting path; gesture
    /// edits go through the machine's own delivery): a **bound** roll forwards
    /// the flat note list straight to the audio server (without the `"notes"`
    /// tag); an unbound one emits `/gui_event id "notes" start dur pitch vel
    /// channel …` to the script.
    #[cfg(feature = "midi")]
    pub(super) fn emit_notes(&self, def_id: i32, widget_id: i32) {
        let Some(args) = self
            .host
            .window_def(def_id)
            .and_then(|t| interact::notes_event_args(t, widget_id))
        else {
            return;
        };
        if self.host.is_bound(widget_id) {
            self.host.forward_args(widget_id, args[1..].to_vec());
            return;
        }
        self.emit(def_id, widget_id, args);
    }

    /// The framebuffer size of a window.
    pub(super) fn fb(&self, def_id: i32) -> (u32, u32) {
        self.windows
            .get(&def_id)
            .map(|w| (w.gpu.config.width.max(1), w.gpu.config.height.max(1)))
            .unwrap_or((1, 1))
    }

    pub(super) fn redraw(&self, def_id: i32) {
        if let Some(ws) = self.windows.get(&def_id) {
            ws.gpu.window.request_redraw();
        }
    }

    /// The navigation window of timeline view `id`'s group:
    /// `(start, len, total)` in timeline samples.
    #[cfg(feature = "midi")]
    pub(super) fn timeline_nav(&self, id: i32) -> Option<(f64, f64, usize)> {
        self.host
            .timeline_nav(id)
            .map(|(nav, total)| (nav.start, nav.len, total))
    }

    /// Renders window `def_id` through the shared frame path ([`frame::render`]),
    /// the same code the browser front drives — here fed the live inputs (the
    /// shared-memory bus, the scope histories, the node trees, the held button).
    fn render(&mut self, def_id: i32) {
        tracing::trace!("rendering window {def_id}");
        let active_button = self
            .windows
            .get(&def_id)
            .and_then(|w| w.gestures.active_button());
        let server_attached = self.host.server().is_some();
        // Disjoint field borrows: the tree (host), the bus (shm), the node trees,
        // and the window's GPU resources are separate fields of `self`.
        let Some(tree) = self.host.window_def(def_id) else {
            return;
        };
        let cursor = self.windows.get(&def_id).map(|w| w.cursor);
        let inputs = frame::FrameInputs {
            bus: self.shm.as_deref(),
            node_trees: &self.node_trees,
            active_button,
            server_attached,
            sample_rate: self.shm.as_ref().map_or(0.0, |s| s.sample_rate()),
            sample_clock: self.shm.as_ref().map_or(0.0, |s| s.sample_clock()),
            cursor,
            timelines: self.host.timelines(),
            // A rewiring drag in flight draws its wire to the pointer.
            wiring: self
                .windows
                .get(&def_id)
                .and_then(|w| w.gestures.wiring())
                .and_then(|(id, port)| cursor.map(|(cx, cy)| (id, port, (cx as f32, cy as f32)))),
        };
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        frame::render(
            &mut ws.gpu,
            &mut ws.painter,
            &mut ws.overlay,
            &mut ws.waveforms,
            &mut ws.spectrograms,
            &mut ws.canvases,
            &ws.scopes,
            &ws.tap_windows,
            &ws.spectra,
            tree,
            &inputs,
        );
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resumed = true;
        // Deliver raw motion while focused, so a locked knob/number drag reads its
        // relative `DeviceEvent::MouseMotion` (the pointer-lock path in `on_press`).
        event_loop.listen_device_events(DeviceEvents::WhenFocused);
        for (id, origin) in std::mem::take(&mut self.pending) {
            self.open_window(event_loop, id, origin);
        }
        // Standalone: a GuiDef pre-loaded into the host before the loop started
        // (no `/gui_def` over the wire) is opened now. Its events have no script
        // to return to, so they go to a placeholder origin. Pre-loaded windows
        // mean this is a standalone app — closing the last one quits it.
        let standalone_origin = PLACEHOLDER_ORIGIN;
        let preloaded = self.host.window_def_ids();
        self.standalone = !preloaded.is_empty();
        for id in preloaded {
            if !self.windows.contains_key(&id) {
                self.open_window(event_loop, id, standalone_origin);
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Osc { from, bytes } => {
                let packet = match clausters_core::osc::decode_packet(&bytes) {
                    Ok(p) => p,
                    Err(e) => return warn!("malformed OSC packet from {from}: {e}"),
                };
                let from = ClientId::Udp(from);
                let effects = self.host.handle_packet(packet, from);
                self.apply(event_loop, from, effects);
            }
            UserEvent::TcpConnected { id, stream } => {
                self.tcp_conns.insert(id, stream);
            }
            UserEvent::TcpOsc { id, bytes } => {
                let packet = match clausters_core::osc::decode_packet(&bytes) {
                    Ok(p) => p,
                    Err(e) => return warn!("malformed OSC packet from tcp client {id}: {e}"),
                };
                let from = ClientId::Tcp(id);
                let effects = self.host.handle_packet(packet, from);
                self.apply(event_loop, from, effects);
            }
            UserEvent::TcpDisconnected { id } => {
                self.tcp_conns.remove(&id);
            }
            UserEvent::ServerOsc { bytes } => match clausters_core::osc::decode_packet(&bytes) {
                Ok(packet) => self.handle_server_packet(packet),
                Err(e) => warn!("malformed OSC reply from the audio server: {e}"),
            },
        }
    }

    /// Raw relative motion drives a *locked* knob/number drag. The pointer is
    /// locked in place (so it cannot wander onto the title bar or out of the
    /// window, where `CursorMoved` is lost), and its movement arrives here as a
    /// device delta instead — the gesture machine applies it incrementally.
    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        let DeviceEvent::MouseMotion { delta: (_, dy) } = event else {
            return;
        };
        self.on_relative_motion(dy);
    }

    /// After handling events, schedule the next wake-up: a ~30 fps repaint for
    /// animated (meter/scope) windows so their shared-memory values keep moving,
    /// and a low-rate re-query for node-tree windows so `/n_set` changes show.
    /// With neither, windows stay event-driven (`Wait`).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next_wake: Option<Instant> = None;

        // Drain replies from an embedded server (standalone): the UDP leg uses a
        // background thread, but the embed ring is polled here on the main thread.
        self.drain_embed_replies();

        // Meter/scope animation, driven from the shared segment.
        let animated: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_is_animated(*id))
            .collect();
        if !animated.is_empty() {
            if now >= self.next_frame {
                // Advance each scope's rolling history exactly once per frame tick
                // (time-based), then repaint. Sampling here rather than in `render`
                // keeps the scroll speed constant: extra repaints from a drag or a
                // resize no longer push extra samples and speed the scope up. The
                // audio-rate scopes refresh their triggered tap windows likewise.
                self.advance_scopes();
                self.advance_tap_windows();
                for id in &animated {
                    if let Some(ws) = self.windows.get(id) {
                        ws.gpu.window.request_redraw();
                    }
                }
                self.next_frame = now + FRAME;
            }
            next_wake = Some(self.next_frame);
        }

        // Live MIDI input painting notes: while any open window has a
        // `midi_in` roll, the virtual input port is held open and drained at
        // the frame cadence (dropping it when the last such roll goes closes
        // the port).
        #[cfg(feature = "midi")]
        {
            let rolls = self.midi_rolls();
            if rolls.is_empty() {
                self.midi_in = None;
            } else {
                if self.midi_in.is_none() {
                    self.midi_in = clausters_midi::live::Input::open("clausters-gui");
                    if self.midi_in.is_none() && !self.midi_warned {
                        tracing::warn!("could not open the virtual MIDI input port");
                        self.midi_warned = true;
                    }
                }
                self.drain_midi(&rolls);
                let t = now + FRAME;
                next_wake = Some(next_wake.map_or(t, |w| w.min(t)));
            }
        }

        // Node-tree polling, driven from the client leg (the `/n_set` poll).
        if self.host.server().is_some() && !self.node_tree_groups().is_empty() {
            if now >= self.next_query {
                self.requery_node_trees();
                self.next_query = now + NODETREE_POLL;
            }
            next_wake = Some(next_wake.map_or(self.next_query, |t| t.min(self.next_query)));
        }

        match next_wake {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(&def_id) = self.by_winit.get(&window_id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => self.user_close(def_id, event_loop),
            WindowEvent::Resized(size) => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.gpu.resize(size.width, size.height);
                    ws.gpu.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.shift = mods.state().shift_key();
                    ws.ctrl = mods.state().control_key();
                    ws.alt = mods.state().alt_key();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    // Off-window: the cursor readout hides (nothing contains it).
                    ws.cursor = (-1.0, -1.0);
                    ws.gpu.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.cursor = (position.x, position.y);
                }
                let dragging = self
                    .windows
                    .get(&def_id)
                    .is_some_and(|w| w.gestures.dragging());
                if dragging {
                    self.on_drag(def_id, position.x, position.y);
                } else if self
                    .host
                    .window_def(def_id)
                    .is_some_and(Widget::has_hover_readout)
                {
                    // The hover readout follows the pointer, so it needs a
                    // frame per move — a static window (a plot's) has no
                    // other frame source.
                    self.redraw(def_id);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => self.on_press(def_id),
                ElementState::Released => self.on_release(def_id),
            },
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                self.on_wheel(def_id, steps);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let ctrl = self.windows.get(&def_id).is_some_and(|w| w.ctrl);
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => self.user_close(def_id, event_loop),
                    Key::Named(NamedKey::Delete) | Key::Named(NamedKey::Backspace) => {
                        self.delete_selected_notes(def_id)
                    }
                    Key::Character(ref c) if ctrl && c.eq_ignore_ascii_case("c") => {
                        self.copy_selected_notes(def_id, false)
                    }
                    Key::Character(ref c) if ctrl && c.eq_ignore_ascii_case("x") => {
                        self.copy_selected_notes(def_id, true)
                    }
                    Key::Character(ref c) if ctrl && c.eq_ignore_ascii_case("v") => {
                        self.paste_notes_at_cursor(def_id)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("q") => {
                        self.quantize_roll(def_id)
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        self.reset_timelines(def_id)
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.render(def_id),
            _ => {}
        }
    }
}

#[cfg(feature = "midi")]
impl App {
    /// Every `midi_in` piano-roll in an open window, as `(window, widget)`.
    pub(super) fn midi_rolls(&self) -> Vec<(i32, i32)> {
        fn collect(w: &Widget, def_id: i32, out: &mut Vec<(i32, i32)>) {
            if let (WidgetKind::PianoRoll { midi_in: true, .. }, Some(id)) = (&w.kind, w.id) {
                out.push((def_id, id));
            }
            for c in &w.children {
                collect(c, def_id, out);
            }
        }
        let mut out = Vec::new();
        for &def_id in self.windows.keys() {
            if let Some(tree) = self.host.window_def(def_id) {
                collect(tree, def_id, &mut out);
            }
        }
        out
    }

    /// The shared playhead's current sample for a widget while it is running
    /// (`playhead_at` anchored to the engine clock), else `None`.
    pub(super) fn playhead_sample(&self, def_id: i32, id: i32) -> Option<f64> {
        let tree = self.host.window_def(def_id)?;
        let e = tree.find(id)?.kind.editor()?;
        let clock = self.shm.as_ref().map_or(0.0, |s| s.sample_clock());
        (e.playhead_at >= 0.0 && clock > 0.0).then_some(clock - e.playhead_at)
    }

    /// A piano-roll's `snap` grid (0 when none or not a roll).
    pub(super) fn roll_snap(&self, def_id: i32, id: i32) -> f64 {
        match self
            .host
            .window_def(def_id)
            .and_then(|t| t.find(id))
            .map(|w| &w.kind)
        {
            Some(WidgetKind::PianoRoll { snap, .. }) => *snap,
            _ => 0.0,
        }
    }
}
