//! The GUI host: an OSC front with a widget command interpreter.
//!
//! `clausters-gui` is **two roles in one process**: a *GUI server (host)* for
//! the language clients — it owns the windows, the widgets and (later) the GPU,
//! and speaks the `/gui_*` widget protocol — and a *client of the audio server*
//! — it reads buffers/buses/the node tree and sends control, exactly as the
//! Python client does. This milestone is the **headless skeleton**: the server
//! front and a scaffolded client leg, a widget [`Registry`], and a command loop
//! that interprets `/gui_def`/`/gui_set`/`/gui_free`/`/gui_query`, logs them and
//! answers `/gui_query` with `/gui_info`. No GPU yet (G3 brings the first
//! pixels).
//!
//! ## Transport decision (recorded here per the milestone)
//!
//! The host does **not** extract or link the audio server's transport layer
//! (`src/osc/{server,tcp,ws}.rs`): that code is tangled with the audio
//! `ServerState`, the engine wake and the IPC ring, so lifting it now would drag
//! server concerns into this crate for no gain. Instead the host **links
//! `clausters-core`** — a path dependency that pulls only `rosc` — for the shared
//! OSC seam (the single [`clausters_core::osc::decode_packet`] door, plus
//! encode/bundle/message), and owns a **thin transport front** of its own
//! ([`transport`]). The default build links no server code; only the optional
//! `standalone` feature pulls the full `clausters` crate, for the in-process
//! embedded server ([`embed`]). G2 ships the
//! **UDP** front (the default Clausters carrier, minimal to drive from a Python
//! client); TCP/WebSocket/ring are added in later milestones behind the same
//! [`transport::ClientId`] and reply seam, which is shaped to generalize. The
//! client leg ([`client::ServerLeg`]) reuses that same encode door, so the gui
//! talks to the audio server with one encoder, not a parallel one.

// The platform-agnostic core: the widget/protocol logic, web-portable (it
// compiles for `wasm32` unchanged). No sockets, no filesystem, no GPU bring-up —
// every such coupling lives behind a trait whose impl is in the native shell
// below.
pub mod bind;
pub mod bpf;
pub mod canvas;
pub mod controls;
pub mod fetch;
pub mod font;
pub mod frame;
pub mod graph;
pub mod guidef;
pub mod interact;
pub mod layout;
pub mod live;
pub mod meters;
pub mod nodetree;
pub mod oscil;
pub mod paint;
pub mod phasescope;
pub mod pianoroll;
pub mod plot;
pub mod registry;
pub mod ruler;
pub mod spectrum;
pub mod timeline;
pub mod track;
pub mod widget;

// The native I/O shell, excluded from `wasm32`: the UDP client leg
// ([`Transport`]), on-disk GuiDef persistence ([`DefStore`]) and the UDP server
// front. The browser fills the same seams over WebSocket/fetch in later
// milestones. The winit/wgpu driver ([`gui`]) and the mmap bulk loader
// ([`bulk`]) are gated below.
#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
#[cfg(not(target_arch = "wasm32"))]
pub mod tcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod transport;

// Reading the audio server's shared-memory segment for zero-message meters/
// scopes (G5), the native [`BusSource`]. Unix-only, as the server's segment is.
#[cfg(unix)]
pub mod shm;

// The in-process embedded server for the standalone mode, a direct dependency on
// the `clausters` crate behind the optional `standalone` feature (off by default,
// since it pulls the engine + audio backend). Native-only.
#[cfg(feature = "standalone")]
pub mod embed;

// Mapping a local file (raw samples or a prebuilt peak cache) for the bulk-data
// path: a multi-megabyte buffer rendered from a shared resource, not over OSC
// (G7). Unix-only, like `shm`.
#[cfg(unix)]
pub mod mapfile;

// The native [`BulkLoader`]: resolves a waveform/plot's local `path`/`cache` to
// samples or a peak pyramid through the mmap path above. The browser resolves
// the same references over the network in a later milestone.
#[cfg(not(target_arch = "wasm32"))]
pub mod bulk;

// The windowed host (winit + wgpu) is native-only; the wasm build swaps it for
// the `<canvas>` surface in [`web`]. Both drive the shared [`frame`] render.
#[cfg(not(target_arch = "wasm32"))]
pub mod gui;

// The browser entry point: a `<canvas>` WebGPU surface with async GPU bring-up,
// rendering a compiled-in GuiDef through the same `frame` path the native front
// uses. No transport yet (G12); the WebSocket carrier lands in G13. wasm-only.
#[cfg(target_arch = "wasm32")]
pub mod web;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use serde_json::Value;
use tracing::{debug, info, warn};

pub use bind::Binding;
#[cfg(not(target_arch = "wasm32"))]
pub use client::ServerLeg;
pub use guidef::GuiNode;
pub use registry::Registry;
pub use widget::Widget;

/// Where a request reached the host and where its replies go. The `/gui_*`
/// *encoding* is transport-independent, so client identity is too — UDP today
/// (the native front), with other carriers (a browser WebSocket) added behind
/// the same seam. It lives in the agnostic core, not the UDP front, so the
/// protocol dispatch names it on every platform.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClientId {
    /// A UDP datagram source (the native server front).
    Udp(SocketAddr),
    /// A TCP connection on the native server front (G25), by connection id —
    /// length-prefixed frames, replies routed back on the same connection.
    Tcp(u64),
    /// The browser's in-page binding surface (the wasm front feeds OSC packets
    /// in and drains events out through it; there is no socket address).
    Web,
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientId::Udp(addr) => write!(f, "{addr}"),
            ClientId::Tcp(id) => write!(f, "tcp client {id}"),
            ClientId::Web => write!(f, "web"),
        }
    }
}

