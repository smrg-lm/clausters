//! The SIGXCPU guard and the RT limit it watches: the embedded server takes
//! only the default disposition, and the soft limit returns to half the hard.
//! Both are process-wide, so they live in their own test binary rather than
//! racing the rest of the suite; the two tests touch different state.
#![cfg(all(feature = "rtprio", target_os = "linux"))]

fn disposition() -> libc::sighandler_t {
    // SAFETY: a query of the current disposition into a zeroed struct.
    unsafe {
        let mut old: libc::sigaction = std::mem::zeroed();
        assert_eq!(
            libc::sigaction(libc::SIGXCPU, std::ptr::null(), &mut old),
            0
        );
        old.sa_sigaction
    }
}

#[test]
fn the_guard_replaces_the_default_and_leaves_a_choice_alone() {
    // A disposition the process chose survives.
    // SAFETY: setting SIGXCPU to ignore in a process that raises none.
    unsafe { libc::signal(libc::SIGXCPU, libc::SIG_IGN) };
    clausters::server::rt::adopt_sigxcpu_guard();
    assert_eq!(disposition(), libc::SIG_IGN);

    // The default, which would kill the process, is replaced.
    // SAFETY: restoring the default disposition.
    unsafe { libc::signal(libc::SIGXCPU, libc::SIG_DFL) };
    clausters::server::rt::adopt_sigxcpu_guard();
    let installed = disposition();
    assert_ne!(installed, libc::SIG_DFL);
    assert_ne!(installed, libc::SIG_IGN);
}

#[test]
fn the_rttime_soft_limit_goes_back_to_half_the_hard_one() {
    // What the real-time promotion leaves: a soft limit of one buffer.
    let set = |soft, hard| {
        let limit = libc::rlimit {
            rlim_cur: soft,
            rlim_max: hard,
        };
        // SAFETY: the process's own limit, from a struct on the stack.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &limit) }, 0);
    };
    let get = || {
        // SAFETY: as above.
        unsafe {
            let mut limit: libc::rlimit = std::mem::zeroed();
            assert_eq!(libc::getrlimit(libc::RLIMIT_RTTIME, &mut limit), 0);
            (limit.rlim_cur, limit.rlim_max)
        }
    };
    set(2_666, 200_000);
    clausters::server::rt::lift_rttime();
    assert_eq!(get(), (100_000, 200_000));
    // A soft limit already above half is left alone.
    set(150_000, 200_000);
    clausters::server::rt::lift_rttime();
    assert_eq!(get(), (150_000, 200_000));
}
