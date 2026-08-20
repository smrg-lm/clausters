//! The OSC server: the network thread, and everything that answers a client.
//!
//! Allocating and doing I/O here is fine — this is the side of the seam the
//! audio thread is protected from. It owns the [`EngineHandle`] and the def
//! store: defs are compiled and stored here, node commands are built here in
//! full (boxed synth included) and pushed to the engine's command FIFO, and the
//! garbage coming back from the audio thread is dropped here. Replies follow
//! scsynth semantics (see the `scsynth-osc` skill).
//!
//! The command set itself is not listed here — `docs/schemas.md` is its
//! reference, and a copy of the list in a doc comment is a copy that goes
//! stale. What this module holds is [`OscServer`]: the struct, its shared
//! helpers (`OscServer::reply`, `OscServer::fail`, the two clocks behind
//! `OscServer::mono_secs`) and the types the rest of it passes around.
//!
//! # Where things live
//!
//! - `lifecycle` — construction, the run loop, and its housekeeping.
//! - `transports` — binding and draining UDP, TCP, WebSocket, MIDI and the
//!   shared-memory ring, all of them into one handler.
//! - `dispatch` — address to handler, plus the two ways a message arrives
//!   early (a timetagged bundle, the `/sched_at` family).
//! - `commands` — the handlers, one module per resource family.
//! - `streams` — the subscriptions the server pushes without being asked
//!   again.
//! - `async_pipes` — the NRT and Faust pipelines, and the `/server_sync`
//!   barrier over both.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType, encoder};
use tracing::{error, info, warn};

use crate::dsp::ReplyKind;
#[cfg(feature = "faust")]
use crate::faust::compiler::{CacheJob, CompilePayload, CompileRequest, CompilerThread};
use crate::node::MAX_NODES;
use crate::osc::ClientId;
use crate::osc::translate::{
    CmdTranslator, control_key, float_value, parse_buffer_gen, parse_buffer_msg,
};

use crate::server::clock_axis::TransportSample;
use crate::server::defstore::{self, DefKind, DefStore};
use crate::server::engine::cmd_target_nodes;
use crate::server::engine::{Cmd, EngineHandle, Garbage, NodeEventKind};
use crate::server::nrt::{NrtAction, NrtJob, NrtRequest, NrtRunner};

mod args;
mod async_pipes;
mod commands;
mod dispatch;
mod lifecycle;
mod overviews;
mod streams;
mod transports;

/// The addresses the server answers, in the order the dispatch table holds
/// them (sorted). Exposed so a test can compare the command set against
/// `docs/schemas.md`; there is no other way to enumerate it, which is the
/// point of the table being data.
pub fn commands() -> Vec<&'static str> {
    dispatch::COMMANDS.iter().map(|(addr, _)| *addr).collect()
}

use args::{Answer, Args};

/// Default scsynth port.
pub const DEFAULT_PORT: u16 = 57110;

/// Largest UDP datagram we accept.
const RECV_BUF_SIZE: usize = 65536;

/// How long `recv_from` blocks before we take a garbage-collection pass.
const GC_INTERVAL: Duration = Duration::from_millis(100);

/// Fastest `/bus_stream` period a client can ask for (faster requests are
/// clamped, not failed): ~3x the interactive 30 Hz a GUI meter needs, and a
/// bound on how much reply traffic one client can subscribe to.
const MIN_STREAM_PERIOD: Duration = Duration::from_millis(10);

/// Most bus indices one `/bus_stream` subscription may list: 128 (index, value)
/// pairs fit comfortably in a single frame on every transport.
const MAX_STREAM_BUSES: usize = 128;

/// Most tap indices one `/bus_tapStream` subscription may list — one `/bus_tapStream.reply`
/// blob goes out per tap per period, so this bounds the reply traffic.
const MAX_STREAM_TAPS: usize = 8;

/// Ceiling on buffers per `/buffer_stream` subscription — a session draws a
/// handful of takes at once, and a client that wants more takes two.
const MAX_STREAM_BUFFERS: usize = 32;

