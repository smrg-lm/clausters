//! Experimental real-time tuning, compiled only with the `rtprio` feature --
//! everything platform-specific the default build must not carry. With the
//! feature on, cpal promotes the audio callback thread to SCHED_FIFO/RR
//! (RTKit over DBus via `audio_thread_priority`); this module adds the three
//! pieces around that promotion:
//!
//! - the **scheduling diagnostic**: the callback thread publishes the policy
//!   the kernel actually gave it (a promotion can fail silently) and
//!   [`spawn_diag_report`] logs it shortly after boot;
//! - **CPU affinity** (`--pin`): [`request_audio_pin`] for the callback
//!   thread (it pins itself on its first callback -- the thread is spawned
//!   deep inside cpal/PipeWire) and [`pin_workers`] for the DSP workers;
//!   `--pin fast` gives both the set [`fastest_cpus`] finds, and the
//!   diagnostic says which CPU the callback runs on;
//! - the **SIGXCPU guard**: RTKit imposes `RLIMIT_RTTIME` on the thread it
//!   promotes, so sustained >100% load -- where the callback no longer sleeps
//!   between cycles -- makes the kernel raise SIGXCPU, which by default kills
//!   the process with a core dump. [`install_sigxcpu_guard`] replaces that
//!   with a demotion of the audio thread back to SCHED_OTHER: the audio
//!   degrades, the server survives. An embedded server arms it through
//!   [`adopt_sigxcpu_guard`], which leaves a disposition the host process
//!   chose in place.
//!
//! Linux only; every entry point is a documented no-op elsewhere. The CPU
//! meter is deliberately *not* here: it is portable and RT-safe, and lives in
//! `Engine::process_block` unconditionally.

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

/// CPUs the audio callback thread may run on (`--pin`), a bit per CPU over
/// the first 1024; all clear = no pin. Written by the binary *before* the
/// backend starts, read by [`RtSetup`] on the thread's first callback.
static PIN_AUDIO: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];

/// The CPU the audio callback ran on when the diagnostic was read (`-1`
/// until then).
#[cfg(target_os = "linux")]
static AUDIO_CPU: AtomicI32 = AtomicI32::new(-1);

/// Kernel tid of the audio callback thread, published on its first callback
/// (`0` until then) -- the thread the SIGXCPU guard demotes.
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
/// (EPERM), so any later policy change on the thread -- the SIGXCPU demotion --
/// must keep it OR'd in, and any policy read must mask it out.
#[cfg(target_os = "linux")]
const SCHED_RESET_ON_FORK: i32 = 0x4000_0000;

/// Requests that the audio callback thread restrict itself to `cpus` on its
/// first callback (`--pin`: its first value, or the fastest cores for
/// `--pin fast`). CPUs past 1023 are ignored. Call before the backend starts.
pub fn request_audio_pin(cpus: &[usize]) {
    for &cpu in cpus.iter().filter(|&&c| c < 1024) {
        PIN_AUDIO[cpu / 64].fetch_or(1 << (cpu % 64), Ordering::Relaxed);
    }
}

/// Callback number at which the scheduling diagnostic is read: late enough
/// that cpal's real-time promotion (first cycle on the PipeWire host, thread
/// start on ALSA) has already happened.
const DIAG_AT_CALLBACK: u32 = 64;

/// One-shot per-stream setup running *on* the callback thread: tid + optional
/// CPU pin on the first callback, scheduling diagnostic at
/// `DIAG_AT_CALLBACK`, and [`lift_rttime`] after each promotion (the first
/// callback and every change of the frame count). All cold paths; after
/// that [`RtSetup::on_callback`] is two compares per callback.
pub struct RtSetup {
    calls: u32,
    frames: usize,
}

impl RtSetup {
    pub fn new() -> Self {
        Self {
            calls: 0,
            frames: 0,
        }
    }

