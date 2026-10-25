//! **What a stream's I/O thread is made of**: its account in the load table,
//! its park, and the wait an offline render makes on it -- shared by the
//! readers and writers that have a thread of their own (`dsp::disk`,
//! `dsp::stream`, `dsp::peaks_out`), natively and in a page whose engine
//! shares its memory with its threads.
//!
//! A page has no clock a thread can read (`std::time::Instant` is not there),
//! so the load table's stretches are not timed in one (`server::meters`
//! says so), and an offline wait counts its sleeps instead of reading a
//! deadline.

use std::sync::Arc;
use std::time::Duration;

use crate::server::meters::{Meters, Role, Stamp, stamp};

/// How long the I/O thread parks when its ring is full/empty before retrying.
const PARK: Duration = Duration::from_millis(2);

/// How long an offline wait sleeps between two looks, once spinning is over.
const NAP: Duration = Duration::from_micros(50);

/// How long an offline wait gives a thread that does nothing.
const GIVE_UP: Duration = Duration::from_secs(10);

/// An I/O thread's account in the load table: the stretches it worked, never
/// the parks between them.
///
/// A stretch runs from one park to the next and is added only if something was
/// done in it -- a thread that wakes to a ring still full, or still empty, did
/// no work and is not a call. Dropping it closes the last stretch, so every
/// way out of a thread's loop is accounted.
pub(crate) struct Turn {
    meters: Arc<Meters>,
    role: Role,
    since: Stamp,
    worked: bool,
}

impl Turn {
    pub(crate) fn new(meters: Arc<Meters>, role: Role) -> Self {
        Self {
            meters,
            role,
            since: stamp(),
            worked: false,
        }
    }

    /// Something was done in this stretch.
    pub(crate) fn worked(&mut self) {
        self.worked = true;
    }

    /// Closes the stretch, parks, and opens the next one. A park is cut short
    /// by an unpark, which is how a reader that follows the transport is
    /// woken for a refill.
    pub(crate) fn park(&mut self) {
        self.close();
        std::thread::park_timeout(PARK);
        self.since = stamp();
    }

    fn close(&mut self) {
        if self.worked {
            self.meters.add(self.role, 0, self.since.elapsed_nanos());
            self.worked = false;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.close();
    }
}

/// **Waits for `ready`**, for a stream an offline render plays: nothing waits
/// on the clock there, so a `DiskIn` waits for its frames and a `DiskOut` for
/// room rather than losing the race. Gives up after ten seconds of nothing --
/// a thread that died -- and says so by answering `false`.
pub(crate) fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    let deadline = std::time::Instant::now() + GIVE_UP;
    #[cfg(target_arch = "wasm32")]
    let mut naps_left = GIVE_UP.as_micros() / NAP.as_micros();
    let mut spins = 0u32;
    while !ready() {
        #[cfg(not(target_arch = "wasm32"))]
        if std::time::Instant::now() > deadline {
            return false;
        }
        spins += 1;
        if spins < 64 {
            std::thread::yield_now();
        } else {
            #[cfg(target_arch = "wasm32")]
            {
                if naps_left == 0 {
                    return false;
                }
                naps_left -= 1;
            }
            std::thread::sleep(NAP);
        }
    }
    true
}
