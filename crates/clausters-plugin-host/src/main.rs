//! One isolated native plugin helper; the parent owns its control socket.
fn main() {
    #[cfg(target_os = "linux")]
    {
        #[cfg(feature = "rtprio")]
        sigxcpu::install();
        let args: Vec<_> = std::env::args().collect();
        let result = if args.len() == 3 && args[1] == "--control-fd" {
            args[2]
                .parse::<i32>()
                .map_err(|e| e.to_string())
                .and_then(|fd| {
                    // SAFETY: the executable contract transfers this inherited descriptor once.
                    unsafe { clausters_plugin_host::runtime::helper_main(fd) }
                })
        } else {
            Err("usage: clausters-plugin-host --control-fd <fd>".into())
        };
        if let Err(error) = result {
            eprintln!("plugin helper: {error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("native plugin hosting currently requires Linux");
        std::process::exit(1);
    }
}

/// RTKit promotes under an `RLIMIT_RTTIME` watchdog whose SIGXCPU kills the
/// process by default. A plugin that spins its processing thread past the
/// budget is demoted back to SCHED_OTHER instead: it keeps running, late.
#[cfg(all(target_os = "linux", feature = "rtprio"))]
mod sigxcpu {
    /// RTKit's promotion carries this flag, and an unprivileged caller may
    /// not clear it, so the demotion passes it again.
    const SCHED_RESET_ON_FORK: i32 = 0x4000_0000;
    pub fn install() {
        // SAFETY: replacing a disposition only RTKit's watchdog raises here,
        // with an async-signal-safe handler.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = demote as *const () as usize;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGXCPU, &sa, std::ptr::null_mut());
        }
    }
    /// The kernel delivers the signal to the thread that exceeded the
    /// budget. Raw syscalls only.
    extern "C" fn demote(_: libc::c_int) {
        let other = libc::sched_param { sched_priority: 0 };
        // SAFETY: a scheduling syscall on the calling thread and one write(2).
        unsafe {
            libc::sched_setscheduler(0, libc::SCHED_OTHER | SCHED_RESET_ON_FORK, &other);
            const MSG: &[u8] = b"clausters-plugin-host: RT CPU budget exceeded, \
processing thread demoted\n";
            libc::write(2, MSG.as_ptr().cast(), MSG.len());
        }
    }
}
