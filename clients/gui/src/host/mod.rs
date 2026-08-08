//! The GUI host: an OSC front with a widget command interpreter.
//!
//! `clausters-gui` is **two roles in one process**: a *GUI host* for the
//! language clients — it owns the windows, the widgets and the GPU, and speaks
//! the `/gui_*` widget protocol — and a *client of the audio server* — it reads
//! buffers/buses/the node tree and sends control, exactly as the Python client
//! does. This module is the host proper: the widget [`Registry`], the typed tree
//! it holds, and the transport-agnostic command loop that interprets
//! `/gui_def`/`/gui_set`/`/gui_free`/`/gui_query`/`/gui_bind`/`/gui_load` and
//! answers with `/gui_info`/`/gui_event`/`/gui_closed`. Everything under it is
//! split along the platform seam (see the module groups below): a web-portable
//! core, and an I/O shell reached only through small traits.
//!
//! ## Why the host owns its transport
//!
//! The host does **not** extract or link the audio server's transport layer
//! (`src/osc/{server,tcp,ws}.rs`): that code is tangled with the audio
//! `ServerState`, the engine wake and the IPC ring, so lifting it would drag
//! server concerns into this crate for no gain. Instead the host **links
//! `clausters-core`** — a path dependency that pulls only `rosc` — for the shared
//! OSC seam (the single [`clausters_core::osc::decode_packet`] door, plus
//! encode/bundle/message), and owns a **thin transport front** of its own
//! ([`transport`]). The default build links no server code; only the optional
//! `standalone` feature pulls the full `clausters` crate, for the in-process
//! embedded server (`embed`).
//!
//! That front now carries UDP and TCP ([`tcp`]) together on one port, plus an
//! opt-in WebSocket leg ([`ws`]) — all behind one [`ClientId`] and
//! reply seam, which is the seam's whole point: each carrier was added without
//! touching the protocol or this command loop, and the next one should be too.
//! The client leg ([`client::ServerLeg`]) reuses that same encode door, so the
//! gui talks to the audio server with one encoder, not a parallel one.

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
pub mod guidef;
pub mod interact;
pub mod layout;
pub mod live;
pub mod meters;
pub mod metrics;
pub mod nodetree;
pub mod paint;
pub mod patch;
pub mod phasescope;
pub mod piano;
pub mod pianoroll;
pub mod plot;
pub mod registry;
pub mod ruler;
pub mod score;
pub mod scroll;
pub mod signal;
pub mod spectrum;
pub mod textedit;
pub mod theme;
pub mod timeline;
pub mod track;
pub mod widget;

// Booting a persisted bundle over the wire — the ordering/encoding half of the
// browser standalone path, platform-agnostic and natively unit-tested (the
// fetching half is page JS).
pub mod bundle;

// The native I/O shell, excluded from `wasm32`: the client leg ([`Transport`]),
// on-disk GuiDef persistence ([`DefStore`]) and the UDP/TCP/WebSocket server
// fronts. The browser fills the same seams over WebSocket and fetch. The
// winit/wgpu driver ([`gui`]) and the mmap bulk loader ([`bulk`]) are gated
// below.
#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
#[cfg(not(target_arch = "wasm32"))]
pub mod tcp;
#[cfg(not(target_arch = "wasm32"))]
pub mod transport;
#[cfg(not(target_arch = "wasm32"))]
pub mod ws;

// Reading the audio server's shared-memory segment for zero-message meters and
// scopes: the native [`BusSource`]. Unix-only, as the server's segment is.
#[cfg(unix)]
pub mod shm;

// The in-process embedded server for the standalone mode, a direct dependency on
// the `clausters` crate behind the optional `standalone` feature (off by default,
// since it pulls the engine + audio backend). Native-only.
#[cfg(feature = "standalone")]
pub mod embed;

// Mapping a local file (raw samples or a prebuilt peak cache) for the bulk-data
// path: a multi-megabyte buffer read from a shared resource, not over OSC.
// Unix-only, like `shm`.
#[cfg(unix)]
pub mod mapfile;

// The native [`BulkLoader`]: resolves a waveform/plot's local `path`/`cache` to
// samples or a peak pyramid through the mmap path above. The browser resolves
// the same references as URLs through `fetch`.
#[cfg(not(target_arch = "wasm32"))]
pub mod bulk;

