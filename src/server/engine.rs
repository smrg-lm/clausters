//! Processing engine, independent of the audio backend.
//!
//! [`engine_pair`] returns the two halves of the server: the [`Engine`] lives
//! on the audio thread, the [`EngineHandle`] on the network thread. They talk
//! exclusively through lock-free SPSC ring buffers: commands flow in fully
//! pre-built (the audio thread only plugs them in), freed memory flows back
//! out as [`Garbage`] to be dropped on the network side, and node lifecycle
//! events flow out as [`NodeEvent`]s for `/node_start`/`/node_end` notifications.
//!
//! Timed bundles arrive as [`Cmd::Schedule`] carrying an absolute
//! target in samples; the engine keeps them in a pre-allocated queue sorted
//! by time (FIFO for equal times) and executes them **sample-accurately**,
//! splitting the block at each event's offset. The engine publishes its
//! sample counter so the network thread can convert NTP timetags.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use rtrb::{Consumer, Producer, PushError, RingBuffer};

pub use crate::dsp::BLOCK_SIZE;
use crate::dsp::StageMask;
use crate::dsp::buffer::{Buffer, BufferPool, empty_pool_with};
use crate::dsp::{
    Buses, ControlBuses, Limits, NUM_AUDIO_BUSES, NUM_CONTROL_BUSES, ProcessCtx, ReplyMsg,
    TransportCtx,
};
use crate::node::{AddAction, FreedNode, Group, NodeKind, NodeTree, Place, Reject, SynthNode};
use crate::server::clock_axis::{DeviceSample, PositionAnchor, TransportPosition, TransportSample};
use crate::server::device_epoch::DeviceEpoch;
use crate::server::ipc::Segment;
use crate::server::meters::{Meters, Role};
use crate::server::workers::WorkerPool;

const CMD_FIFO_CAPACITY: usize = 1024;
/// Floor for the garbage FIFO; scaled to `2 * max_nodes` at boot so a
/// mass-free of a full tree never spills into the leak path.
const GARBAGE_FIFO_CAPACITY: usize = 1024;
/// Local holding list for when the garbage FIFO is full.
const PENDING_GARBAGE_CAPACITY: usize = 64;
/// Floor for the node-event FIFO; scaled to `2 * max_nodes` at boot. Events
/// stay best-effort, but the id registries recycle off `/node_end`, so the
/// capacity must cover at least one full-tree turnover per drain -- a dropped
/// end event is a client-side id that never comes back.
const EVENT_FIFO_CAPACITY: usize = 2048;
/// Side-effect reply messages (`SendReply`/`SendTrig`/`Poll`) buffered from
/// the audio thread to the network thread; over capacity they drop, best-effort
/// like the node events.
const REPLY_FIFO_CAPACITY: usize = 2048;
/// Pre-allocated capacity of the scheduled-bundle queue; bundles beyond it
/// are rejected (shipped back through the garbage FIFO).
const SCHED_CAPACITY: usize = 1024;

/// Commands are built **completely** on the network thread (including boxed
/// synths and pre-reserved group child lists); applying them on the audio
/// thread never allocates.
pub enum Cmd {
    AddSynth {
        id: i32,
        target: i32,
        action: AddAction,
        synth: Box<dyn SynthNode>,
        /// Bus masks analyzed at build time; the parallel scheduler
        /// partitions stages from this engine-owned copy.
        usage: StageMask,
    },
    AddGroup {
        id: i32,
        target: i32,
        action: AddAction,
        group: Group,
    },
    FreeNode {
        id: i32,
    },
    /// `/group_freeAll`: free all children of a group; the group stays.
    FreeAllInGroup {
        id: i32,
    },
    /// `/group_deepFree`: free all synths in a group and its subgroups.
    DeepFreeGroup {
        id: i32,
    },
    /// `/node_run`: pause (`run = false`) or resume (`true`) a node -- a synth or a
    /// whole group. Makes `DoneAction::PauseSelf` non-terminal.
    RunNode {
        id: i32,
        run: bool,
    },
    /// Rolls (`rolling = true`) or stops (`false`) transport `transport`.
    /// Stopped, its clock holds and its queue cannot fall due; the device
    /// clock is untouched either way.
    ///
    /// Every transport command names its transport, an index below the
    /// server's `--transports`; the network thread refuses any other, and the
    /// engine ignores one that slips past.
    TransportRun {
        transport: usize,
        rolling: bool,
    },
    /// Binds the group transport `transport` governs; `id < 0` unbinds.
    /// Unbinding thaws the group, so no frozen ownerless subtree is left
    /// behind.
    TransportGroup {
        transport: usize,
        id: i32,
    },
    /// Has group `id` follow transport `transport` -- its nodes read it, and it
    /// is not frozen -- or ends that with `id < 0`.
    TransportFollow {
        transport: usize,
        id: i32,
    },
    /// `/transport_locate`: moves the transport's position, leaving both clocks
    /// alone. One store of the anchor -- see `server::clock_axis`.
    TransportLocate {
        transport: usize,
        position: u64,
    },
    /// `/transport_loop`: the span the position wraps inside, `None` to stop
    /// looping. An empty or inverted span is not a loop and is rejected before
    /// it reaches here.
    TransportLoop {
        transport: usize,
        span: Option<Range<u64>>,
    },
    /// `/transport_end`: the end mark, where a rolling transport stops, and
    /// where it is located once it has. `None` clears the mark.
    TransportEnd {
        transport: usize,
        mark: Option<EndMark>,
    },
    /// `/transport_fade`: how long a stop and a play ramp, in samples. `0`
    /// is no ramp: a stop freezes on its sample.
    TransportFade {
        transport: usize,
        samples: u64,
    },
    /// `/node_before` / `/node_after`.
    MoveNode {
        id: i32,
        target: i32,
        place: Place,
    },
    SetControl {
        id: i32,
        index: u32,
        value: f32,
    },
    /// `/node_map` (`audio = false`) / `/node_mapAudio` (`audio = true`): binds a
    /// control to a bus the synth reads at the start of every block, or
    /// `bus = -1` to unbind. RT-safe: it only flips an entry in the synth's
    /// pre-allocated mapping table.
    MapControl {
        id: i32,
        index: u32,
        bus: i32,
        audio: bool,
    },
    /// Installs (`Some`) or removes (`None`) a buffer in the pool. The
    /// buffer arrives fully built by the NRT thread; the replaced one leaves
    /// through the garbage FIFO.
    SetBuffer {
        index: usize,
        buffer: Option<Arc<Buffer>>,
    },
    /// `/bus_set` inside a timed bundle: the immediate form writes the shared
    /// atomics from the network thread, but a scheduled write must land at
    /// its exact sample, so it travels to the audio thread like any command.
    SetControlBus {
        index: usize,
        value: f32,
    },
    /// `/bus_tap`: routes audio bus `bus` into audio-tap ring `tap` of the IPC
    /// segment (the engine appends that bus's block to the ring at the end of
    /// every block); `bus = -1` stops the tap. RT-safe: it only flips an entry
    /// in the engine's pre-allocated tap table.
    SetTap {
        tap: usize,
        bus: i32,
    },
    /// `/node_set` on a control used as a bus index: ships the re-analyzed
    /// masks so the parallel scheduler stays in sync.
    SetUsage {
        id: i32,
        usage: StageMask,
    },
    /// `/group_parallel`: children of this group run in dependency stages on
    /// the worker pool.
    SetGroupParallel {
        id: i32,
        parallel: bool,
    },
    /// A timed bundle: `cmds` execute back to back when the stream reaches
    /// `time` (absolute, in samples), splitting the block at that offset.
    /// Built on the network thread; the spent `Vec` shell returns as
    /// [`Garbage::SpentBundle`].
    Schedule {
        time: u64,
        cmds: Vec<Cmd>,
    },
    /// `/sched_clear`: drop pending timed bundles. Each drained bundle's
    /// `Vec<Cmd>` (with its boxed synths) leaves through the garbage FIFO as
    /// [`Garbage::SpentBundle`], so nothing is freed on the audio thread.
    ///
    /// `only` names one transport whose queue goes **alone**, leaving the
    /// device queue and every other transport's standing: that is what a
    /// client re-cueing a plan after a locate needs, since a transport queue
    /// rides a clock that does not jump, so what was queued for the old
    /// position would otherwise sound there. `None` drops every queue.
    ClearSched {
        only: Option<usize>,
    },
    /// `/node_ugenCmd`: a typed command addressed to one UGen instance inside a synth.
    /// The payload is inline (no heap), so applying it allocates nothing.
    UGenCommand {
        id: i32,
        ugen_index: u32,
        command: crate::dsp::UGenCmd,
    },
}

/// Logs one rejected node at the level its reason deserves.
///
/// A node whose **target is gone** is not a fault: the client aimed at a group
/// that was alive when the bundle was emitted and freed before it landed, which
/// is what re-cueing a pass does -- the emission headroom is a quarter second,
/// and everything queued inside it is aimed at the arrangement being replaced.
/// Dropping it is the right answer, so this says so at `debug` rather than
/// warning about an ordinary transport gesture. Everything else is somebody's
/// mistake or a real limit, and keeps its warning.
pub(crate) fn report_rejected(id: i32, why: Reject, who: &str) {
    if why.is_fault() {
        tracing::warn!("{who} rejected node {id}: {}", why.as_str());
    } else {
        tracing::debug!("{who} dropped node {id}: {}", why.as_str());
    }
}

