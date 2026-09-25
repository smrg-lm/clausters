//! The GUI host: an OSC front with a widget command interpreter.
//!
//! `clausters-gui` is **two roles in one process**: a *GUI host* for the
//! language clients -- it owns the windows, the widgets and the GPU, and speaks
//! the `/gui_*` widget protocol -- and a *client of the audio server* -- it reads
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
//! `clausters-core`** -- a path dependency that pulls only `rosc` -- for the shared
//! OSC seam (the single [`clausters_core::osc::decode_packet`] door, plus
//! encode/bundle/message), and owns a **thin transport front** of its own
//! ([`transport`]). The default build links no server code; only the optional
//! `standalone` feature pulls the full `clausters` crate, for the in-process
//! embedded server (`embed`).
//!
//! That front now carries UDP and TCP ([`tcp`]) together on one port, plus an
//! opt-in WebSocket leg ([`ws`]) -- all behind one [`ClientId`] and
//! reply seam, which is the seam's whole point: each carrier was added without
//! touching the protocol or this command loop, and the next one should be too.
//! The client leg ([`client::ServerLeg`]) reuses that same encode door, so the
//! gui talks to the audio server with one encoder, not a parallel one.

// The platform-agnostic core: the widget/protocol logic, web-portable (it
// compiles for `wasm32` unchanged). No sockets, no filesystem, no GPU bring-up --
// every such coupling lives behind a trait whose impl is in the native shell
// below.
//
// It is four families, and the grouping below is the second thing this file
// says after the platform seam: what the wire means, what draws, what a pass
// does, and the vocabulary all three read. A module stays flat here when the
// *whole host* reads it, and goes in a directory when only its own consumers
// do -- which is why `paint` and `metrics` are files while the models are a
// tree.

// The protocol and the tree it holds: the generic document, the ids, the typed
// schema the renderer reads, the leaves behind the trait, and the two places a
// widget's value can go instead of the script -- plus the voices the host plays
// on an element's behalf, which are its own and not the element's.
pub mod ack;
pub mod bind;
// What the host says to itself: one door for the platform log, in both builds,
// and the debug channel that also lands on a window's status bar. The status
// bar beside it is what the host says to the person at the window; this is the
// other half, and it compiles out of a release below the warning level.
pub mod diag;
// The host-wide clipboard: one typed document plus the bulk it names. Here
// rather than with the elements because it is nobody's -- one clipboard serves
// every field, roll and view of every window.
pub mod clipboard;
pub mod document;
pub mod elements;
pub mod guidef;
// Which of a container's layered contents a hand is editing -- the one rule
// that decides between claimants over the same pixels, read by the drawing,
// the press and the wire alike.
// The vertical axis: the stack of bands a timeline view places its boxes on --
// a roll's semitone rows and a multitrack's lanes, which differ only in whether
// they are all the same height.
pub mod bands;
pub mod layers;
// One geometry for every box that lives on a time axis: a note in a roll and a
// clip on a lane are the same object with respect to editing and positioning,
// and the arithmetic is written here once.
pub mod play;
// What the multitrack is playing through: the instance, the handle tables and the
// allocators. Beside `play` and not under `document`, because what it holds is
// the server's -- nodes, buses, buffers -- and nodes are not the document.
pub mod instance;
// The node ids, buses and buffers this host allocates on its server, by the
// one policy every client allocates by, and the replies that give them back.
pub mod ids;
pub mod registry;
pub mod voices;
pub mod widget;

// The models: what a visual thing is shaped like, how it is drawn and where a
// click on that drawing lands. Read by the elements, never the reverse.
pub mod graphics;
// The structures a hand edits and the verbs over them, apart from what draws
// them: a box on a time axis, a note, a clip and a break-point. `graphics`
// beside it holds only what puts pixels down, and the three applications over
// the document share the structures rather than the pictures.
pub mod structures;

// The drawing vocabulary every widget and every pass names things in: one mesh
// primitive, one face, and the two role tables (no paint site names an RGBA, no
// layout site names a number).
pub mod font;
pub mod metrics;
pub mod paint;
pub mod theme;

// Geometry and navigation: where a widget lands, and the axes several of them
// share.
pub mod layout;
pub mod ruler;
pub mod scroll;
pub mod timeline;

// The passes over that tree, and the read-only facts a frame hands them.
pub mod frame;
pub mod interact;
pub mod status;
pub mod world;

// Where values and samples come from, on the agnostic side of the seam: the
// per-frame bus reads and the buffer-fetch conversation. Their I/O ends are in
// the shells below.
pub mod fetch;
pub mod live;
// The server's replies about buffers, read once for both fronts.
pub mod replies;
// The winit key both shells read, mapped once.
pub mod winit_keys;

// The host's own concerns, one module each: the counters a playhead reads, the
// protocol as it arrives, a definition and a free, a set, the answer to a
// gesture, and the takes the pictures draw.
mod answer;
mod clocks;
mod define;
mod set;
mod takes;
#[cfg(test)]
mod tests;
mod wire;
#[cfg(test)]
mod write_tests;

// Booting a persisted bundle over the wire -- the ordering/encoding half of the
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

// The samples behind that segment's directory: a take's samples, mapped
// read/write, so drawing one costs no conversation and editing one costs no
// message. Unix-only, like `shm`.
#[cfg(unix)]
pub mod mapped;

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