    /// Runs on every callback of `frames` frames. cpal promotes the thread
    /// before the callback, on the first cycle and again whenever the device
    /// quantum changes, and each promotion leaves `RLIMIT_RTTIME` at one
    /// buffer: [`lift_rttime`] runs after each of them.
    #[inline]
    pub fn on_callback(&mut self, frames: usize) {
        if frames != self.frames {
            self.frames = frames;
            lift_rttime();
        }
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
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            let mut any = false;
            for (word, bits) in PIN_AUDIO.iter().enumerate() {
                let bits = bits.load(Ordering::Relaxed);
                for bit in 0..64 {
                    if bits & (1 << bit) != 0 {
                        libc::CPU_SET(word * 64 + bit, &mut set);
                        any = true;
                    }
                }
            }
            if any {
                libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn publish_diag(&self) {
        // SAFETY: read-only scheduling queries on the calling thread (tid 0).
        unsafe {
            AUDIO_CPU.store(libc::sched_getcpu(), Ordering::Relaxed);
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
/// are logged and ignored -- pinning is a tuning aid, never a boot blocker.
#[cfg(target_os = "linux")]
pub fn pin_workers(cpus: &[usize]) {
    pin_worker_threads(cpus, false);
}

/// Restricts every `clausters-dsp-N` worker to the whole of `cpus` (`--pin
/// fast`), leaving the kernel to balance them within it.
#[cfg(target_os = "linux")]
pub fn pin_workers_within(cpus: &[usize]) {
    pin_worker_threads(cpus, true);
}

#[cfg(target_os = "linux")]
fn pin_worker_threads(cpus: &[usize], within: bool) {
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
        let chosen: &[usize] = if within {
            cpus
        } else {
            std::slice::from_ref(&cpus[i % cpus.len()])
        };
        // SAFETY: plain affinity syscall on one of our own thread ids.
        let rc = unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            for &cpu in chosen {
                libc::CPU_SET(cpu, &mut set);
            }
            libc::sched_setaffinity(*tid, std::mem::size_of::<libc::cpu_set_t>(), &set)
        };
        let named = cpu_list(chosen);
        if rc == 0 {
            tracing::info!("pinned DSP worker (tid {tid}) to CPU {named}");
        } else {
            tracing::warn!("--pin: could not pin tid {tid} to CPU {named}");
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn pin_workers(_cpus: &[usize]) {
    tracing::warn!("--pin is only supported on Linux");
}

#[cfg(not(target_os = "linux"))]
pub fn pin_workers_within(_cpus: &[usize]) {
    tracing::warn!("--pin is only supported on Linux");
}

/// `0,1,4-7`: a CPU list the way the kernel writes one.
pub fn cpu_list(cpus: &[usize]) -> String {
    let mut sorted = cpus.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 1 {
            j += 1;
        }
        parts.push(if i == j {
            sorted[i].to_string()
        } else {
            format!("{}-{}", sorted[i], sorted[j])
        });
        i = j + 1;
    }
    parts.join(",")
}

/// Parses a kernel CPU list (`0-11`, `0,2,4-7`).
#[cfg(any(target_os = "linux", test))]
fn parse_cpu_list(text: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => cpus.extend(a.parse::<usize>().ok()?..=b.parse::<usize>().ok()?),
            None => cpus.push(part.parse().ok()?),
        }
    }
    Some(cpus)
}

/// The machine's fastest CPUs, for `--pin fast`, and the reason they were
/// chosen; `None` when nothing says the CPUs differ.
///
/// On a hybrid CPU the kernel's real-time scheduler places a SCHED_FIFO/RR
/// thread for latency, not for the core's speed, so the audio thread lands
/// on an efficiency core as readily as on a performance one -- measured at
/// 53 % against 32 % CPU for the same load. Three sources, in order: the
/// performance-core list Linux publishes for a hybrid x86 CPU
/// (`/sys/devices/cpu_core/cpus`), the per-CPU capacity of an asymmetric
/// ARM system (`cpu_capacity`, the highest kept), and the CPUs' maximum
/// frequencies (those within 10 % of the highest).
#[cfg(target_os = "linux")]
pub fn fastest_cpus() -> Option<(Vec<usize>, &'static str)> {
    let read = |path: String| std::fs::read_to_string(path).ok();
    let online = read("/sys/devices/system/cpu/online".into()).and_then(|t| parse_cpu_list(&t))?;
    if let Some(cores) = read("/sys/devices/cpu_core/cpus".into()).and_then(|t| parse_cpu_list(&t))
        && !cores.is_empty()
        && cores.len() < online.len()
    {
        return Some((cores, "the performance cores"));
    }
    let per_cpu = |file: &str| -> Option<Vec<(usize, u64)>> {
        online
            .iter()
            .map(|&c| {
                let v = read(format!("/sys/devices/system/cpu/cpu{c}/{file}"))?;
                Some((c, v.trim().parse().ok()?))
            })
            .collect()
    };
    let keep = |values: Vec<(usize, u64)>, at_least: fn(u64) -> u64| {
        let max = values.iter().map(|&(_, v)| v).max()?;
        let floor = at_least(max);
        let fast: Vec<usize> = values
            .iter()
            .filter(|&&(_, v)| v >= floor)
            .map(|&(c, _)| c)
            .collect();
        (fast.len() < values.len()).then_some(fast)
    };
    if let Some(fast) = per_cpu("cpu_capacity").and_then(|v| keep(v, |max| max)) {
        return Some((fast, "the highest-capacity cores"));
    }
    per_cpu("cpufreq/cpuinfo_max_freq")
        .and_then(|v| keep(v, |max| max - max / 10))
        .map(|fast| (fast, "the cores with the highest maximum frequency"))
}

#[cfg(not(target_os = "linux"))]
pub fn fastest_cpus() -> Option<(Vec<usize>, &'static str)> {
    None
}

/// Raises the soft `RLIMIT_RTTIME` back to half of the hard one.
///
/// To obtain real-time scheduling from RTKit a process must hold an
/// `RLIMIT_RTTIME` no larger than RTKit's own maximum (200 ms by default),
/// and the promotion cpal uses sets the soft limit to the duration of *one
/// buffer* while doing so -- 2.67 ms at 128 frames and 48 kHz. Left there, a
/// single callback that overruns its period by a scheduler tick raises
/// SIGXCPU, and the guard demotes the thread for the rest of the process's
/// life: one late buffer degrades every buffer after it. The watchdog is
/// meant for a thread that has stopped yielding, so the soft limit goes back
/// to half the hard one -- 100 ms under RTKit's default -- which leaves the
/// guard room to demote the thread before the hard limit's SIGKILL. Raising
/// a soft limit up to the hard one needs no privilege. One syscall, run only
/// after a promotion.
#[cfg(target_os = "linux")]
pub fn lift_rttime() {
    // SAFETY: reads and writes the process's own resource limit through a
    // struct on the stack.
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_RTTIME, &mut limit) != 0
            || limit.rlim_max == libc::RLIM_INFINITY
        {
            return;
        }
        let soft = limit.rlim_max / 2;
        if limit.rlim_cur < soft {
            limit.rlim_cur = soft;
            libc::setrlimit(libc::RLIMIT_RTTIME, &limit);
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn lift_rttime() {}

/// Installs the SIGXCPU guard. RTKit grants real-time scheduling under an
/// `RLIMIT_RTTIME` watchdog (the soft limit [`lift_rttime`] keeps at 100 ms
/// of *continuous* RT CPU without blocking); a server driven past sustained
/// 100% load trips it, and the
/// signal's default disposition kills the process with a core dump. The
/// handler instead demotes the audio thread back to SCHED_OTHER -- the audio
/// degrades, the server survives (a restart re-promotes it). Call once at
/// boot, before the audio stream exists. A signal disposition is
/// process-global, so the server binary installs it unconditionally and an
/// embedded server goes through [`adopt_sigxcpu_guard`] instead.
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

/// The embedded server's form of [`install_sigxcpu_guard`]: installs the
/// guard only while SIGXCPU still has its default disposition. The process
/// belongs to whoever embeds the server, so a handler it set (or an
/// explicit ignore) is left alone; what is replaced is only the default,
/// which would kill that process with a core dump.
#[cfg(target_os = "linux")]
pub fn adopt_sigxcpu_guard() {
    // SAFETY: a query of the current disposition into a zeroed struct.
    let default = unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGXCPU, std::ptr::null(), &mut old) == 0
            && old.sa_sigaction == libc::SIG_DFL
    };
    if default {
        install_sigxcpu_guard();
    }
}

#[cfg(not(target_os = "linux"))]
pub fn adopt_sigxcpu_guard() {}

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
    // stderr -- all async-signal-safe.
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
/// promotion attempt) -- the way to verify the server's real-time permissions:
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
                    report_core();
                }
            }
            return;
        }
        tracing::warn!("audio thread scheduling unknown: no audio callback ran in 10 s");
    });
}