/// Heap memory leaving the audio thread to be dropped on the network side.
pub enum Garbage {
    FreedSynth {
        id: i32,
        synth: Box<dyn SynthNode>,
    },
    FreedGroup {
        id: i32,
        group: Group,
    },
    /// Command the engine could not apply, and **why** -- a vanished target is
    /// a race the protocol allows, the rest are faults (see `Reject`).
    RejectedSynth {
        id: i32,
        synth: Box<dyn SynthNode>,
        why: Reject,
    },
    RejectedGroup {
        id: i32,
        group: Group,
        why: Reject,
    },
    /// A buffer replaced or removed from the pool; this clone is dropped on
    /// the network side so the deallocation (if it is the last `Arc`) never
    /// happens on the audio thread.
    FreedBuffer(Arc<Buffer>),
    /// The drained shell of an executed scheduled bundle, or a bundle a
    /// [`Cmd::ClearSched`] dropped: either way its heap must be freed on the
    /// network side, and either way it is what the client asked for.
    SpentBundle(Vec<Cmd>),
    /// A bundle the engine **rejected** because the schedule queue was full --
    /// the one case worth a warning, and the reason a cleared bundle is not
    /// one: a clear drops what a client asked to drop, and reporting it as a
    /// rejection made a re-cue look like an overflow.
    RejectedBundle(Vec<Cmd>),
    /// A transport reached its end mark and stopped there, so whoever
    /// mirrors the rolling state learns it without polling. Not garbage, and
    /// carried here for the reason a rejection is: it is the one FIFO the
    /// network thread drains on every turn, idle ticks included.
    TransportEnded {
        transport: usize,
    },
}

/// Where a rolling transport stops (`end`) and where it is located once it
/// has (`back`; `None` leaves it at `end`), both on the transport's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndMark {
    pub end: u64,
    pub back: Option<u64>,
}

/// A timed bundle waiting in the engine's queue.
struct ScheduledBundle {
    time: u64,
    cmds: Vec<Cmd>,
}

/// A timed bundle on a transport's axis. Same shape as [`ScheduledBundle`];
/// the type of `time` is the whole difference, and it is what keeps the two
/// kinds of queue from being fed each other's stamps.
struct ScheduledBundleT {
    time: TransportSample,
    cmds: Vec<Cmd>,
}

/// One transport, as the audio thread holds it.
///
/// Every transport is independent: its own rolling state, its own clock (the
/// device clock minus what *it* has spent stopped), its own position, loop,
/// end mark, governed group and queue. They share nothing but the device
/// clock they are all measured from, so stopping one moves nothing about
/// another.
struct TransportState {
    /// Whether it rolls. Stopped, `frozen_total` accumulates and its clock
    /// holds.
    rolling: bool,
    /// Total samples it has spent stopped since boot. The whole of the
    /// device -> transport conversion (see `server::clock_axis`).
    frozen_total: u64,
    /// The group it governs, frozen while it is stopped.
    group: Option<i32>,
    /// The group that follows it: its nodes read this transport and are
    /// never frozen by it -- what sits beside the governed group and must go
    /// on running while it is stopped.
    follow: Option<i32>,
    /// Where it stands, as an anchor onto its clock: the position a locate put
    /// it at, and the transport sample that locate landed on. A read is one
    /// add, so the position costs the per-sample path nothing.
    position: PositionAnchor,
    /// The span the position wraps inside while looping. Always non-empty:
    /// an empty or inverted span is refused before it reaches the engine, and
    /// the wrap would not terminate over one.
    looping: Option<Range<u64>>,
    /// The end mark: where it stops when rolling and no loop is set.
    end: Option<EndMark>,
    /// Pending timed bundles on its axis. Frozen with it: while stopped
    /// nothing here can fall due, and nothing here is rewritten. Pre-allocated
    /// like the device queue, to the same capacity.
    sched: Vec<ScheduledBundleT>,
    /// Where inside the current block its frozen run began, if it is stopped
    /// -- scratch for [`Engine::process_block`], `None` outside it.
    frozen_from: Option<usize>,
    /// How long a stop and a play ramp, in samples (`/transport_fade`); `0`
    /// is no ramp, and a stop freezes on its own sample.
    fade_len: u64,
    /// The declick level `TransportFade` reads: `1` rolling, `0` stopped, and
    /// a ramp between the two across a stop's stopping phase and a play.
    fade: Ramp,
    /// A stop in its stopping phase: the transport still rolls, and freezes
    /// when the ramp reaches zero.
    stopping: Option<Stopping>,
}

/// **The declick level**, as a straight line over device samples: `from` at
/// `start`, `to` from `start + len` on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ramp {
    from: f32,
    to: f32,
    start: u64,
    len: u64,
}

impl Ramp {
    /// A level that does not move.
    const fn level(level: f32) -> Self {
        Self {
            from: level,
            to: level,
            start: 0,
            len: 0,
        }
    }

    /// From the level at `device` to `to`, over `len` samples.
    fn toward(self, device: u64, to: f32, len: u64) -> Self {
        Self {
            from: self.at(device),
            to,
            start: device,
            len,
        }
    }

    /// The level at device sample `device`.
    fn at(&self, device: u64) -> f32 {
        let done = device.saturating_sub(self.start);
        if done >= self.len {
            return self.to;
        }
        self.from + (self.to - self.from) * (done as f32 / self.len as f32)
    }

    /// How much the level moves per sample at `device`: `0` once it has
    /// arrived.
    fn step(&self, device: u64) -> f32 {
        if device >= self.start + self.len {
            0.0
        } else {
            (self.to - self.from) / self.len as f32
        }
    }
}

/// A stop ramping out: the device sample the transport freezes on, where it
/// goes back to once it has, and whether the end mark caused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stopping {
    at: u64,
    /// Where the position is located on the freeze: the end mark's return,
    /// or a locate that arrived during the ramp -- which lands there rather
    /// than mid-ramp, so the fade plays what was playing and the position
    /// rests where it was sent.
    back: Option<u64>,
    /// Whether the end mark caused it: the engine then reports the end.
    ended: bool,
}

/// What a transport's next edge is, when it is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    /// Its stopping phase is over: freeze.
    Freeze,
    /// The loop's end: wrap.
    Wrap,
    /// The end mark, with no ramp: stop on it.
    End,
    /// Its ramp's length before the end mark: start the stopping phase, so
    /// it freezes on the mark.
    EndRamp,
}

impl TransportState {
    fn new() -> Self {
        Self {
            rolling: false,
            frozen_total: 0,
            group: None,
            follow: None,
            position: PositionAnchor::default(),
            looping: None,
            end: None,
            sched: Vec::with_capacity(SCHED_CAPACITY),
            frozen_from: None,
            fade_len: 0,
            fade: Ramp::level(0.0),
            stopping: None,
        }
    }

    /// Its clock at device sample `device`.
    fn at(&self, device: u64) -> TransportSample {
        DeviceSample::new(device).to_transport(self.frozen_total)
    }

    /// The device sample at which its position reaches `position`, or
    /// `device` itself when it already has.
    fn reaching(&self, position: u64, device: u64) -> u64 {
        let here = self.at(device);
        self.position
            .reaching(TransportPosition::new(position), here)
            .unwrap_or(here)
            .to_device(self.frozen_total)
            .get()
    }

    /// The edge that cuts a block next, and the device sample it is due on,
    /// when it rolls: the end of a stopping phase first; else the loop's end
    /// while a loop is set; else the end mark -- or, with a ramp, the ramp's
    /// length before it, so the stopping phase ends on the mark. A position
    /// already at or past the mark is due at once: a transport never rolls
    /// past its mark. A loop wins, since the position wraps before it could
    /// reach the mark.
    fn edge_due(&self, device: u64) -> Option<(Edge, u64)> {
        if !self.rolling {
            return None;
        }
        if let Some(stopping) = self.stopping {
            return Some((Edge::Freeze, stopping.at.max(device)));
        }
        if let Some(span) = &self.looping {
            let here = self.at(device);
            return self
                .position
                .reaching(TransportPosition::new(span.end), here)
                .map(|t| (Edge::Wrap, t.to_device(self.frozen_total).get()));
        }
        let mark = self.end?;
        if self.fade_len == 0 {
            return Some((Edge::End, self.reaching(mark.end, device)));
        }
        let ramp = mark.end.saturating_sub(self.fade_len);
        Some((Edge::EndRamp, self.reaching(ramp, device)))
    }

