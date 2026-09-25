//! The windowed host's state and winit handler: the [`App`] (one per process)
//! and the per-window [`WindowState`], plus the plumbing every other `gui`
//! submodule drives -- sending replies/events over the right transport, the
//! bound-vs-event delivery door, the animation tick and the shared-frame render.

use std::collections::HashMap;
use std::net::{TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clausters_core::osc::{OscMessage, OscPacket, encode};
use tracing::warn;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;

use crate::canvas::CanvasView;
use crate::gpu::Gpu;
use crate::host::fetch::BufferFetches;
use crate::host::frame::{self, SlotAt, SpectrogramSlot, WaveformSlot};
use crate::host::gestures::{ClipVerb, Gestures, Wheel, WheelDelta};
use crate::host::graphics::nodetree::NodeTree;
use crate::host::live::{self, tree_animates, tree_has_live_widget};
use crate::host::paint::Painter;
// Only the MIDI painting reaches a roll by its navigation group.
#[cfg(feature = "midi")]
use crate::host::timeline::group_key;
use crate::host::widget::Widget;
use crate::host::widget::element::Live;
use crate::host::winit_keys::{is_space, to_key};
use crate::host::world::World;
use crate::host::{BusSource, ClientId, Host, HostEffect};
use crate::view::Renderers;

use super::{FRAME, NODETREE_POLL, PLACEHOLDER_ORIGIN, UserEvent};

/// One open window: its GPU surface, the per-waveform slots, the painter, the
/// script address its events go to, the pointer/drag state, and the per-`scope`
/// rolling history. The widget tree itself lives in the [`Host`] (single source
/// of truth).
pub(super) struct WindowState {
    pub(super) gpu: Gpu,
    pub(super) waveforms: HashMap<SlotAt, WaveformSlot>,
    /// Per-`spectrogram` GPU resources (one STFT view per channel lane).
    pub(super) spectrograms: HashMap<SlotAt, SpectrogramSlot>,
    /// Per-`canvas` GPU resources (the compiled user shader + uniforms).
    pub(super) canvases: HashMap<i32, CanvasView>,
    /// The heavy views' shared pipelines -- one set per window, drawing every
    /// waveform and spectrogram slot above.
    pub(super) renderers: Renderers,
    pub(super) painter: Painter,
    /// The second mesh pass: editor chrome drawn over the heavy views
    /// (selection, playhead, rulers' overlay parts, cursor readout).
    pub(super) overlay: Painter,
    pub(super) origin: ClientId,
    /// **Where the pointer is in this window, or `None` when it is not over
    /// it** -- and an `Option` rather than a pair for exactly that reason.
    ///
    /// It used to be a pair with `(-1.0, -1.0)` standing in for *not here*,
    /// which is a sentinel in the field that means *a position*. Everything
    /// reading it as one got a point a hand cannot be at: the edge auto-scroll
    /// saw a cursor to the left of every lane and pulled the view to the start
    /// of the timeline at full tilt, so a clip dragged across the window
    /// manager's own border jumped to the beginning. Absent and *at minus one*
    /// are different facts, and only one of them is a coordinate.
    pub(super) cursor: Option<(f64, f64)>,
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
    /// `retention` span on -- the addressable past a forward-only source has
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
    /// The last whole second the playhead clock was logged at, so the debug
    /// line is one a second rather than one a frame.
    pub(super) head_said: u64,
    /// Next scheduled repaint for animated (meter/scope) windows.
    /// When this front started -- what a wall clock in milliseconds is measured
    /// from, since the gesture machine wants elapsed time and not a date.
    pub(super) started: Instant,
    pub(super) next_frame: Instant,
    /// The server-buffer fetch machine (`/buffer_query` -> chunked `/buffer_getRange`),
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
    /// Next check of the write frontiers -- the recording tick, on the frame
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
    /// The host-wide clipboard (Ctrl+C/X/V) -- the native front's internal one,
    /// no OS-clipboard dependency -- so what is cut in one window pastes into
    /// another. A block of notes rides it in the same JSON a `/gui_set notes`
    /// takes, which is the carrier every non-scalar already uses.
    pub(super) text_clipboard: crate::host::clipboard::Clip,
    /// How long the last tick was, for whatever advances in time.
    tick_clock: crate::host::live::TickClock,
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
            head_said: u64::MAX,
            started: Instant::now(),
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
            tick_clock: Default::default(),
        }
    }

    /// **One tick of the outside**, once per animation frame (not per repaint,
    /// so a scope scrolls at a steady, time-based rate however often a window
    /// happens to redraw).
    ///
    /// Two steps, in this order and for one reason: a **history is the bus's**
    /// and is filled first, then every widget of the tree advances whatever it
    /// keeps of its own -- a rolling trace, a triggered window, an analysis, a
    /// waterfall's transform -- reading a history where it needs one. Without a
    /// segment there is nothing to read and the live views stay empty, drawing
    /// their framed field.
    fn advance_live(&mut self) {
        let Some(shm) = self.shm.clone() else {
            return;
        };
        let sample_rate = shm.sample_rate();
        let dt = self.tick_clock.delta();
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
                    dt,
                    histories: &ws.histories,
                },
            );
        }
        self.refresh_slots();
    }

    /// Uploads whatever the trees have for their GPU slots this tick -- the
    /// columns a waterfall just analyzed, the picture an element that got its
    /// data rebuilt -- and only what moved, so a still window costs no upload.
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
        // **First, whatever was told to read itself again.** A `reload` is the
        // element forgetting what it resolved, so a fill that ran before the
        // reload was served would fill from nothing and leave the stale picture
        // on the card.
        self.reload_bulk_for(def_id);
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
        self.host.apply_extents(extents);
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

    /// Emits `/gui_event widget_id seq version <args...>` to the window's script.
    ///
    /// The stamp and the version are the **second and third** arguments, before
    /// any tag, so one rule reads every event whatever its payload: a control's
    /// bare value and a roll's variable-length note list are both
    /// `<id> <seq> <version> ...`. A `seq` of zero means the event is not an edit
    /// anyone will acknowledge; a `version` of zero means the host cannot say
    /// what state it drew, which is what an owner that never speaks of versions
    /// leaves it with.
    ///
    /// The message is built by [`Host::event_message`], the one place both
    /// fronts build an event, and a host that owns the window was offered it
    /// first ([`Host::deliver`]).
    pub(super) fn emit(&self, def_id: i32, message: OscMessage) {
        let Some(ws) = self.windows.get(&def_id) else {
            return;
        };
        self.send(ws.origin, message);
    }

    /// Delivers what an element reported outside the gesture machine -- the
    /// live-MIDI painting path -- by the one rule the machine also follows: a
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
        let message = self.host.event_message(widget_id, seq, args);
        self.emit(def_id, message);
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
    /// the same code the browser front drives -- here fed the live inputs (the
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
        let cursor = self.windows.get(&def_id).and_then(|w| w.cursor);
        // Held for the length of the frame it feeds: the log is the host's, and
        // a front that copied it would be a second log.
        let statuses = self.host.statuses();
        let inputs = frame::FrameInputs {
            metrics: self.host.metrics_for(def_id),
            world: World {
                bus: self.shm.as_deref(),
                node_trees: &self.node_trees,
                server_attached,
                sample_rate: self.shm.as_ref().map_or(0.0, |s| s.sample_rate()),
                clocks: {
                    let clocks = self.host.head_clocks(def_id, self.shm.as_deref());
                    let now = clocks.window;
                    // Once a second, and only under `debug`: what the head is
                    // being drawn from. A line that does not move is either a
                    // transport that is not rolling or a segment nobody is
                    // publishing into, and from the outside those look the same.
                    if tracing::enabled!(tracing::Level::DEBUG) {
                        let whole = now as u64 / 48_000;
                        if self.head_said != whole {
                            self.head_said = whole;
                            tracing::debug!("playhead clock: {now}");
                        }
                    }
                    clocks
                },
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
            status: statuses.get(&def_id),
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
        for (id, origin) in std::mem::take(&mut self.pending) {
            self.open_window(event_loop, id, origin);
        }
        // Standalone: a GuiDef pre-loaded into the host before the loop started
        // (no `/gui_def` over the wire) is opened now. Its events have no script
        // to return to, so they go to a placeholder origin. Pre-loaded windows
        // mean this is a standalone app -- closing the last one quits it.
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
            UserEvent::ServerOsc { leg, bytes } => match clausters_core::osc::decode_packet(&bytes)
            {
                Ok(packet) => self.handle_server_packet(packet, leg),
                Err(e) => warn!("malformed OSC reply from the audio server: {e}"),
            },
        }
    }

    /// After handling events, schedule the next wake-up: a ~30 fps repaint for
    /// animated (meter/scope) windows so their shared-memory values keep moving,
    /// and a low-rate re-query for node-tree windows so `/node_set` changes show.
    /// With neither, windows stay event-driven (`Wait`).
    ///
    /// **Every source of a wake-up asks for its own time and the soonest wins.**
    /// Two of them used to assign instead of taking the minimum, so whichever
    /// ran last decided and the other's deadline was dropped -- a window
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
        // window does -- and it must run before the repaint below.
        self.advance_edge_scroll(FRAME.as_secs_f64());

        // **A multitrack's clock reads where the transport is**, for a host that
        // edits one with nobody else in the process to write the label.
        if let Some(position) = self.shm.as_deref().map(|bus| {
            bus.transport_position(clausters_editing::apply::MULTITRACK_TRANSPORT as usize)
        }) && let Some(def_id) = self.host.tick_multitrack_clock(position)
            && let Some(ws) = self.windows.get(&def_id)
        {
            ws.gpu.window.request_redraw();
        }

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
            // amount of summarizing and a repaint per view per block -- with
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
            // extent the window had -- a 800x600 shell stays a 800x600 shell, in
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
                    // **A drag owns the pointer until the button comes up.** A
                    // press captures it, so crossing onto the window manager's
                    // chrome -- a resize border, the title bar -- is the
                    // platform saying the pointer left the *content*, not that
                    // it stopped belonging to this gesture: motion keeps
                    // arriving and the drag keeps following. Forgetting the
                    // position there is what made a held clip jump.
                    if !ws.gestures.dragging() {
                        // Off-window: the cursor readout hides (nothing
                        // contains it).
                        ws.cursor = None;
                        ws.gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ws) = self.windows.get_mut(&def_id) {
                    ws.cursor = Some((position.x, position.y));
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
                    // frame per move -- a static window (a plot's) has no
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
                // The shell translates its own event; how many steps that is
                // belongs to the wheel and is written once (NATIVE).
                let delta = match delta {
                    MouseScrollDelta::LineDelta(_, y) => WheelDelta::Lines(y as f64),
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels(p.y),
                };
                let steps = Wheel::NATIVE.steps(delta, self.host.ui_scale(def_id) as f64);
                self.on_wheel(def_id, steps);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // The letter under a chord, the typed character otherwise --
                // see `key_pressed`.
                let pressed = key_pressed(&event, self.ctrl(def_id) || self.alt(def_id));
                tracing::debug!(
                    "key: pressed={pressed:?} logical={:?} ctrl={} shift={}",
                    event.logical_key,
                    self.ctrl(def_id),
                    self.shift(def_id)
                );
                // The focus consumes the key first -- Tab walks the ring, and a
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
                    // The transport: the space bar rolls the multitrack, or plays
                    // what the cursor is over and stops what is playing. Last
                    // among the window's own keys for the usual reason -- a
                    // focused field types a space, and a widget that wanted it
                    // answered already.
                    //
                    // **Both spellings, because a space is a typed character.**
                    // `key_pressed` hands back what the key *produced* when no
                    // chord is held, and a space produces `" "` -- so an arm
                    // matching only `NamedKey::Space` never fired at all, on
                    // any keyboard, and the take monitor had no key.
                    ref key if is_space(key) => {
                        self.play_key(def_id);
                    }
                    // `L` switches the take monitor's loop.
                    Key::Character(ref c) if c.eq_ignore_ascii_case("l") && !self.ctrl(def_id) => {
                        self.loop_key(def_id);
                    }
                    // Home and End put the position cursor at the start or the
                    // end of the samples under the pointer.
                    Key::Named(NamedKey::Home) => {
                        self.ends_key(def_id, false);
                    }
                    Key::Named(NamedKey::End) => {
                        self.ends_key(def_id, true);
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
                    // Ctrl+Shift+V pastes by adding: the block is mixed onto
                    // what is under it rather than put in.
                    Key::Character(ref c) if c.eq_ignore_ascii_case("v") && self.ctrl(def_id) => {
                        let verb = if self.shift(def_id) {
                            ClipVerb::Mix
                        } else {
                            ClipVerb::Paste
                        };
                        self.clipboard_key(def_id, verb);
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
    /// widget)` -- what the front opens its input port for.
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
    /// widget's navigation group that is running or not -- the recording keeps
    /// time with what the lanes draw, which is the group's sweep.
    pub(super) fn playhead_sample(&self, def_id: i32, id: i32) -> Option<f64> {
        let tree = self.host.window_def(def_id)?;
        let e = tree.find(id)?.kind.editor()?;
        let clock = self
            .host
            .head_clocks(def_id, self.shm.as_deref())
            .at(Some(id));
        self.host
            .timelines()
            .state(group_key(id, e.link))?
            .swept_at(clock)
    }
}

/// **The key a chord was pressed on, and the text everything else produced.**
///
/// winit's `logical_key` is the key *with modifiers applied*, which is right
/// for typing and wrong for a shortcut: `Ctrl`+`Z` arrives as the control
/// character `\u{1a}` and never equals `"z"`, so every chord in this host
/// matched nothing and vanished -- undo, redo and the clipboard verbs alike,
/// none of which had ever run outside a test. `key_without_modifiers` is
/// winit's own answer to exactly this, and it ignores `Shift` too, so a
/// `Ctrl`+`Shift`+`Z` is read as `z` with the shift taken from the tracked
/// modifier state (which is where the rest of the host already reads it).
///
/// **Which is why it is asked only when a chord is held.** Applied to every
/// key it also unshifts plain typing, and a text field could then hold no
/// capital and no accented letter: `A` arrived as `a`, `Á` as `a`, and the
/// field looked like it only spoke lowercase ASCII. So `chord` (Ctrl or Alt
/// down, the two that turn a letter into a command) picks the reading.
///
/// What everything else reads is **`text`**, not `logical_key`, and the two
/// part company exactly where composition happens. A dead key composes onto
/// the *next* press, and that press keeps its own identity: `` ` `` then Space
/// is still the Space key, so `logical_key` says `Space` while `text` says
/// `` ` ``. Reading the key there types a blank where the accent should be and
/// loses the accent altogether. `text` is what this press actually put on the
/// screen, which is the only question a field is asking.
///
/// A press with no text (an arrow, Escape) or whose text is a control
/// character (Enter's `\r`, Tab's `\t`, Backspace) falls back to
/// `logical_key`, where those are the named keys the editing verbs match on.
fn key_pressed(event: &winit::event::KeyEvent, chord: bool) -> Key {
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
        if chord && let Key::Character(c) = event.key_without_modifiers() {
            return Key::Character(c);
        }
    }
    if !chord
        && let Some(text) = event.text.as_ref()
        && !text.chars().any(char::is_control)
    {
        return Key::Character(text.clone());
    }
    event.logical_key.clone()
}
