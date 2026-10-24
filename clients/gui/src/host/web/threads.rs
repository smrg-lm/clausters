//! **The application's threads in a page**: a worker each, over the module
//! and its shared memory (the applications' build only).
//!
//! `clausters_core::threads` spawns through the spawner this registers: the
//! body is boxed into the shared memory, the page's `__claustersApps.spawn`
//! makes a worker that instantiates the module over that memory, and the
//! worker hands the box back to [`apps_thread_run`], which runs it on the
//! worker's own stack. The page side is `clients/web/src/gui/apps.ts`.

use std::io;

use wasm_bindgen::prelude::*;

/// A body to run on a thread, as it crosses to the worker.
type Body = Box<dyn FnOnce() + Send>;

#[wasm_bindgen]
extern "C" {
    /// The page's door to a worker: makes one named `name` that runs the body
    /// at `body` ([`apps_thread_run`]), and answers whether it could.
    #[wasm_bindgen(js_namespace = __claustersApps, js_name = spawn)]
    fn page_spawn(name: &str, body: u32) -> bool;
}

/// **Registers the page's workers as the application's threads**: from now
/// on `clausters_core::threads::spawn` makes one.
pub(super) fn register() {
    clausters_core::threads::set_spawner(spawn);
}

fn spawn(name: &str, body: Body) -> io::Result<()> {
    let ptr = Box::into_raw(Box::new(body));
    if page_spawn(name, ptr as u32) {
        return Ok(());
    }
    // SAFETY: the page made no worker, so nothing else holds the box.
    drop(unsafe { Box::from_raw(ptr) });
    Err(io::Error::other("the page could not make a worker"))
}

/// **Runs a thread's body on the worker that was made for it**: the page's
/// worker calls this once, with the pointer [`spawn`] handed it.
#[wasm_bindgen]
pub fn apps_thread_run(body: u32) {
    // SAFETY: `body` is the box `spawn` leaked for this one worker, handed
    // over once.
    let body = unsafe { Box::from_raw(body as *mut Body) };
    body();
}

/// What the suite's check of a thread left: 0 until the thread ran, 1 while
/// it waits to be woken, 2 once it was.
static CHECKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

thread_local! {
    /// The thread the suite's check spawned, to wake.
    static CHECK: std::cell::RefCell<Option<clausters_core::threads::Spawned>> =
        const { std::cell::RefCell::new(None) };
}

/// **The suite's check of a page's threads** (`clients/web/tests/apps.html`):
/// spawns one that parks until [`apps_thread_check_wake`] wakes it from the
/// main thread, with what it did read by [`apps_thread_checked`].
#[wasm_bindgen]
pub fn apps_thread_check() -> Result<(), JsError> {
    use std::sync::atomic::Ordering;
    CHECKED.store(0, Ordering::Release);
    let spawned = clausters_core::threads::spawn("check", || {
        CHECKED.store(1, Ordering::Release);
        while CHECKED.load(Ordering::Acquire) == 1 {
            std::thread::park();
        }
    })
    .map_err(|e| JsError::new(&e.to_string()))?;
    CHECK.with(|c| *c.borrow_mut() = Some(spawned));
    Ok(())
}

/// Wakes the check's thread, telling it it was woken.
#[wasm_bindgen]
pub fn apps_thread_check_wake() {
    use std::sync::atomic::Ordering;
    let _ = CHECKED.compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire);
    CHECK.with(|c| {
        if let Some(spawned) = c.borrow().as_ref() {
            spawned.unpark();
        }
    });
}

/// What the check's thread did: 0, 1 or 2 ([`apps_thread_check`]).
#[wasm_bindgen]
pub fn apps_thread_checked() -> u32 {
    CHECKED.load(std::sync::atomic::Ordering::Acquire)
}