    /// **A stop**: with no ramp, it freezes now; with one, the stopping phase
    /// begins -- the level falls from where it is, over as much of the ramp as
    /// it has left to fall, and the transport freezes when it reaches zero.
    /// Returns whether it froze now.
    fn stop(&mut self, device: u64) -> bool {
        if !self.rolling || self.stopping.is_some() {
            return false;
        }
        if self.fade_len == 0 {
            self.rolling = false;
            self.fade = Ramp::level(0.0);
            return true;
        }
        let level = self.fade.at(device);
        let len = ((level as f64 * self.fade_len as f64).round() as u64).max(1);
        self.fade = self.fade.toward(device, 0.0, len);
        self.stopping = Some(Stopping {
            at: device + len,
            back: None,
            ended: false,
        });
        false
    }

    /// **A play**: a stopping phase is called off and the level rises from
    /// where it had fallen to; a stopped transport rolls and ramps up from
    /// zero. Returns whether it thawed.
    fn play(&mut self, device: u64) -> bool {
        if self.stopping.take().is_some() {
            let level = self.fade.at(device);
            let len = ((1.0 - level as f64) * self.fade_len as f64).round() as u64;
            self.fade = self.fade.toward(device, 1.0, len);
            return false;
        }
        if self.rolling {
            return false;
        }
        self.rolling = true;
        self.fade = Ramp::level(0.0).toward(device, 1.0, self.fade_len);
        true
    }
}

/// One transport's numbers, mirrored for the network thread once per block.
#[derive(Default)]
struct TransportClocks {
    /// Its clock: samples elapsed under it.
    clock: AtomicU64,
    /// Total samples it has spent stopped. Published beside the clock rather
    /// than derived from the device clock minus it, because those are two
    /// separate loads and can straddle a block.
    frozen: AtomicU64,
    /// Where it stands. Not a clock: it jumps and it wraps (see
    /// `server::clock_axis`).
    position: AtomicU64,
}

/// The node a command acts on, if it acts on one. For a node being created it
/// is the **target** it is added relative to -- the node itself does not exist
/// yet, so it cannot be walked.
///
/// Every variant is listed: no catch-all arm, so a `Cmd` added later fails to
/// compile here rather than being silently classified as ungoverned.
pub(crate) fn cmd_target_nodes(cmd: &Cmd) -> [Option<i32>; 2] {
    match cmd {
        // A node being created does not exist yet, so the end that can be
        // walked is where it is going.
        Cmd::AddSynth { target, .. } | Cmd::AddGroup { target, .. } => [Some(*target), None],
        // A move touches **both** ends, and either one being governed governs
        // the bundle. Classifying a move by its source alone would let
        // `/node_before` splice a node into a frozen subtree while it is
        // frozen, while `/node_add` -- the same structural edit -- waited for
        // the resume: two answers to one question, decided by which command the
        // client happened to use.
        Cmd::MoveNode { id, target, .. } => [Some(*id), Some(*target)],
        Cmd::FreeNode { id }
        | Cmd::FreeAllInGroup { id }
        | Cmd::DeepFreeGroup { id }
        | Cmd::RunNode { id, .. }
        | Cmd::SetControl { id, .. }
        | Cmd::MapControl { id, .. }
        | Cmd::SetUsage { id, .. }
        | Cmd::SetGroupParallel { id, .. }
        | Cmd::UGenCommand { id, .. } => [Some(*id), None],
        // No node target: a bus write, a buffer swap, a tap route, the
        // transport's own controls, or a queue-wide operation. These carry no
        // opinion about which axis the bundle belongs to.
        Cmd::TransportRun { .. }
        | Cmd::TransportGroup { .. }
        | Cmd::TransportFollow { .. }
        | Cmd::TransportLocate { .. }
        | Cmd::TransportLoop { .. }
        | Cmd::TransportEnd { .. }
        | Cmd::TransportFade { .. }
        | Cmd::SetBuffer { .. }
        | Cmd::SetControlBus { .. }
        | Cmd::SetTap { .. }
        | Cmd::ClearSched { .. } => [None, None],
        // A nested bundle classifies itself when it is applied, against the
        // tree and the frozen total of that moment; deciding for it here would
        // only duplicate that, at a time when it is not yet due.
        Cmd::Schedule { .. } => [None, None],
    }
}

/// Node lifecycle event for `/node_start`/`/node_end` notifications. POD; delivery is
/// best-effort (dropped silently if the FIFO is full).
#[derive(Clone, Copy, Debug)]
pub struct NodeEvent {
    pub kind: NodeEventKind,
    pub id: i32,
    pub parent_id: i32,
    pub is_group: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeEventKind {
    Go,
    End,
}

/// Counts published by the audio thread (relaxed stores) and read by the
/// network thread for `/server_status.reply`.
pub struct Counters {
    pub synths: AtomicU32,
    pub ugens: AtomicU32,
    pub groups: AtomicU32,
    /// Average DSP load as a fraction of the block budget
    /// (`BLOCK_SIZE / sample_rate` wall time), an EMA with a ~1 s time
    /// constant. `f32` bits in an `AtomicU32`; only meaningful in real time
    /// (NRT renders run unpaced, so the fraction is just render speed).
    pub avg_cpu: AtomicU32,
    /// Highest per-block load since the last [`Counters::take_peak_cpu`]
    /// (`f32` bits; non-negative floats order like their bit patterns, so
    /// `fetch_max` on the bits is a float max).
    pub peak_cpu: AtomicU32,
    /// Blocks whose processing exceeded their real-time budget (cumulative
    /// since boot) -- the engine-side xrun proxy: the callback cannot have met
    /// its deadline for that block unless the host buffered extra latency.
    pub late_blocks: AtomicU32,
}

impl Counters {
    pub fn avg_cpu(&self) -> f32 {
        f32::from_bits(self.avg_cpu.load(Ordering::Relaxed))
    }

    /// Returns the peak per-block load since the previous call and resets it,
    /// so every `/server_status` poll reports the peak of its own window.
    pub fn take_peak_cpu(&self) -> f32 {
        f32::from_bits(self.peak_cpu.swap(0, Ordering::Relaxed))
    }

    pub fn late_blocks(&self) -> u32 {
        self.late_blocks.load(Ordering::Relaxed)
    }
}

/// A [`GarbageSink`] over an engine's own FIFOs. A macro rather than a method
/// because it borrows the three fields one by one, which is what leaves the
/// tree free to be borrowed beside it; a `&mut self` method would take all of
/// the engine.
macro_rules! garbage_sink {
    ($engine:expr) => {
        GarbageSink {
            garbage_tx: &mut $engine.garbage_tx,
            pending_garbage: &mut $engine.pending_garbage,
            events_tx: &mut $engine.events_tx,
        }
    };
}

/// Routes freed nodes to the garbage and event FIFOs. Borrows the individual
/// engine fields so the tree (also a field) can stay mutably borrowed.
struct GarbageSink<'a> {
    garbage_tx: &'a mut Producer<Garbage>,
    pending_garbage: &'a mut Vec<Garbage>,
    events_tx: &'a mut Producer<NodeEvent>,
}

impl GarbageSink<'_> {
    fn consume(&mut self, freed: FreedNode) {
        match freed {
            FreedNode::Synth {
                id,
                parent_id,
                synth,
            } => {
                self.event(id, parent_id, false);
                self.push(Garbage::FreedSynth { id, synth });
            }
            FreedNode::Group {
                id,
                parent_id,
                group,
            } => {
                self.event(id, parent_id, true);
                self.push(Garbage::FreedGroup { id, group });
            }
        }
    }

    /// A node went in: `/node_start` for the notify clients.
    fn started(&mut self, id: i32, parent_id: i32, is_group: bool) {
        let _ = self.events_tx.push(NodeEvent {
            kind: NodeEventKind::Go,
            id,
            parent_id,
            is_group,
        });
    }

    /// The tree refused a new node: it goes back through the garbage FIFO
    /// with the reason, never dropped on this thread.
    fn rejected(&mut self, id: i32, kind: NodeKind, why: Reject) {
        match kind {
            NodeKind::Synth { node: synth, .. } => {
                self.push(Garbage::RejectedSynth { id, synth, why });
            }
            NodeKind::Group(group) => self.push(Garbage::RejectedGroup { id, group, why }),
        }
    }

    fn event(&mut self, id: i32, parent_id: i32, is_group: bool) {
        let _ = self.events_tx.push(NodeEvent {
            kind: NodeEventKind::End,
            id,
            parent_id,
            is_group,
        });
    }

    fn push(&mut self, garbage: Garbage) {
        if let Err(PushError::Full(g)) = self.garbage_tx.push(garbage) {
            if self.pending_garbage.len() < self.pending_garbage.capacity() {
                self.pending_garbage.push(g);
            } else {
                // FIFO and holding list both full. Leaking is the only RT-safe
                // option left: dropping here would free memory on this thread.
                std::mem::forget(g);
            }
        }
    }
}

/// Per-block release factor of the published audio-bus levels: how much a
/// held peak decays each block, so a meter reading at any rate -- a display
/// frame is a dozen blocks -- sees a transient instead of missing it between
/// looks. [`LEVEL_RELEASE_DB_PER_SEC`] dB per second, the usual peak-meter
/// ballistic; a decay (rather than a max the reader clears) is what keeps it
/// correct for **several** readers of the same bus at once.
fn level_release(sample_rate: f32) -> f32 {
    let block_secs = BLOCK_SIZE as f32 / sample_rate.max(1.0);
    10.0f32.powf(-LEVEL_RELEASE_DB_PER_SEC / 20.0 * block_secs)
}

