//! The transports, as the audio thread holds them: each one's rolling state,
//! clock, position, loop, end mark and queues, the edges that cut a block
//! (a wrap, the end mark, a stop's freeze), the cues that wait for disk
//! readers, and the lane entries a position fires.
//!
//! A child of `server::engine`: the block loop there decides *when* (what
//! is due next, and on which sample), and what is here says what a
//! transport does once it is.

use std::ops::Range;
use std::sync::atomic::AtomicU64;

use clausters_core::transport_pass::TransportPass;

use super::{Cmd, Engine, Garbage, LANE_CAPACITY, SCHED_CAPACITY, cmd_target_nodes};
use crate::dsp::{BLOCK_SIZE, ProcessCtx, TransportCtx};
use crate::server::clock_axis::{DeviceSample, PositionAnchor, TransportPosition, TransportSample};

/// **How long a cue waits for its readers**, in seconds, before it answers
/// that they are not ready: a play after it then starts as one with no cue
/// did, and nothing waits on a disk that does not answer.
pub const CUE_WAIT_SECS: f64 = 0.2;

/// Where a rolling transport stops (`end`) and where it is located once it
/// has (`back`; `None` leaves it at `end`), both on the transport's position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndMark {
    pub end: u64,
    pub back: Option<u64>,
}

/// A timed bundle on a transport's axis. Same shape as the device queue's
/// `ScheduledBundle`; the type of `time` is the whole difference, and it is
/// what keeps the two kinds of queue from being fed each other's stamps.
pub(super) struct ScheduledBundleT {
    pub(super) time: TransportSample,
    pub(super) cmds: Vec<Cmd>,
    /// The event lane whose note this releases, when it is one: a locate
    /// runs it at once, and a client's `/sched_clear` leaves it standing.
    pub(super) lane: Option<i32>,
}

/// **Which event and which data** a lane entry comes from: the lane, the
/// generation of its data (bumped by every `/lane_set`) and the event's index
/// in it. The network thread keeps what it has queued by this, and learns an
/// entry was spent by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventLaneTag {
    pub lane: i32,
    pub generation: u32,
    pub event: u32,
}

/// An event-lane entry on a transport's lane queue ([`Cmd::EventLaneEntry`]).
pub(super) struct EventLaneEntryT {
    pub(super) position: u64,
    pub(super) tag: EventLaneTag,
    pub(super) start: Vec<Cmd>,
    pub(super) release: Vec<Cmd>,
    pub(super) length: u64,
}

