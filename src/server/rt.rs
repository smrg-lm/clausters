//! Experimental real-time tuning, compiled only with the `rtprio` feature —
//! everything platform-specific the default build must not carry. With the
//! feature on, cpal promotes the audio callback thread to SCHED_FIFO/RR
//! (RTKit over DBus via `audio_thread_priority`); this module adds the three
//! pieces around that promotion:
//!
//! - the **scheduling diagnostic**: the callback thread publishes the policy
//!   the kernel actually gave it (a promotion can fail silently) and
//!   [`spawn_diag_report`] logs it shortly after boot;
//! - **CPU affinity** (`--pin`): [`request_audio_pin`] for the callback
//!   thread (it pins itself on its first callback — the thread is spawned
//!   deep inside cpal/PipeWire) and [`pin_workers`] for the DSP workers;
//! - the **SIGXCPU guard**: RTKit imposes `RLIMIT_RTTIME` on the thread it
//!   promotes, so sustained >100% load — where the callback no longer sleeps
//!   between cycles — makes the kernel raise SIGXCPU, which by default kills
//!   the process with a core dump. [`install_sigxcpu_guard`] replaces that
//!   with a demotion of the audio thread back to SCHED_OTHER: the audio
//!   degrades, the server survives.
//!
//! Linux only; every entry point is a documented no-op elsewhere. The CPU
//! meter is deliberately *not* here: it is portable and RT-safe, and lives in
//! `Engine::process_block` unconditionally.

use std::sync::atomic::{AtomicI32, Ordering};

/// CPU requested for the audio callback thread (`--pin`, first value); `-1`
/// = no pin. Written by the binary *before* the backend starts, read by
/// [`RtSetup`] on the thread's first callback.
static PIN_AUDIO: AtomicI32 = AtomicI32::new(-1);

/// Kernel tid of the audio callback thread, published on its first callback
/// (`0` until then) — the thread the SIGXCPU guard demotes.
#[cfg(target_os = "linux")]
static AUDIO_TID: AtomicI32 = AtomicI32::new(0);

/// Scheduling policy the audio callback thread actually got
/// (`sched_getscheduler`: `SCHED_OTHER` = 0, `SCHED_FIFO` = 1, `SCHED_RR` =
/// 2; `-1` = not yet published).
#[cfg(target_os = "linux")]
static POLICY: AtomicI32 = AtomicI32::new(-1);

/// `sched_priority` of the audio callback thread; meaningful for
/// `SCHED_FIFO`/`SCHED_RR` only.
#[cfg(target_os = "linux")]
static PRIORITY: AtomicI32 = AtomicI32::new(0);

/// RTKit promotes with this flag OR'd into the policy, and it sticks: the
/// kernel refuses an unprivileged `sched_setscheduler` that would *clear* it
/// (EPERM), so any later policy change on the thread — the SIGXCPU demotion —
/// must keep it OR'd in, and any policy read must mask it out.
#[cfg(target_os = "linux")]
const SCHED_RESET_ON_FORK: i32 = 0x4000_0000;

/// Requests that the audio callback thread pin itself to `cpu` on its first
/// callback (`--pin`, first value). Call before the backend starts.
pub fn request_audio_pin(cpu: usize) {
    PIN_AUDIO.store(cpu as i32, Ordering::Relaxed);
}

/// Callback number at which the scheduling diagnostic is read: late enough
/// that cpal's real-time promotion (first cycle on the PipeWire host, thread
/// start on ALSA) has already happened.
const DIAG_AT_CALLBACK: u32 = 64;

/// One-shot per-stream setup running *on* the callback thread: tid + optional
/// CPU pin on the first callback, scheduling diagnostic at
/// `DIAG_AT_CALLBACK`. All cold paths (a syscall each, once); after that
/// [`RtSetup::on_callback`] is a single compare per callback.
pub struct RtSetup {
    calls: u32,
}

impl RtSetup {
    pub fn new() -> Self {
        Self { calls: 0 }
    }

    #[inline]
    pub fn on_callback(&mut self) {
        if self.calls > DIAG_AT_CALLBACK {
            return;
        }
        if self.calls == 0 {
            self.publish_tid_and_pin();
        }
        if self.calls == DIAG_AT_CALLBACK {
            self.publish_diag();
        }
        self.calls += 1;
    }