/// Ceiling on the **bytes** of one overview reply (`/buffer_stream.reply`,
/// `/buffer_peaks.reply`), so a subscription that stalled — or a request for a
/// long take — does not answer in one message.
///
/// It is a byte count and not a bucket count because a bucket is not a size: a
/// bucket costs `channels * 3` floats, so 4096 of them are 96 kB of a stereo
/// take and 400 kB of an eight-channel one. The carriers have real limits and
/// the smallest of them is the shared ring's **64 KiB**, which drops what does
/// not fit *silently* — a page that asked for a summary and got nothing back,
/// forever, is exactly what this ceiling exists to prevent. An eighth of the
/// ring leaves room for the several replies in the air while a session draws,
/// which is the same reasoning that sizes a client's `/buffer_getRange` chunk.
///
/// What is left over is asked for again (`/buffer_peaks`, whose reply says
/// where it ended) or sent by the next report (`/buffer_stream`, whose
/// frontier is where the report ended and not where the samples end).
const MAX_STREAM_BYTES: usize = 8 * 1024;

/// The above as a bucket count, for a buffer of `channels` channels: each
/// bucket carries `min`, `max` and mean square per channel.
fn max_stream_buckets(channels: usize) -> usize {
    (MAX_STREAM_BYTES / (channels.max(1) * 3 * 4)).max(1)
}

/// Largest `/bus_tapStream` window in samples for a **datagram-bounded** client
/// (UDP, and the 64 KiB IPC reply ring): a 32 KB blob (8192 × `f32`) leaves
/// room for the OSC envelope. A stream client (TCP/WebSocket) is bounded by
/// the configurable frame ceiling instead. Every window is also clamped
/// to half the tap ring, the `tap_read_latest` tear-free bound.
const MAX_TAP_WINDOW: usize = 8192;

/// Information reported in `/server_status.reply` that does not come from the
/// engine counters.
pub struct ServerInfo {
    pub nominal_sample_rate: f64,
    pub actual_sample_rate: f64,
}

enum Flow {
    Continue,
    Quit,
}

/// One queued `/buffer_render`: run the graph for `frames` frames and install
/// the result in buffer `index`.
///
/// It exists because the two halves of that command live in different places
/// and must stay there. The **server** owns the wire — it parses the message,
/// validates the buffer and answers `/done` or `/fail` — and the **driver**
/// owns the engine, because only whoever calls `process_block` can run one.
/// That is the same split the NRT queue already has, with the driver in the
/// worker's seat, and it is why the command is a request rather than a call.
#[derive(Debug, Clone, Copy)]
pub struct OfflineRender {
    /// The buffer the result is installed into. Already validated as
    /// allocated when the request was queued.
    pub index: usize,
    /// How many frames to run. Never zero.
    pub frames: u64,
    /// Who asked, so the answer goes back to them.
    client: ClientId,
}