/// A source of live control-bus values for the meter/scope views (see
/// [`BusSource`] below) — kept near the other platform seams.
///
/// Where the host's client leg points: a UDP audio server (the normal case), an
/// in-process embedded server (standalone, the `standalone` feature), or a
/// browser WebSocket to a `--ws` server (wasm). All speak the same OSC through
/// the one encode door, so the host forwards bound-widget values and queries the
/// same way regardless of which is behind the link. The link is a concrete enum
/// (its reply path differs per carrier); the protocol logic reaches it only
/// through [`Transport::send`]/[`ServerLink::send`], so a new carrier plugs in
/// behind the same seam as one more cfg-gated variant.
pub enum ServerLink {
    /// A UDP audio server (the `--server host:port` leg).
    #[cfg(not(target_arch = "wasm32"))]
    Udp(ServerLeg),
    /// An in-process server linked directly from the `clausters` crate
    /// (standalone boot; the `standalone` feature).
    #[cfg(feature = "standalone")]
    Embed(embed::EmbedServer),
    /// A browser WebSocket to a `--ws` audio server (the only carrier a browser
    /// can open to a separate process). Bound widgets forward through it.
    #[cfg(target_arch = "wasm32")]
    Ws(web::WsServerLink),
}

impl ServerLink {
    /// Sends one OSC message to the server (a UDP datagram, the embed ring, or a
    /// browser WebSocket binary frame).
    pub fn send(&self, msg: OscMessage) -> std::io::Result<()> {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            ServerLink::Udp(leg) => leg.send(msg),
            #[cfg(feature = "standalone")]
            ServerLink::Embed(srv) => {
                let bytes = clausters_core::osc::encode(&OscPacket::Message(msg)).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
                if srv.send(&bytes) {
                    Ok(())
                } else {
                    Err(std::io::Error::other("embed command ring full"))
                }
            }
            #[cfg(target_arch = "wasm32")]
            ServerLink::Ws(link) => link.send(msg),
        }
    }

    /// The UDP socket of a `Udp` link, for the background reply thread; `None`
    /// for the embed link, whose replies are polled in the event loop instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn udp_socket(&self) -> Option<std::sync::Arc<std::net::UdpSocket>> {
        match self {
            ServerLink::Udp(leg) => Some(leg.socket()),
            #[cfg(feature = "standalone")]
            ServerLink::Embed(_) => None,
        }
    }

    /// The embedded server behind this link, if any (the front polls its replies).
    #[cfg(feature = "standalone")]
    pub fn embed(&self) -> Option<&embed::EmbedServer> {
        match self {
            ServerLink::Embed(srv) => Some(srv),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }
}

/// The host's outbound link to the audio server (the third topology leg): send
/// one OSC message. The native carriers are UDP ([`ServerLeg`]) and the embedded
/// ring ([`ServerLink`]); a browser WebSocket carrier plugs in behind this same
/// trait in the web milestones. The protocol logic ([`Host::forward`] and the
/// buffer/node-tree queries) sends through this seam, so it never names a
/// concrete transport.
pub trait Transport: Send {
    /// Sends one OSC message to the audio server.
    fn send(&self, msg: OscMessage) -> std::io::Result<()>;
}

// The native carriers are `Send`; the browser `Ws` link wraps a non-`Send`
// `web_sys::WebSocket`, but it never crosses a thread (the browser is
// single-threaded) and the host reaches it through the inherent
// [`ServerLink::send`], so the `Transport` seam is only needed/implemented
// natively.
#[cfg(not(target_arch = "wasm32"))]
impl Transport for ServerLink {
    fn send(&self, msg: OscMessage) -> std::io::Result<()> {
        ServerLink::send(self, msg)
    }
}

/// On-disk (or otherwise persisted) GuiDefs: named-GuiDef auto-save and the
/// `/gui_load` path. The native filesystem store ([`store::GuiStore`])
/// implements it; a browser has no filesystem, so a wasm host simply runs
/// without one. Behind the trait so the protocol dispatch saves and loads
/// without naming the filesystem store.
pub trait DefStore: Send {
    /// Persists GuiDef `id` (its verbatim tree JSON) under `name`.
    fn save(&self, name: &str, id: i32, tree_json: &[u8]) -> std::io::Result<()>;
    /// Loads the GuiDef saved under `name`: its id and tree JSON, ready to replay
    /// as a `/gui_def`.
    fn load(&self, name: &str) -> std::io::Result<(i32, Vec<u8>)>;
}

/// Resolves a waveform/plot widget's **local** bulk resource (its `path` or
/// prebuilt `cache`) to ready data, off the OSC path (the G7 bulk principle).
/// The native loader ([`bulk::MmapLoader`]) maps the file read-only; a browser
/// fetches the same reference over the network in a later milestone. The seam
/// returns platform-agnostic data ([`WaveformData`]/samples) so the GPU views
/// are built the same way on either platform.
///
/// [`WaveformData`]: crate::waveform::WaveformData
pub trait BulkLoader {
    /// Resolves a waveform's local resource: a prebuilt peak-pyramid `cache`
    /// (used directly, no raw samples), or a raw-`f32` `path` de-interleaved to
    /// channel 0 of `channels` whose pyramid is built at `base_bucket`. `None`
    /// on an unsupported platform or an I/O/format error (already logged).
    fn waveform(
        &self,
        cache: Option<&Path>,
        path: Option<&Path>,
        channels: usize,
        base_bucket: usize,
    ) -> Option<crate::waveform::WaveformData>;