/// Release rate of a held bus level, in dB per second: the shared core's, so
/// the level a bus publishes falls at the rate every meter drawing it falls at.
pub const LEVEL_RELEASE_DB_PER_SEC: f32 = clausters_core::measure::METER_FALL_DB;

/// Audio-thread half. `process_block` does not allocate, lock or do I/O.
pub struct Engine {
    sample_rate: f32,
    /// Per-block decay applied to the published bus levels (see
    /// [`level_release`]), computed once from the sample rate.
    level_release: f32,
    channels: usize,
    tree: NodeTree,
    /// DSP workers for parallel groups; empty pool = sequential.
    pool: WorkerPool,
    buses: Buses,
    buffers: BufferPool,
    /// Live hardware input: decoded interleaved frames arriving from the
    /// cpal input stream through a lock-free ring. `0` channels / `None`
    /// consumer means no input stream is open. Read at each block start into
    /// audio buses `channels..channels + input_channels`, which `In`/`In.ar`
    /// then read like any bus.
    input_channels: usize,
    input_rx: Option<Consumer<f32>>,
    /// Samples processed since start; the stream clock scheduled bundles
    /// are measured against.
    now: u64,
    /// The transports, sized at boot (`--transports`) and never grown.
    /// Transport 0 is the one a server has always had.
    transports: Vec<TransportState>,
    /// Block-accurate mirror of each transport's clocks and position for the
    /// network thread, indexed like `transports`.
    transport_clocks: Arc<[TransportClocks]>,
    /// How far into the current block the engine is standing, in samples.
    /// Zero outside [`Engine::process_block`]'s cut loop; see
    /// [`Engine::transport_here`] for why anything reads it.
    cursor: usize,
    /// Pending timed bundles, sorted by time (stable for equal times).
    /// Pre-allocated: insertion and removal never allocate.
    sched: Vec<ScheduledBundle>,
    sample_clock: Arc<AtomicU64>,
    /// Block-accurate mirror of the sample clock into the IPC segment
    /// (one extra Release store per block); the Arc pins the mapping.
    ipc: Option<Arc<Segment>>,
    /// Which audio bus each segment tap ring records (`-1` = off), indexed by
    /// tap. Pre-allocated to the segment's tap count; `/bus_tap` flips entries.
    tap_buses: Vec<i32>,
    /// Whether this engine publishes **time** into the segment: the clocks,
    /// the taps and the per-bus levels.
    ///
    /// They belong to the process running an audio device, because they say
    /// where playback *is* -- and an on-demand session has no device and no
    /// clock, only frames it was asked to run. Two engines on one segment is
    /// the arrangement this exists for (an editor's session owns the samples,
    /// the RT server owns the devices): a session that published here would
    /// jog the playhead every time somebody applied a fade.
    publishes_time: bool,
    cmd_rx: Consumer<Cmd>,
    garbage_tx: Producer<Garbage>,
    pending_garbage: Vec<Garbage>,
    events_tx: Producer<NodeEvent>,
    reply_tx: Producer<ReplyMsg>,
    counters: Arc<Counters>,
    /// The per-role load table (`/server_load`). The audio thread adds this
    /// block's own time to `Role::Audio` from the measurement the CPU meter
    /// already takes, so metering costs no extra clock read here.
    meters: Arc<Meters>,
    /// EMA state of the CPU meter (fraction of the block budget, ~1 s time
    /// constant); published to `counters.avg_cpu` every block. Compiled out
    /// on wasm32 with the meter itself.
    #[cfg(not(target_arch = "wasm32"))]
    avg_cpu: f32,
}

/// Network-thread half: sends commands, collects garbage and events, reads
/// counters, serves the control buses directly.
pub struct EngineHandle {
    pub sample_rate: f32,
    pub channels: usize,
    /// Configured audio bus count (after clamping to the 128 ceiling).
    pub audio_buses: usize,
    /// Live hardware input channels; `0` when no input stream is open.
    /// Set by the backend once it has negotiated the input device.
    pub input_channels: usize,
    /// Boot-time pool capacities, surfaced in `/server_query.reply` so a client
    /// can discover the server's limits instead of hardcoding them.
    pub limits: Limits,
    cmd_tx: Producer<Cmd>,
    garbage_rx: Consumer<Garbage>,
    events_rx: Consumer<NodeEvent>,
    reply_rx: Consumer<ReplyMsg>,
    control_buses: ControlBuses,
    sample_clock: Arc<AtomicU64>,
    /// Each transport's clocks and position, as the engine last published
    /// them.
    transport_clocks: Arc<[TransportClocks]>,
    counters: Arc<Counters>,
    meters: Arc<Meters>,
    /// The IPC segment when one exists -- the network thread reads the audio
    /// taps from here (`/bus_tapStream`) without an engine round-trip.
    segment: Option<Arc<Segment>>,
    /// Where the device's sample axis sits on the wall clock, published by a
    /// backend with a device callback and unknown otherwise. See
    /// `server::device_epoch`.
    device_epoch: DeviceEpoch,
}

pub fn engine_pair(sample_rate: f32, channels: usize) -> (Engine, EngineHandle) {
    engine_pair_with_workers(sample_rate, channels, 0)
}

/// Default bus counts (scsynth `-a`/`-c`), used by the simple constructors and
/// the NRT renderer. The live server can override them with `--audio-buses`/
/// `--control-buses`; both are configured resources with no cap in the code.
pub const DEFAULT_AUDIO_BUSES: usize = NUM_AUDIO_BUSES;
pub const DEFAULT_CONTROL_BUSES: usize = NUM_CONTROL_BUSES;

/// Like [`engine_pair`], plus a worker pool of `workers` DSP threads
/// for parallel groups (`/group_parallel`). `workers == 0` is fully sequential
/// -- identical behavior and output either way (stages are bit-identical to
/// sequential execution by construction).
pub fn engine_pair_with_workers(
    sample_rate: f32,
    channels: usize,
    workers: usize,
) -> (Engine, EngineHandle) {
    engine_pair_full(
        sample_rate,
        channels,
        workers,
        None,
        DEFAULT_AUDIO_BUSES,
        DEFAULT_CONTROL_BUSES,
        Limits::default(),
    )
}

/// Full form: with an IPC segment, the control buses live *inside the
/// segment* (clients on the other side write the very atomics `InCtl`
/// reads) and the engine mirrors its sample clock into it every block.
pub fn engine_pair_full(
    sample_rate: f32,
    channels: usize,
    workers: usize,
    ipc: Option<Arc<Segment>>,
    audio_buses: usize,
    control_buses: usize,
    limits: Limits,
) -> (Engine, EngineHandle) {
    let limits = limits.clamped();
    // No ceiling: how many buses there are is a configured resource, and what
    // it costs is memory plus the per-block clear (`tests/bus_scale.rs`
    // measures both). The floor is the hardware channels, which are buses.
    let audio_buses = audio_buses.max(channels.max(1));
    assert!(channels > 0 && channels <= audio_buses);
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_FIFO_CAPACITY);
    let (garbage_tx, garbage_rx) = RingBuffer::new(GARBAGE_FIFO_CAPACITY.max(2 * limits.max_nodes));
    let (events_tx, events_rx) = RingBuffer::new(EVENT_FIFO_CAPACITY.max(2 * limits.max_nodes));
    let (reply_tx, reply_rx) = RingBuffer::new(REPLY_FIFO_CAPACITY);
    let counters = Arc::new(Counters {
        synths: AtomicU32::new(0),
        ugens: AtomicU32::new(0),
        // The root group exists before the first tick publishes counts.
        groups: AtomicU32::new(1),
        avg_cpu: AtomicU32::new(0),
        peak_cpu: AtomicU32::new(0),
        late_blocks: AtomicU32::new(0),
    });
    // With an IPC segment the control buses live inside it, so their count is
    // whatever the segment was created with (read back from its header).
    let control_buses = match &ipc {
        Some(segment) => {
            segment.set_sample_rate(sample_rate as f64);
            segment.control_buses()
        }
        None => ControlBuses::new(control_buses),
    };
    let meters = Meters::new(workers);
    let sample_clock = Arc::new(AtomicU64::new(0));
    let transport_clocks: Arc<[TransportClocks]> = (0..limits.transports)
        .map(|_| TransportClocks::default())
        .collect();
    if let Some(segment) = &ipc {
        segment.set_transports(limits.transports);
    }
    let mut tree = NodeTree::with_capacity(limits.max_nodes);
    tree.set_transports(limits.transports);
    let tap_buses = vec![-1i32; ipc.as_ref().map_or(0, |s| s.taps())];
    let segment = ipc.clone();
    let engine = Engine {
        sample_rate,
        level_release: level_release(sample_rate),
        channels,
        tree,
        pool: WorkerPool::new(workers, &meters),
        buses: Buses::new(control_buses.clone(), audio_buses),
        buffers: empty_pool_with(limits.max_buffers),
        input_channels: 0,
        input_rx: None,
        now: 0,
        transports: (0..limits.transports)
            .map(|_| TransportState::new())
            .collect(),
        transport_clocks: Arc::clone(&transport_clocks),
        cursor: 0,
        sched: Vec::with_capacity(SCHED_CAPACITY),
        sample_clock: Arc::clone(&sample_clock),
        ipc,
        tap_buses,
        publishes_time: true,
        cmd_rx,
        garbage_tx,
        pending_garbage: Vec::with_capacity(PENDING_GARBAGE_CAPACITY),
        events_tx,
        reply_tx,
        counters: Arc::clone(&counters),
        meters: Arc::clone(&meters),
        #[cfg(not(target_arch = "wasm32"))]
        avg_cpu: 0.0,
    };
    let handle = EngineHandle {
        sample_rate,
        channels,
        audio_buses,
        input_channels: 0,
        limits,
        cmd_tx,
        garbage_rx,
        events_rx,
        reply_rx,
        control_buses,
        sample_clock,
        transport_clocks,
        counters,
        meters,
        segment,
        device_epoch: DeviceEpoch::default(),
    };
    (engine, handle)
}