/// One transport, as the audio thread holds it.
///
/// Every transport is independent: its own rolling state, its own clock (the
/// device clock minus what *it* has spent stopped), its own position, loop,
/// end mark, governed group and queue. They share nothing but the device
/// clock they are all measured from, so stopping one moves nothing about
/// another.
pub(super) struct TransportState {
    /// Whether it rolls. Stopped, `frozen_total` accumulates and its clock
    /// holds.
    pub(super) rolling: bool,
    pub(super) bpm: f64,
    /// Total samples it has spent stopped since boot. The whole of the
    /// device -> transport conversion (see `server::clock_axis`).
    pub(super) frozen_total: u64,
    /// The group it governs, frozen while it is stopped.
    pub(super) group: Option<i32>,
    /// The group that follows it: its nodes read this transport and are
    /// never frozen by it -- what sits beside the governed group and must go
    /// on running while it is stopped.
    pub(super) follow: Option<i32>,
    /// Where it stands, as an anchor onto its clock: the position a locate put
    /// it at, and the transport sample that locate landed on. A read is one
    /// add, so the position costs the per-sample path nothing.
    pub(super) position: PositionAnchor,
    /// The span the position wraps inside while looping. Always non-empty:
    /// an empty or inverted span is refused before it reaches the engine, and
    /// the wrap would not terminate over one.
    pub(super) looping: Option<Range<u64>>,
    /// The end mark: where it stops when rolling and no loop is set.
    pub(super) end: Option<EndMark>,
    /// Pending timed bundles on its axis. Frozen with it: while stopped
    /// nothing here can fall due, and nothing here is rewritten. Pre-allocated
    /// like the device queue, to the same capacity.
    pub(super) sched: Vec<ScheduledBundleT>,
    /// **Its event lanes' entries**, sorted by the position they fire on
    /// (stable among equals), fed ahead of the position by the network thread.
    /// Keyed by position and not by clock, so a locate or a wrap needs nothing
    /// re-stamped: an entry fires when the position reaches it, and one the
    /// position jumped over waits for it to come back.
    pub(super) lanes: Vec<EventLaneEntryT>,
    /// Where inside the current block its frozen run began, if it is stopped
    /// -- scratch for [`Engine::process_block`], `None` outside it.
    pub(super) frozen_from: Option<usize>,
    /// How long a stop and a play ramp, in samples (`/transport_fade`); `0`
    /// is no ramp, and a stop freezes on its own sample.
    pub(super) fade_len: u64,
    /// The declick level `TransportFade` reads: `1` rolling, `0` stopped, and
    /// a ramp between the two across a stop's stopping phase and a play.
    pub(super) fade: Ramp,
    /// A stop in its stopping phase: the transport still rolls, and freezes
    /// when the ramp reaches zero.
    pub(super) stopping: Option<Stopping>,
    /// It stopped on its end mark inside this block: said once the block is
    /// published, so the network thread that hears it reads the position the
    /// pass went back to and not the one before the jump.
    pub(super) ended: bool,
    /// A cue in progress ([`Cmd::TransportCue`]), and the blocks it has waited.
    pub(super) cue: Option<u32>,
    /// **A loop change waiting for its readers** (`/transport_loop` while it
    /// rolls over disk readers): the span asked for, and the blocks it has
    /// waited. The transport keeps the loop it has until every reader holds
    /// the reading the new one asks for ([`Engine::land_loop_cues`]), so a
    /// change is never heard -- one asked for too late takes effect a pass
    /// later.
    pub(super) loop_cue: Option<(Option<Range<u64>>, u32)>,
}

/// **The declick level**, as a straight line over device samples: `from` at
/// `start`, `to` from `start + len` on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ramp {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) start: u64,
    pub(super) len: u64,
}

impl Ramp {
    /// A level that does not move.
    pub(super) const fn level(level: f32) -> Self {
        Self {
            from: level,
            to: level,
            start: 0,
            len: 0,
        }
    }

    /// From the level at `device` to `to`, over `len` samples.
    pub(super) fn toward(self, device: u64, to: f32, len: u64) -> Self {
        Self {
            from: self.at(device),
            to,
            start: device,
            len,
        }
    }

    /// The level at device sample `device`.
    pub(super) fn at(&self, device: u64) -> f32 {
        let done = device.saturating_sub(self.start);
        if done >= self.len {
            return self.to;
        }
        self.from + (self.to - self.from) * (done as f32 / self.len as f32)
    }

