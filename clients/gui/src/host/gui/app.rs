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
use crate::host::live::TapWindow;
use crate::host::live::{self, tree_animates, tree_has_live_widget};
use crate::host::nodetree::NodeTree;
use crate::host::paint::Painter;
use crate::host::spectrum::SpectrumState;
#[cfg(feature = "midi")]
use crate::host::timeline::group_key;
use crate::host::widget::Widget;
#[cfg(feature = "midi")]
use crate::host::widget::WidgetKind;
use crate::host::widget::element::Key as HostKey;
use crate::host::world::World;
use crate::host::{BusSource, ClientId, GUI_EVENT, Host, HostEffect};
use crate::view::Renderers;

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
    /// The heavy views' shared pipelines — one set per window, drawing every
    /// waveform and spectrogram slot above.
    pub(super) renderers: Renderers,
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
    /// Triggered multichannel display window per audio-rate `scope` widget id,
    /// refreshed on the frame tick from the shared segment's tap rings. Also
    /// holds each `phasescope`'s interleaved L/R window (ids do not collide).
    pub(super) tap_windows: HashMap<i32, TapWindow>,
    /// Persistent FFT analysis states per `spectrum` widget id (the smoothed
    /// and peak-hold curves, one entry per channel), advanced on the frame
    /// tick.
    pub(super) spectra: HashMap<i32, Vec<SpectrumState>>,
    /// The retained history of every bus this window's tree declares a
    /// `retention` span on — the addressable past a forward-only source has
    /// none of. Keyed by **bus**: one history, however many views read it.
    pub(super) histories: HashMap<i32, crate::host::live::BusHistory>,
    /// The rolling time-frequency analysis of every retained waterfall, keyed
    /// by **widget**: two views of one bus may analyze it differently.
    pub(super) rolls: HashMap<i32, crate::host::waterfall::Waterfall>,
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
    /// WebSocket reply channels by connection id (each connection's thread
    /// writes them; the raw handle force-drops a slow consumer); registered
    /// on `WsConnected`, pruned on `WsDisconnected`.
    pub(super) ws_conns: HashMap<u64, (std::sync::mpsc::SyncSender<Vec<u8>>, TcpStream)>,
    /// Window opens requested before the first `resumed`, flushed on resume.
    pub(super) pending: Vec<(i32, ClientId)>,
    pub(super) resumed: bool,
    /// Next scheduled repaint for animated (meter/scope) windows.
    pub(super) next_frame: Instant,
    /// The server-buffer fetch machine (`/buffer_query` → chunked `/buffer_getRange`),
    /// shared with the browser front.
    pub(super) fetches: BufferFetches,
    /// The node tree last read from the server, by group id, feeding `nodetree`
    /// widgets (filled by `/group_queryTree.reply`).
    pub(super) node_trees: HashMap<i32, NodeTree>,
    /// Whether the client leg has registered for node notifications
    /// (`/server_notify 1`), so it is sent once even with several node-tree windows.
    pub(super) notified: bool,
    /// Next scheduled re-query of the server's node tree (the `/node_set` poll).
    pub(super) next_query: Instant,
    /// Standalone mode: the host booted a pre-loaded GuiDef with no script front
    /// (`--standalone`). Closing the last window then quits the app, so the
    /// embedded audio server is dropped (and `/server_quit`ed) instead of left running.
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
    /// The `text` field clipboard (Ctrl+C/X/V) — the native front's internal
    /// clipboard (no OS-clipboard dependency); host-wide so text travels between
    /// fields and windows.
    pub(super) text_clipboard: String,
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
            ws_conns: HashMap::new(),
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
            text_clipboard: String::new(),
        }
    }

    /// Pushes one fresh sample into every `scope`'s rolling history, read from the
    /// shared segment. Called once per animation frame tick (not per `render`), so
    /// the scope scrolls at a steady, time-based rate regardless of how often the
    /// window happens to repaint.
    fn advance_scopes(&mut self) {
        // The segment is taken out first (a cheap `Arc` clone), which is what
        // frees `self` for the per-window mutation — the same shape
        // `advance_tap_windows` uses, so both read the tick through the one
        // shared advance rather than a front-side copy of it.
        let shm = self.shm.clone();
        let world = World {
            bus: shm.as_deref(),
            ..Default::default()
        };
        let read = |bus: i32| world.control(bus);
        for (def_id, ws) in &mut self.windows {
            if let Some(tree) = self.host.window_def(*def_id) {
                live::advance_scope_histories(tree, read, &mut ws.scopes);
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
                    |bus, out| shm.read_bus(bus, out),
                    &mut ws.tap_windows,
                );
                crate::host::live::update_phase_windows(
                    tree,
                    sample_rate,
                    |bus, out| shm.read_bus(bus, out),
                    &mut ws.tap_windows,
                );
                crate::host::live::update_spectra(
                    tree,
                    |bus, out| shm.read_bus(bus, out),
                    &mut ws.spectra,
                );
                // The retained half: a history per watched bus, then the
                // rolling transform each waterfall makes of it.
                crate::host::live::update_retention(
                    tree,
                    sample_rate,
                    crate::host::live::retention_window(sample_rate, shm.window_limit()),
                    |bus, out| shm.read_bus_at(bus, out),
                    &mut ws.histories,
                );
                crate::host::live::update_waterfalls(
                    tree,
                    sample_rate,
                    &ws.histories,
                    &mut ws.rolls,
                );
            }
        }
        self.refresh_waterfall_slots();
    }

    /// Pushes the columns every waterfall analyzed this tick into its ring —
    /// and only the rolls that moved, so a still picture costs no upload.
    fn refresh_waterfall_slots(&mut self) {
        let mut totals: Vec<(i32, usize)> = Vec::new();
        for ws in self.windows.values_mut() {
            let dirty: Vec<i32> = ws
                .rolls
                .iter()
                .filter(|(_, roll)| roll.is_dirty())
                .map(|(id, _)| *id)
                .collect();
            for id in dirty {
                let Some(roll) = ws.rolls.get_mut(&id) else {
                    continue;
                };
                if let Some(total) =
                    frame::roll_into_slot(&mut ws.spectrograms, id, roll, &ws.gpu, &ws.renderers)
                {
                    totals.push((id, total));
                }
            }
        }
        // The axis has to know how long it is, or the navigation window falls
        // back to a span the size of the body in *samples* and the whole
        // history draws as one stretched column. It is the *live* setter: a
        // retained axis slides, so it follows the newest until someone
        // navigates it and then holds where they left it.
        for (id, total) in totals {
            self.host.set_live_timeline_total(id, total);
        }
    }

    /// Whether window `def_id` should repaint continuously: it has a `canvas`
    /// (time-driven, always), or a meter/scope with a shared segment to feed it.
    fn window_is_animated(&self, def_id: i32) -> bool {
        self.host.window_def(def_id).is_some_and(|tree| {
            tree_animates(tree)
                || (self.shm.is_some() && tree_has_live_widget(tree, self.host.timelines()))
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
            // Queued to the originating connection's thread, which writes it
            // as one binary message (WsDisconnected prunes it).
            ClientId::Ws(id) => crate::host::ws::reply(&self.ws_conns, id, &bytes),
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
    pub(super) fn emit_notes(&mut self, def_id: i32, widget_id: i32) {
        let Some(args) = self
            .host
            .window_def(def_id)
            .and_then(|t| interact::notes_event_args(t, widget_id))
        else {
            return;
        };
        if self.host.is_bound(widget_id) {
            // A bound roll may be driving another widget, whose window then
            // has to repaint: the apply behind a widget binding reports it the
            // same way a `/gui_set` does.
            let mut effects = Vec::new();
            self.host
                .forward_args(widget_id, args[1..].to_vec(), &mut effects);
            for effect in effects {
                if let HostEffect::Redraw(id) = effect {
                    self.redraw(id);
                }
            }
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
        let server_attached = self.host.server().is_some();
        // Disjoint field borrows: the tree (host), the bus (shm), the node trees,
        // and the window's GPU resources are separate fields of `self`.
        let Some(tree) = self.host.window_def(def_id) else {
            return;
        };
        let cursor = self.windows.get(&def_id).map(|w| w.cursor);
        let inputs = frame::FrameInputs {
            metrics: self.host.metrics_for(def_id),
            world: World {
                bus: self.shm.as_deref(),
                node_trees: &self.node_trees,
                server_attached,
                sample_rate: self.shm.as_ref().map_or(0.0, |s| s.sample_rate()),
                sample_clock: self.shm.as_ref().map_or(0.0, |s| s.sample_clock()),
                cursor,
                timelines: self.host.timelines(),
            },
            focused: self
                .host
                .focused()
                .filter(|(d, _)| *d == def_id)
                .map(|(_, id)| id),
            // A rewiring drag in flight draws its wire to the pointer.
            wiring: self
                .windows
                .get(&def_id)
                .and_then(|w| w.gestures.wiring())
                .and_then(|(id, port)| cursor.map(|(cx, cy)| (id, port, (cx as f32, cy as f32)))),
            marquee: self.windows.get(&def_id).and_then(|w| w.gestures.marquee()),
        };
        let Some(ws) = self.windows.get_mut(&def_id) else {
            return;
        };
        frame::render(
            &mut ws.gpu,
            &mut ws.renderers,
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
            &self.host.theme,
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
            UserEvent::WsConnected { id, reply, raw } => {
                self.ws_conns.insert(id, (reply, raw));
            }
            UserEvent::WsOsc { id, bytes } => {
                let packet = match clausters_core::osc::decode_packet(&bytes) {
                    Ok(p) => p,
                    Err(e) => return warn!("malformed OSC packet from ws client {id}: {e}"),
                };
                let from = ClientId::Ws(id);
                let effects = self.host.handle_packet(packet, from);
                self.apply(event_loop, from, effects);
            }
            UserEvent::WsDisconnected { id } => {
                self.ws_conns.remove(&id);
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
    /// and a low-rate re-query for node-tree windows so `/node_set` changes show.
    /// With neither, windows stay event-driven (`Wait`).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut next_wake: Option<Instant> = None;

        // Drain replies from an embedded server (standalone): the UDP leg uses a
        // background thread, but the embed ring is polled here on the main thread.
        self.drain_embed_replies();

        // A clip drag held against a lane's edge scrolls the view under a
        // standing cursor, so it needs the frame tick exactly as an animated
        // window does — and it must run before the repaint below.
        self.advance_edge_scroll(FRAME.as_secs_f64());

        // Meter/scope animation, driven from the shared segment.
        let animated: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_is_animated(*id) || self.window_is_edge_scrolling(*id))
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

        // Node-tree polling, driven from the client leg (the `/node_set` poll).
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
            // The window moved to a display of another density (or the desktop's
            // scaling changed under it): re-resolve this window's size table at
            // the new factor, and *answer* the writer with the same **logical**
            // extent the window had — a 800x600 shell stays a 800x600 shell, in
            // the pixels the new display measures it by. The surface resize
            // arrives as the `Resized` that follows.
            WindowEvent::ScaleFactorChanged {
                scale_factor,
                mut inner_size_writer,
            } => {
                let previous = self.host.ui_scale(def_id) as f64;
                if self.host.set_ui_scale(def_id, scale_factor as f32)
                    && let Some(ws) = self.windows.get_mut(&def_id)
                {
                    let logical = ws.gpu.window.inner_size().to_logical::<f64>(previous);
                    let want = logical.to_physical(scale_factor);
                    if let Err(e) = inner_size_writer.request_inner_size(want) {
                        tracing::debug!("window {def_id}: keeping the size at the new scale: {e}");
                    }
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
                // The focus consumes the key first — Tab walks the ring, and a
                // focused element edits (typing, caret motion, cut/copy/paste).
                // Only what nothing there answered reaches the global shortcuts
                // below, which are addressed to what is under the cursor.
                if let Some(k) = to_key(&event.logical_key)
                    && self.key_input(def_id, k)
                {
                    return;
                }
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
        let mut out = Vec::new();
        for &def_id in self.windows.keys() {
            let Some(tree) = self.host.window_def(def_id) else {
                continue;
            };
            out.extend(tree.descendants().filter_map(|w| {
                matches!(w.kind, WidgetKind::PianoRoll { midi_in: true, .. })
                    .then_some((def_id, w.id?))
            }));
        }
        out
    }

    /// The shared playhead's current sample for a widget while it is running
    /// (`playhead_at` anchored to the engine clock), else `None`. It is the
    /// widget's navigation group that is running or not — the recording keeps
    /// time with what the lanes draw, which is the group's sweep.
    pub(super) fn playhead_sample(&self, def_id: i32, id: i32) -> Option<f64> {
        let tree = self.host.window_def(def_id)?;
        let e = tree.find(id)?.kind.editor()?;
        let clock = self.shm.as_ref().map_or(0.0, |s| s.sample_clock());
        self.host
            .timelines()
            .state(group_key(id, e.link))?
            .swept_at(clock)
    }

    /// A piano-roll's `snap` grid (0 when none or not a roll).
    pub(super) fn roll_snap(&self, def_id: i32, id: i32) -> f64 {
        match self.host.widget_kind(def_id, id) {
            Some(WidgetKind::PianoRoll { snap, .. }) => *snap,
            _ => 0.0,
        }
    }
}

/// Translates a winit key into the platform-neutral [`HostKey`] the focus reads,
/// or `None` for one nothing focusable answers (the global shortcuts then run).
/// A printable character (including Space) inserts; the named editing keys and
/// Tab map one-to-one.
pub(super) fn to_key(key: &Key) -> Option<HostKey> {
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