pub struct OscServer {
    /// The UDP front. `None` for a headless pulled server ([`Self::headless`]):
    /// commands come only through the attached ring, replies only through it,
    /// and the host drives the loop by calling [`Self::step`].
    socket: Option<UdpSocket>,
    info: ServerInfo,
    handle: EngineHandle,
    /// Def tables, node→def mirror and message→command translation, shared
    /// with the NRT renderer (see [`crate::osc::translate`]).
    /// Owns the network-side buffer mirror (`translator.buffers`), updated
    /// when NRT results are installed: serves `/buffer_query` and gives `/buffer_read`,
    /// `/buffer_write` and `/buffer_zero` the current contents/shape, and a Faust
    /// instance its `soundfile` data.
    translator: CmdTranslator,
    nrt: NrtRunner,
    /// Clients registered via `/server_notify 1`; the client ID is index + 1.
    clients: Vec<ClientId>,
    /// Active `/bus_stream` subscriptions, at most one per client: the network
    /// counterpart of the shared-memory control-bus segment, for clients (a
    /// browser) that cannot map it. Pumped by the run loop.
    streams: Vec<BusStream>,
    /// Active `/bus_tapStream` subscriptions, at most one per client: the same
    /// network counterpart for the audio-tap rings. Pumped by the run loop.
    tap_streams: Vec<TapStream>,
    /// Active `/buffer_stream` subscriptions, at most one per client: the
    /// **overview** of samples as it is written, for a client that cannot map
    /// the region and watch it fill. Pumped by the run loop.
    buffer_streams: Vec<BufferStream>,
    /// Which audio bus each tap ring is recording (`-1` = free), and how many
    /// watchers asked for it. **The server owns the rings**: a client names a
    /// bus and never an index, so this table is the whole of the bus -> ring
    /// assignment (its inverse is published in the segment for readers).
    tap_rings: Vec<i32>,
    tap_refs: Vec<u32>,
    /// Scratch window for tap snapshots, sized to the largest subscribed
    /// window; reused across pumps.
    tap_buf: Vec<f32>,
    recv_buf: Vec<u8>,
    /// Where streams and timetags read time from (see [`TimeSource`]).
    clock: TimeSource,
    /// the shared-memory / in-process ring endpoint, when attached.
    ipc: Option<crate::server::ipc::IpcPeer>,
    /// The segment itself, whether or not this server serves its rings. A
    /// server that attached to somebody else's segment reads the samples out
    /// of it and has no ring at all, so the two cannot be one field.
    segment: Option<std::sync::Arc<crate::server::ipc::Segment>>,
    /// Where the segment's file is, when it has one: what a buffer's region is
    /// named from — on both sides, the server that writes the regions and the
    /// one that maps them.
    shm_path: Option<std::path::PathBuf>,
    /// Whether this server **owns** the samples: publishes a directory row
    /// and a region for every buffer it installs. Exactly one process may, so
    /// it follows the control-plane claim; a server that attached without it
    /// maps what the owner published and keeps its own allocations private.
    owns_samples: bool,
    /// Per buffer, the region file backing it — kept so freeing one can unlink
    /// its name. Sized with the pool. Off Unix there are no regions and the
    /// list stays empty, which is why it is written and never read there.
    #[cfg_attr(not(unix), allow(dead_code))]
    shared_buffers: Vec<Option<std::path::PathBuf>>,
    /// Per buffer, the **overview** beside its region: the peak pyramid a peer
    /// maps instead of summarizing the samples itself, kept current span by
    /// span as writes land. See [`overviews`].
    #[cfg_attr(not(unix), allow(dead_code))]
    overviews: overviews::Overviews,
    /// TCP transport, when `listen_tcp` was called: accepts length-prefixed OSC
    /// connections multiplexed into the same loop. See [`crate::osc::tcp`].
    tcp: Option<crate::osc::tcp::TcpHub>,
    /// WebSocket transport, when `listen_ws` was called: the same OSC encoding
    /// over WebSocket binary messages, reachable from a browser. Multiplexed
    /// into the same loop as TCP. See [`crate::osc::ws`]. Native only: on
    /// wasm32 the engine lives in the page and is fed through the ring.
    #[cfg(not(target_arch = "wasm32"))]
    ws: Option<crate::osc::ws::WsHub>,
    /// Live MIDI input, when `listen_midi` was called: a virtual ALSA port
    /// whose decoded messages the loop drains. See [`crate::midi::live`].
    #[cfg(feature = "midi")]
    midi: Option<crate::midi::live::MidiHub>,
    /// On-disk def persistence, when a data directory is configured. Defs
    /// loaded from it on startup; `/def_send` write to it,
    /// `/def_free` deletes from it.
    store: Option<DefStore>,
    /// Whether a persisted def that will not load is dropped from the store
    /// (`--prune-defs`) instead of warned about. Off by default: a build
    /// missing a def family fails to load one for a reason that is the
    /// build's, not the def's.
    prune_dead_defs: bool,
    /// The compiler thread is owned here and dies with the server.
    #[cfg(feature = "faust")]
    faust_compiler: CompilerThread,
    /// `/server_sync` barrier bookkeeping. Each async pipeline (NRT buffers, Faust
    /// compiles) completes FIFO on its own thread, so a monotonic
    /// submitted/drained counter per pipeline is enough: a `/server_sync` records the
    /// current submitted counts as its targets and is answered with `/server_sync.reply`
    /// once both drained counts have caught up. See [`Self::handle_server_sync`].
    nrt_submitted: u64,
    /// Jobs submitted to the NRT queue and not yet drained, per buffer index.
    /// Non-zero means the network-side mirror is behind the queue for that
    /// buffer, which is what `NrtChain` needs to know (see `submit_nrt`).
    nrt_in_flight: std::collections::HashMap<i32, u32>,
    nrt_drained: u64,
    faust_submitted: u64,
    faust_drained: u64,
    pending_syncs: Vec<PendingSync>,
    /// The shared beat grid (`/transport_set`), once a client defines one.
    transport: Transport,
    /// `/server_errorMode` mode: post command failures to the server console. The `/fail`
    /// OSC reply is always sent; this only gates the console logging. On by
    /// default (matches scsynth's default error-posting).
    post_errors: bool,
    /// Frame ceiling for the stream transports (TCP/WebSocket), in bytes
    /// (`--max-frame`, default [`crate::osc::DEFAULT_MAX_FRAME`]). Bounds what
    /// the hubs accept and what transport-aware replies (the `/bus_tapStream`
    /// window) may grow to; advertised in `/server_query.reply` so clients size
    /// their requests from it. UDP keeps the datagram cap regardless.
    max_frame: usize,
    /// Ceiling for concurrent stream clients, TCP + WebSocket combined
    /// (`--max-clients`, default [`crate::osc::DEFAULT_MAX_CLIENTS`]).
    max_clients: usize,
    /// The live-client slots both stream fronts share, created when the first
    /// of them binds (so `set_max_clients` can still change the ceiling).
    client_slots: Option<std::sync::Arc<crate::osc::ClientSlots>>,
    /// Queued `/buffer_render` operations, when an offline driver has said it
    /// will perform them ([`Self::enable_offline_renders`]). `None` is every
    /// other server, and the command fails there rather than queueing work
    /// nobody will do — see [`OfflineRender`].
    offline: Option<Vec<OfflineRender>>,
}