    #[cfg(target_os = "linux")]
    fn publish_tid_and_pin(&self) {
        // SAFETY: gettid never fails; the affinity syscall targets the
        // calling thread (tid 0).
        unsafe {
            AUDIO_TID.store(libc::syscall(libc::SYS_gettid) as i32, Ordering::Relaxed);
            let cpu = PIN_AUDIO.load(Ordering::Relaxed);
            if cpu >= 0 {
                let mut set: libc::cpu_set_t = std::mem::zeroed();
                libc::CPU_SET(cpu as usize, &mut set);
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn publish_diag(&self) {
        // SAFETY: read-only scheduling queries on the calling thread (tid 0).
        unsafe {
            let policy = libc::sched_getscheduler(0);
            let mut param: libc::sched_param = std::mem::zeroed();
            libc::sched_getparam(0, &mut param);
            PRIORITY.store(param.sched_priority, Ordering::Relaxed);
            POLICY.store(policy, Ordering::Relaxed);
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn publish_tid_and_pin(&self) {}

    #[cfg(not(target_os = "linux"))]
    fn publish_diag(&self) {}
}

impl Default for RtSetup {
    fn default() -> Self {
        Self::new()
    }
}

/// Pins the `clausters-dsp-N` worker threads round-robin onto `cpus`
/// (`--pin`, all CPUs after the first). Runs on the main thread right after
/// boot: it scans `/proc/self/task` for the workers by thread name. Failures
/// are logged and ignored — pinning is a tuning aid, never a boot blocker.
#[cfg(target_os = "linux")]
pub fn pin_workers(cpus: &[usize]) {
    let entries = match std::fs::read_dir("/proc/self/task") {
        Ok(entries) => entries,
        Err(e) => return tracing::warn!("--pin: cannot scan threads: {e}"),
    };
    let mut tids: Vec<i32> = entries
        .flatten()
        .filter_map(|entry| {
            let tid: i32 = entry.file_name().to_str()?.parse().ok()?;
            let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            comm.starts_with("clausters-dsp-").then_some(tid)
        })
        .collect();
    tids.sort_unstable();
    if tids.is_empty() {
        tracing::warn!("--pin: no DSP workers to pin (running with --workers 0?)");
        return;
    }
    for (i, tid) in tids.iter().enumerate() {
        let cpu = cpus[i % cpus.len()];
        // SAFETY: plain affinity syscall on one of our own thread ids.
        let rc = unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            libc::sched_setaffinity(*tid, std::mem::size_of::<libc::cpu_set_t>(), &set)
        };
        if rc == 0 {
            tracing::info!("pinned DSP worker (tid {tid}) to CPU {cpu}");
        } else {
            tracing::warn!("--pin: could not pin tid {tid} to CPU {cpu}");
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn pin_workers(_cpus: &[usize]) {
    tracing::warn!("--pin is only supported on Linux");
}

/// Installs the SIGXCPU guard. RTKit grants real-time scheduling under an
/// `RLIMIT_RTTIME` watchdog (~200 ms of *continuous* RT CPU without
/// blocking); a server driven past sustained 100% load trips it, and the
/// signal's default disposition kills the process with a core dump. The
/// handler instead demotes the audio thread back to SCHED_OTHER — the audio
/// degrades, the server survives (a restart re-promotes it). Call once at
/// boot, before the audio stream exists; a signal disposition is
/// process-global, which is why the *binary* installs it, not the library.
#[cfg(target_os = "linux")]
pub fn install_sigxcpu_guard() {
    // SAFETY: replacing the disposition of a signal only RTKit's watchdog
    // raises in this process, with a handler that is async-signal-safe.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_sigxcpu as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGXCPU, &sa, std::ptr::null_mut());
    }
}

#[cfg(not(target_os = "linux"))]
pub fn install_sigxcpu_guard() {}

/// The guard itself. Async-signal-safe only: raw syscalls, no allocation, no
/// locks, no formatting. The kernel delivers `RLIMIT_RTTIME`'s SIGXCPU to the
/// offending thread, but demote both the published audio tid and the calling
/// thread in case they ever differ. Once demoted to SCHED_OTHER the RT clock
/// no longer accrues, so the signal stops firing.
#[cfg(target_os = "linux")]
extern "C" fn on_sigxcpu(_signal: libc::c_int) {
    let other = libc::sched_param { sched_priority: 0 };
    // The RTKit-promoted thread carries SCHED_RESET_ON_FORK; demoting it
    // without re-passing the flag is EPERM for an unprivileged caller.
    let policy = libc::SCHED_OTHER | SCHED_RESET_ON_FORK;
    // SAFETY: scheduling syscalls on our own threads plus one write(2) to
    // stderr — all async-signal-safe.
    unsafe {
        let tid = AUDIO_TID.load(Ordering::Relaxed);
        if tid > 0 {
            libc::sched_setscheduler(tid, policy, &other);
        }
        libc::sched_setscheduler(0, policy, &other);
        const MSG: &[u8] = b"clausters: RT CPU budget exceeded (SIGXCPU): \
audio thread demoted to SCHED_OTHER, expect glitches until restart\n";
        libc::write(2, MSG.as_ptr().cast(), MSG.len());
    }
}

/// Logs, a moment after boot, the scheduling the audio callback thread
/// **actually** got (the callback publishes it after cpal's real-time
/// promotion attempt) — the way to verify the server's real-time permissions:
/// SCHED_FIFO/SCHED_RR is healthy, SCHED_OTHER means the promotion failed and
/// xruns will appear well below full CPU load.
#[cfg(target_os = "linux")]
pub fn spawn_diag_report() {
    std::thread::spawn(|| {
        // The callback publishes at its 64th call: ~90 ms at 48 kHz with a
        // 64-frame quantum, a few seconds with large buffers. Poll briefly.
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            // `sched_getscheduler` reports SCHED_RESET_ON_FORK OR'd into the
            // policy; mask the flag to read it.
            match POLICY.load(Ordering::Relaxed) {
                -1 => continue,
                0 => {
                    tracing::warn!(
                        "audio thread runs WITHOUT real-time scheduling (SCHED_OTHER): \
                         expect xruns under load. Is rtkit running? (built with `rtprio`: \
                         the promotion goes through RTKit over DBus)"
                    );
                }
                policy => {
                    let name = match policy & !SCHED_RESET_ON_FORK {
                        1 => "SCHED_FIFO",
                        2 => "SCHED_RR",
                        _ => "SCHED_?",
                    };
                    tracing::info!(
                        "audio thread is real-time: {name} priority {}",
                        PRIORITY.load(Ordering::Relaxed)
                    );
                }
            }
            return;
        }
        tracing::warn!("audio thread scheduling unknown: no audio callback ran in 10 s");
    });
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_diag_report() {}
