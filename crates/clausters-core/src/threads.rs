//! **A thread of an application's own**: `std::thread` natively, and in a
//! page a worker over the same module and its shared memory.
//!
//! An editing application does its slow work off its window's turn: a render
//! on a thread of its own, a take's reading on another. `std::thread` cannot
//! spawn on `wasm32-unknown-unknown`, where a thread is a worker the page
//! makes and instantiates the module in, over the memory the module shares;
//! so the one door here is `std::thread` natively, and in a page whatever the
//! host registers (`set_spawner`) once it can make workers.
//!
//! **A page's main thread cannot block**: a wasm `memory.atomic.wait` there
//! traps. So what is spawned here is woken ([`Spawned::unpark`]) and never
//! joined from a page: natively [`Spawned::join`] waits for the thread, and
//! in a page it lets it go, to end on its own.

use std::io;
use std::sync::{Arc, OnceLock};
use std::thread::Thread;

/// **A thread spawned here**, which can be woken and, natively, waited for.
#[derive(Debug)]
pub struct Spawned {
    /// The thread, as it says itself once it runs: a waker before that has
    /// nothing to wake, and the thread looks for its work when it starts.
    thread: Arc<OnceLock<Thread>>,
    #[cfg(not(target_arch = "wasm32"))]
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spawned {
    /// **Wakes the thread** from a [`std::thread::park`]; one that has not
    /// started yet has nothing to wake.
    pub fn unpark(&self) {
        if let Some(thread) = self.thread.get() {
            thread.unpark();
        }
    }

    /// **Waits for the thread to end** natively; in a page lets it go, since
    /// a page's main thread cannot wait.
    pub fn join(mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = &mut self;
    }
}

/// What makes a thread in a page: the host's, registered with [`set_spawner`].
#[cfg(target_arch = "wasm32")]
pub type Spawner = fn(&str, Box<dyn FnOnce() + Send>) -> io::Result<()>;

#[cfg(target_arch = "wasm32")]
static SPAWNER: OnceLock<Spawner> = OnceLock::new();

/// **How a page makes a thread**, from now on: the host registers it once it
/// can make workers over the module and its memory. The first one stays.
#[cfg(target_arch = "wasm32")]
pub fn set_spawner(spawner: Spawner) {
    let _ = SPAWNER.set(spawner);
}

/// **Spawns `body` on a thread named `name`**: a `std::thread` natively, a
/// worker in a page, or `Unsupported` in a page whose host makes none.
pub fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<Spawned> {
    let thread = Arc::new(OnceLock::new());
    let told = Arc::clone(&thread);
    let run = move || {
        let _ = told.set(std::thread::current());
        body();
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let handle = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(run)?;
        Ok(Spawned {
            thread,
            handle: Some(handle),
        })
    }
    #[cfg(target_arch = "wasm32")]
    {
        let spawner = SPAWNER.get().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "this page makes no threads")
        })?;
        spawner(name, Box::new(run))?;
        Ok(Spawned { thread })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// **A spawned thread runs, is woken and is waited for.**
    #[test]
    fn a_thread_runs_is_woken_and_is_joined() {
        let count = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&count);
        let spawned = spawn("test", move || {
            seen.fetch_add(1, Ordering::AcqRel);
            // woken once by the spawner, then done
            while seen.load(Ordering::Acquire) < 2 {
                std::thread::park();
            }
        })
        .unwrap();
        while count.load(Ordering::Acquire) < 1 {
            std::thread::yield_now();
        }
        count.fetch_add(1, Ordering::AcqRel);
        spawned.unpark();
        spawned.join();
        assert_eq!(count.load(Ordering::Acquire), 2);
    }
}