// The shared pointer-gesture state machine (no winit, no web-sys): both the
// native windowed front and the browser front drive it, so every editing
// gesture behaves identically on either platform by construction.
pub mod gestures;

// The windowed host (winit + wgpu) is native-only; the wasm build swaps it for
// the `<canvas>` surface in [`web`]. Both drive the shared [`frame`] render.
#[cfg(not(target_arch = "wasm32"))]
pub mod gui;

// The browser entry point: a `<canvas>` WebGPU surface with async GPU bring-up,
// rendering through the same `frame` path the native front uses, driven over a
// WebSocket or the in-page binding surface. wasm-only.
#[cfg(target_arch = "wasm32")]
pub mod web;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

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
/// *encoding* is transport-independent, so client identity is too: every
/// carrier is a variant here, and a new one is added without the protocol
/// dispatch changing. It lives in the agnostic core, not in any one front, so
/// that dispatch names it on every platform.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClientId {
    /// A UDP datagram source (the native server front).
    Udp(SocketAddr),
    /// A TCP connection on the native server front, by connection id —
    /// length-prefixed frames, replies routed back on the same connection.
    Tcp(u64),
    /// A WebSocket connection on the native server front (`--ws`), by
    /// connection id — one OSC packet per binary message, replies routed back
    /// on the same connection. The browser's carrier into a native host.
    Ws(u64),
    /// The browser's in-page binding surface (the wasm front feeds OSC packets
    /// in and drains events out through it; there is no socket address).
    Web,
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientId::Udp(addr) => write!(f, "{addr}"),
            ClientId::Tcp(id) => write!(f, "tcp client {id}"),
            ClientId::Ws(id) => write!(f, "ws client {id}"),
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
    /// The in-page engine (the AudioWorklet backend): outbound OSC handed to a
    /// page-registered callback, which forwards it to the worklet; replies come
    /// back through `GuiBridge::server_reply`. No process, no socket.
    #[cfg(target_arch = "wasm32")]
    Page(web::PageServerLink),
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
            #[cfg(target_arch = "wasm32")]
            ServerLink::Page(link) => link.send(msg),
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
/// prebuilt `cache`) to ready data, off the OSC path (the bulk-data rule).
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

    /// Resolves a plot's local `path` of raw `f32` to its samples, kept
    /// interleaved (`channels` only trims a trailing partial frame — the plot
    /// draws every channel). `None` on an unsupported platform or an I/O error.
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

    /// Fills `out` with the newest raw samples of **audio bus** `bus` (newest
    /// last), returning `false` when this source has none for it — the
    /// default. Where those samples physically live is this source's business:
    /// the shared-memory segment looks the bus up in the server's directory
    /// and reads that ring lock-free, the browser reads its `/bus_tapStream.reply` store.
    /// Above here, a bus is the only thing anyone names.
    fn read_bus(&self, _bus: i32, _out: &mut [f32]) -> bool {
        false
    }

    /// Audio bus `bus`'s published level — the peak of the engine's last
    /// block, held with a decay — or `0.0` where this source has none. What a
    /// meter draws; it needs no recording, so it costs no tap.
    fn level(&self, _bus: i32) -> f32 {
        0.0
    }

    /// The server's sample rate when this source knows it (`0.0` otherwise);
    /// sizes the oscilloscope windows (`window_ms` → samples).
    fn sample_rate(&self) -> f64 {
        0.0
    }

    /// The engine's sample clock (samples processed since boot) when this
    /// source carries it (`0.0` otherwise). Drives the timeline playhead with
    /// zero messages natively; the browser polls `/clock_query` instead.
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
    /// The audio buses the host has asked the audio server to record, so the
    /// sample views can read them. Kept as a set and re-diffed whenever the
    /// documents change: the host is the one that turns "this scope watches
    /// bus 4" into the server's `/bus_tap`, which is why no client -- and no
    /// widget -- ever names a recording ring.
    watched_buses: Vec<i32>,
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
    /// The live host-managed piano voices, per widget id: one `(pitch, node)`
    /// entry per held key of a `piano` in voice mode. The press sends the
    /// `/synth_new`, the release the `gate 0`; the def frees the node itself, so
    /// no `/node_end` tracking is needed.
    voices: HashMap<i32, Vec<(i32, i32)>>,
    /// The next voice node-id offset over [`VOICE_ID_BASE`] (wrapping).
    voice_counter: i32,
    /// The host's color roles — one look per host, every paint site reads it
    /// (see [`theme`]).
    pub theme: theme::Theme,
    /// The host's size roles in **logical** pixels — one density per host, the
    /// table the config declares (see [`metrics`]). A window paints with its
    /// own resolution of it, from [`metrics_for`](Self::metrics_for), so
    /// changing this table once windows exist means calling
    /// [`refresh_metrics`](Self::refresh_metrics) after it.
    pub metrics: metrics::Metrics,
    /// The resolved (physical) metrics of each window, by def id — this table
    /// at that window's `ui_scale`. Written when a shell reports a scale
    /// ([`set_ui_scale`](Self::set_ui_scale)), which is the only side that may
    /// know one: the core never reads a platform API. Absent = scale 1.
    resolved_metrics: HashMap<i32, metrics::Metrics>,
    /// The `text` field currently receiving keystrokes, as `(def_id, widget_id)`.
    /// A press on a text field focuses it; a press elsewhere (or freeing the
    /// widget) clears it. While set, key input edits that field instead of
    /// running the global editor shortcuts (see [`textedit`]/[`gestures`]).
    focused_text: Option<(i32, i32)>,
}