    /// Resolves a plot's local `path` of raw `f32` to mono samples (channel 0 of
    /// `channels`). `None` on an unsupported platform or an I/O error.
    fn plot_samples(&self, path: &Path, channels: usize) -> Option<std::sync::Arc<[f32]>>;

    /// Reads a local `path` of raw little-endian `f32` into its de-interleaved
    /// channels (all of them) — the spectrogram's lane source; each channel is
    /// analyzed separately. `None` on an unsupported platform or an I/O error.
    fn raw_channels(&self, path: &Path, channels: usize) -> Option<Vec<Vec<f32>>>;

    /// The raw bytes of a local resource (a prebuilt STFT cache the
    /// spectrogram parses with `Stft::from_bytes`). `None` on an unsupported
    /// platform or an I/O error.
    fn file_bytes(&self, path: &Path) -> Option<Vec<u8>>;
}

/// A source of live control-bus values for the meter/scope views. Implemented by
/// the shared-memory segment ([`shm::SharedSegment`]) on Unix; the trait lets the
/// windowed front hold the source without platform `cfg`s and read a bus each
/// frame with no OSC traffic.
pub trait BusSource: Send + Sync {
    /// The current value of control bus `index` (`0.0` if out of range).
    fn control(&self, index: usize) -> f32;

    /// Fills `out` with the newest raw samples of audio tap `tap` (newest
    /// last), returning `false` when this source carries no tap data — the
    /// default. The shared-memory segment overrides it with the lock-free
    /// ring read; the browser reads its `/tap_data` store instead.
    fn read_tap(&self, _tap: i32, _out: &mut [f32]) -> bool {
        false
    }

    /// The server's sample rate when this source knows it (`0.0` otherwise);
    /// sizes the oscilloscope windows (`window_ms` → samples).
    fn sample_rate(&self) -> f64 {
        0.0
    }

    /// The engine's sample clock (samples processed since boot) when this
    /// source carries it (`0.0` otherwise). Drives the timeline playhead with
    /// zero messages natively; the browser polls `/clock` instead.
    fn sample_clock(&self) -> f64 {
        0.0
    }
}

// The `/gui_*` vocabulary (canonical tables in clients/gui/PLAN.md).
pub const GUI_DEF: &str = "/gui_def";
pub const GUI_SET: &str = "/gui_set";
pub const GUI_FREE: &str = "/gui_free";
pub const GUI_QUERY: &str = "/gui_query";
pub const GUI_BIND: &str = "/gui_bind";
pub const GUI_LOAD: &str = "/gui_load";
pub const GUI_INFO: &str = "/gui_info";
pub const GUI_EVENT: &str = "/gui_event";
pub const GUI_CLOSED: &str = "/gui_closed";

/// What handling a packet asks the host's *front* to do, beyond mutating the
/// host's own state. The protocol logic stays transport- and GPU-agnostic and
/// *returns* these, so the caller decides how to act: the windowed front opens
/// and closes OS windows and sends replies; the headless front sends replies and
/// logs the window effects (no display). That keeps the logic unit-testable
/// without a socket or a GPU.
#[derive(Debug)]
pub enum HostEffect {
    /// Send this message back to the requesting client.
    Reply(OscMessage),
    /// Open (or rebuild) the window for the GuiDef rooted at this id.
    OpenWindow(i32),
    /// Close the window for the GuiDef rooted at this id, if any.
    CloseWindow(i32),
    /// A live `/gui_set` changed a widget in the window rooted at this id; the
    /// front should repaint it (the typed tree is already updated in place).
    Redraw(i32),
}

/// The widget-protocol interpreter (transport- and GPU-agnostic). See
/// [`handle_packet`](Self::handle_packet) and [`HostEffect`].
pub struct Host {
    registry: Registry,
    /// Typed widget trees for window-rooted defs, by def id — the renderable
    /// documents the windowed front builds windows from. Non-window roots live
    /// only in the generic registry.
    window_defs: HashMap<i32, Widget>,
    /// The audio-server client leg (the third topology leg). Present when the
    /// host was started with a `--server` target or, in standalone mode, an
    /// embedded server; [`forward`](Self::forward) sends bound-widget values
    /// through it.
    server: Option<ServerLink>,
    /// Widget id -> the audio-server destination its value forwards to
    /// (`/gui_bind`). A bound widget bypasses the script: its value goes
    /// straight to the audio server instead of emitting a `/gui_event`.
    bindings: HashMap<i32, Binding>,
    /// The verbatim `/gui_def` JSON per def id — the source of truth for
    /// persistence (a GuiDef with a `name` is saved as-is) and for replaying a
    /// `/gui_load`.
    def_json: HashMap<i32, Vec<u8>>,
    /// The GuiDef store, when persistence is configured (the native filesystem
    /// store). Enables auto-persist of named GuiDefs and `/gui_load`. Held behind
    /// [`DefStore`] so the dispatch never names the filesystem store.
    store: Option<Box<dyn DefStore>>,
    /// The shared timeline navigation groups of the linked editor views: one
    /// horizontal view + selection + playhead per group, referenced by member
    /// widgets (see [`timeline`]).
    timelines: timeline::TimelineGroups,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn new() -> Self {
        Self {
            registry: Registry::new(),
            window_defs: HashMap::new(),
            server: None,
            bindings: HashMap::new(),
            def_json: HashMap::new(),
            store: None,
            timelines: timeline::TimelineGroups::default(),
        }
    }

