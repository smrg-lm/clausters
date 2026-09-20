//! Where the server's time goes: a load meter per **role**.
//!
//! `/server_status` reports one number for the whole engine -- the audio
//! thread's per-block load. That answers *is the server keeping up* and not
//! *what is it spending the time on*, which is the question a slow session
//! actually asks: a def compiling, a file loading, a burst of commands and the
//! DSP itself all cost, and only one of them shows in that number.
//!
//! **The server times itself; it does not ask the operating system.** Each
//! piece of work brackets itself with [`stamp`] and adds the elapsed nanos to
//! its slot, so the measurement is the same on every platform and needs no
//! per-OS thread-inspection API. [`std::time::Instant`] is
//! `clock_gettime(CLOCK_MONOTONIC)` through the vDSO on Linux,
//! `mach_absolute_time` on macOS and `QueryPerformanceCounter` on Windows --
//! no allocation, no lock, no kernel trap, so a bracket is legal on the audio
//! thread (the same reasoning the engine's own CPU meter is built on). On
//! `wasm32` `Instant::now` panics, so there the stamp is inert and every slot
//! reports zero, exactly as the engine's meter does.
//!
//! **A role, not a thread.** What is measured is the work, not the thread that
//! happened to run it: the callback's block, one stage of a parallel group, a
//! serving turn, an NRT job, a Faust compilation. That is what makes the
//! reading portable -- a build where one of those runs somewhere else reports
//! the same roles -- and it is why the wire says `dsp 2` rather than a thread
//! name or a thread id.
//!
//! **Busy over wall time, not per cent of a core.** A slot accumulates the
//! time work was *in progress*. A DSP worker spinning for the next stage is
//! burning a core and is not busy here, which is the honest reading for the
//! question this answers (how much of the block budget the stage took); a
//! system-level profiler is the tool for the other one.
//!
//! **Cumulative, and the window belongs to the reader.** Slots only ever grow,
//! so any number of clients can each measure their own interval by
//! differencing two reports -- unlike `Counters::take_peak_cpu`, whose reset
//! makes two pollers steal each other's window.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a slot measures. Roles with several instances (only `Dsp` today)
/// carry an index; the rest are always index `0`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// The audio callback's whole block: draining commands, running the tree
    /// and handing back garbage. The conductor's share of a parallel group is
    /// here and not in [`Role::Dsp`], since it is the same thread doing it.
    Audio,
    /// One DSP worker thread, by index: the stages it took off the conductor.
    Dsp,
    /// The serving turn -- decoding a packet, translating it, pumping the
    /// subscriptions, collecting what finished. Never the blocking wait for
    /// the next packet.
    Net,
    /// The NRT job queue: reading and writing soundfiles, `/buffer_gen`, the
    /// buffer-editing verbs.
    Nrt,
    /// Compiling a FaustDef.
    Faust,
}

impl Role {
    /// The name the wire and both clients use.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Audio => "audio",
            Role::Dsp => "dsp",
            Role::Net => "net",
            Role::Nrt => "nrt",
            Role::Faust => "faust",
        }
    }
}

/// Roles past the audio slot and the workers: net, nrt and (with the feature)
/// faust. A build without `faust` has no compiler thread, so it reports no
/// such role rather than a row that can only ever read zero.
#[cfg(feature = "faust")]
const TAIL_ROLES: [Role; 3] = [Role::Net, Role::Nrt, Role::Faust];
#[cfg(not(feature = "faust"))]
const TAIL_ROLES: [Role; 2] = [Role::Net, Role::Nrt];

struct Slot {
    role: Role,
    index: u32,
    /// Nanoseconds of work, since boot. Relaxed: a reader wants a recent
    /// value, not a synchronized one.
    busy_nanos: AtomicU64,
    /// Times the work ran -- blocks, stages, turns, jobs, compilations.
    calls: AtomicU64,
}

/// One role's reading, as [`Meters::report`] renders it.
#[derive(Clone, Copy, Debug)]
pub struct Load {
    pub role: Role,
    pub index: u32,
    /// Seconds of work since boot.
    pub busy: f64,
    pub calls: u64,
}

/// The server's load table. Built at boot with one slot per role (and one per
/// DSP worker), never resized, never locked.
pub struct Meters {
    slots: Box<[Slot]>,
    workers: usize,
    #[cfg(not(target_arch = "wasm32"))]
    epoch: std::time::Instant,
}