/// Says which CPU the audio callback was on and, on a machine whose cores
/// differ, whether it is one of the fast ones: the same load costs half
/// again as much on an efficiency core, and nothing else makes that visible.
#[cfg(target_os = "linux")]
fn report_core() {
    let cpu = AUDIO_CPU.load(Ordering::Relaxed);
    if cpu < 0 {
        return;
    }
    let pinned = PIN_AUDIO.iter().any(|w| w.load(Ordering::Relaxed) != 0);
    match fastest_cpus() {
        Some((fast, _)) if fast.contains(&(cpu as usize)) => {
            tracing::info!(
                "audio thread on CPU {cpu}, one of the fastest ({})",
                cpu_list(&fast)
            );
        }
        Some((fast, _)) if !pinned => tracing::info!(
            "audio thread on CPU {cpu}, a slower core of this hybrid CPU (the fastest are \
             {}); `--pin fast` keeps it on those",
            cpu_list(&fast)
        ),
        _ => tracing::info!("audio thread on CPU {cpu}"),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn spawn_diag_report() {}

#[cfg(test)]
mod tests {
    use super::{cpu_list, parse_cpu_list};

    #[test]
    fn a_cpu_list_reads_and_writes_the_kernel_spelling() {
        assert_eq!(
            parse_cpu_list("0-3,6,8-9\n"),
            Some(vec![0, 1, 2, 3, 6, 8, 9])
        );
        assert_eq!(parse_cpu_list("12"), Some(vec![12]));
        assert_eq!(parse_cpu_list("0-x"), None);
        assert_eq!(cpu_list(&[9, 0, 1, 2, 3, 6, 8]), "0-3,6,8-9");
        assert_eq!(cpu_list(&[4]), "4");
    }
}