// The native [`FontSource`] (the `font-atlas` feature): the typeface a build
// draws with, read from a file the command line names or from the system's own
// faces. The browser fetches one instead and pushes the bytes through the same
// seam.
#[cfg(all(not(target_arch = "wasm32"), feature = "font-atlas"))]
pub mod fontfile;

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

use crate::host::world::HeadClocks;
use serde_json::Value;

pub use bind::Binding;
#[cfg(not(target_arch = "wasm32"))]
pub use client::ServerLeg;
pub use clocks::HeadClock;
pub use guidef::GuiNode;
pub use registry::Registry;
pub(crate) use takes::{buffer_views, span_to_read_back, stream_report};
pub use widget::{Widget, WidgetKind};

/// Where a request reached the host and where its replies go. The `/gui_*`
/// *encoding* is transport-independent, so client identity is too: every
/// carrier is a variant here, and a new one is added without the protocol
/// dispatch changing. It lives in the agnostic core, not in any one front, so
/// that dispatch names it on every platform.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClientId {
    /// A UDP datagram source (the native server front).
    Udp(SocketAddr),
    /// A TCP connection on the native server front, by connection id --
    /// length-prefixed frames, replies routed back on the same connection.
    Tcp(u64),
    /// A WebSocket connection on the native server front (`--ws`), by
    /// connection id -- one OSC packet per binary message, replies routed back
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
/// [`BusSource`] below) -- kept near the other platform seams.
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
    /// An in-process **on-demand session**: the same server with no audio
    /// device, which performs the editing verbs and owns the samples. What an
    /// editor sends its allocations, its edits and its renders to, while the
    /// sound goes to a player that is another process entirely.
    #[cfg(feature = "standalone")]
    Session(embed::EmbedSession),
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
            #[cfg(feature = "standalone")]
            ServerLink::Session(session) => {
                let bytes = clausters_core::osc::encode(&OscPacket::Message(msg)).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
                if session.send(&bytes) {
                    Ok(())
                } else {
                    Err(std::io::Error::other("session command ring full"))
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
            ServerLink::Embed(_) | ServerLink::Session(_) => None,
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

    /// The in-process session behind this link, if any -- polled for replies
    /// exactly as the embedded server is.
    #[cfg(feature = "standalone")]
    pub fn session(&self) -> Option<&embed::EmbedSession> {
        match self {
            ServerLink::Session(session) => Some(session),
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
    /// interleaved (`channels` only trims a trailing partial frame -- the plot
    /// draws every channel). `None` on an unsupported platform or an I/O error.
    fn plot_samples(&self, path: &Path, channels: usize) -> Option<std::sync::Arc<[f32]>>;

    /// Reads a local `path` of raw little-endian `f32` into its de-interleaved
    /// channels (all of them) -- the spectrogram's row source; each channel is
    /// analyzed separately. `None` on an unsupported platform or an I/O error.
    fn raw_channels(&self, path: &Path, channels: usize) -> Option<Vec<Vec<f32>>>;

    /// The raw bytes of a local resource (a prebuilt STFT cache the
    /// spectrogram parses with `Stft::from_bytes`). `None` on an unsupported
    /// platform or an I/O error.
    fn file_bytes(&self, path: &Path) -> Option<Vec<u8>>;
}

/// Where the host's **typeface** comes from -- the fifth platform seam, and the
/// one that only exists when the crate was built with a rasterizer (the
/// `font-atlas` feature).
///
/// A face is bytes, and every platform has its own way of reaching them: a
/// native host maps a file (`fontfile::FontFile` -- one the command line names,
/// or one of the system's), a page fetches a URL and pushes what came back
/// (`web::FetchedFace`). Above the seam neither is named: the host asks
/// for bytes once ([`Host::load_face`]) and every window draws with them.
///
/// Answering `None` is ordinary, not an error: the embedded bitmap face is the
/// floor this crate always draws on, so a host with no typeface renders exactly
/// what a host built without the feature renders.
#[cfg(feature = "font-atlas")]
pub trait FontSource {
    /// The bytes of the face to draw with (TrueType/OpenType), or `None` where
    /// this platform has none to offer.
    fn face(&self) -> Option<Vec<u8>>;
}

/// A source of live control-bus values for the meter/scope views. Implemented by
/// the shared-memory segment ([`shm::SharedSegment`]) on Unix; the trait lets the
/// windowed front hold the source without platform `cfg`s and read a bus each
/// frame with no OSC traffic.
pub trait BusSource: Send + Sync {
    /// The current value of control bus `index` (`0.0` if out of range).
    fn control(&self, index: usize) -> f32;

    /// Fills `out` with the newest raw samples of **audio bus** `bus` (newest
    /// last), returning `false` when this source has none for it -- the
    /// default. Where those samples physically live is this source's business:
    /// the shared-memory segment looks the bus up in the server's directory
    /// and reads that ring lock-free, the browser reads its `/bus_tapStream.reply` store.
    /// Above here, a bus is the only thing anyone names.
    fn read_bus(&self, _bus: i32, _out: &mut [f32]) -> bool {
        false
    }

    /// [`read_bus`](Self::read_bus), plus **where the window ends in the bus's
    /// own stream** -- the count of samples the engine has ever written to it.
    ///
    /// The newest window alone cannot be retained: two ticks read overlapping
    /// windows, and how much they overlap depends on the frame rate, so
    /// appending them would stretch or compress the history. The position is
    /// what makes the append exact -- a retainer keeps the last one it saw and
    /// takes only the samples past it. `None` where the source has no window
    /// for the bus, or carries no position (nothing is retained then, rather
    /// than something wrong being retained).
    fn read_bus_at(&self, _bus: i32, _out: &mut [f32]) -> Option<u64> {
        None
    }

    /// The largest window this source can serve in **one** read (0 = it does
    /// not say). A reader asking for more than this gets nothing back, which is
    /// silence that looks exactly like a bus nobody is writing -- so a retaining
    /// read sizes itself by this rather than by a duration it picked.
    fn window_limit(&self) -> usize {
        0
    }

    /// Audio bus `bus`'s published level -- the peak of the engine's last
    /// block, held with a decay -- or `0.0` where this source has none. What a
    /// meter draws; it needs no recording, so it costs no tap.
    fn level(&self, _bus: i32) -> f32 {
        0.0
    }

    /// The server's sample rate when this source knows it (`0.0` otherwise);
    /// sizes the oscilloscope windows (`window_ms` -> samples).
    fn sample_rate(&self) -> f64 {
        0.0
    }

    /// The engine's sample clock (samples processed since boot) when this
    /// source carries it (`0.0` otherwise). Drives the timeline playhead with
    /// zero messages natively; the browser polls `/clock_query` instead.
    fn sample_clock(&self) -> f64 {
        0.0
    }

    /// Transport `transport`'s **position**, in samples, when this source
    /// carries it (`0.0` otherwise).
    ///
    /// The other counter a playhead can be drawn from, and the one an editor
    /// wants: it holds while the transport is stopped, jumps wherever a locate
    /// puts it and wraps at a loop's end, all in the engine. Natively it is a
    /// field of the shared segment; the browser polls `/transport_query`.
    /// Which of the two a view draws is [`Host::head_clock_of`], read once a
    /// frame in [`Host::head_clocks`].
    fn transport_position(&self, transport: usize) -> f64 {
        let _ = transport;
        0.0
    }
}

// The `/gui_*` vocabulary (canonical tables in clients/gui/PLAN.md).
pub const GUI_DEF: &str = "/gui_def";
pub const GUI_SET: &str = "/gui_set";
pub const GUI_FREE: &str = "/gui_free";
pub const GUI_QUERY: &str = "/gui_query";
/// `/gui_ack <seq> <docVersion> [<source> <generation>...] [<reason>]` -- the
/// owner's answer to the edits this host emitted.
///
/// The reply `/gui_event` never had. Everything else the host asks has one
/// (`/gui_query` -> `/gui_info`), and without this an edit the owner refused or
/// transformed was indistinguishable from one it took: the host went on drawing
/// what the hand did, forever.
///
/// It is a verb rather than a property because it is scoped to the
/// **conversation** and not to the tree -- `seq` is per client, so two clients
/// driving one window would collide on a single prop -- and because it does not
/// round-trip, which is what a property has to do here. It rides in the same
/// bundle as the value pushes it accompanies, after them, and it is sent
/// **always**, including when nothing changed: that is exactly what a refusal
/// is.
pub const GUI_ACK: &str = "/gui_ack";
pub const GUI_BIND: &str = "/gui_bind";
pub const GUI_LOAD: &str = "/gui_load";
/// `/gui_font <blob>` -- the typeface every window draws text with, handed over
/// after launch.
///
/// A face is bytes on both fronts -- a native host maps a file, a page fetches a
/// URL -- so *reaching* them is the platform's and *when they may be handed
/// over* is not. Without this verb the browser could change its face at runtime
/// through the raw binding and a window could only take one at launch
/// (`--font`), which is a wasm export growing surface the protocol never had.
///
/// It carries no id: a face is a property of the **host**, not of a window (the
/// size table never followed the typeface), which is also what makes a late
/// hand-over safe -- nothing relayouts, every open window simply redraws.
pub const GUI_FONT: &str = "/gui_font";
/// `/gui_theme <json>` -- the colors this **host** draws its chrome from, handed
/// over after launch.
///
/// The same partial `{"role": "#rrggbb[aa]"}` table a container's `theme` prop
/// takes, scoped to the host instead of to a subtree: the base every theme
/// group is resolved over. It carries no id for that reason -- a look is a
/// property of the host, exactly as a typeface is.
///
/// Without it the browser front could re-theme at runtime through the raw
/// binding while a native one could only take a table at launch (`--theme
/// file.toml`, `[gui.theme]`), which is a wasm export growing surface the
/// protocol never had -- the case `/gui_font` already answered once.
pub const GUI_THEME: &str = "/gui_theme";
/// `/gui_metrics <json>` -- the sizes this **host** lays out with, handed over
/// after launch.
///
/// The theme's counterpart for lengths: a partial `{"role": number}` table over
/// the metrics every widget reads its paddings, strips and hit slop from, with
/// the reserved `scale` key regenerating the whole set at a density. Same
/// reasoning, same shape, same absence before this verb.
pub const GUI_METRICS: &str = "/gui_metrics";
/// `/gui_headClock <"device"|"transport">` -- which of the engine's counters every
/// playhead in this **host** is drawn from.
///
/// The third of the host-wide verbs, and here for the reason the other two
/// are: a native host could say it at launch (`--clock`) and a page could not
/// say it at all, so a script driving a multitrack had no way to ask for the
/// only counter that means anything to an editor.
///
/// `device` is the sample clock, which never stops -- what a host watching a
/// live server wants, since its meters, scopes and taps are all on that axis.
/// `transport` is the transport's **position**: it holds while it is
/// stopped, jumps wherever `/transport_locate` puts it and wraps at a loop's
/// end, all in the engine. A window drawing it needs no anchor of its own
/// (`playhead_at` of `0`) and no message per frame, which is what lets a client
/// hand playback to the transport and stop computing time.
pub const GUI_CLOCK: &str = "/gui_headClock";
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
    /// Open the window for the GuiDef rooted at this id -- **or bring the one it
    /// already has up to this tree**, which is what most of these are: a
    /// redefine is how a structural edit reaches a window, and destroying the
    /// window to answer one is not a redraw. A front rebuilds the def's own
    /// state and keeps the shell.
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
    /// Typed widget trees for window-rooted defs, by def id -- the renderable
    /// documents the windowed front builds windows from. Non-window roots live
    /// only in the generic registry.
    window_defs: HashMap<i32, Widget>,
    /// The audio buses the host has asked the audio server to record, so the
    /// sample views can read them. Kept as a set and re-diffed whenever the
    /// documents change: the host is the one that turns "this scope watches
    /// bus 4" into the server's `/bus_tap`, which is why no client -- and no
    /// widget -- ever names a recording ring.
    watched_buses: Vec<i32>,
    /// The `/buffer_stream` subscription this host currently holds, as
    /// `(buffers, bucket)` -- empty for none.
    ///
    /// One subscription covers every view of every window (the server keeps one
    /// per client and replaces it on each call), so what is kept here is the
    /// last thing asked for, and a resync that would ask for the same thing
    /// sends nothing.
    buffer_stream: (Vec<i32>, usize),
    /// The audio-server client leg (the third topology leg). Present when the
    /// host was started with a `--server` target or, in standalone mode, an
    /// embedded server; [`forward`](Self::forward) sends bound-widget values
    /// through it.
    server: Option<ServerLink>,
    /// The server that **makes sound**, when it is not the one that holds the
    /// samples.
    ///
    /// In an editor's arrangement they are two processes on purpose: the
    /// on-demand session owns the document's takes and computes, the RT server
    /// holds the machine's input and output and can be killed and restarted
    /// without the samples moving. Playing, sounding a key, the transport and
    /// the bus taps go here; allocation, the editing verbs and the renders go
    /// to [`Self::server`]. With nothing attached here the two are one server
    /// and everything takes the leg it always took.
    player: Option<ServerLink>,
    /// The **samples** the host can reach without asking for it: the takes of
    /// a shared segment, mapped read/write.
    ///
    /// Present when the host was pointed at a segment whose samples it may
    /// touch -- its own session's, or an external server's `--shm` path. With
    /// it a take is drawn from memory rather than fetched, and a stroke is a
    /// store rather than a `/buffer_setRangeChannel`; without it both go over
    /// the wire exactly as they always have, which is what every remote client
    /// and every page keeps doing.
    #[cfg(unix)]
    shared_buffers: Option<mapped::SharedBuffers>,
    /// Widget id -> the audio-server destination its value forwards to
    /// (`/gui_bind`). A bound widget bypasses the script: its value goes
    /// straight to the audio server instead of emitting a `/gui_event`.
    bindings: HashMap<i32, Binding>,
    /// The verbatim `/gui_def` JSON per def id -- the source of truth for
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
    /// `/synth_new`, the release the `gate 0`; the def frees the node itself,
    /// and its id comes back on the `/node_end` that says so.
    voices: HashMap<i32, Vec<(i32, i32)>>,
    /// **The ids this host allocates on the server it plays through** -- every
    /// node, control bus and buffer it makes, by the one policy every client
    /// allocates by ([`ids`]).
    ids: clausters_core::ids::IdSpaces,
    /// **The take monitor's nodes** -- the audio editor's playback, the one
    /// every endpoint holds ([`clausters_editing::audio_playback`]), on the
    /// monitor's own transport ([`play::MONITOR_TRANSPORT`]).
    monitor: clausters_editing::audio_playback::AudioEditorPlayback,
    /// The engine's sample rate, from `/server_query.reply`; `0.0` until it
    /// answers.
    server_rate: f64,
    /// The take the **monitor** plays, while it plays (see [`play`]). One take
    /// at a time, so this is one entry and not a list.
    playing: Option<play::Monitor>,
    /// Whether the monitor **loops** (`L`), and what it has heard of the
    /// transport's rolling state -- how an end the engine reached on its own is
    /// told from a stop this host sent ([`play::Follow`]).
    follow: play::Follow,
    /// **What the multitrack is playing through** -- the instance, the handle tables
    /// and the allocators a standalone host keeps as any other endpoint does
    /// ([`instance`]).
    instance: instance::Playing,
    /// Whether this host **drives the server's transport** -- whether it is the
    /// one that gave the session its server.
    ///
    /// It is what separates a host that owns its playback from one that is a
    /// guest on somebody else's server: a script owns its own transport, and a
    /// sweep in a window it happens to be drawing is not a request to seek it.
    /// Set by whoever gives this host a server to govern; false everywhere
    /// else, and every `/transport_*` this host would send is read off it.
    owns_transport: bool,
    /// **What the host has emitted and not heard back about**, plus the stamps
    /// it issues (see [`ack`]). Behind a `RefCell` because stamping happens
    /// where an edit is *produced* -- deep inside the gesture machine, which
    /// holds the tree immutably at that point -- and a second implementation at
    /// the two fronts is exactly what the one-gesture-machine rule forbids.
    pub outbox: std::cell::RefCell<ack::Outbox>,
    /// **What each window has said**: the status bar's lines, per def id (see
    /// [`status`]). Behind a `RefCell` for the reason
    /// [`outbox`](Self::outbox) is -- a line is written where an edit is
    /// *produced*, with the tree borrowed immutably -- and beside it because
    /// they are fed by the same two events: an edit going out, and the
    /// acknowledgement coming back.
    status: std::cell::RefCell<HashMap<i32, status::Status>>,
    /// The document this host owns, when it is its own owner.
    ///
    /// `None` is every host driven by a script: a gesture emits and waits, and
    /// the script answers. `Some` is the **third writer** -- a standalone
    /// editor, which has no script to wait for and must apply its own intents
    /// (`document::Owner`).
    pub owner: Option<document::Owner>,
    /// What the clock of a multitrack this host edits alone last read, so an
    /// unchanged reading is not set again every frame.
    pub(crate) clock_shown: Option<String>,
    /// What this host told itself and asked of the playback, for the tests
    /// that replay a client's recorded exchange against it.
    #[cfg(test)]
    pub(crate) exchange: instance::Exchange,
    /// The host's color roles -- one look per host, every paint site reads it
    /// (see [`theme`]).
    pub theme: theme::Theme,
    /// The host's size roles in **logical** pixels -- one density per host, the
    /// table the config declares (see [`metrics`]). A window paints with its
    /// own resolution of it, from [`metrics_for`](Self::metrics_for), so
    /// changing this table once windows exist means calling
    /// [`refresh_metrics`](Self::refresh_metrics) after it.
    pub metrics: metrics::Metrics,
    /// The antialiasing every window this host opens is drawn with: the MSAA
    /// sample count of its render pass (`1` = none, the default). Like
    /// [`theme`](Self::theme) and [`metrics`](Self::metrics) it is one setting
    /// per host that the *shell* consumes -- a window reads it when its GPU
    /// comes up, and a window already open keeps the pass it was built with,
    /// since every pipeline in a pass agrees on the count.
    pub msaa: u32,
    /// **How much recorded audio a picture waits for** before it re-reads
    /// its summary, in seconds (`--follow-block`, default `0` -- every frame).
    ///
    /// A recording announces nothing: the host reads the buffer's write
    /// frontier and re-summarizes what appeared. That work is the **block's**,
    /// not the take's -- the summary of a span touches the buckets over it and
    /// their parents -- so following at the frame is what the picture should
    /// do, and does. The number is here for the case where it should not: a
    /// bigger block is cheaper and choppier, and neither the sound nor a
    /// playhead over it reads this.
    ///
    /// It was one second, and the second was paying for a **copy**. A slot
    /// holds the pyramid it draws, so a refresh could not write in place and
    /// copied the whole take first -- a cost that does not shrink with the
    /// block, which is why the block had to grow instead. The slot gives the
    /// samples back before the write now
    /// ([`crate::waveform::WaveformView::release_data`]), so what a step costs
    /// is the step.
    pub follow_block: f64,
    /// The resolved (physical) metrics of each window, by def id -- this table
    /// at that window's `ui_scale`. Written when a shell reports a scale
    /// ([`set_ui_scale`](Self::set_ui_scale)), which is the only side that may
    /// know one: the core never reads a platform API. Absent = scale 1.
    resolved_metrics: HashMap<i32, metrics::Metrics>,
    /// The widget currently receiving keystrokes, as `(def_id, widget_id)` --
    /// **one focus per host**, not one per window, because there is one
    /// keyboard.
    ///
    /// A press on a widget that accepts focus moves it there; a press elsewhere
    /// (or freeing the widget) clears it, and Tab walks the window's ring. While
    /// set, a key goes to that widget's
    /// [`Element::key`](widget::Element::key) and only falls through to the
    /// front's own shortcuts when the element does not answer it.
    focused: Option<(i32, i32)>,
    /// The counter a playhead is drawn from where nothing names one: the
    /// launch-time `--clock`. See [`HeadClock`].
    head_clock: HeadClock,
    /// The counters named on the wire ([`GUI_CLOCK`]), by window or widget id.
    /// A widget draws from its own entry or its nearest ancestor's
    /// ([`Host::head_clocks`]); an entry goes when its widget is freed.
    head_clocks: HashMap<i32, HeadClock>,
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
            head_clock: HeadClock::default(),
            head_clocks: HashMap::new(),
            window_defs: HashMap::new(),
            watched_buses: Vec::new(),
            buffer_stream: (Vec::new(), 0),
            server: None,
            player: None,
            #[cfg(unix)]
            shared_buffers: None,
            bindings: HashMap::new(),
            def_json: HashMap::new(),
            store: None,
            timelines: timeline::TimelineGroups::default(),
            voices: HashMap::new(),
            ids: clausters_core::ids::IdSpaces::new(
                clausters_core::ids::ServerShape::DEFAULT,
                clausters_core::ids::IdShare::WHOLE,
            ),
            monitor: clausters_editing::audio_playback::AudioEditorPlayback::new(
                clausters_editing::apply::Endpoint::default(),
                play::MONITOR_TRANSPORT,
            ),
            server_rate: 0.0,
            playing: None,
            follow: play::Follow::default(),
            instance: instance::Playing::default(),
            owns_transport: false,
            outbox: Default::default(),
            status: Default::default(),
            owner: None,
            clock_shown: None,
            #[cfg(test)]
            exchange: instance::Exchange::default(),
            theme: theme::Theme::default(),
            metrics: metrics::Metrics::default(),
            msaa: 1,
            follow_block: 0.0,
            resolved_metrics: HashMap::new(),
            focused: None,
        }
    }

    /// The widget currently holding the keyboard focus, as `(def_id,
    /// widget_id)`.
    pub fn focused(&self) -> Option<(i32, i32)> {
        self.focused
    }

    /// Whether the focus in `def_id` is on a widget that **takes typed text**
    /// ([`WidgetKind::takes_text`]).
    ///
    /// One question, asked by one caller: the browser shell, which must move
    /// the keyboard to a hidden editable element while a field is being typed
    /// into and give it back to the canvas otherwise. The native front needs
    /// none of this -- winit composes for it -- so the answer lives here rather
    /// than in either shell.
    pub fn focus_takes_text(&self, def_id: i32) -> bool {
        self.focused
            .filter(|(def, _)| *def == def_id)
            .and_then(|(_, id)| self.window_def(def_id)?.find(id))
            .is_some_and(|w| w.kind.takes_text())
    }

    /// Moves the focus to `widget_id` in window `def_id`, replacing whatever
    /// held it. Returns the def id that lost it, when another window did (so the
    /// front repaints that one too).
    pub fn focus(&mut self, def_id: i32, widget_id: i32) -> Option<i32> {
        let previous = self.focused.replace((def_id, widget_id));
        previous.map(|(d, _)| d).filter(|d| *d != def_id)
    }

    /// Clears the focus, returning the def id that held it (so the front can
    /// repaint it without its ring) when there was one.
    pub fn clear_focus(&mut self) -> Option<i32> {
        self.focused.take().map(|(def_id, _)| def_id)
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

    /// Attaches (or replaces) the server link in place -- for a front that learns
    /// its audio server after construction (the browser connecting a WebSocket
    /// leg on demand).
    pub fn set_server_link(&mut self, link: ServerLink) {
        self.server = Some(link);
    }

    /// Points the host at the **samples** of a shared segment: from here a
    /// take is drawn from the mapped region and a stroke is stored into it,
    /// with nothing sent either way.
    ///
    /// The host must be entitled to write it, which in this design means it is
    /// the owner of the document those takes belong to (its own session's
    /// samples, or a server it was explicitly pointed at). A host that is a
    /// guest on somebody else's document keeps sending intents and waiting for
    /// the acknowledgement -- that machinery answers *may I*, and this changes
    /// only how the samples get there.
    #[cfg(unix)]
    pub fn set_shared_buffers(&mut self, samples: mapped::SharedBuffers) {
        self.shared_buffers = Some(samples);
    }

    /// The mapped samples, when the host has any.
    #[cfg(unix)]
    pub fn shared_buffers(&self) -> Option<&mapped::SharedBuffers> {
        self.shared_buffers.as_ref()
    }

    /// Attaches the server that makes sound, when it is a different one from
    /// the server that holds the samples (see the `player` field).
    pub fn set_player_link(&mut self, link: ServerLink) {
        self.player = Some(link);
    }

    /// The player, only when one was attached apart from the server leg -- the
    /// link whose replies a front reads on its own.
    pub fn player_link(&self) -> Option<&ServerLink> {
        self.player.as_ref()
    }

    /// The link everything audible goes out of: the player when one was
    /// attached, otherwise the ordinary server leg.
    pub fn player(&self) -> Option<&ServerLink> {
        self.player.as_ref().or(self.server.as_ref())
    }

    /// Attaches the GuiDef store (named GuiDefs auto-persist; `/gui_load` reads
    /// from it).
    pub fn with_store<S: DefStore + 'static>(mut self, store: S) -> Self {
        self.store = Some(Box::new(store));
        self
    }

    /// Loads the typeface `source` offers, if it offers one and the rasterizer
    /// reads it -- returning whether text now draws through the glyph atlas.
    ///
    /// It is the host that asks, and it asks **once**: a face is a property of
    /// the build (one `--font`, one fetched URL), not of a window, so every
    /// window that opens afterwards draws with it and no size table changes.
    /// A refusal is silent to the drawing code -- the bitmap face keeps
    /// drawing -- and the caller logs it.
    #[cfg(feature = "font-atlas")]
    pub fn load_face(&mut self, source: &dyn FontSource) -> bool {
        source
            .face()
            .is_some_and(|bytes| font::atlas::set_face(&bytes))
    }

    /// The GuiDef store, if persistence was configured.
    pub fn store(&self) -> Option<&dyn DefStore> {
        self.store.as_deref()
    }

    /// The ids of the currently-defined window GuiDefs -- for the standalone
    /// front to open a pre-loaded def on resume, and for a pass that has to
    /// visit every tree (a broadcast the host is a consumer of rather than the
    /// addressee of).
    pub fn window_def_ids(&self) -> Vec<i32> {
        self.window_defs.keys().copied().collect()
    }

    /// The size table window `def_id` lays out and paints with: this host's
    /// logical [`metrics`](Self::metrics) resolved to that window's physical
    /// pixels. Every layout, paint and hit-test site of a window reads *this*
    /// one, never the logical table -- a document can sit on a HiDPI screen
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
    /// the page's `devicePixelRatio` -- a platform reading this core may not
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

    /// Whether window `id`'s owner plays it (the `plays` prop): the space bar
    /// is then the window's own verb, and the monitor stays out.
    pub fn window_plays(&self, id: i32) -> bool {
        matches!(
            self.window_def(id).map(|w| &w.kind),
            Some(WidgetKind::Window { plays: true, .. })
        )
    }

    /// **The window's `play` verb**, as it goes out: the verb and the loop
    /// switch (`L`) beside it, `1` or `0` -- an owner that plays its own take
    /// reads how the pass ends from it, and one that does not ignores it.
    pub fn play_verb(&self) -> Vec<OscType> {
        vec![
            OscType::String(clausters_apps::multitrack::editor::PLAY_KEY.into()),
            OscType::Int(i32::from(self.monitor_loops())),
        ]
    }

    /// The inner size window `id` asks its shell for, in **logical** pixels:
    /// what its `w`/`h` declared, or -- when it carries `hug` -- what its content
    /// wants ([`Widget::hug_size`]) on the axes where that composition is
    /// defined, keeping the declared number on the others.
    ///
    /// Measured against the host's logical table at scale 1, because that is
    /// the space a window's size is declared in and resolving at scale 1 is the
    /// identity; the window's own `ui_scale` is the shell's business and only
    /// exists once the window does. Which is exactly why a hugging window is
    /// asked **twice** -- see [`window_size_px`](Self::window_size_px), the
    /// exact answer once there is a scale to resolve against.
    ///
    /// A shell with a window of its own reads this when it creates one. In a
    /// page there is none -- the element owns its box and reports its pixels --
    /// so the browser front lays the same tree out inside whatever box it is
    /// given, and only the *containers* inside it hug. That is a platform
    /// truth, not a fork: the composition is identical in both builds.
    pub fn window_size(&self, id: i32) -> Option<(u32, u32)> {
        self.sized_window(id, &self.metrics, 1.0)
    }

    /// The inner size a **hugging** window wants in **physical** pixels, at its
    /// own resolved size table -- `None` for a window that declares its size, so
    /// a shell only resizes what asked to be fitted.
    ///
    /// The second half of one question. A window is created before anything
    /// knows what display it landed on, so the first answer is measured in the
    /// logical table; the resolved table then **snaps every role to a whole
    /// pixel**, and on a fractional scale the two disagree by a pixel or two --
    /// enough for a label measured one way and drawn the other to ellipsize
    /// inside the box that was supposed to fit it (found by eye at 1.25). So
    /// the shell asks again as soon as it has written the scale, and this
    /// answer is exact by construction: it is measured with the very table the
    /// layout will use.
    pub fn window_size_px(&self, id: i32) -> Option<(u32, u32)> {
        let tree = self.window_def(id)?;
        if !matches!(tree.kind, WidgetKind::Window { hug: true, .. }) {
            return None;
        }
        let metrics = self.metrics_for(id);
        self.sized_window(id, metrics, metrics.ui_scale)
    }

    /// One window's requested size, measured with `metrics` at `scale`: its
    /// content where it hugs and the composition settles, its declared `w`/`h`
    /// everywhere else (scaled to the same space, so the two mix cleanly).
    fn sized_window(&self, id: i32, metrics: &metrics::Metrics, scale: f32) -> Option<(u32, u32)> {
        let tree = self.window_def(id)?;
        let WidgetKind::Window {
            width, height, hug, ..
        } = &tree.kind
        else {
            return None;
        };
        let declared = |v: u32| ((v as f32) * scale).ceil().max(1.0) as u32;
        if !*hug {
            return Some((declared(*width), declared(*height)));
        }
        let round = |want: Option<f32>, fallback: u32| {
            want.map_or_else(|| declared(fallback), |v| (v.ceil().max(1.0) as u32).max(1))
        };
        let (w, h) = tree.hug_size(metrics, scale);
        // A window fitted to its content is fitted to the bar as well, or the
        // bar would be taken out of the content it was measured to hold and a
        // hugging window would open one line short of what it asked for.
        let h = h.map(|h| h + status::bar_h(tree, metrics));
        Some((round(w, *width), round(h, *height)))
    }

    /// Mutable access to a window document, for the front to write back a value
    /// a user interaction produced (a turned knob, a moved slider).
    pub fn window_def_mut(&mut self, id: i32) -> Option<&mut Widget> {
        self.window_defs.get_mut(&id)
    }

    /// The typed kind of widget `widget_id` inside window `def_id` -- the whole
    /// of what an interaction addresses, since a gesture reaches a widget by
    /// the pair of ids the wire gave it and then matches on what it is.
    ///
    /// Spelling the walk out (`window_def(def_id)?.find(widget_id)?.kind`) is
    /// what the interaction layer did at every one of its doors; this is that
    /// walk, named once.
    pub fn widget_kind(&self, def_id: i32, widget_id: i32) -> Option<&WidgetKind> {
        Some(&self.window_def(def_id)?.find(widget_id)?.kind)
    }

    /// [`widget_kind`](Self::widget_kind), mutably -- the write half of an edit.
    pub fn widget_kind_mut(&mut self, def_id: i32, widget_id: i32) -> Option<&mut WidgetKind> {
        Some(&mut self.window_def_mut(def_id)?.find_mut(widget_id)?.kind)
    }

    /// Window `def_id`'s tree laid out over a `fb_w` x `fb_h` framebuffer, on
    /// the **same** time axes the renderer drew it on -- every timeline widget
    /// resolved against its navigation group.
    ///
    /// That agreement is the point: a clip is hit on the pixels it was drawn
    /// on, so hit-testing must not re-derive an axis the frame already chose.
    /// The renderer runs the same call with its own metrics and groups
    /// (`frame::render`), which is why this takes neither from a caller.
    pub(crate) fn layout_window(
        &self,
        def_id: i32,
        fb_w: u32,
        fb_h: u32,
    ) -> Option<Vec<layout::Placed<'_>>> {
        let tree = self.window_def(def_id)?;
        let metrics = self.metrics_for(def_id);
        let area = self.content_area(def_id, fb_w, fb_h);
        Some(layout::layout_on(area, tree, metrics))
    }

    /// The framebuffer of window `def_id` **minus its status bar** -- the area
    /// its tree is laid out in.
    ///
    /// One function because two passes read it: the renderer draws the tree in
    /// it ([`frame::render`]) and the hit test places the tree in it
    /// ([`layout_window`](Self::layout_window)). A bar drawn over pixels the
    /// layout also handed out would swallow presses meant for the widget under
    /// it; a bar the layout avoided and the frame did not draw would be a strip
    /// of dead window.
    pub(crate) fn content_area(&self, def_id: i32, fb_w: u32, fb_h: u32) -> layout::Rect {
        let area = layout::Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let Some(tree) = self.window_def(def_id) else {
            return area;
        };
        status::content(
            tree,
            self.status.borrow().get(&def_id),
            area,
            self.metrics_for(def_id),
        )
    }

    /// The status bar's band in window `def_id`'s framebuffer, when it has one
    /// -- what a press is tested against before the tree is.
    pub(crate) fn status_bar_rect(
        &self,
        def_id: i32,
        fb_w: u32,
        fb_h: u32,
    ) -> Option<layout::Rect> {
        let area = layout::Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
        let tree = self.window_def(def_id)?;
        status::bar(
            tree,
            self.status.borrow().get(&def_id),
            area,
            self.metrics_for(def_id),
        )
    }

    /// **What the server records and streams for the open trees**, re-diffed:
    /// the bus taps the views read and the recordings they follow. Run
    /// wherever what is drawn can have changed.
    fn sync_subscriptions(&mut self) {
        self.sync_bus_watches();
        self.sync_buffer_streams();
    }

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
                self.send_to_player(watch_msg(*bus, true));
            }
        }
        for bus in &self.watched_buses {
            if !wanted.contains(bus) {
                self.send_to_player(watch_msg(*bus, false));
            }
        }
        self.watched_buses = wanted;
    }

    /// Sends one message to the server that **makes sound**: the player.
    ///
    /// Playing a take, sounding a key, rolling the transport and recording a
    /// bus are all addressed to whoever holds the audio device, which is not
    /// always the server that holds the samples. With no separate player
    /// attached the two are the same server and this is the leg it always was.
    fn send_to_player(&self, msg: OscMessage) {
        let Some(link) = self.player.as_ref().or(self.server.as_ref()) else {
            return;
        };
        if let Err(e) = link.send(msg) {
            diag::warn!("cannot send to the audio server: {e}");
        }
    }
}

/// `/bus_tap bus watch`: what the host sends the audio server to start or stop
/// recording an audio bus. The bus is the whole address -- the server picks and
/// publishes where the samples land.
fn watch_msg(bus: i32, watch: bool) -> OscMessage {
    OscMessage {
        addr: "/bus_tap".into(),
        args: vec![OscType::Int(bus), OscType::Int(if watch { 1 } else { 0 })],
    }
}

/// Appends every audio bus whose samples a tree's views read, deduplicated.
fn collect_audio_buses(tree: &Widget, out: &mut Vec<i32>) {
    let mut mine = Vec::new();
    for widget in tree.descendants() {
        mine.extend(widget.kind.needs().taps);
    }
    for bus in mine {
        if bus >= 0 && !out.contains(&bus) {
            out.push(bus);
        }
    }
}