impl Engine {
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Samples processed so far: the counter [`EngineHandle::current_samples`]
    /// mirrors, read on the thread that advances it.
    pub fn processed_samples(&self) -> u64 {
        self.now
    }

    /// How many transports this engine has (`--transports`).
    pub fn transports(&self) -> usize {
        self.transports.len()
    }

    /// Transport `transport`'s clock: samples elapsed under it. Panics past
    /// the table, like any index.
    pub fn transport_now(&self, transport: usize) -> TransportSample {
        self.transports[transport].at(self.now)
    }

    /// The device sample **at the cursor** -- where inside the current block
    /// the engine is standing, rather than at its first sample.
    ///
    /// A locate arrives inside a timed bundle and lands on an exact sample, so
    /// anchoring it at the block's start would put the transport up to a block
    /// away from where the client asked. Same reason `frozen_total` is
    /// credited at the sample a transport flips rather than a block at a
    /// time. Outside the block-cut loop the cursor is 0 and this is the
    /// block's start.
    fn device_here(&self) -> u64 {
        self.now + self.cursor as u64
    }

    /// Whether transport `transport`'s queue holds nothing. A server that
    /// never binds a group never puts a bundle there, so this staying true is
    /// the observable form of "scheduling behaves exactly as it did before the
    /// transport".
    pub fn transport_queue_is_empty(&self, transport: usize) -> bool {
        self.transports[transport].sched.is_empty()
    }

    /// The transport whose queue a scheduled bundle belongs to, or `None` for
    /// the device queue.
    ///
    /// A bundle is atomic, so it goes whole to one queue: the first message
    /// that targets a governed node decides it, and the transport is the one
    /// governing that node -- the nearest governed group above it
    /// ([`NodeTree::governing`]). A command with no node target (a bus write,
    /// a buffer, a def) carries no opinion and rides the bundle's verdict; a
    /// bundle of nothing but those goes to the device queue.
    ///
    /// RT-safe: the walk up the `parent` links is bounded and allocates
    /// nothing, so this is a plain scan of the bundle.
    fn governing_transport(&self, cmds: &[Cmd]) -> Option<usize> {
        if self.transports.iter().all(|t| t.group.is_none()) {
            return None;
        }
        cmds.iter().find_map(|cmd| {
            cmd_target_nodes(cmd)
                .iter()
                .flatten()
                .find_map(|id| self.tree.governing(*id))
        })
    }

    /// Wires live hardware input to this engine: `channels` interleaved
    /// input channels arrive through `rx`, filled every block into audio buses
    /// `channels..channels + input_channels` (scsynth's convention: outputs
    /// first, then inputs). Call once, before the engine starts processing. The
    /// producer end lives in the cpal input callback.
    pub fn attach_input(&mut self, channels: usize, rx: Consumer<f32>) {
        self.input_channels = channels;
        self.input_rx = Some(rx);
    }

    /// Creates the input ring, attaches its consumer to this engine, and hands
    /// back the producer to push interleaved frames -- the test-side counterpart
    /// of the cpal input stream. `capacity` is in samples (channels * frames).
    pub fn input_ring(&mut self, channels: usize, capacity: usize) -> Producer<f32> {
        let (tx, rx) = RingBuffer::new(capacity.max(1));
        self.attach_input(channels, rx);
        tx
    }

    /// Drains one block's worth of interleaved input frames into the hardware
    /// input buses. An underrun (producer behind) reads as silence for the
    /// missing samples -- never a stall. RT-safe: ring pops and bus writes only.
    fn fill_input_buses(&mut self) {
        let Some(rx) = &mut self.input_rx else { return };
        let ich = self.input_channels;
        if ich == 0 {
            return;
        }
        for f in 0..BLOCK_SIZE {
            for ch in 0..ich {
                let s = rx.pop().unwrap_or(0.0);
                // The output buses are `0..channels`; inputs follow them.
                // `audio_mut` is sound here: single-threaded at block start,
                // before the parallel stage scheduler runs.
                unsafe {
                    self.buses.audio_mut(self.channels + ch)[f] = s;
                }
            }
        }
    }

    /// Stops this engine publishing **time** into the segment -- the clocks,
    /// the taps and the per-bus levels (see `publishes_time`).
    ///
    /// What it keeps publishing is the samples and the control buses, which
    /// are the data plane proper: state a peer reads and writes, rather than a
    /// report of where a device is.
    pub fn silence_time_publication(&mut self) {
        self.publishes_time = false;
    }

    /// Applies what has arrived **without advancing time**: the two steps
    /// [`Self::process_block`] begins with, and none of the rest.
    ///
    /// A pulled driver needs this because a command can only take effect
    /// through the FIFO -- installing a buffer is `Cmd::SetBuffer`, so a
    /// `/buffer_alloc` that has completed on the NRT side is still not in the
    /// pool until the engine drains. In real time the next block does that a
    /// millisecond later and nobody notices; a driver whose clock only moves
    /// during an operation (`server::nrtsession`) would otherwise have to
    /// process a block it does not want in order to load a buffer, which is
    /// exactly the clock it is defined as not having.
    ///
    /// Same RT discipline as `process_block`: no allocation, no locking. It
    /// is safe to call from the audio thread, and nothing there needs to.
    pub fn drain(&mut self) {
        self.drain_commands();
        self.flush_pending_garbage();
    }