/// The base of the node-id window the host's piano voices allocate from —
/// far above the Python client's ids (1000..) and the server's own auto
/// range, so an explicit voice id can never collide (see `docs/decisions.md`).
const VOICE_ID_BASE: i32 = 0x1000_0000;
/// The wrapping window of voice ids over the base.
const VOICE_ID_SPAN: i32 = 1 << 16;

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
            watched_buses: Vec::new(),
            server: None,
            bindings: HashMap::new(),
            def_json: HashMap::new(),
            store: None,
            timelines: timeline::TimelineGroups::default(),
            voices: HashMap::new(),
            voice_counter: 0,
            theme: theme::Theme::default(),
            metrics: metrics::Metrics::default(),
            resolved_metrics: HashMap::new(),
            focused_text: None,
        }
    }

    /// The text field currently focused, as `(def_id, widget_id)`.
    pub fn focused_text(&self) -> Option<(i32, i32)> {
        self.focused_text
    }

    /// Focuses a text field for keyboard input (a press on it), replacing any
    /// previous focus.
    pub fn focus_text(&mut self, def_id: i32, widget_id: i32) {
        self.focused_text = Some((def_id, widget_id));
    }

    /// Clears the text focus, returning the def id that held it (so the front
    /// can repaint it to drop the caret) when it changed.
    pub fn clear_text_focus(&mut self) -> Option<i32> {
        self.focused_text.take().map(|(def_id, _)| def_id)
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

    /// The size table window `def_id` lays out and paints with: this host's
    /// logical [`metrics`](Self::metrics) resolved to that window's physical
    /// pixels. Every layout, paint and hit-test site of a window reads *this*
    /// one, never the logical table — a document can sit on a HiDPI screen
    /// while another sits on an ordinary one.
    pub fn metrics_for(&self, def_id: i32) -> &metrics::Metrics {
        self.resolved_metrics.get(&def_id).unwrap_or(&self.metrics)
    }

    /// Records window `def_id`'s **UI scale** and resolves its size table once,
    /// returning whether anything changed (so a shell can relayout and repaint
    /// only when it did).
    ///
    /// The scale is the shell's to write and the core's to obey: natively it is
    /// winit's `scale_factor` (re-armed on `ScaleFactorChanged`), in the browser
    /// the page's `devicePixelRatio` — a platform reading this core may not
    /// make, which is exactly why it arrives through this door.
    pub fn set_ui_scale(&mut self, def_id: i32, ui_scale: f32) -> bool {
        let next = self.metrics.resolved(ui_scale);
        if self.metrics_for(def_id) == &next {
            return false;
        }
        self.resolved_metrics.insert(def_id, next);
        true
    }

    /// Window `def_id`'s UI scale (1.0 until a shell reports one).
    pub fn ui_scale(&self, def_id: i32) -> f32 {
        self.metrics_for(def_id).ui_scale
    }

    /// Re-resolves every window's size table after the logical one changed (a
    /// `[gui.metrics]` overlay, the browser's `metrics(json)`): each window
    /// keeps its own scale and gets the new roles.
    pub fn refresh_metrics(&mut self) {
        let scales: Vec<(i32, f32)> = self
            .resolved_metrics
            .iter()
            .map(|(id, m)| (*id, m.ui_scale))
            .collect();
        for (id, scale) in scales {
            self.resolved_metrics
                .insert(id, self.metrics.resolved(scale));
        }
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
                Ok(mut tree) => {
                    // Theme groups and per-widget accents resolve here — at
                    // the mutation point, never per frame.
                    widget::resolve_themes(&mut tree, &Arc::new(self.theme.clone()));
                    self.window_defs.insert(id, tree);
                    self.sync_bus_watches();
                    // The def's timeline views (re)join their navigation
                    // groups; rebuild semantics for state confined to this def.
                    self.sync_timeline_groups(Some(id));
                    effects.push(HostEffect::OpenWindow(id));
                }
                Err(e) => warn!("{from}: {GUI_DEF} {id}: cannot build window: {e}"),
            }
        }
        // A redefine frees the old subtree first; drop any binding whose widget
        // did not survive into the new tree, and release its live voices.
        if outcome.replaced {
            self.prune_bindings();
            self.prune_voices();
            self.prune_text_focus();
        }
        // Inline `bind` props register a binding declaratively, so a saved GuiDef
        // carries its own bindings (the standalone path) and a live script may
        // bind without a separate `/gui_bind`.
        self.register_inline_bindings(&node);
        // A GuiDef with a `name` persists to the store the way a named SynthDef
        // does on `/def_send synth` — no separate save command.
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
        let keys: Vec<String> = props.iter().map(|(k, _)| k.clone()).collect();
        if !self.set_props(id, props, effects) {
            return warn!("{from}: {GUI_SET} {id}: no such widget");
        }
        info!("{from}: {GUI_SET} {id}: updated {keys:?}");
    }

    /// The mutation a `/gui_set` performs, without its wire form: apply `props`
    /// to widget `id` in the generic registry and, when it is inside an open
    /// window, in the typed render tree. Returns whether the widget exists.
    ///
    /// It is a method of its own because a **widget binding** performs exactly
    /// this and nothing else — one apply, never another delivery — so the two
    /// paths cannot drift and a binding cannot cascade
    /// ([`bind`](super::bind)).
    pub fn set_props(
        &mut self,
        id: i32,
        props: Vec<(String, Value)>,
        effects: &mut Vec<HostEffect>,
    ) -> bool {
        let keys: Vec<&String> = props.iter().map(|(k, _)| k).collect();
        if !self.registry.set(id, props.clone()) {
            return false;
        }
        // A set can retarget a view's source or widen its channel run, which
        // changes what has to be recorded; the diff below is a no-op otherwise.
        let touches_source = keys
            .iter()
            .any(|k| matches!(k.as_str(), "bus" | "rate" | "channels"));
        // Mirror the change into the typed window tree the front renders. A
        // timeline view's shared keys (`view_*`, `sel_*`, `playhead_at`,
        // `link`) route through its navigation group instead, so a set on any
        // member applies group-wide (linked views).
        let mut is_timeline = false;
        let mut is_clip = false;
        let mut is_roll = false;
        if let Some(root) = self.registry.root_of(id)
            && let Some(tree) = self.window_defs.get_mut(&root)
        {
            let mut changed = false;
            let mut styled = false;
            if let Some(widget) = tree.find_mut(id) {
                is_timeline = widget.is_timeline();
                is_clip = matches!(widget.kind, widget::WidgetKind::Clip { .. });
                is_roll = matches!(widget.kind, widget::WidgetKind::PianoRoll { .. });
                for (k, v) in &props {
                    if !(is_timeline && timeline::is_timeline_key(k)) {
                        // The generic place props (`w`/`h`/`weight`/`x`/`y`)
                        // and the style props (`theme`/`color`) apply to any
                        // widget; everything else is the kind's own.
                        let style = widget.style_apply(k, v);
                        styled |= style;
                        changed |= style
                            || (k == "gestures" && widget.gestures_apply(v))
                            || widget.place.apply(k, v)
                            || widget::apply_widget(widget, k, v);
                    }
                }
            }
            // A style change re-resolves the window's theme references — the
            // mutation point where a theme group cascades to its subtree.
            if styled {
                widget::resolve_themes(tree, &Arc::new(self.theme.clone()));
            }
            if changed {
                effects.push(HostEffect::Redraw(root));
            }
        }
        if is_timeline {
            self.set_timeline_props(id, &props, effects);
        }
        // A content change moves the extent the shared axis spans, so it has to
        // be re-registered: a moved or resized clip lengthens its lane, and a
        // piano-roll's extent *is* its notes and OSC events. Without this a roll
        // stays on the axis it was defined with, so everything written into an
        // empty one lands outside the window -- the roll never shows what is
        // painted into it. (The host's own MIDI painting already re-registers;
        // see `gui::midi`.)
        if is_clip {
            self.sync_track_totals();
        } else if is_roll && keys.iter().any(|k| matches!(k.as_str(), "notes" | "osc")) {
            // Keeping the window, not refitting it: a roll is *written into*, a
            // note at a time, so a take that grows must scroll under a still
            // axis rather than zoom it out from under the notes just drawn --
            // the same rule a dragged clip follows, for the same reason. And
            // once the take runs past the right edge, the axis pages forward to
            // where it is still being written.
            self.sync_track_totals_keeping_view();
            self.follow_timeline_end(id, effects);
        }
        if touches_source {
            self.sync_bus_watches();
        }
        true
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
            // The window goes, and with it the scale a shell reported for it.
            self.resolved_metrics.remove(&id);
            effects.push(HostEffect::CloseWindow(id));
        }
        self.sync_bus_watches();
        // A freed widget can no longer forward (its subtree is gone), its
        // timeline group state goes with it, and its live voices are released.
        self.prune_bindings();
        self.prune_voices();
        self.prune_timeline_groups();
        self.prune_text_focus();
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
        match &binding {
            Binding::Server { addr, prefix } => {
                if self.server.is_none() {
                    warn!(
                        "{from}: {GUI_BIND} {id}: no audio server attached (--server); the \
                         binding will swallow the value but cannot forward it"
                    );
                }
                info!("{from}: {GUI_BIND} {id} -> audio server {addr} {prefix:?}");
            }
            Binding::Widget {
                id: target,
                prop: key,
            } => info!("{from}: {GUI_BIND} {id} -> widget {target} {key}"),
        }
        self.bindings.insert(id, binding);
    }

    /// Forwards `widget_id`'s `value` to wherever it is bound, returning whether
    /// the binding handled it. When it returns `true` the caller must **not**
    /// also emit a `/gui_event` — bypassing the script is the whole point. A
    /// widget bound to an audio server that is not attached still returns
    /// `true` (the value is swallowed, not sent to the script); the missing
    /// `--server` was already warned about at bind time.
    pub fn forward(
        &mut self,
        widget_id: i32,
        value: OscType,
        effects: &mut Vec<HostEffect>,
    ) -> bool {
        self.forward_args(widget_id, vec![value], effects)
    }

    /// [`forward`](Self::forward) for a **flat list** of values — the edit-back
    /// payload of an editor widget (a `bpf`'s breakpoint list today, a drawn
    /// buffer region later): a bound editor sends `addr prefix… values…` to
    /// the audio server, or the payload's JSON carrier to another widget's
    /// prop, bypassing the script exactly as a bound knob does.
    pub fn forward_args(
        &mut self,
        widget_id: i32,
        values: Vec<OscType>,
        effects: &mut Vec<HostEffect>,
    ) -> bool {
        let Some(binding) = self.bindings.get(&widget_id) else {
            return false;
        };
        // Bound to another widget: the value lands on that widget's prop, as
        // the one apply a `/gui_set` would perform. It never re-enters this
        // path, so a binding fires an apply and never another binding.
        if let Binding::Widget { .. } = binding {
            if let Some((target, key, value)) = binding.prop(&values)
                && !self.set_props(target, vec![(key.clone(), value)], effects)
            {
                warn!("{GUI_BIND} {widget_id}: no widget {target} to set {key:?} on");
            }
            return true;
        }
        if let Some(msg) = binding.message_args(values)
            && let Some(server) = self.server.as_ref()
            && let Err(e) = server.send(msg)
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

    /// Clears the text focus if the focused widget was freed or redefined away,
    /// so keystrokes never edit a widget that no longer exists.
    fn prune_text_focus(&mut self) {
        if let Some((_, id)) = self.focused_text
            && !self.registry.contains(id)
        {
            self.focused_text = None;
        }
    }

    /// Starts a host-managed voice for a held piano key, when widget
    /// `widget_id` is a `piano` in voice mode (`voice` set): allocates an
    /// explicit node id, sends the `/synth_new` and records the `(pitch, node)`
    /// pair so the release can gate it. A re-press of an already-sounding
    /// pitch releases the old voice first. Bookkeeping happens even with no
    /// server attached, so the logic is testable without a transport.
    pub fn piano_voice_on(&mut self, def_id: i32, widget_id: i32, pitch: i32, velocity: i32) {
        let Some(widget::WidgetKind::Piano {
            voice: Some(name),
            voice_args,
            ..
        }) = self
            .window_def(def_id)
            .and_then(|t| t.find(widget_id))
            .map(|w| &w.kind)
        else {
            return;
        };
        let (name, extra) = (name.clone(), voice_args.clone());
        self.piano_voice_off(widget_id, pitch);
        let node = VOICE_ID_BASE + self.voice_counter;
        self.voice_counter = (self.voice_counter + 1) % VOICE_ID_SPAN;
        self.send_to_server(piano::voice_on_msg(&name, node, pitch, velocity, &extra));
        self.voices
            .entry(widget_id)
            .or_default()
            .push((pitch, node));
    }

    /// Releases the host-managed voice of a piano key (`gate 0`; the def frees
    /// the node itself). A no-op when no voice is sounding for the pitch —
    /// including when `voice` was unset mid-hold, so a recorded voice always
    /// gets its release.
    pub fn piano_voice_off(&mut self, widget_id: i32, pitch: i32) {
        let Some(list) = self.voices.get_mut(&widget_id) else {
            return;
        };
        let mut nodes = Vec::new();
        list.retain(|&(p, node)| {
            if p == pitch {
                nodes.push(node);
                false
            } else {
                true
            }
        });
        if list.is_empty() {
            self.voices.remove(&widget_id);
        }
        for node in nodes {
            self.send_to_server(piano::voice_off_msg(node));
        }
    }

    /// The live voice nodes of widget `widget_id` (for tests/introspection).
    pub fn piano_voices(&self, widget_id: i32) -> &[(i32, i32)] {
        self.voices.get(&widget_id).map_or(&[], Vec::as_slice)
    }

    /// Releases every live voice of widgets that no longer exist (after a
    /// `/gui_free` or a redefining `/gui_def`) — a freed piano must not leave
    /// keys sounding.
    fn prune_voices(&mut self) {
        let stale: Vec<i32> = self
            .voices
            .keys()
            .filter(|id| !self.registry.contains(**id))
            .copied()
            .collect();
        for id in stale {
            if let Some(list) = self.voices.remove(&id) {
                for (_, node) in list {
                    self.send_to_server(piano::voice_off_msg(node));
                }
            }
        }
    }

    /// Sends one message out the audio-server leg, if one is attached.
    /// Re-diffs the audio buses the open documents read against the ones the
    /// server is recording, and asks it to start or stop the difference
    /// (`/bus_tap bus 1` / `/bus_tap bus 0`). Watches are counted server-side, so two
    /// views of one bus cost one recording and the last to go frees it.
    ///
    /// Called after anything that can change what is drawn: a def, a free, a
    /// `/gui_set` of a view's bus, rate or channel count.
    fn sync_bus_watches(&mut self) {
        let mut wanted: Vec<i32> = Vec::new();
        for tree in self.window_defs.values() {
            collect_audio_buses(tree, &mut wanted);
        }
        for bus in &wanted {
            if !self.watched_buses.contains(bus) {
                self.send_to_server(watch_msg(*bus, true));
            }
        }
        for bus in &self.watched_buses {
            if !wanted.contains(bus) {
                self.send_to_server(watch_msg(*bus, false));
            }
        }
        self.watched_buses = wanted;
    }

    fn send_to_server(&self, msg: OscMessage) {
        if let Some(server) = self.server.as_ref()
            && let Err(e) = server.send(msg)
        {
            warn!("cannot send to the audio server: {e}");
        }
    }

    /// Registers a [`Binding`] for every widget that declares an inline `bind`
    /// array in the GuiDef (`{"id":…,"type":…,"bind":["/node_set",node,"freq"]}`).
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
/// `/bus_tap bus watch`: what the host sends the audio server to start or stop
/// recording an audio bus. The bus is the whole address — the server picks and
/// publishes where the samples land.
fn watch_msg(bus: i32, watch: bool) -> OscMessage {
    OscMessage {
        addr: "/bus_tap".into(),
        args: vec![OscType::Int(bus), OscType::Int(if watch { 1 } else { 0 })],
    }
}

/// Appends every audio bus whose samples a tree's views read, deduplicated.
fn collect_audio_buses(widget: &Widget, out: &mut Vec<i32>) {
    let mut mine = Vec::new();
    widget.kind.audio_buses_read(&mut mine);
    for bus in mine {
        if bus >= 0 && !out.contains(&bus) {
            out.push(bus);
        }
    }
    for child in &widget.children {
        collect_audio_buses(child, out);
    }
}

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
/// `/def_send synth` accepts a SynthDef either way).
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

    /// One host, one logical table, one resolved table per window — and the
    /// shell is the only side that says what a window's scale is.
    #[test]
    fn each_window_resolves_the_table_at_its_own_scale() {
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TREE), from());
        host.handle_packet(def_msg(2, TREE), from());
        assert_eq!(host.ui_scale(1), 1.0, "no shell has reported one yet");
        assert_eq!(host.metrics_for(1), &host.metrics);

        assert!(host.set_ui_scale(1, 2.0));
        assert!(!host.set_ui_scale(1, 2.0), "the same scale changes nothing");
        assert_eq!(host.ui_scale(1), 2.0);
        assert_eq!(host.metrics_for(1).control_h, host.metrics.control_h * 2.0);
        assert_eq!(
            host.metrics_for(2),
            &host.metrics,
            "the other window is on its own display"
        );

        // A new logical table (a `[gui.metrics]` overlay, the browser's
        // `metrics(json)`) reaches every window at the scale it is on.
        host.metrics.overlay([("control_h", 30.0)]);
        host.refresh_metrics();
        assert_eq!(host.metrics_for(1).control_h, 60.0);
        assert_eq!(host.metrics_for(2).control_h, 30.0);

        // Freeing the window drops what the shell reported for it.
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_FREE.into(),
                args: vec![OscType::Int(1)],
            }),
            from(),
        );
        assert_eq!(host.ui_scale(1), 1.0);
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
        let data = tree.children[0]
            .signal()
            .and_then(|el| el.source.data())
            .expect("expected a waveform");
        assert_eq!(&data.samples[..], &[0.5, -0.5]);
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
                    OscType::String("/node_set".into()),
                    OscType::Int(1000),
                    OscType::String("cutoff".into()),
                ],
            ),
            from(),
        );
        assert!(host.is_bound(10));

        // A value change goes straight to the server (bypassing the script).
        assert!(host.forward(10, OscType::Float(440.0), &mut Vec::new()));
        let mut buf = [0u8; 1024];
        let (len, _) = fake_server.recv_from(&mut buf).expect("forwarded datagram");
        let msg = match decode_packet(&buf[..len]).unwrap() {
            OscPacket::Message(m) => m,
            other => panic!("expected a message, got {other:?}"),
        };
        assert_eq!(msg.addr, "/node_set");
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
        assert!(!host.forward(10, OscType::Float(1.0), &mut Vec::new()));
    }

    /// The other destination: a widget bound to a widget. A toggle drives a
    /// `stack`'s page with no script and no server in the process — the whole
    /// of tabs, and what makes a persisted GuiDef an autonomous application.
    #[test]
    fn a_widget_binding_applies_to_the_other_widget_and_never_cascades() {
        const TABS: &str = r#"{"type":"window","children":[
            {"id":10,"type":"toggle","label":"view"},
            {"id":20,"type":"stack","index":0,"children":[
                {"id":21,"type":"label","text":"one"},
                {"id":22,"type":"label","text":"two"}]}]}"#;
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TABS), from());
        host.handle_packet(
            bind_msg(
                10,
                vec![
                    OscType::String("widget".into()),
                    OscType::Int(20),
                    OscType::String("index".into()),
                ],
            ),
            from(),
        );
        assert!(host.is_bound(10));

        // The toggle's value applies as a `/gui_set 20 index 1` would: the
        // typed tree switches page, and the window is asked to repaint.
        let mut effects = Vec::new();
        assert!(
            host.forward(10, OscType::Int(1), &mut effects),
            "bound: the script never sees it"
        );
        assert!(matches!(effects.as_slice(), [HostEffect::Redraw(1)]));
        let index = match host.window_def(1).unwrap().find(20).unwrap().kind {
            widget::WidgetKind::Stack { index, .. } => index,
            ref other => panic!("expected a stack, got {other:?}"),
        };
        assert_eq!(index, 1, "the page the toggle names");
        // The generic registry moved with it, so a `/gui_query` agrees.
        assert_eq!(
            host.registry().get(20).unwrap().props.get("index"),
            Some(&Value::from(1))
        );

        // A binding fires an apply, never another binding: binding the stack
        // back to the toggle cannot make the apply re-enter delivery, so the
        // pair settles instead of cascading.
        host.handle_packet(
            bind_msg(
                20,
                vec![
                    OscType::String("widget".into()),
                    OscType::Int(10),
                    OscType::String("value".into()),
                ],
            ),
            from(),
        );
        let mut effects = Vec::new();
        host.forward(10, OscType::Int(0), &mut effects);
        assert_eq!(
            host.registry().get(10).unwrap().props.get("value"),
            None,
            "the stack's own binding did not fire from the apply"
        );
    }

    /// A hidden page is still on the axis: a `stack` skips a page's *layout*,
    /// not its membership, so a scroll bound to one view moves the one behind
    /// it too and a switch shows it already there. Same property as the GPU
    /// slot it keeps — both are read from the tree, not from the placements.
    #[test]
    fn a_hidden_stack_page_still_belongs_to_its_navigation_group() {
        const PAGES: &str = r#"{"type":"window","children":[
            {"id":20,"type":"stack","index":0,"children":[
                {"id":21,"type":"waveform","data":[0.0,1.0,0.0,-1.0,0.0,1.0,0.0,-1.0],"link":1},
                {"id":22,"type":"spectrogram","data":[0.0,1.0,0.0,-1.0,0.0,1.0,0.0,-1.0],"link":1}]}]}"#;
        let mut host = Host::new();
        host.handle_packet(def_msg(1, PAGES), from());
        let nav = |host: &Host, id: i32| {
            host.timelines()
                .nav(timeline::group_key(id, Some(1)))
                .expect("a linked view is on a group")
        };
        // The extent is the front's to report (it is the loaded data's), so a
        // headless test says it: eight samples on the axis.
        host.set_timeline_total(21, 8);
        host.set_timeline_total(22, 8);
        // One group, so the hidden page reads the same window as the shown one.
        host.handle_packet(
            OscPacket::Message(OscMessage {
                addr: GUI_SET.into(),
                args: vec![
                    OscType::Int(21),
                    OscType::String("view_len".into()),
                    OscType::Float(4.0),
                    OscType::String("view_start".into()),
                    OscType::Float(2.0),
                ],
            }),
            from(),
        );
        assert!((nav(&host, 21).start - 2.0).abs() < 0.001);
        assert_eq!(
            nav(&host, 22),
            nav(&host, 21),
            "the page nobody is looking at moved with the axis"
        );
        // And it is a member because the tree says so: the collector that
        // registers the groups walks the widgets, not the rectangles.
        assert!(host.window_def(1).unwrap().find(22).unwrap().is_timeline());
    }

    /// An inline `bind` carries a widget target too, which is what lets a saved
    /// GuiDef boot with its pages already wired.
    #[test]
    fn an_inline_widget_bind_is_registered_at_define_time() {
        const TABS: &str = r#"{"type":"window","children":[
            {"id":10,"type":"menu","items":["a","b"],"bind":["widget",20,"index"]},
            {"id":20,"type":"stack","children":[
                {"id":21,"type":"label","text":"one"},
                {"id":22,"type":"label","text":"two"}]}]}"#;
        let mut host = Host::new();
        host.handle_packet(def_msg(1, TABS), from());
        assert!(host.is_bound(10));
        host.forward(10, OscType::Int(1), &mut Vec::new());
        assert!(matches!(
            host.window_def(1).unwrap().find(20).unwrap().kind,
            widget::WidgetKind::Stack { index: 1, .. }
        ));
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
                    OscType::String("/bus_set".into()),
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
                    OscType::String("/node_set".into()),
                    OscType::Int(1000),
                    OscType::String("cutoff".into()),
                ],
            ),
            from(),
        );
        assert!(host.is_bound(10));
        assert!(
            host.forward(10, OscType::Float(1.0), &mut Vec::new()),
            "swallowed, not emitted"
        );
    }
}