/// The shared transport: a DAW-style **rolling state** (play / stop / where the
/// piece is), plus an optional beat grid clients read to phase-align on the
/// master sample clock. The server stores and **broadcasts** all of it
/// (in-memory; resets on restart); with a group bound the engine also enforces
/// it. See [`OscServer::handle_transport`].
///
/// **It always exists**, which is why this is not an `Option` on the server.
/// Rolling, stopping and saying where the piece is need no beats — an audio
/// editor addresses frames and has no tempo to declare — so the thing that is
/// optional is the **grid**, not the transport. `defined` is what says whether
/// `origin_sample` and `tempo` mean anything, and it is the wire field of the
/// same name.
#[derive(Clone, Copy, Default)]
struct Transport {
    /// Whether a client has defined the beat grid (`/transport_set`). Until one
    /// has, the two fields below are 0 and only the commands that speak beats
    /// refuse.
    defined: bool,
    /// Beat `b` maps to sample `origin_sample + b·rate/tempo`. Meaningless
    /// while `defined` is false.
    origin_sample: i64,
    tempo: f64,
    playing: bool,
    /// The song position in **beats** — the grid's spelling of the piece's
    /// position, which the engine keeps in samples. 0 while no grid is defined,
    /// where the sample spelling is still live.
    position: f64,
    /// The loop the position wraps inside, in **samples of the piece**, or
    /// `None` when looping is off. Held here as well as in the engine so
    /// `/transport_query` can report it without asking the audio thread.
    loop_span: Option<(i64, i64)>,
    /// The group the transport governs, when one is bound (`/transport_group`).
    ///
    /// This is what separates the transport's two intensities. With no group
    /// bound it is what it has always been: a grid plus a rolling state the
    /// server stores and broadcasts, which clients obey by choice. With a group
    /// bound, the engine enforces it -- `/transport_stop` freezes that subtree
    /// and the transport clock, `/transport_play` thaws them.
    group: Option<i32>,
}

/// One client's `/bus_stream` subscription: which control buses it watches and
/// when its next `/bus_set` snapshot is due.
struct BusStream {
    client: ClientId,
    period: Duration,
    buses: Vec<i32>,
    /// In [`OscServer::mono_secs`] seconds (wall or sample time).
    next_due: f64,
}

/// One client's `/bus_tapStream` subscription: which audio taps it watches, the
/// window size of each `/bus_tapStream.reply` snapshot, and when the next one is due.
struct TapStream {
    client: ClientId,
    period: Duration,
    /// Snapshot window in samples (≤ [`MAX_TAP_WINDOW`], ≤ half the tap ring).
    frames: usize,
    /// The audio buses this subscription watches. It holds a watch on each for
    /// its lifetime, so a streaming client never issues `/bus_tap` itself.
    buses: Vec<i32>,
    /// In [`OscServer::mono_secs`] seconds (wall or sample time).
    next_due: f64,
}