    /// Attaches the audio-server client leg (host -> audio server) over UDP.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_server(mut self, server: ServerLeg) -> Self {
        self.server = Some(ServerLink::Udp(server));
        self
    }

    /// Attaches an arbitrary server link (UDP or, for standalone, an embedded
    /// server).
    pub fn with_server_link(mut self, link: ServerLink) -> Self {
        self.server = Some(link);
        self
    }

    /// Attaches (or replaces) the server link in place — for a front that learns
    /// its audio server after construction (the browser connecting a WebSocket
    /// leg on demand).
    pub fn set_server_link(&mut self, link: ServerLink) {
        self.server = Some(link);
    }

    /// Attaches the GuiDef store (named GuiDefs auto-persist; `/gui_load` reads
    /// from it).
    pub fn with_store<S: DefStore + 'static>(mut self, store: S) -> Self {
        self.store = Some(Box::new(store));
        self
    }

    /// The GuiDef store, if persistence was configured.
    pub fn store(&self) -> Option<&dyn DefStore> {
        self.store.as_deref()
    }

    /// The ids of the currently-defined window GuiDefs (for the standalone front
    /// to open a pre-loaded def on resume).
    pub fn window_def_ids(&self) -> Vec<i32> {
        self.window_defs.keys().copied().collect()
    }

    /// The audio-server client link, if one was attached (`--server` or the
    /// standalone embed). The windowed front uses it to query and fetch buffers,
    /// and to forward bound-widget values.
    pub fn server(&self) -> Option<&ServerLink> {
        self.server.as_ref()
    }

    /// Read access to the widget tree (for tests and introspection).
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The typed window document for def `id`, if it is a window-rooted def the
    /// front should render. The single source of truth: the windowed front
    /// renders and hit-tests from it, and live `/gui_set`s mutate it in place
    /// (see [`window_def_mut`](Self::window_def_mut)).
    pub fn window_def(&self, id: i32) -> Option<&Widget> {
        self.window_defs.get(&id)
    }

    /// Mutable access to a window document, for the front to write back a value
    /// a user interaction produced (a turned knob, a moved slider).
    pub fn window_def_mut(&mut self, id: i32) -> Option<&mut Widget> {
        self.window_defs.get_mut(&id)
    }

    /// Handles one decoded packet from `from`, returning the effects its front
    /// should carry out (replies plus window open/close). A bundle is unwrapped
    /// and its messages run in order (the timetag is treated as immediate at this
    /// milestone — no scheduling yet).
    pub fn handle_packet(&mut self, packet: OscPacket, from: ClientId) -> Vec<HostEffect> {
        let mut effects = Vec::new();
        self.dispatch_packet(packet, from, &mut effects);
        effects
    }

    fn dispatch_packet(
        &mut self,
        packet: OscPacket,
        from: ClientId,
        effects: &mut Vec<HostEffect>,
    ) {
        match packet {
            OscPacket::Message(msg) => self.dispatch(msg, from, effects),
            OscPacket::Bundle(bundle) => {
                for inner in bundle.content {
                    self.dispatch_packet(inner, from, effects);
                }
            }
        }
    }

    fn dispatch(&mut self, msg: OscMessage, from: ClientId, effects: &mut Vec<HostEffect>) {
        match msg.addr.as_str() {
            GUI_DEF => self.on_def(&msg.args, from, effects),
            GUI_SET => self.on_set(&msg.args, from, effects),
            GUI_FREE => self.on_free(&msg.args, from, effects),
            GUI_QUERY => self.on_query(&msg.args, from, effects),
            GUI_BIND => self.on_bind(&msg.args, from),
            GUI_LOAD => self.on_load(&msg.args, from, effects),
            other => debug!("{from}: ignoring unhandled address {other}"),
        }
    }

    /// `/gui_def <id> <json> [blob…]` — build a whole widget tree from one JSON
    /// GuiDef (with any bulk data, e.g. waveform samples, as trailing blobs). A
    /// `window` root also opens (or rebuilds) a window.
    fn on_def(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_DEF} needs an integer id");
        };
        let Some(bytes) = json_arg(args, 1) else {
            return warn!("{from}: {GUI_DEF} needs a JSON string or blob argument");
        };
        let node = match GuiNode::parse(bytes) {
            Ok(node) => node,
            Err(e) => return warn!("{from}: {GUI_DEF} {id}: invalid GuiDef JSON: {e}"),
        };
        // Keep the verbatim JSON: the source of truth for persistence and reload.
        self.def_json.insert(id, bytes.to_vec());
        let outcome = self.registry.define(id, &node);
        // The acceptance criterion: log the parsed tree.
        info!(
            "{from}: {GUI_DEF} {id}: {} widget(s){}{}\n{}",
            outcome.inserted,
            if outcome.replaced { " (replaced)" } else { "" },
            if outcome.skipped > 0 {
                format!(", {} skipped", outcome.skipped)
            } else {
                String::new()
            },
            node.dump(id).trim_end(),
        );
        // A window root becomes a renderable typed document; the front opens it.
        if node.kind == "window" {
            let blobs = blob_args(&args[2.min(args.len())..]);
            match Widget::from_node(id, &node, &blobs) {
                Ok(tree) => {
                    self.window_defs.insert(id, tree);
                    // The def's timeline views (re)join their navigation
                    // groups; rebuild semantics for state confined to this def.
                    self.sync_timeline_groups(Some(id));
                    effects.push(HostEffect::OpenWindow(id));
                }
                Err(e) => warn!("{from}: {GUI_DEF} {id}: cannot build window: {e}"),
            }
        }
        // A redefine frees the old subtree first; drop any binding whose widget
        // did not survive into the new tree.
        if outcome.replaced {
            self.prune_bindings();
        }
        // Inline `bind` props register a binding declaratively, so a saved GuiDef
        // carries its own bindings (the standalone path) and a live script may
        // bind without a separate `/gui_bind`.
        self.register_inline_bindings(&node);
        // A GuiDef with a `name` persists to the store the way a named SynthDef
        // does on `/d_recv` — no separate save command.
        if let Some(name) = node.props.get("name").and_then(Value::as_str)
            && let Some(store) = self.store.as_ref()
        {
            match store.save(name, id, bytes) {
                Ok(()) => info!("{from}: {GUI_DEF} {id}: saved as \"{name}\""),
                Err(e) => warn!("{from}: {GUI_DEF} {id}: cannot save \"{name}\": {e}"),
            }
        }
    }

    /// `/gui_load <name>` — load a persisted GuiDef and instantiate it (build its
    /// tree and open its window), replaying it as a `/gui_def` under the id it was
    /// saved with.
    fn on_load(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(name) = string_arg(args, 0) else {
            return warn!("{from}: {GUI_LOAD} needs a name argument");
        };
        let Some(store) = self.store.as_ref() else {
            return warn!("{from}: {GUI_LOAD} {name}: no data directory configured");
        };
        let (id, json) = match store.load(name) {
            Ok(loaded) => loaded,
            Err(e) => return warn!("{from}: {GUI_LOAD} {name}: {e}"),
        };
        info!("{from}: {GUI_LOAD} {name}: instantiating GuiDef {id}");
        self.on_def(
            &[
                OscType::Int(id),
                OscType::String(String::from_utf8_lossy(&json).into_owned()),
            ],
            from,
            effects,
        );
    }

    /// `/gui_set <id> <k> <v> ...` — update one live widget's properties, in the
    /// generic registry (for `/gui_query`) and, if it is inside an open window,
    /// in the typed render tree (so the change shows live).
    fn on_set(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_SET} needs an integer id");
        };
        let props = key_value_pairs(&args[1..]);
        if props.is_empty() {
            return warn!("{from}: {GUI_SET} {id}: no key/value pairs");
        }
        let keys: Vec<&String> = props.iter().map(|(k, _)| k).collect();
        if !self.registry.set(id, props.clone()) {
            return warn!("{from}: {GUI_SET} {id}: no such widget");
        }
        info!("{from}: {GUI_SET} {id}: updated {keys:?}");
        // Mirror the change into the typed window tree the front renders. A
        // timeline view's shared keys (`view_*`, `sel_*`, `playhead_at`,
        // `link`) route through its navigation group instead, so a set on any
        // member applies group-wide (linked views).
        let mut is_timeline = false;
        let mut is_clip = false;
        if let Some(root) = self.registry.root_of(id)
            && let Some(tree) = self.window_defs.get_mut(&root)
            && let Some(widget) = tree.find_mut(id)
        {
            is_timeline = widget.is_timeline();
            is_clip = matches!(widget.kind, widget::WidgetKind::Clip { .. });
            let mut changed = false;
            for (k, v) in &props {
                if !(is_timeline && timeline::is_timeline_key(k)) {
                    changed |= widget.kind.apply(k, v);
                }
            }
            if changed {
                effects.push(HostEffect::Redraw(root));
            }
        }
        if is_timeline {
            self.set_timeline_props(id, &props, effects);
        }
        if is_clip {
            // A moved or resized clip changes its lane's extent, and so the
            // shared axis (a clip pushed past the end lengthens the timeline).
            self.sync_track_totals();
        }
    }

    /// `/gui_free <id>` — destroy a widget and its subtree (and its window, if
    /// `id` is a window-rooted def).
    fn on_free(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_FREE} needs an integer id");
        };
        let removed = self.registry.free(id);
        self.def_json.remove(&id);
        if self.window_defs.remove(&id).is_some() {
            effects.push(HostEffect::CloseWindow(id));
        }
        // A freed widget can no longer forward (its subtree is gone), and its
        // timeline group state goes with it.
        self.prune_bindings();
        self.prune_timeline_groups();
        if removed > 0 {
            info!("{from}: {GUI_FREE} {id}: freed {removed} widget(s)");
        } else {
            warn!("{from}: {GUI_FREE} {id}: no such widget");
        }
    }

    /// `/gui_query <id>` — reply `/gui_info <id> <type> <k> <v> ...`.
    fn on_query(&mut self, args: &[OscType], from: ClientId, effects: &mut Vec<HostEffect>) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_QUERY} needs an integer id");
        };
        let mut out = vec![OscType::Int(id)];
        match self.registry.get(id) {
            Some(widget) => {
                out.push(OscType::String(widget.kind.clone()));
                for (k, v) in &widget.props {
                    if let Some(arg) = scalar_arg(v) {
                        out.push(OscType::String(k.clone()));
                        out.push(arg);
                    }
                }
                info!("{from}: {GUI_QUERY} {id} -> {GUI_INFO} ({})", widget.kind);
            }
            None => {
                // An empty type string means "no such widget" — the query still
                // gets an answer, the way the server replies even on a miss. A
                // miss is *not* a warning: it is how a client pings a host that is
                // still empty (the launcher's readiness check does exactly that).
                out.push(OscType::String(String::new()));
                debug!("{from}: {GUI_QUERY} {id}: no such widget");
            }
        }
        effects.push(HostEffect::Reply(OscMessage {
            addr: GUI_INFO.into(),
            args: out,
        }));
    }

    /// `/gui_bind <id> "server" <addr> <prefix…>` — forward this widget's value
    /// straight to the audio server on every change, bypassing the script (the
    /// low-latency interactive path). With no target (`/gui_bind <id>`) the
    /// binding is removed and the `/gui_event` path restored.
    fn on_bind(&mut self, args: &[OscType], from: ClientId) {
        let Some(id) = int_arg(args, 0) else {
            return warn!("{from}: {GUI_BIND} needs an integer id");
        };
        if args.len() <= 1 {
            if self.bindings.remove(&id).is_some() {
                info!("{from}: {GUI_BIND} {id}: unbound (events restored)");
            } else {
                warn!("{from}: {GUI_BIND} {id}: no binding to remove");
            }
            return;
        }
        let binding = match Binding::parse(&args[1..]) {
            Ok(b) => b,
            Err(e) => return warn!("{from}: {GUI_BIND} {id}: {e}"),
        };
        if self.server.is_none() {
            warn!(
                "{from}: {GUI_BIND} {id}: no audio server attached (--server); the binding \
                 will swallow the value but cannot forward it"
            );
        }
        info!(
            "{from}: {GUI_BIND} {id} -> audio server {} {:?}",
            binding.addr, binding.prefix
        );
        self.bindings.insert(id, binding);
    }

    /// Forwards `widget_id`'s `value` to the audio server when it is bound,
    /// returning whether the binding handled it. When it returns `true` the
    /// caller must **not** also emit a `/gui_event` — bypassing the script is
    /// the whole point. A bound widget with no audio server attached still
    /// returns `true` (the value is swallowed, not sent to the script); the
    /// missing `--server` was already warned about at bind time.
    pub fn forward(&self, widget_id: i32, value: OscType) -> bool {
        self.forward_args(widget_id, vec![value])
    }

    /// [`forward`](Self::forward) for a **flat list** of values — the edit-back
    /// payload of an editor widget (a `bpf`'s breakpoint list today, a drawn
    /// buffer region later): a bound editor sends `addr prefix… values…` to
    /// the audio server, bypassing the script exactly as a bound knob does.
    pub fn forward_args(&self, widget_id: i32, values: Vec<OscType>) -> bool {
        let Some(binding) = self.bindings.get(&widget_id) else {
            return false;
        };
        if let Some(server) = self.server.as_ref()
            && let Err(e) = server.send(binding.message_args(values))
        {
            warn!("{GUI_BIND} {widget_id}: failed to forward to the audio server: {e}");
        }
        true
    }

    /// Whether widget `id` currently has a binding (its value goes to the audio
    /// server, not the script).
    pub fn is_bound(&self, id: i32) -> bool {
        self.bindings.contains_key(&id)
    }

    /// Drops bindings whose widget no longer exists (after a `/gui_free` or a
    /// redefining `/gui_def`), so a freed id cannot keep forwarding.
    fn prune_bindings(&mut self) {
        self.bindings.retain(|id, _| self.registry.contains(*id));
    }

    /// Registers a [`Binding`] for every widget that declares an inline `bind`
    /// array in the GuiDef (`{"id":…,"type":…,"bind":["/n_set",node,"freq"]}`).
    fn register_inline_bindings(&mut self, node: &GuiNode) {
        if let Some(id) = node.id
            && let Some(Value::Array(items)) = node.props.get("bind")
        {
            match Binding::from_json(items) {
                Ok(binding) => {
                    self.bindings.insert(id, binding);
                }
                Err(e) => warn!("widget {id}: invalid inline `bind`: {e}"),
            }
        }
        for child in &node.children {
            self.register_inline_bindings(child);
        }
    }
}