    /// Processes one block. `out` is interleaved and its length must be
    /// `BLOCK_SIZE * channels`. Runs on the audio thread: does not allocate.
    ///
    /// Immediate commands apply at the block start; scheduled bundles whose
    /// time falls inside this block execute at their exact sample, splitting
    /// the processing into slices around each event (late ones at offset 0).
    pub fn process_block(&mut self, out: &mut [f32]) {
        debug_assert_eq!(out.len(), BLOCK_SIZE * self.channels);
        // CPU meter start. The stamp is RT-safe on the platforms we target:
        // `clock_gettime(CLOCK_MONOTONIC)` through the vDSO -- no allocation,
        // no lock, no kernel trap. On wasm32 there is no monotonic clock, so
        // the stamp is inert and both this meter and `/server_load` read 0
        // there (`server::meters`).
        let meter_start = crate::server::meters::stamp();
        self.drain_commands();
        self.flush_pending_garbage();

        self.buses.clear_audio();
        // Live input: fill the input buses after clearing, before any node
        // runs, so `In` reads this block's captured samples.
        self.fill_input_buses();
        let block_start = self.now;
        let block_end = block_start + BLOCK_SIZE as u64;
        // The block is cut by the union of every queue: a transport entry is
        // projected onto the device axis with its transport's frozen total
        // known at this instant, and a stopped transport can never reach its
        // own queue.
        let mut offset = 0usize;
        // Where inside this block each transport's current frozen run began,
        // if it is stopped. Frozen time is credited **at the sample the
        // transport flips**, not a whole block at a time: a stop and a resume
        // both land mid-block, and crediting a flat `BLOCK_SIZE` whenever the
        // transport happened to be stopped at the boundary loses (stop offset
        // - resume offset) samples on every cycle, an error that accumulates
        // without bound. Crediting at the flip also keeps `frozen_total`
        // correct *during* the block, which is what the transport-queue
        // projection below reads.
        for t in &mut self.transports {
            t.frozen_from = if t.rolling { None } else { Some(0) };
        }
        loop {
            let device_due = self.sched.first().map(|b| b.time);
            // The earliest entry of any rolling transport's queue. A
            // transport entry's device time only exists while it rolls: a
            // stopped transport can never reach it. Read afresh on every
            // iteration, because a bundle applied below may have carried a
            // `TransportRun` -- a stop scheduled mid-block freezes that queue
            // from that sample on, which is the wanted behaviour. Ties between
            // transports go to the lower id.
            let transport_due = self
                .transports
                .iter()
                .enumerate()
                .filter(|(_, t)| t.rolling)
                .filter_map(|(i, t)| {
                    t.sched
                        .first()
                        .map(|b| (i, b.time.to_device(t.frozen_total).get()))
                })
                .min_by_key(|&(_, due)| due);
            let take_transport = match (device_due, transport_due) {
                (_, None) => false,
                (None, Some(_)) => true,
                // Ties go to the device queue: a fixed preference, because
                // cross-queue enqueue order is not recoverable at fire time
                // (a transport entry's device time is not fixed when it is
                // enqueued). Device-first is the right side, since it makes
                // an empty transport queue indistinguishable from a single
                // queue over the device axis.
                (Some(d), Some((_, t))) => t < d,
            };
            let queue_due = if take_transport {
                transport_due.map(|(_, due)| due)
            } else {
                device_due
            };
            let queue_due = queue_due.filter(|t| *t < block_end);
            // A transport's edge -- its loop's end, or its end mark -- is the
            // third thing that cuts a block, and it is cut for the same reason
            // the other two are: the wrap or the stop lands on an exact
            // sample. Cutting there is also what keeps each position *linear
            // inside every slice*, so a reader following it ramps by one per
            // sample and never has to know a loop exists.
            //
            // `<= block_end`, where a bundle is `<`: a bundle at the boundary
            // belongs to the next block, but a wrap there belongs to *this*
            // one, because the position published at the end of a block is
            // what the next block's first sample plays -- and that sample is
            // the loop's start. Reading it a block late is a playhead that
            // overshoots the loop by a block, once per pass.
            let here = block_start + offset as u64;
            let edge_due = self
                .transports
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.edge_due(here).map(|(edge, due)| ((i, edge), due)))
                .min_by_key(|&(_, due)| due)
                .filter(|&(_, due)| due <= block_end);
            // An edge ties with a bundle by yielding to it: the queues keep the
            // device-first preference they already had among themselves, and
            // an edge that stays due is taken on the next turn of the loop.
            let take_edge = match (edge_due, queue_due) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some((_, w)), Some(q)) => w < q,
            };
            let Some(due_time) = (if take_edge {
                edge_due.map(|(_, due)| due)
            } else {
                queue_due
            }) else {
                break;
            };
            let at = due_time.saturating_sub(block_start) as usize;
            if at > offset {
                self.process_slice(offset, at - offset);
                offset = at;
            }
            self.cursor = offset;
            if let Some(((k, edge), _)) = edge_due.filter(|_| take_edge) {
                let here = self.device_here();
                let t = &mut self.transports[k];
                // What freezing it on this sample leaves to do: whether the
                // end mark caused it, and where the pass goes back to.
                let froze = match edge {
                    Edge::Wrap => {
                        // Back to the loop's start, re-anchored here so the
                        // position goes on advancing by one per sample from
                        // the seam. The span is half-open, so the end sample
                        // is never played and the first sample after the last
                        // one of the loop is its first.
                        let start = t.looping.as_ref().map_or(0, |span| span.start);
                        t.position = t
                            .position
                            .wrapped_to(TransportPosition::new(start), t.at(here));
                        None
                    }
                    // **The end mark: stop here, on this sample**, as a
                    // `/transport_stop` landing on it would.
                    Edge::End => Some((t.end.expect("an end was due").back, true)),
                    // **A ramp's length before the end mark**: the stopping
                    // phase starts here and ends on the mark, so the fade is
                    // over when the pass is. It falls from wherever the level
                    // is, over what is left before the mark.
                    Edge::EndRamp => {
                        let mark = t.end.expect("an end was due");
                        let at = t.reaching(mark.end, here);
                        if at > here {
                            t.fade = t.fade.toward(here, 0.0, at - here);
                            t.stopping = Some(Stopping {
                                at,
                                back: mark.back,
                                ended: true,
                            });
                            None
                        } else {
                            Some((mark.back, true))
                        }
                    }
                    Edge::Freeze => {
                        let stopping = t.stopping.take().expect("a stop was due");
                        Some((stopping.back, stopping.ended))
                    }
                };
                if let Some((back, ended)) = froze {
                    // The governed group and the transport's clock freeze on
                    // this sample, and the level is zero. An end then locates
                    // to where the pass goes back to; the locate is anchored
                    // at a stopped transport, so it holds until the next play.
                    t.rolling = false;
                    t.fade = Ramp::level(0.0);
                    if let Some(group) = t.group {
                        self.tree.set_paused(group, true);
                    }
                    if t.frozen_from.is_none() {
                        t.frozen_from = Some(offset);
                    }
                    if let Some(back) = back {
                        t.position =
                            PositionAnchor::located(TransportPosition::new(back), t.at(here));
                    }
                    if ended {
                        self.push_garbage(Garbage::TransportEnded { transport: k });
                    }
                }
                continue;
            }
            // Vec::remove on the pre-allocated queue: memmove, no (de)alloc.
            let mut cmds = match transport_due.filter(|_| take_transport) {
                Some((k, _)) => self.transports[k].sched.remove(0).cmds,
                None => self.sched.remove(0).cmds,
            };
            for cmd in cmds.drain(..) {
                self.apply(cmd);
            }
            self.push_garbage(Garbage::SpentBundle(cmds));
            // The bundle may have carried a `TransportRun`, for any
            // transport. Close or open each frozen run at this exact sample.
            // A bundle holding both a stop and a resume nets to no frozen
            // time, which is right: they land on the same sample.
            for t in &mut self.transports {
                match (t.frozen_from, t.rolling) {
                    (Some(from), true) => {
                        t.frozen_total += (offset - from) as u64;
                        t.frozen_from = None;
                    }
                    (None, false) => t.frozen_from = Some(offset),
                    _ => {}
                }
            }
        }
        self.cursor = 0;
        self.process_slice(offset, BLOCK_SIZE - offset);
        // The block ends with a transport still stopped: credit the tail.
        for t in &mut self.transports {
            if let Some(from) = t.frozen_from.take() {
                t.frozen_total += (BLOCK_SIZE - from) as u64;
            }
        }

        // Buses 0..channels are the hardware outputs.
        for (f, frame) in out.chunks_exact_mut(self.channels).enumerate() {
            for (ch, s) in frame.iter_mut().enumerate() {
                *s = self.buses.audio(ch)[f];
            }
        }

        self.now = block_end;
        // `frozen_total` was already credited to the sample inside the
        // block-cut loop above; here the clocks are only published. The
        // position is published at the block's end like the clocks, and read
        // there too: the position at `block_end` is where the next block
        // starts playing.
        for (t, clocks) in self.transports.iter().zip(self.transport_clocks.iter()) {
            let now = t.at(block_end);
            clocks.clock.store(now.get(), Ordering::Relaxed);
            clocks.frozen.store(t.frozen_total, Ordering::Relaxed);
            clocks
                .position
                .store(t.position.at(now).get(), Ordering::Relaxed);
        }
        self.sample_clock.store(block_end, Ordering::Relaxed);
        if let Some(segment) = self.ipc.as_ref().filter(|_| self.publishes_time) {
            // Audio taps first, then the clock: a reader that sees clock N
            // sees every tap sample of block N. One memcpy + one Release
            // store per active tap -- no allocation, no lock (RT-safe).
            for (i, &bus) in self.tap_buses.iter().enumerate() {
                if bus >= 0 && (bus as usize) < self.buses.audio_count() {
                    segment.tap_write(i, self.buses.audio(bus as usize));
                }
            }
            // Then the per-bus level a meter reads: this block's peak, held
            // against the decaying previous one. The hold is what makes the
            // number correct for a reader running slower than the engine -- a
            // display frame is a dozen blocks -- and the decay (rather than a
            // max the reader clears) keeps it correct for several readers of
            // the same bus at once. One pass over the block per bus, one load
            // and one relaxed store: no allocation, no lock.
            for bus in 0..self.buses.audio_count().min(segment.audio_buses()) {
                let peak = self
                    .buses
                    .audio(bus)
                    .iter()
                    .fold(0.0f32, |acc, s| acc.max(s.abs()));
                let held = segment.level(bus) * self.level_release;
                segment.set_level(bus, peak.max(held));
            }
            // The transport clocks go out before the device clock, for the
            // same reason the taps do: a reader that sees device clock N has
            // seen everything block N published.
            for (i, t) in self.transports.iter().enumerate() {
                let now = t.at(block_end);
                if let Some(cell) = segment.transport_clock(i) {
                    cell.store(now.get(), Ordering::Relaxed);
                }
                if let Some(cell) = segment.transport_position(i) {
                    cell.store(t.position.at(now).get(), Ordering::Relaxed);
                }
            }
            segment.clock().store(block_end, Ordering::Release);
        }
        self.counters
            .synths
            .store(self.tree.synth_count() as u32, Ordering::Relaxed);
        self.counters
            .ugens
            .store(self.tree.ugen_count() as u32, Ordering::Relaxed);
        self.counters
            .groups
            .store(self.tree.group_count() as u32, Ordering::Relaxed);

        // Apply the freeing done actions collected during this block's walk
        // (`PauseSelf` was applied inline in the tree). Read id + action and act
        // one at a time so the tree is never borrowed twice at once; a `free` of
        // an already-gone id (a split block can queue one twice, or two synths
        // in a group both request `FreeGroup`) is a harmless no-op.
        let n_done = self.tree.take_done_count();
        for k in 0..n_done {
            let id = self.tree.done_node(k);
            let action = self.tree.done_action_at(k);
            let mut sink = garbage_sink!(self);
            self.tree
                .apply_done_action(id, action, &mut |f| sink.consume(f));
        }

        // Drain the side-effect replies buffered this block (`SendReply`/
        // `SendTrig`/`Poll`) into the reply FIFO for the network thread to
        // turn into OSC. Disjoint field borrows: the tree walk reads the synths,
        // the producer takes the messages.
        let tree = &mut self.tree;
        let reply_tx = &mut self.reply_tx;
        tree.drain_replies(&mut |msg| {
            let _ = reply_tx.push(msg);
        });

        // CPU meter end: this block's wall time as a fraction of its real-time
        // budget (`BLOCK_SIZE / sample_rate`). Only meaningful when the caller
        // is paced by an audio device; NRT renders just measure render speed.
        // The same measurement feeds the per-role table, so the block's share
        // of `/server_load` costs no second clock read.
        let elapsed_nanos = meter_start.elapsed_nanos();
        self.meters.add(Role::Audio, 0, elapsed_nanos);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let budget = BLOCK_SIZE as f64 / self.sample_rate as f64;
            let busy = (elapsed_nanos as f64 / 1e9 / budget) as f32;
            // EMA with a ~1 s time constant: alpha = block duration / 1 s.
            self.avg_cpu += (busy - self.avg_cpu) * budget as f32;
            self.counters
                .avg_cpu
                .store(self.avg_cpu.to_bits(), Ordering::Relaxed);
            // Non-negative floats order like their bit patterns: a bitwise
            // `fetch_max` is a float max.
            self.counters
                .peak_cpu
                .fetch_max(busy.to_bits(), Ordering::Relaxed);
            if busy > 1.0 {
                self.counters.late_blocks.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Runs the node tree over `offset..offset+frames` of the current block.
    fn process_slice(&mut self, offset: usize, frames: usize) {
        if frames == 0 {
            return;
        }
        // Where every transport stands at this slice's **first** frame. The
        // block is cut at every loop wrap, so each position advances by
        // exactly one per sample for the whole slice and a UGen reading it
        // only has to ramp -- no wrap arithmetic, and nothing needs to know the
        // loop points but the engine. The tree hands each governed subtree
        // its own transport's row; everything else reads transport 0's.
        let device = self.now + offset as u64;
        for (i, t) in self.transports.iter().enumerate() {
            self.tree.set_transport_view(
                i,
                TransportCtx {
                    position: t.position.at(t.at(device)).get(),
                    rolling: t.rolling,
                    fade: t.fade.at(device),
                    fade_step: t.fade.step(device),
                },
            );
        }
        let ctx = ProcessCtx {
            // The two agree at the engine boundary: a node runs at the engine
            // rate, and the per-UGen rate is derived inside the synth.
            sample_rate: self.sample_rate,
            full_sample_rate: self.sample_rate,
            buses: &self.buses,
            buffers: &self.buffers,
            offset,
            frames,
            transport: self.tree.transport_view(0),
        };
        self.tree.process(&ctx, &self.pool);
    }

    fn push_garbage(&mut self, garbage: Garbage) {
        let mut sink = garbage_sink!(self);
        sink.push(garbage);
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.cmd_rx.pop() {
            self.apply(cmd);
        }
    }

    /// Applies one command. Called when draining the FIFO at block start and
    /// when a scheduled bundle fires mid-block.
    fn apply(&mut self, cmd: Cmd) {
        // Classified here rather than in the arm below because the garbage
        // sink borrows three of our fields for the whole `match`, and the
        // classification wants `&self`. A discriminant test per command.
        let governed = match &cmd {
            Cmd::Schedule { cmds, .. } => self.governing_transport(cmds),
            _ => None,
        };
        let here = self.device_here();
        {
            let mut sink = garbage_sink!(self);
            let Some(cmd) = apply_to_tree(&mut self.tree, &mut sink, cmd) else {
                return;
            };
            match cmd {
                Cmd::TransportRun { transport, rolling } => {
                    let Some(t) = self.transports.get_mut(transport) else {
                        return;
                    };
                    // With a ramp, a stop only starts the stopping phase and
                    // the freeze is an edge of the block; a play during one
                    // calls it off, and the group never froze.
                    let flipped = if rolling { t.play(here) } else { t.stop(here) };
                    if !flipped {
                        return;
                    }
                    if let Some(group) = t.group {
                        self.tree.set_paused(group, !rolling);
                        if rolling {
                            // **Playback comes back where the transport is, not
                            // where it stopped.** The subtree saw none of the
                            // time that passed, so every smoother in it still
                            // holds the value the music ended on while the
                            // curves driving them have long since moved --
                            // see `SynthNode::resume`.
                            self.tree.resume_subtree(group);
                        }
                    }
                }
                Cmd::TransportGroup { transport, id } => {
                    let Some(t) = self.transports.get_mut(transport) else {
                        return;
                    };
                    // Thaw whatever it governed before letting it go, and
                    // freeze the new one if it is already stopped.
                    if let Some(previous) = t.group.take() {
                        self.tree.set_paused(previous, false);
                        self.tree.set_governing(previous, None);
                    }
                    if id >= 0 && self.tree.set_governing(id, Some(transport)) {
                        t.group = Some(id);
                        if !t.rolling {
                            self.tree.set_paused(id, true);
                        }
                    }
                }
                Cmd::TransportFollow { transport, id } => {
                    let Some(t) = self.transports.get_mut(transport) else {
                        return;
                    };
                    if let Some(previous) = t.follow.take() {
                        self.tree.set_following(previous, None);
                    }
                    if id >= 0 && self.tree.set_following(id, Some(transport)) {
                        t.follow = Some(id);
                    }
                }
                Cmd::TransportLocate {
                    transport,
                    position,
                } => {
                    // One store, at the sample the locate lands on: the
                    // position is anchored rather than accumulated, so this
                    // is the whole of a seek on the audio thread.
                    // During a stopping phase the locate is where the
                    // position rests once it freezes: the ramp goes on
                    // fading what was playing.
                    if let Some(t) = self.transports.get_mut(transport) {
                        match t.stopping.as_mut() {
                            Some(stopping) => stopping.back = Some(position),
                            None => {
                                t.position = PositionAnchor::located(
                                    TransportPosition::new(position),
                                    t.at(here),
                                )
                            }
                        }
                    }
                }
                Cmd::TransportLoop { transport, span } => {
                    // Re-anchored at this sample, so turning a loop on does
                    // not move the transport: it keeps playing from where it is
                    // and wraps when it first reaches the end.
                    if let Some(t) = self.transports.get_mut(transport) {
                        let now = t.at(here);
                        t.position = t.position.wrapped_to(t.position.at(now), now);
                        t.looping = span.filter(|s| s.start < s.end);
                    }
                }
                Cmd::TransportEnd { transport, mark } => {
                    if let Some(t) = self.transports.get_mut(transport) {
                        t.end = mark;
                    }
                }
                Cmd::TransportFade { transport, samples } => {
                    // The next stop and play take it; one already ramping
                    // keeps the line it started on.
                    if let Some(t) = self.transports.get_mut(transport) {
                        t.fade_len = samples;
                    }
                }
                Cmd::SetBuffer { index, buffer } => {
                    if let Some(slot) = self.buffers.get_mut(index) {
                        if let Some(old) = std::mem::replace(slot, buffer) {
                            sink.push(Garbage::FreedBuffer(old));
                        }
                    } else if let Some(buffer) = buffer {
                        // Out-of-range index (the network thread validates,
                        // so this is belt and braces): ship it back.
                        sink.push(Garbage::FreedBuffer(buffer));
                    }
                }
                Cmd::SetControlBus { index, value } => {
                    self.buses.control.set(index, value);
                }
                Cmd::SetTap { tap, bus } => {
                    // Out-of-range indices were rejected on the network side.
                    if let Some(slot) = self.tap_buses.get_mut(tap) {
                        let previous = *slot;
                        *slot = bus;
                        // Publish the inverse in the segment, so a reader looks
                        // the bus up instead of being told a ring index. One
                        // relaxed-path store each, no allocation (RT-safe).
                        if let Some(segment) = &self.ipc {
                            if previous >= 0 {
                                segment.set_tap_of_bus(previous as usize, None);
                            }
                            if bus >= 0 {
                                segment.set_tap_of_bus(bus as usize, Some(tap));
                            }
                        }
                    }
                }
                Cmd::Schedule { time, cmds } => {
                    if let Some(t) = governed.and_then(|k| self.transports.get_mut(k)) {
                        // The stamp arrives on the device axis (the network
                        // thread built it against the device clock); convert
                        // here, once, where the frozen total is known.
                        let at = t.at(time);
                        if t.sched.len() == t.sched.capacity() {
                            sink.push(Garbage::RejectedBundle(cmds));
                        } else {
                            // Sorted insert, after equal times, exactly as the
                            // device queue does.
                            let pos = t.sched.partition_point(|b| b.time <= at);
                            t.sched.insert(pos, ScheduledBundleT { time: at, cmds });
                        }
                    } else if self.sched.len() == self.sched.capacity() {
                        sink.push(Garbage::RejectedBundle(cmds));
                    } else {
                        let pos = self.sched.partition_point(|b| b.time <= time);
                        self.sched.insert(pos, ScheduledBundle { time, cmds });
                    }
                }
                Cmd::ClearSched { only } => {
                    // `drain` keeps the queue's capacity (no dealloc here); each
                    // bundle's heap is freed on the network side.
                    //
                    // A bare `/sched_clear` drops *every* pending bundle, the
                    // transports' included: a governed one left behind would
                    // fire on the next resume with nothing left to explain
                    // it. Asked for one transport, only its queue goes.
                    if only.is_none() {
                        for bundle in self.sched.drain(..) {
                            sink.push(Garbage::SpentBundle(bundle.cmds));
                        }
                    }
                    for (i, t) in self.transports.iter_mut().enumerate() {
                        if only.is_none_or(|k| k == i) {
                            for bundle in t.sched.drain(..) {
                                sink.push(Garbage::SpentBundle(bundle.cmds));
                            }
                        }
                    }
                }
                // Named rather than left to a `_`, so the compiler still
                // refuses a `Cmd` variant that neither this match nor
                // `apply_to_tree` handles -- a wildcard here would turn that
                // omission into a panic on the audio thread.
                Cmd::AddSynth { .. }
                | Cmd::AddGroup { .. }
                | Cmd::FreeNode { .. }
                | Cmd::FreeAllInGroup { .. }
                | Cmd::DeepFreeGroup { .. }
                | Cmd::RunNode { .. }
                | Cmd::MoveNode { .. }
                | Cmd::SetControl { .. }
                | Cmd::MapControl { .. }
                | Cmd::SetUsage { .. }
                | Cmd::SetGroupParallel { .. }
                | Cmd::UGenCommand { .. } => {
                    debug_assert!(false, "apply_to_tree returned a node command");
                }
            }
        }
    }

    fn flush_pending_garbage(&mut self) {
        while let Some(g) = self.pending_garbage.pop() {
            if let Err(PushError::Full(g)) = self.garbage_tx.push(g) {
                self.pending_garbage.push(g);
                break;
            }
        }
    }
}