impl Meters {
    /// The table for a server with `workers` DSP worker threads.
    pub fn new(workers: usize) -> Arc<Self> {
        let mut slots = Vec::with_capacity(workers + 1 + TAIL_ROLES.len());
        let mut push = |role: Role, index: u32| {
            slots.push(Slot {
                role,
                index,
                busy_nanos: AtomicU64::new(0),
                calls: AtomicU64::new(0),
            });
        };
        push(Role::Audio, 0);
        for i in 0..workers {
            push(Role::Dsp, i as u32);
        }
        for role in TAIL_ROLES {
            push(role, 0);
        }
        Arc::new(Self {
            slots: slots.into_boxed_slice(),
            workers,
            #[cfg(not(target_arch = "wasm32"))]
            epoch: std::time::Instant::now(),
        })
    }

    /// A table nobody reads -- for the engines and worker threads that tests
    /// and offline renders build, where there is no `/server_load` to answer.
    pub fn detached() -> Arc<Self> {
        Self::new(0)
    }

    fn slot_of(&self, role: Role, index: u32) -> Option<&Slot> {
        let i = match role {
            Role::Audio => 0,
            Role::Dsp => {
                if index as usize >= self.workers {
                    return None;
                }
                1 + index as usize
            }
            other => {
                let tail = TAIL_ROLES.iter().position(|r| *r == other)?;
                1 + self.workers + tail
            }
        };
        self.slots.get(i)
    }

    /// Accounts one run of `role`'s work. Allocation-free and lock-free: two
    /// relaxed read-modify-writes, so the audio thread may call it.
    pub fn add(&self, role: Role, index: u32, nanos: u64) {
        if let Some(slot) = self.slot_of(role, index) {
            slot.busy_nanos.fetch_add(nanos, Ordering::Relaxed);
            slot.calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Seconds since the table was built -- the wall time the busy figures are
    /// a fraction of. `0.0` on wasm32, which has no monotonic clock.
    pub fn uptime(&self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.epoch.elapsed().as_secs_f64()
        }
        #[cfg(target_arch = "wasm32")]
        {
            0.0
        }
    }

    /// Every slot, in wire order: audio, the workers by index, then net, nrt
    /// and faust. Allocates, so it is the network thread's call and not the
    /// audio thread's.
    pub fn report(&self) -> Vec<Load> {
        self.slots
            .iter()
            .map(|s| Load {
                role: s.role,
                index: s.index,
                busy: s.busy_nanos.load(Ordering::Relaxed) as f64 / 1e9,
                calls: s.calls.load(Ordering::Relaxed),
            })
            .collect()
    }
}

/// The start of a bracket. Read it back with [`Stamp::elapsed_nanos`] and hand
/// that to [`Meters::add`].
#[derive(Clone, Copy)]
pub struct Stamp {
    #[cfg(not(target_arch = "wasm32"))]
    at: std::time::Instant,
}

/// Opens a bracket. RT-safe (see the module docs); inert on wasm32.
pub fn stamp() -> Stamp {
    Stamp {
        #[cfg(not(target_arch = "wasm32"))]
        at: std::time::Instant::now(),
    }
}

impl Stamp {
    /// Nanoseconds since the bracket opened; `0` on wasm32.
    pub fn elapsed_nanos(self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.at.elapsed().as_nanos() as u64
        }
        #[cfg(target_arch = "wasm32")]
        {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_lists_every_role_and_one_slot_per_worker() {
        let meters = Meters::new(3);
        let report = meters.report();
        assert_eq!(report.len(), 4 + TAIL_ROLES.len());
        assert_eq!(report[0].role, Role::Audio);
        assert_eq!(report[3].index, 2);
        assert!(report.iter().all(|l| l.busy == 0.0 && l.calls == 0));
    }

    #[test]
    fn adding_accumulates_on_the_named_slot_only() {
        let meters = Meters::new(2);
        meters.add(Role::Dsp, 1, 1_000_000);
        meters.add(Role::Dsp, 1, 500_000);
        let report = meters.report();
        let dsp1 = report
            .iter()
            .find(|l| l.role == Role::Dsp && l.index == 1)
            .unwrap();
        assert_eq!(dsp1.calls, 2);
        assert!((dsp1.busy - 0.0015).abs() < 1e-9);
        assert!(report.iter().filter(|l| l.calls > 0).count() == 1);
    }

    #[test]
    fn a_worker_index_the_server_does_not_have_is_dropped() {
        let meters = Meters::new(1);
        meters.add(Role::Dsp, 7, 1_000);
        assert!(meters.report().iter().all(|l| l.calls == 0));
    }
}