/// One client's `/buffer_stream` subscription: which buffers it watches, how
/// far each has been reported, and when the next report is due.
///
/// What it carries is the **summary** and not the samples: the buckets a
/// recording produced since the last report, which is what a picture of it
/// needs and is two orders of magnitude smaller than the audio.
struct BufferStream {
    client: ClientId,
    period: Duration,
    /// The buffers this subscription watches, each with the frame its last
    /// report ended at — so a report carries what is new and nothing else.
    buffers: Vec<(i32, u64)>,
    /// Samples per bucket, the pyramid's own level-0 granularity.
    bucket: usize,
    /// In [`OscServer::mono_secs`] seconds (wall or sample time).
    next_due: f64,
}

/// A `/server_sync` waiting for the async pipelines to drain up to its targets.
struct PendingSync {
    client: ClientId,
    id: i32,
    nrt_target: u64,
    faust_target: u64,
}

/// Where the server reads time from. `Wall` is the native default: streams
/// pace on the monotonic clock and NTP timetags convert through the system
/// wall clock, as always. `Sample` is the headless/pulled mode: both
/// derive from the **engine sample clock** — the only clock a wasm build has,
/// and the natural one for a host that drives `process_block` itself (an
/// offline host makes streams and timetags follow render time, not wall
/// time). `unix_epoch` anchors sample 0 on the Unix axis so wall-clocked
/// clients' timetags still land correctly.
enum TimeSource {
    /// Monotonic seconds since `epoch` (construction time).
    Wall { epoch: Instant },
    /// Seconds = engine sample clock / sample rate.
    Sample { unix_epoch: f64 },
}

impl OscServer {
    /// Monotonic seconds for stream pacing: wall time natively, engine sample
    /// time in the headless mode (so an offline drive paces streams in render
    /// time — deterministic, and the only clock wasm has).
    fn mono_secs(&self) -> f64 {
        match &self.clock {
            TimeSource::Wall { epoch } => epoch.elapsed().as_secs_f64(),
            TimeSource::Sample { .. } => {
                self.handle.current_samples() as f64 / self.handle.sample_rate as f64
            }
        }
    }

    /// Unix seconds for NTP timetag conversion (`/clock_query.reply`, bundle
    /// scheduling): the system wall clock natively; in the headless mode the
    /// sample axis anchored at `unix_epoch`, so the advertised clock anchor
    /// and incoming timetags stay mutually consistent.
    fn unix_secs(&self) -> f64 {
        match &self.clock {
            TimeSource::Wall { .. } => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            TimeSource::Sample { unix_epoch } => {
                unix_epoch + self.handle.current_samples() as f64 / self.handle.sample_rate as f64
            }
        }
    }

    /// Sets the stream-transport frame ceiling (`--max-frame`), the largest
    /// OSC frame accepted from — and sent to — a TCP or WebSocket client.
    /// Clamped to at least the UDP receive buffer, so no transport ever
    /// carries less than a datagram. Call before [`Self::listen_tcp`] /
    /// [`Self::listen_ws`]: the hubs capture the ceiling when they bind.
    pub fn set_max_frame(&mut self, bytes: usize) {
        self.max_frame = bytes.max(RECV_BUF_SIZE);
    }

    /// Sets the ceiling for concurrent stream clients, TCP + WebSocket
    /// combined (`--max-clients`); a connection past it is dropped at accept.
    /// Call before [`Self::listen_tcp`] / [`Self::listen_ws`]: the shared
    /// slot pool is created when the first front binds.
    pub fn set_max_clients(&mut self, n: usize) {
        self.max_clients = n.max(1);
    }

    /// Declares that an offline driver owns the engine and will perform
    /// `/buffer_render` operations, which is what makes that command legal on
    /// this server. Without it the command fails, because the server can parse
    /// and answer a render but cannot *run* one: only whoever drives
    /// `Engine::process_block` can, and in real time that is the audio device.
    /// See `server::nrtsession`, the one caller.
    pub fn enable_offline_renders(&mut self) {
        self.offline.get_or_insert_with(Vec::new);
    }