/// The node-tree half of [`Engine::apply`] -- every command whose only engine
/// state is the tree itself, which is twelve of the nineteen.
///
/// A free function taking the two things it touches, rather than a method:
/// `sink` borrows three of the engine's fields for the whole match, so a
/// `&mut self` helper could not coexist with it. Returns the command back when
/// it is not one of these, so the caller matches the remaining seven and no
/// command is classified twice.
#[inline]
fn apply_to_tree(tree: &mut NodeTree, sink: &mut GarbageSink, cmd: Cmd) -> Option<Cmd> {
    match cmd {
        Cmd::AddSynth {
            id,
            target,
            action,
            mut synth,
            usage,
        } => {
            // Every add path funnels here, so this is the one place a
            // synth learns its id (arithmetic only -- RT-safe). See
            // `SynthNode::set_node_id`.
            synth.set_node_id(id);
            match tree.insert(
                id,
                NodeKind::Synth { node: synth, usage },
                target,
                action,
                &mut |f| sink.consume(f),
            ) {
                Ok(parent_id) => sink.started(id, parent_id, false),
                Err((kind, why)) => sink.rejected(id, kind, why),
            }
        }
        Cmd::SetUsage { id, usage } => tree.set_usage(id, usage),
        Cmd::SetGroupParallel { id, parallel } => {
            // Unknown or non-group IDs are ignored, like /node_set.
            let _ = tree.set_parallel(id, parallel);
        }
        Cmd::AddGroup {
            id,
            target,
            action,
            group,
        } => {
            match tree.insert(id, NodeKind::Group(group), target, action, &mut |f| {
                sink.consume(f)
            }) {
                Ok(parent_id) => sink.started(id, parent_id, true),
                Err((kind, why)) => sink.rejected(id, kind, why),
            }
        }
        Cmd::FreeNode { id } => {
            // Unknown IDs are silently ignored here; the network
            // thread already replied /fail where it could tell.
            tree.free(id, &mut |f| sink.consume(f));
        }
        Cmd::FreeAllInGroup { id } => {
            tree.free_all(id, &mut |f| sink.consume(f));
        }
        Cmd::DeepFreeGroup { id } => {
            tree.deep_free(id, &mut |f| sink.consume(f));
        }
        Cmd::RunNode { id, run } => {
            tree.set_paused(id, !run);
        }
        Cmd::MoveNode { id, target, place } => {
            tree.move_node(id, target, place);
        }
        Cmd::SetControl { id, index, value } => {
            if let Some(synth) = tree.synth_mut(id) {
                synth.set_control(index, value);
            }
        }
        Cmd::MapControl {
            id,
            index,
            bus,
            audio,
        } => {
            if let Some(synth) = tree.synth_mut(id) {
                synth.map_control(index, bus, audio);
            }
        }
        Cmd::UGenCommand {
            id,
            ugen_index,
            command,
        } => {
            if let Some(synth) = tree.synth_mut(id) {
                synth.ugen_command(ugen_index, &command);
            }
        }
        other => return Some(other),
    }
    None
}

