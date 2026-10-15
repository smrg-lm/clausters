//! The embedded server's SIGXCPU guard takes only the default disposition.
//! A signal disposition is process-wide, so this lives in its own test
//! binary, in one test, rather than racing the rest of the suite.
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