    /// The oldest queued render, for the driver to perform. The driver must
    /// answer it with [`Self::finish_offline_render`], the way the NRT queue's
    /// results are collected and replied to.
    pub fn take_offline_render(&mut self) -> Option<OfflineRender> {
        let queue = self.offline.as_mut()?;
        if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        }
    }

    /// Answers a render the driver has performed: `/done /buffer_render index`
    /// or `/fail`, to the client that asked.
    pub fn finish_offline_render(&mut self, req: OfflineRender, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => self.reply(
                req.client,
                "/done",
                vec![
                    OscType::String("/buffer_render".into()),
                    OscType::Int(req.index as i32),
                ],
            ),
            Err(e) => self.fail(req.client, "/buffer_render", &e),
        }
    }

    /// Restarts the stochastic-UGen seed sequence, so a caller that resolved a
    /// seed can hand the server the one it will report. The offline renderer
    /// does this on its own translator (`server::render`); an offline *session*
    /// (`server::nrtsession`) needs the same door on the server, and for the
    /// same reason: an operation is only repeatable if its seed is.
    pub fn set_seed(&mut self, seed: u64) {
        self.translator.set_seed(seed);
    }

    /// The live-client slots both stream fronts share, created on first use.
    fn client_slots(&mut self) -> std::sync::Arc<crate::osc::ClientSlots> {
        let max = self.max_clients;
        self.client_slots
            .get_or_insert_with(|| std::sync::Arc::new(crate::osc::ClientSlots::new(max)))
            .clone()
    }

    fn fail(&self, to: ClientId, cmd: &str, why: impl Into<String>) {
        let why = why.into();
        // The console post is gated by `/server_errorMode`; the OSC `/fail` reply always
        // goes out (clients rely on it).
        if self.post_errors {
            warn!(target: crate::logging::OSC_TARGET, "FAILURE {cmd}: {why}");
        }
        self.reply(
            to,
            "/fail",
            vec![OscType::String(cmd.into()), OscType::String(why)],
        );
    }

    fn reply(&self, to: ClientId, addr: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: addr.into(),
            args,
        });
        let bytes = match encoder::encode(&packet) {
            Ok(bytes) => bytes,
            Err(e) => return warn!("failed to encode {addr}: {e}"),
        };
        match to {
            ClientId::Udp(addr_to) => {
                // A headless server has no UDP clients; dropped if so.
                if let Some(socket) = &self.socket
                    && let Err(e) = socket.send_to(&bytes, addr_to)
                {
                    warn!("failed to send {addr} to {addr_to}: {e}");
                }
            }
            ClientId::Tcp(id) => {
                // Length-prefixed reply on the originating connection; dropped
                // if it has since closed.
                if let Some(hub) = &self.tcp {
                    hub.reply(id, &bytes);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            ClientId::Ws(id) => {
                // Binary-message reply on the originating connection; dropped if
                // it has since closed.
                if let Some(hub) = &self.ws {
                    hub.reply(id, &bytes);
                }
            }
            // No WebSocket hub exists on wasm32; the variant is unreachable.
            #[cfg(target_arch = "wasm32")]
            ClientId::Ws(_) => {}
            ClientId::Ring(peer) => {
                // Tagged for the peer that asked, so the embedder's demux hands
                // it to that client and not to whoever else shares the segment.
                // Backpressure, not loss: a full reply ring means the client
                // stopped draining; dropping the reply is all we can do
                // without blocking the server.
                if let Some(ipc) = &self.ipc
                    && !ipc.push(peer, &bytes)
                {
                    warn!("reply ring full: dropping {addr}");
                }
            }
        }
    }
}

/// The raw `SynthDefSpec` JSON of a `/def_send synth` message (blob or string form),
/// for persisting it verbatim. Mirrors the argument parsing in
/// [`CmdTranslator::d_recv`].
/// The `/ugen_query.reply` argument vectors for a `/ugen_query`: the whole catalog
/// when `names` is empty, otherwise one per requested kind — an unknown one
/// coming back with an empty rate set and no inputs, so a batch never fails
/// wholesale (the `/buffer_query` convention).
#[cfg(feature = "synth")]
fn ugen_infos(names: &[String]) -> Vec<Vec<OscType>> {
    if names.is_empty() {
        return crate::dsp::registry::all().map(ugen_info).collect();
    }
    names
        .iter()
        .map(|name| match crate::dsp::registry::lookup(name) {
            Some(d) => ugen_info(d),
            None => vec![
                OscType::String(name.clone()),
                OscType::Int(0),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::Int(0),
                OscType::String(String::new()),
                OscType::String(String::new()),
                OscType::Int(0),
            ],
        })
        .collect()
}