impl EngineHandle {
    /// Enqueues a command. Returns it back if the FIFO is full so the caller
    /// can retry or report failure.
    pub fn send(&mut self, cmd: Cmd) -> Result<(), Cmd> {
        self.cmd_tx.push(cmd).map_err(|PushError::Full(c)| c)
    }

    /// Pops one item of garbage, if any. The caller drops it (we are on the
    /// network thread) and may use the ID for bookkeeping.
    pub fn pop_garbage(&mut self) -> Option<Garbage> {
        self.garbage_rx.pop().ok()
    }

    /// Pops one node lifecycle event, if any.
    pub fn pop_event(&mut self) -> Option<NodeEvent> {
        self.events_rx.pop().ok()
    }

    /// Pops one side-effect reply message (`SendReply`/`SendTrig`/`Poll`),
    /// if any. The network thread turns each into an OSC reply / console line.
    pub fn pop_reply(&mut self) -> Option<ReplyMsg> {
        self.reply_rx.pop().ok()
    }

    /// Drops everything the audio thread discarded. Returns how many items
    /// were collected.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while let Some(g) = self.pop_garbage() {
            if let Garbage::RejectedSynth { id, why, .. } | Garbage::RejectedGroup { id, why, .. } =
                &g
            {
                report_rejected(*id, *why, "engine");
            }
            drop(g);
            n += 1;
        }
        n
    }

    /// Control buses are shared atomics: `/bus_set`/`/bus_get` are served right
    /// here on the network thread, no command round-trip.
    pub fn control_buses(&self) -> &ControlBuses {
        &self.control_buses
    }

    /// The IPC segment when one exists. The audio taps are read from here
    /// (`/bus_tapStream` snapshots), like the control buses: shared memory, no
    /// engine round-trip.
    pub fn segment(&self) -> Option<&Arc<Segment>> {
        self.segment.as_ref()
    }

    /// The engine's stream clock: samples processed so far, published once
    /// per block. Timetag->sample conversion anchors on this.
    pub fn current_samples(&self) -> u64 {
        self.sample_clock.load(Ordering::Relaxed)
    }

    /// Where the device's sample axis sits on the wall clock: the shared
    /// estimate a device callback publishes and the network thread places
    /// wall-clock timetags with.
    pub fn device_epoch(&self) -> &DeviceEpoch {
        &self.device_epoch
    }

    /// How many transports the engine has (`--transports`).
    pub fn transports(&self) -> usize {
        self.transport_clocks.len()
    }

    /// Transport `transport`'s clock as of the last completed block; 0 past
    /// the table.
    pub fn current_transport_samples(&self, transport: usize) -> u64 {
        self.transport_clocks
            .get(transport)
            .map_or(0, |c| c.clock.load(Ordering::Relaxed))
    }

    /// Where transport `transport` stands, as of the last completed block.
    /// Unlike the two clocks this one jumps: a locate moves it and a loop
    /// wraps it. 0 past the table.
    pub fn current_transport_position(&self, transport: usize) -> u64 {
        self.transport_clocks
            .get(transport)
            .map_or(0, |c| c.position.load(Ordering::Relaxed))
    }

    /// Total samples transport `transport` has spent stopped, as of the last
    /// completed block -- the whole of the device <-> transport axis
    /// conversion. 0 past the table.
    pub fn current_frozen_total(&self, transport: usize) -> u64 {
        self.transport_clocks
            .get(transport)
            .map_or(0, |c| c.frozen.load(Ordering::Relaxed))
    }

    /// The per-role load table, for `/server_load`.
    pub fn meters(&self) -> &Arc<Meters> {
        &self.meters
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }
}