/// Collects the trailing OSC blob arguments of a `/gui_def` (the bulk data, e.g.
/// waveform samples) into a list a `Widget` can index by `"blob"`.
fn blob_args(args: &[OscType]) -> Vec<Vec<u8>> {
    args.iter()
        .filter_map(|a| match a {
            OscType::Blob(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

/// The i-th argument as an `i32`, if present and integer-typed.
fn int_arg(args: &[OscType], i: usize) -> Option<i32> {
    match args.get(i) {
        Some(OscType::Int(n)) => Some(*n),
        Some(OscType::Long(n)) => Some(*n as i32),
        _ => None,
    }
}

/// The i-th argument as a string slice, if present and string-typed.
fn string_arg(args: &[OscType], i: usize) -> Option<&str> {
    match args.get(i) {
        Some(OscType::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// The i-th argument as JSON bytes: a string or a blob (both accepted, as
/// `/d_recv` accepts a SynthDef either way).
fn json_arg(args: &[OscType], i: usize) -> Option<&[u8]> {
    match args.get(i) {
        Some(OscType::String(s)) => Some(s.as_bytes()),
        Some(OscType::Blob(b)) => Some(b.as_slice()),
        _ => None,
    }
}

/// Turns a flat `k, v, k, v, ...` OSC tail into `(String, Value)` pairs,
/// preserving the int/float distinction (an OSC `Int` stays an integer JSON
/// number, a `Float` a floating one). A trailing unpaired key is ignored.
fn key_value_pairs(tail: &[OscType]) -> Vec<(String, Value)> {
    let mut pairs = Vec::new();
    let mut it = tail.iter();
    while let (Some(k), Some(v)) = (it.next(), it.next()) {
        if let OscType::String(key) = k
            && let Some(value) = osc_to_value(v)
        {
            pairs.push((key.clone(), value));
        }
    }
    pairs
}

/// One OSC primitive as a JSON value, keeping integers and floats apart.
fn osc_to_value(arg: &OscType) -> Option<Value> {
    match arg {
        OscType::Int(n) => Some(Value::from(*n)),
        OscType::Long(n) => Some(Value::from(*n)),
        OscType::Float(x) => Some(Value::from(*x)),
        OscType::Double(x) => Some(Value::from(*x)),
        OscType::String(s) => Some(Value::from(s.clone())),
        _ => None,
    }
}

/// One scalar JSON value as an OSC primitive for a `/gui_info` reply, keeping
/// integers (`Int`) and floats (`Float`) apart; `None` for structural values.
fn scalar_arg(v: &Value) -> Option<OscType> {
    match v {
        Value::Bool(b) => Some(OscType::Int(*b as i32)),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(OscType::Int(n.as_i64()? as i32)),
        Value::Number(n) => Some(OscType::Float(n.as_f64()? as f32)),
        Value::String(s) => Some(OscType::String(s.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    fn from() -> ClientId {
        ClientId::Udp(SocketAddr::from((Ipv4Addr::LOCALHOST, 9000)))
    }

    fn def_msg(id: i32, json: &str) -> OscPacket {
        OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![OscType::Int(id), OscType::String(json.into())],
        })
    }

    /// The reply messages among a batch of effects.
    fn replies(effects: Vec<HostEffect>) -> Vec<OscMessage> {
        effects
            .into_iter()
            .filter_map(|e| match e {
                HostEffect::Reply(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    /// The def ids of any OpenWindow effects.
    fn opened(effects: &[HostEffect]) -> Vec<i32> {
        effects
            .iter()
            .filter_map(|e| match e {
                HostEffect::OpenWindow(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    const TREE: &str = r#"{"type":"window","title":"Filter","children":[
        {"id":10,"type":"knob","label":"cutoff","min":20.0,"max":20000.0,"value":800.0}
    ]}"#;

    #[test]
    fn window_def_opens_a_window_and_stores_the_typed_def() {
        let mut host = Host::new();
        let effects = host.handle_packet(def_msg(1, TREE), from());
        assert_eq!(opened(&effects), vec![1], "a window root opens a window");
        assert_eq!(host.registry().len(), 2, "window + knob in the registry");
        assert!(
            host.window_def(1).is_some(),
            "the typed window def is stored"
        );
    }

    #[test]
    fn def_then_query_replies_with_gui_info() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());

        let query = OscPacket::Message(OscMessage {
            addr: GUI_QUERY.into(),
            args: vec![OscType::Int(10)],
        });
        let out = replies(host.handle_packet(query, from()));
        assert_eq!(out.len(), 1);
        let info = &out[0];
        assert_eq!(info.addr, GUI_INFO);
        assert_eq!(info.args[0], OscType::Int(10));
        assert_eq!(info.args[1], OscType::String("knob".into()));
        // The reply carries the knob's props as k/v pairs, ints and floats kept
        // apart; `value` is a float.
        let pos = info
            .args
            .iter()
            .position(|a| *a == OscType::String("value".into()))
            .expect("value key present");
        assert_eq!(info.args[pos + 1], OscType::Float(800.0));
    }

    #[test]
    fn query_for_unknown_id_still_answers() {
        let mut host = Host::new();
        let query = OscPacket::Message(OscMessage {
            addr: GUI_QUERY.into(),
            args: vec![OscType::Int(42)],
        });
        let out = replies(host.handle_packet(query, from()));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].args[0], OscType::Int(42));
        assert_eq!(out[0].args[1], OscType::String(String::new()));
    }

    #[test]
    fn set_updates_a_live_widget() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        let set = OscPacket::Message(OscMessage {
            addr: GUI_SET.into(),
            args: vec![
                OscType::Int(10),
                OscType::String("value".into()),
                OscType::Float(440.0),
            ],
        });
        host.handle_packet(set, from());
        assert_eq!(
            host.registry().get(10).unwrap().props["value"],
            Value::from(440.0)
        );
    }

    #[test]
    fn free_drops_the_subtree_and_closes_the_window() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        let free = OscPacket::Message(OscMessage {
            addr: GUI_FREE.into(),
            args: vec![OscType::Int(1)],
        });
        let effects = host.handle_packet(free, from());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, HostEffect::CloseWindow(1))),
            "freeing a window def closes its window"
        );
        assert!(host.registry().is_empty());
        assert!(host.window_def(1).is_none());
    }

    #[test]
    fn waveform_blob_rides_the_def_message() {
        let mut host = Host::new();
        let blob: Vec<u8> = [0.5f32, -0.5]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let json = r#"{"type":"window","children":[{"id":9,"type":"waveform","blob":0}]}"#;
        let msg = OscPacket::Message(OscMessage {
            addr: GUI_DEF.into(),
            args: vec![
                OscType::Int(2),
                OscType::String(json.into()),
                OscType::Blob(blob),
            ],
        });
        let effects = host.handle_packet(msg, from());
        assert_eq!(opened(&effects), vec![2]);
        let tree = host.window_def(2).unwrap();
        match &tree.children[0].kind {
            widget::WidgetKind::Waveform { samples, .. } => assert_eq!(&samples[..], &[0.5, -0.5]),
            other => panic!("expected a waveform, got {other:?}"),
        }
    }

    #[test]
    fn named_def_persists_and_gui_load_reinstantiates_it() {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "clausters_gui_host_store_{:?}",
            std::time::Instant::now()
        ));
        let store = store::GuiStore::open(&dir).unwrap();
        let mut host = Host::new().with_store(store);

        // A named GuiDef auto-persists on /gui_def.
        let tree = r#"{"type":"window","name":"inst","title":"I","children":[
            {"id":10,"type":"knob","value":0.5}
        ]}"#;
        host.handle_packet(def_msg(3, tree), from());
        assert!(host.window_def(3).is_some());

        // Free it: the live def is gone, but the persisted copy remains.
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_FREE.into(),
                args: vec![OscType::Int(3)],
            }),
            from(),
        );
        assert!(host.window_def(3).is_none());

        // /gui_load rebuilds it under its saved id and reopens the window.
        let effects = host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_LOAD.into(),
                args: vec![OscType::String("inst".into())],
            }),
            from(),
        );
        assert_eq!(opened(&effects), vec![3], "loading reopens the window");
        assert!(host.window_def(3).is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundle_is_unwrapped_in_order() {
        use clausters_core::osc::{IMMEDIATE, OscBundle};
        let mut host = Host::new();
        let bundle = OscPacket::Bundle(OscBundle {
            timetag: IMMEDIATE,
            content: vec![
                def_msg(1, TREE),
                OscPacket::Message(OscMessage {
                    addr: GUI_QUERY.into(),
                    args: vec![OscType::Int(1)],
                }),
            ],
        });
        let out = replies(host.handle_packet(bundle, from()));
        assert_eq!(out.len(), 1, "the query inside the bundle is answered");
        assert_eq!(out[0].args[1], OscType::String("window".into()));
    }

    fn bind_msg(id: i32, target: Vec<OscType>) -> OscPacket {
        let mut args = vec![OscType::Int(id)];
        args.extend(target);
        OscPacket::Message(OscMessage {
            addr: GUI_BIND.into(),
            args,
        })
    }

    #[test]
    fn bound_widget_forwards_to_the_audio_server_and_unbinds() {
        use clausters_core::osc::decode_packet;
        use std::net::UdpSocket;
        use std::time::Duration;

        // A throwaway socket standing in for the audio server, to capture the
        // message a bound widget forwards (one process, so loopback delivers).
        let fake_server = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        fake_server
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let leg = ServerLeg::connect(fake_server.local_addr().unwrap()).unwrap();

        let mut host = Host::new().with_server(leg);
        host.handle_packet(def_msg(1, TREE), from()); // a window with knob id 10

        host.handle_packet(
            bind_msg(
                10,
                vec![
                    OscType::String("server".into()),
                    OscType::String("/n_set".into()),
                    OscType::Int(1000),
                    OscType::String("cutoff".into()),
                ],
            ),
            from(),
        );
        assert!(host.is_bound(10));

        // A value change goes straight to the server (bypassing the script).
        assert!(host.forward(10, OscType::Float(440.0)));
        let mut buf = [0u8; 1024];
        let (len, _) = fake_server.recv_from(&mut buf).expect("forwarded datagram");
        let msg = match decode_packet(&buf[..len]).unwrap() {
            OscPacket::Message(m) => m,
            other => panic!("expected a message, got {other:?}"),
        };
        assert_eq!(msg.addr, "/n_set");
        assert_eq!(
            msg.args,
            vec![
                OscType::Int(1000),
                OscType::String("cutoff".into()),
                OscType::Float(440.0)
            ]
        );

        // Unbinding (no target) restores the event path: forward stops handling.
        host.handle_packet(bind_msg(10, vec![]), from());
        assert!(!host.is_bound(10));
        assert!(!host.forward(10, OscType::Float(1.0)));
    }

    #[test]
    fn freeing_a_bound_widget_drops_its_binding() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        host.handle_packet(
            bind_msg(
                10,
                vec![
                    OscType::String("server".into()),
                    OscType::String("/c_set".into()),
                    OscType::Int(7),
                ],
            ),
            from(),
        );
        assert!(host.is_bound(10));
        // Freeing the window (root 1) takes knob 10 — and its binding — with it.
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_FREE.into(),
                args: vec![OscType::Int(1)],
            }),
            from(),
        );
        assert!(!host.is_bound(10));
    }

    #[test]
    fn binding_without_a_server_is_registered_but_swallows() {
        // No --server: the bind is accepted (and warned), and forward still
        // reports it handled the value so it does not leak to the script.
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        host.handle_packet(
            bind_msg(
                10,
                vec![
                    OscType::String("server".into()),
                    OscType::String("/n_set".into()),
                    OscType::Int(1000),
                    OscType::String("cutoff".into()),
                ],
            ),
            from(),
        );
        assert!(host.is_bound(10));
        assert!(
            host.forward(10, OscType::Float(1.0)),
            "swallowed, not emitted"
        );
    }
}
