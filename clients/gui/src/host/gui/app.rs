//! The windowed host's state and winit handler: the [`App`] (one per process)
//! and the per-window [`WindowState`], plus the plumbing every other `gui`
//! submodule drives — sending replies/events over the right transport, the
//! bound-vs-event delivery door, the animation tick and the shared-frame render.

use std::collections::HashMap;
use std::net::{TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clausters_core::osc::{OscMessage, OscPacket, OscType, encode};
use tracing::warn;
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, DeviceEvents};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::canvas::CanvasView;
use crate::gpu::Gpu;
use crate::host::fetch::BufferFetches;
use crate::host::frame::{self, SpectrogramSlot, WaveformSlot};
use crate::host::gestures::{ClipEdit, ClipVerb, Gestures};
use crate::host::graphics::nodetree::NodeTree;
use crate::host::live::{self, tree_animates, tree_has_live_widget};
use crate::host::paint::Painter;
// Only the MIDI painting reaches a roll by its navigation group.
#[cfg(feature = "midi")]
use crate::host::timeline::group_key;
use crate::host::widget::Widget;
use crate::host::widget::element::{Key as HostKey, Live};
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
    /// The retained history of every bus this window's tree declares a
    /// `retention` span on — the addressable past a forward-only source has
    /// none of. Keyed by **bus**: one history, however many views read it.
    pub(super) histories: HashMap<i32, crate::host::live::BusHistory>,
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
    /// The write frontier last **drawn**, per `(def_id, widget_id)`: how far
    /// the samples of that view had been written when its summary was last
    /// refreshed. What moves it is a recording (the server's S20), and the
    /// difference is exactly the span to re-read.
    pub(super) frontiers: HashMap<(i32, i32), u64>,
    /// Next scheduled re-query of the server's node tree (the `/node_set` poll).
    pub(super) next_query: Instant,
    /// Next check of the write frontiers — the recording tick, on the frame
    /// cadence and separate from the animated one because it redraws only
    /// when the samples actually grew.
    pub(super) next_follow: Instant,
    /// Standalone mode: the host booted a pre-loaded GuiDef with no script front
    /// (`--standalone`). Closing the last window then quits the app, so the
    /// embedded audio server is dropped (and `/server_quit`ed) instead of left running.
    pub(super) standalone: bool,
    /// Live MIDI input: the virtual input port, held open while any open
    /// window holds an element that declared it reads MIDI (dropping it closes
    /// the port).
    #[cfg(feature = "midi")]
    pub(super) midi_in: Option<clausters_midi::live::Input>,
    /// Whether the port-open failure was already reported (retrying is cheap,
    /// warning every frame is not).
    #[cfg(feature = "midi")]
    pub(super) midi_warned: bool,
    /// The host-wide clipboard (Ctrl+C/X/V) — the native front's internal one,
    /// no OS-clipboard dependency — so what is cut in one window pastes into
    /// another. A block of notes rides it in the same JSON a `/gui_set notes`
    /// takes, which is the carrier every non-scalar already uses.
    pub(super) text_clipboard: crate::host::clipboard::Clip,
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
            frontiers: HashMap::new(),
            next_query: Instant::now(),
            next_follow: Instant::now(),
            standalone: false,
            #[cfg(feature = "midi")]
            midi_in: None,
            #[cfg(feature = "midi")]
            midi_warned: false,
            text_clipboard: crate::host::clipboard::Clip::default(),
        }
    }

    /// **One tick of the outside**, once per animation frame (not per repaint,
    /// so a scope scrolls at a steady, time-based rate however often a window
    /// happens to redraw).
    ///
    /// Two steps, in this order and for one reason: a **history is the bus's**
    /// and is filled first, then every widget of the tree advances whatever it
    /// keeps of its own — a rolling trace, a triggered window, an analysis, a
    /// waterfall's transform — reading a history where it needs one. Without a
    /// segment there is nothing to read and the live views stay empty, drawing
    /// their framed field.
    fn advance_live(&mut self) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let sample_rate = shm.sample_rate();
        let window = live::retention_window(sample_rate, shm.window_limit());
        for (def_id, ws) in &mut self.windows {
            let Some(tree) = self.host.window_def_mut(*def_id) else {
                continue;
            };
            live::update_retention(
                tree,
                sample_rate,
                window,
                |bus, out| shm.read_bus_at(bus, out),
                &mut ws.histories,
            );
            live::tick_tree(
                tree,
                &Live {
                    bus: Some(shm.as_ref()),
                    sample_rate,
                    histories: &ws.histories,
                },
            );
        }
        self.refresh_slots();
    }

    /// Uploads whatever the trees have for their GPU slots this tick — the
    /// columns a waterfall just analyzed, the picture an element that got its
    /// data rebuilt — and only what moved, so a still window costs no upload.
    ///
    /// One walk over each window, asking the widgets rather than looking for
    /// them: the front knows nothing here about what a rolling transform is or
    /// which presentation makes a texture of its samples.
    fn refresh_slots(&mut self) {
        for def_id in self.windows.keys().copied().collect::<Vec<_>>() {
            self.refresh_slots_for(def_id);
        }
    }

    /// One window's share of [`refresh_slots`](Self::refresh_slots). Also run
    /// **before a repaint**, since a window with nothing live in it never ticks
    /// at all: a `/gui_set` that rebuilt a picture would otherwise wait for a
    /// tick that never comes.
    pub(super) fn refresh_slots_for(&mut self, def_id: i32) {
        let mut extents: Vec<(i32, frame::Extent)> = Vec::new();
        // Disjoint field borrows: the tree is the host's, the slots the
        // window's.
        if let (Some(ws), Some(tree)) = (
            self.windows.get_mut(&def_id),
            self.host.window_def_mut(def_id),
        ) {
            frame::fill_slots(
                tree,
                None,
                &ws.gpu,
                &ws.renderers,
                &mut ws.waveforms,
                &mut ws.spectrograms,
                &mut extents,
            );
        }
        self.apply_extents(extents);
    }

    /// Registers what a fill turned out to be worth with the widget's
    /// navigation axis, which has to know how long it is or the visible window
    /// falls back to a span the size of the body in *samples* and the whole
    /// picture draws as one stretched column.
    ///
    /// A **rolling** extent goes through the live setter: a retained axis
    /// slides, so it follows the newest column until someone navigates it and
    /// then holds where they left it.
    pub(super) fn apply_extents(&mut self, extents: Vec<(i32, frame::Extent)>) {
        for (id, extent) in extents {
            match extent {
                frame::Extent::Stored(total) => self.host.set_timeline_total(id, total),
                frame::Extent::Rolling(total) => self.host.set_live_timeline_total(id, total),
            }
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
                // Port 0 is **nobody**: what a def opened by this binary itself
                // carries (a session, a standalone bundle), where there is no
                // script to answer. Sending there fails on every event, which
                // is a warning a second rather than a fact worth reporting.
                if to.port() == 0 {
                    return;
                }
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

    /// Emits `/gui_event widget_id seq version <args…>` to the window's script.
    ///
    /// The stamp and the version are the **second and third** arguments, before
    /// any tag, so one rule reads every event whatever its payload: a control's
    /// bare value and a roll's variable-length note list are both
    /// `<id> <seq> <version> …`. A `seq` of zero means the event is not an edit
    /// anyone will acknowledge; a `version` of zero means the host cannot say
    /// what state it drew, which is what an owner that never speaks of versions
    /// leaves it with.
    ///
    /// The version is read here rather than carried in the gesture's effect
    /// because it is the **conversation's** state and not the gesture's: what
    /// the edit was made against is what the host had been told when it went
    /// out, which is this moment.
    pub(super) fn emit(&self, def_id: i32, widget_id: i32, seq: i32, mut args: Vec<OscType>) {
        let Some(ws) = self.windows.get(&def_id) else {
            return;
        };
        let mut msg_args = vec![
            OscType::Int(widget_id),
            OscType::Int(seq),
            OscType::Long(self.host.outbox.borrow().version()),
        ];
        msg_args.append(&mut args);
        self.send(
            ws.origin,
            OscMessage {
                addr: GUI_EVENT.into(),
                args: msg_args,
            },
        );
    }

    /// Delivers what an element reported outside the gesture machine — the
    /// live-MIDI painting path — by the one rule the machine also follows: a
    /// **bound** widget forwards the payload without its tag straight to the
    /// audio server, an unbound one emits the whole tagged list to the script.
    #[cfg(feature = "midi")]
    pub(super) fn emit_element(
        &mut self,
        def_id: i32,
        widget_id: i32,
        args: Vec<clausters_core::osc::OscType>,
    ) {
        if self.host.is_bound(widget_id) {
            // A bound widget may be driving another one, whose window then has
            // to repaint: the apply behind a widget binding reports it the same
            // way a `/gui_set` does.
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
        // Stamped like any other edit: live MIDI painting reports the same
        // payloads a hand does, and the owner has no way to tell them apart.
        let seq = self.host.outbox.borrow_mut().stamp(def_id, widget_id);
        self.emit(def_id, widget_id, seq, args);
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

    /// Renders window `def_id` through the shared frame path ([`frame::render`]),
    /// the same code the browser front drives — here fed the live inputs (the
    /// shared-memory bus, the scope histories, the node trees, the held button).
    fn render(&mut self, def_id: i32) {
        tracing::trace!("rendering window {def_id}");
        // Whatever an element has for its slot reaches the card before the
        // frame that draws it.
        self.refresh_slots_for(def_id);
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
            // What this window's drag is holding: while one is in flight the
            // grips are its own and nothing else lights up.
            grab: self
                .windows
                .get(&def_id)
                .map_or(frame::Grab::None, |w| w.gestures.grab()),
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
    ///
    /// **Every source of a wake-up asks for its own time and the soonest wins.**
    /// Two of them used to assign instead of taking the minimum, so whichever
    /// ran last decided and the other's deadline was dropped — a window
    /// following a recording *and* animating would keep only one of the two,
    /// depending on the order this function happens to be written in.
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

        // **What the last frame could not draw.** A view zoomed finer than its
        // summary leaves the span it was asked for on its slot; here that note
        // becomes a read of exactly that span, so a picture that cannot map
        // the samples still resolves to the sample where the eye is.
        self.fetch_wanted_spans();

        // **A recording is drawn as it fills.** The samples are mapped, so
        // they need nothing; what moves is the frontier its writer
        // publishes, and the summary over the frames it added.
        //
        // It keeps the loop waking while a take fills and **redraws only what
        // actually grew**, which is why it is not folded into the animated set
        // below: a meter repaints every tick because its value may have
        // changed and nothing says so, while a recording says exactly when it
        // changed and by how much. Joining them would repaint a still take
        // thirty times a second for a number that did not move.
        if now >= self.next_follow {
            for def_id in self.follow_recordings() {
                if let Some(ws) = self.windows.get(&def_id) {
                    ws.gpu.window.request_redraw();
                }
            }
            // **The block is the tick, and it is one tick for every view.**
            // Letting each view wait for its own block would be the same
            // amount of summarizing and a repaint per view per block — with
            // thirty-two takes recording at once, thirty-two window repaints a
            // second instead of one, which is a cost that grows with the
            // square of the track count and was measured doing exactly that.
            // On a shared tick every take that grew is caught up together and
            // the window is repainted once, whatever the count.
            let follow = Duration::from_secs_f64(self.host.follow_block.max(0.0)).max(FRAME);
            self.next_follow = now + follow;
        }
        if self
            .windows
            .keys()
            .any(|id| self.window_follows_a_recording(*id))
        {
            next_wake = Some(next_wake.map_or(self.next_follow, |t| t.min(self.next_follow)));
        }

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
                self.advance_live();
                for id in &animated {
                    if let Some(ws) = self.windows.get(id) {
                        ws.gpu.window.request_redraw();
                    }
                }
                self.next_frame = now + FRAME;
            }
            next_wake = Some(next_wake.map_or(self.next_frame, |t| t.min(self.next_frame)));
        }

        // Live MIDI input: while any open window holds an element that
        // declared it reads MIDI, the virtual input port is held open and
        // drained at the frame cadence (the last such element closing drops
        // the port).
        #[cfg(feature = "midi")]
        {
            let readers = self.midi_readers();
            if readers.is_empty() {
                self.midi_in = None;
            } else {
                if self.midi_in.is_none() {
                    self.midi_in = clausters_midi::live::Input::open("clausters-gui");
                    if self.midi_in.is_none() && !self.midi_warned {
                        tracing::warn!("could not open the virtual MIDI input port");
                        self.midi_warned = true;
                    }
                }
                self.drain_midi(&readers);
                let t = now + FRAME;
                next_wake = Some(next_wake.map_or(t, |w| w.min(t)));
            }
        }

        // **A download in flight keeps the loop awake.** The embed ring is
        // polled right here on the main thread, so a window that has asked for
        // a buffer and then sleeps until the next input never reads the reply:
        // the take appears when the pointer happens to move, which reads as a
        // picture that does not load.
        if self.fetches.pending() {
            let t = now + FRAME;
            next_wake = Some(next_wake.map_or(t, |w| w.min(t)));
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
                // The letter, not the control character a chord would arrive
                // as — see `key_pressed`.
                let pressed = key_pressed(&event);
                tracing::debug!(
                    "key: pressed={pressed:?} logical={:?} ctrl={} shift={}",
                    event.logical_key,
                    self.ctrl(def_id),
                    self.shift(def_id)
                );
                // The focus consumes the key first — Tab walks the ring, and a
                // focused element edits (typing, caret motion, cut/copy/paste).
                // Only what nothing there answered reaches the global shortcuts
                // below, which are addressed to what is under the cursor.
                if let Some(k) = to_key(&pressed)
                    && self.key_input(def_id, k)
                {
                    tracing::debug!("key: consumed by the focus");
                    return;
                }
                // ...then the element **under the cursor**, which is where a
                // block operation is addressed: a selection is already where
                // the pointer has been. Only what nothing there answered
                // reaches the window's own shortcuts.
                if let Some(k) = to_key(&pressed)
                    && self.key_at_cursor(def_id, k)
                {
                    tracing::debug!("key: consumed by the element under the cursor");
                    return;
                }
                tracing::debug!("key: reached the window's own shortcuts");
                match pressed {
                    Key::Named(NamedKey::Escape) => self.user_close(def_id, event_loop),
                    // Undo and redo are the window's, not a widget's: they are
                    // addressed to the document behind it rather than to
                    // whatever is under the cursor. Ctrl+Shift+Z redoes, which
                    // is the spelling that works on a keyboard with no Y where
                    // an English one has one.
                    Key::Character(ref c) if c.eq_ignore_ascii_case("z") && self.ctrl(def_id) => {
                        self.history(def_id, self.shift(def_id))
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("y") && self.ctrl(def_id) => {
                        self.history(def_id, true)
                    }
                    // Saving is the window's too, and for the same reason:
                    // what is saved is the document behind it, not whatever is
                    // under the cursor. A host that owns nothing emits it and a
                    // script may answer; one that owns a session writes it.
                    Key::Character(ref c) if c.eq_ignore_ascii_case("s") && self.ctrl(def_id) => {
                        self.window_verb(def_id, "save")
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => {
                        self.reset_timelines(def_id)
                    }
                    // A clip's own edit verbs, over the clip under the cursor:
                    // cut it at the time cursor, or read it and what touches it
                    // as one. Plain letters, like `r`, and reached only by a
                    // key nothing focused wanted.
                    Key::Character(ref c) if c.eq_ignore_ascii_case("e") => {
                        self.clip_verb(def_id, ClipEdit::Split);
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("j") => {
                        self.clip_verb(def_id, ClipEdit::Join);
                    }
                    // The monitor: the space bar plays what the cursor is over
                    // and stops what is playing. Last among the window's own
                    // keys for the usual reason — a focused field types a
                    // space, and a widget that wanted it answered already.
                    Key::Named(NamedKey::Space) => {
                        self.play_key(def_id);
                    }
                    // The clipboard verbs over the view under the cursor. They
                    // are last, so a focused field and a roll's own block keys
                    // both answer first: this is what nothing else wanted.
                    Key::Character(ref c) if c.eq_ignore_ascii_case("c") && self.ctrl(def_id) => {
                        self.clipboard_key(def_id, ClipVerb::Copy);
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("x") && self.ctrl(def_id) => {
                        self.clipboard_key(def_id, ClipVerb::Cut);
                    }
                    Key::Character(ref c) if c.eq_ignore_ascii_case("v") && self.ctrl(def_id) => {
                        self.clipboard_key(def_id, ClipVerb::Paste);
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
    /// Every element that **declared** it reads live MIDI, as `(window,
    /// widget)` — what the front opens its input port for.
    pub(super) fn midi_readers(&self) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        for &def_id in self.windows.keys() {
            let Some(tree) = self.host.window_def(def_id) else {
                continue;
            };
            out.extend(
                tree.descendants()
                    .filter_map(|w| w.kind.needs().midi.then_some((def_id, w.id?))),
            );
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
}

/// Translates a winit key into the platform-neutral [`HostKey`] the focus reads,
/// or `None` for one nothing focusable answers (the global shortcuts then run).
/// A printable character (including Space) inserts; the named editing keys and
/// Tab map one-to-one.
/// **The key that was pressed, whatever was held over it.**
///
/// winit's `logical_key` is the key *with modifiers applied*, which is right
/// for typing and wrong for a shortcut: `Ctrl`+`Z` arrives as the control
/// character `\u{1a}` and never equals `"z"`, so every chord in this host
/// matched nothing and vanished — undo, redo and the clipboard verbs alike,
/// none of which had ever run outside a test. `key_without_modifiers` is
/// winit's own answer to exactly this, and it ignores `Shift` too, so a
/// `Ctrl`+`Shift`+`Z` is read as `z` with the shift taken from the tracked
/// modifier state (which is where the rest of the host already reads it).
///
/// Only the **letter** is restored: a named key (Escape, Tab, the arrows) is
/// the same either way, and taking `logical_key` for those keeps the dead-key
/// and IME behaviour the text path depends on.
fn key_pressed(event: &winit::event::KeyEvent) -> Key {
    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd",
    ))]
    {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        if let Key::Character(c) = event.key_without_modifiers() {
            return Key::Character(c);
        }
    }
    event.logical_key.clone()
}

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