/// One `/ugen_query.reply` argument vector from a catalog descriptor.
///
/// Layout: `name, arity, defaultRate, rates, exec, bus, needsPath, opFamily,
/// spectral, numInputs` then per input `name, default`. `arity` is `-1` for a
/// variadic kind, whose named inputs are its fixed head only. The enum-valued
/// fields are lowercase names, `""` for the "not applicable" variant.
#[cfg(feature = "synth")]
fn ugen_info(d: &crate::dsp::registry::UGenDescriptor) -> Vec<OscType> {
    use crate::dsp::registry::{Arity, BusRole, ExecMode, OpFamily, SpectralRole};
    let rates: Vec<&str> = d.rates.iter().map(|r| r.as_str()).collect();
    let mut args = vec![
        OscType::String(d.name.into()),
        OscType::Int(match d.arity {
            Arity::Fixed(n) => n as i32,
            Arity::Variadic => -1,
        }),
        OscType::String(d.default_rate.as_str().into()),
        OscType::String(rates.join(",")),
        OscType::String(
            match d.exec {
                ExecMode::Normal => "normal",
                ExecMode::LocalIn => "local_in",
                ExecMode::LocalOut => "local_out",
                ExecMode::DemandDriver => "demand_driver",
                ExecMode::DoneQuery => "done_query",
                ExecMode::Spectral => "spectral",
            }
            .into(),
        ),
        OscType::String(
            match d.bus {
                BusRole::None => "",
                BusRole::Read => "read",
                BusRole::Write => "write",
                BusRole::ReadWrite => "read_write",
            }
            .into(),
        ),
        OscType::Int(d.needs_path as i32),
        OscType::String(
            match d.op_family {
                None => "",
                Some(OpFamily::Unary) => "unary",
                Some(OpFamily::Binary) => "binary",
            }
            .into(),
        ),
        OscType::String(
            match d.spectral {
                SpectralRole::None => "",
                SpectralRole::Source => "source",
                SpectralRole::Filter => "filter",
                SpectralRole::Filter2 => "filter2",
                SpectralRole::Sink => "sink",
            }
            .into(),
        ),
        OscType::Int(d.inputs.len() as i32),
    ];
    for i in d.inputs {
        args.push(OscType::String(i.name.into()));
        args.push(OscType::Float(i.default));
    }
    args
}

fn synthdef_spec_bytes(args: &[OscType]) -> Option<&[u8]> {
    match args.first() {
        Some(OscType::Blob(b)) => Some(b),
        Some(OscType::String(s)) => Some(s.as_bytes()),
        _ => None,
    }
}

/// Seconds between the NTP epoch (1900) and the Unix epoch (1970).
const NTP_UNIX_OFFSET: f64 = 2_208_988_800.0;

/// The current wall-clock instant as an OSC/NTP timetag (seconds since 1900 in
/// a 32-bit count, plus a 32-bit binary fraction) — the inverse of the NTP→Unix
/// math in `timetag_delta_secs`. Published alongside the sample counter in
/// `/clock_query.reply` so a client gets the anchor `(osc_time, sample)` it needs to
/// place its logical OSC time on this server's sample axis.
fn unix_to_ntp(unix: f64) -> OscTime {
    let ntp = unix + NTP_UNIX_OFFSET;
    let seconds = ntp.trunc();
    OscTime {
        seconds: seconds as u32,
        fractional: ((ntp - seconds) * 2f64.powi(32)) as u32,
    }
}

impl OscServer {
    /// The server's current time as an OSC/NTP timetag, from its
    /// [`TimeSource`] (wall natively, the anchored sample axis headless).
    fn now_ntp(&self) -> OscTime {
        unix_to_ntp(self.unix_secs())
    }

    /// Seconds from now until the timetag fires. `None` is the OSC
    /// "immediately" tag (seconds 0, fractional 1 — rosc keeps it verbatim).
    fn timetag_delta_secs(&self, t: OscTime) -> Option<f64> {
        if t.seconds == 0 && t.fractional <= 1 {
            return None;
        }
        let target = t.seconds as f64 - NTP_UNIX_OFFSET + t.fractional as f64 / 2f64.powi(32);
        Some(target - self.unix_secs())
    }
}