    /// How much the level moves per sample at `device`: `0` once it has
    /// arrived.
    pub(super) fn step(&self, device: u64) -> f32 {
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
pub(super) struct Stopping {
    pub(super) at: u64,
    /// Where the position is located on the freeze: the end mark's return,
    /// or a locate that arrived during the ramp -- which lands there rather
    /// than mid-ramp, so the fade plays what was playing and the position
    /// rests where it was sent.
    pub(super) back: Option<u64>,
    /// Whether the end mark caused it: the engine then reports the end.
    pub(super) ended: bool,
}

/// What a transport's next edge is, when it is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
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
    pub(super) fn new() -> Self {
        Self {
            rolling: false,
            bpm: 120.0,
            frozen_total: 0,
            group: None,
            follow: None,
            position: PositionAnchor::default(),
            looping: None,
            end: None,
            sched: Vec::with_capacity(SCHED_CAPACITY),
            lanes: Vec::with_capacity(LANE_CAPACITY),
            frozen_from: None,
            fade_len: 0,
            fade: Ramp::level(0.0),
            stopping: None,
            ended: false,
            cue: None,
            loop_cue: None,
        }
    }

    /// The pass it is on, as a reader carrying its position on wants it:
    /// rolling, the loop in force (not one waiting for its readers) and the
    /// end mark.
    pub(super) fn pass(&self) -> TransportPass {
        TransportPass {
            rolling: self.rolling,
            looping: self.looping.as_ref().map(|span| (span.start, span.end)),
            end: self.end.map(|mark| mark.end),
        }
    }

    /// Its clock at device sample `device`.
    pub(super) fn at(&self, device: u64) -> TransportSample {
        DeviceSample::new(device).to_transport(self.frozen_total)
    }

    /// The device sample at which its position reaches `position`, or
    /// `device` itself when it already has.
    pub(super) fn reaching(&self, position: u64, device: u64) -> u64 {
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
    pub(super) fn edge_due(&self, device: u64) -> Option<(Edge, u64)> {
        if !self.rolling {
            return None;
        }
        if let Some(stopping) = self.stopping {
            return Some((Edge::Freeze, stopping.at.max(device)));
        }
        if let Some(span) = &self.looping {
            let here = self.at(device);
            // **A loop put in place on its own end wraps at once.** It was
            // asked for while the position was short of the end, and its
            // readers read it through the wrap as they were asked; landing
            // on the block that starts at the end, it is due now rather than
            // never, or the transport would play on past it while the
            // readers hold the loop's start.
            if span.start < span.end && self.position.at(here).get() == span.end {
                return Some((Edge::Wrap, device));
            }
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
    pub(super) fn stop(&mut self, device: u64) -> bool {
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
    pub(super) fn play(&mut self, device: u64) -> bool {
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
pub(super) struct TransportClocks {
    /// Its clock: samples elapsed under it.
    pub(super) clock: AtomicU64,
    /// Total samples it has spent stopped. Published beside the clock rather
    /// than derived from the device clock minus it, because those are two
    /// separate loads and can straddle a block.
    pub(super) frozen: AtomicU64,
    /// Where it stands. Not a clock: it jumps and it wraps (see
    /// `server::clock_axis`).
    pub(super) position: AtomicU64,
    /// The pass it is on, as [`TransportPass::to_words`]: what a play cursor
    /// carried on between two blocks is bounded by.
    pub(super) pass: [AtomicU64; 4],
}

impl Engine {
    /// Transport `transport`'s clock: samples elapsed under it. Panics past
    /// the table, like any index.
    pub fn transport_now(&self, transport: usize) -> TransportSample {
        self.transports[transport].at(self.now)
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
    pub(super) fn governing_transport(&self, cmds: &[Cmd]) -> Option<usize> {
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

    /// **A loop put in place** at device sample `here`, re-anchored there, so
    /// turning a loop on does not move the transport: it keeps playing from
    /// where it is and wraps when it first reaches the end.
    pub(super) fn put_loop(t: &mut TransportState, here: u64, span: Option<Range<u64>>) {
        let now = t.at(here);
        t.position = t.position.wrapped_to(t.position.at(now), now);
        t.looping = span;
    }

    /// Transport `t`'s view at device sample `device`, as its readers read
    /// it, with `pending` the loop asked for and not yet in place.
    pub(super) fn view_of(
        t: &TransportState,
        device: u64,
        pending: Option<Option<Range<u64>>>,
    ) -> TransportCtx {
        TransportCtx {
            position: t.position.at(t.at(device)).get(),
            rolling: t.rolling,
            fade: t.fade.at(device),
            fade_step: t.fade.step(device),
            looping: t.looping.as_ref().map(|span| (span.start, span.end)),
            pending_looping: pending.map(|p| p.map(|span| (span.start, span.end))),
        }
    }

    /// **Every loop change waiting for its readers, at the start of a
    /// block**: put in place once every reader of the transport's governed
    /// group holds the reading it asks for (each said so as it played the
    /// block before, `UGen::looped`), once it has waited [`CUE_WAIT_SECS`],
    /// or once the transport no longer rolls.
    pub(super) fn land_loop_cues(&mut self) {
        let here = self.now;
        for k in 0..self.transports.len() {
            let Some((span, waited)) = self.transports[k].loop_cue.clone() else {
                continue;
            };
            let ready = self.loop_cue_ready(k, here).is_some();
            let t = &mut self.transports[k];
            if ready {
                t.loop_cue = None;
                Self::put_loop(t, here, span);
            } else {
                t.loop_cue = Some((span, waited + 1));
            }
        }
    }

    /// **The loop change transport `k` waits for, when it may be put in
    /// place** at device sample `here`: every reader of its governed group
    /// holds the reading it asks for, it has waited [`CUE_WAIT_SECS`], or
    /// the transport no longer rolls.
    pub(super) fn loop_cue_ready(&mut self, k: usize, here: u64) -> Option<Option<Range<u64>>> {
        let wait = (CUE_WAIT_SECS * self.sample_rate as f64 / BLOCK_SIZE as f64).ceil() as u32;
        let t = &self.transports[k];
        let (span, waited) = t.loop_cue.clone()?;
        let ready = !t.rolling
            || waited >= wait
            || t.group.is_none_or(|g| {
                let view = Self::view_of(t, here, Some(span.clone()));
                self.tree.looped_subtree(g, &view) != Some(false)
            });
        ready.then_some(span)
    }

    /// **Every cue in progress, one block further**: a stopped transport's
    /// governed group runs the synths that stream, standing
    /// ([`NodeTree::cue_subtree`](crate::node::NodeTree::cue_subtree)), after
    /// the block's output is out, so nothing it writes is heard. A cue is
    /// answered once its readers are ready, once it has waited
    /// [`CUE_WAIT_SECS`], or as soon as the transport rolls.
    pub(super) fn cue_transports(&mut self) {
        let wait = (CUE_WAIT_SECS * self.sample_rate as f64 / BLOCK_SIZE as f64).ceil() as u32;
        for k in 0..self.transports.len() {
            let t = &self.transports[k];
            let Some(waited) = t.cue else {
                continue;
            };
            let ready = match (t.rolling, t.group) {
                (true, _) => Some(false),
                (false, None) => Some(true),
                (false, Some(group)) => {
                    let ctx = ProcessCtx {
                        sample_rate: self.sample_rate,
                        full_sample_rate: self.sample_rate,
                        buses: &self.buses,
                        buffers: &self.buffers,
                        offset: 0,
                        frames: BLOCK_SIZE,
                        transport: self.tree.transport_view(k),
                    };
                    let ready = self.tree.cue_subtree(group, &ctx);
                    (ready || waited >= wait).then_some(ready)
                }
            };
            match ready {
                Some(ready) => {
                    self.transports[k].cue = None;
                    self.push_garbage(Garbage::TransportCued {
                        transport: k,
                        ready,
                    });
                }
                None => self.transports[k].cue = Some(waited + 1),
            }
        }
    }

    /// Transport `k` reaches `edge` on sample `offset` of the block: a wrap,
    /// the end mark, the start of the end mark's ramp, or the freeze a
    /// stopping phase ends in.
    pub(super) fn cross_edge(&mut self, k: usize, edge: Edge, offset: usize) {
        let here = self.device_here();
        // **A loop change whose readers are ready lands on the wrap**, before
        // it: they read the take through the loop asked for, so the position
        // goes where they read (the new loop's start, or straight on when
        // the loop was turned off) rather than to the old loop's start a
        // block before the change would have landed anyway.
        let landing = match edge {
            Edge::Wrap => self.loop_cue_ready(k, here),
            _ => None,
        };
        let t = &mut self.transports[k];
        if let Some(span) = landing {
            t.loop_cue = None;
            Self::put_loop(t, here, span);
            if t.looping.is_none() {
                self.run_released();
                return;
            }
        }
        // What freezing it on this sample leaves to do: whether the
        // end mark caused it, and where the pass goes back to.
        let froze = match edge {
            Edge::Wrap => {
                self.tree.release_transport_notes(k, offset);
                if let Some(group) = t.group {
                    self.tree.release_notes(Some(group), None, offset);
                }
                // Back to the loop's start, re-anchored here so the
                // position goes on advancing by one per sample from
                // the seam. The span is half-open, so the end sample
                // is never played and the first sample after the last
                // one of the loop is its first.
                let start = t.looping.as_ref().map_or(0, |span| span.start);
                t.position = t
                    .position
                    .wrapped_to(TransportPosition::new(start), t.at(here));
                // **A wrap is a jump**, as a locate is: what the lanes were
                // sounding is released on the seam, and the next pass plays
                // it again from its start -- a note held across the loop's
                // end would otherwise ring into the next pass beside itself.
                take_releases(&mut t.sched, &mut self.released, None);
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
            self.tree.release_transport_notes(k, offset);
            // The governed group and the transport's clock freeze on
            // this sample, and the level is zero. An end then locates
            // to where the pass goes back to; the locate is anchored
            // at a stopped transport, so it holds until the next play.
            t.rolling = false;
            t.fade = Ramp::level(0.0);
            if let Some(group) = t.group {
                self.tree.release_notes(Some(group), None, offset);
                self.tree.set_paused(group, true);
            }
            if t.frozen_from.is_none() {
                t.frozen_from = Some(offset);
            }
            if let Some(back) = back {
                t.position = PositionAnchor::located(TransportPosition::new(back), t.at(here));
            }
            // A stop, the end mark's included, releases what the lanes
            // sounded -- and going back is a jump, which would too.
            take_releases(&mut t.sched, &mut self.released, None);
            if ended {
                t.ended = true;
            }
        }
        self.run_released();
    }

    /// **The lane entry transport `k`'s position has reached**, run: its start
    /// now, and its release onto the transport's clock queue a note's length
    /// later -- or at once, when that queue is full, since a note cut short is
    /// better than one that never ends.
    pub(super) fn apply_due_lane(&mut self, k: usize) {
        let here = self.device_here();
        let t = &mut self.transports[k];
        let now = t.at(here);
        let position = t.position.at(now).get();
        let i = t.lanes.partition_point(|e| e.position < position);
        if i >= t.lanes.len() {
            return;
        }
        let mut entry = t.lanes.remove(i);
        let release = std::mem::take(&mut entry.release);
        if !release.is_empty() {
            if t.sched.len() < t.sched.capacity() {
                let at = now.saturating_add(entry.length);
                let pos = t.sched.partition_point(|b| b.time <= at);
                t.sched.insert(
                    pos,
                    ScheduledBundleT {
                        time: at,
                        cmds: release,
                        lane: Some(entry.tag.lane),
                    },
                );
            } else if self.released.len() < self.released.capacity() {
                self.released.push(ScheduledBundleT {
                    time: now,
                    cmds: release,
                    lane: Some(entry.tag.lane),
                });
            }
        }
        for cmd in entry.start.drain(..) {
            self.apply(cmd);
        }
        self.run_released();
        self.push_garbage(Garbage::EventLaneSpent {
            tag: entry.tag,
            start: entry.start,
            release: Vec::new(),
            fired: true,
        });
    }
}

/// Moves the lane releases of `sched` -- every lane's, or `lane`'s alone --
/// into `released`, to be run at once. Allocation-free: `released` has a
/// queue's capacity, and what would not fit stays where it was.
pub(super) fn take_releases(
    sched: &mut Vec<ScheduledBundleT>,
    released: &mut Vec<ScheduledBundleT>,
    lane: Option<i32>,
) {
    let mut j = 0;
    while j < sched.len() {
        let theirs = sched[j]
            .lane
            .is_some_and(|l| lane.is_none_or(|want| want == l));
        if theirs && released.len() < released.capacity() {
            released.push(sched.remove(j));
        } else {
            j += 1;
        }
    }
}
