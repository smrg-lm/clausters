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

#[wasm_bindgen]
extern "C" {
    /// The page's door to its storage, on a thread: hands the request at
    /// `request` to the file worker and waits until it is answered.
    #[wasm_bindgen(js_namespace = __claustersApps, js_name = file)]
    fn page_file(request: u32);
}

#[wasm_bindgen]
extern "C" {
    /// The page's door to its loop, from a thread: an application of host
    /// `host` has said something, which the page hands the loop
    /// ([`apps_said`](super::apps_said)).
    #[wasm_bindgen(js_namespace = __claustersApps, js_name = said)]
    fn page_said(host: u32);
}

thread_local! {
    /// Whether this thread is one of the page's workers, which may wait: the
    /// main thread may not, and touches no file.
    static WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// **Registers the page's workers as the application's threads, and its
/// storage as their files**: from now on `clausters_core::threads::spawn`
/// makes a worker and `clausters_core::files` reads and writes the origin's
/// storage from one.
pub(super) fn register() {
    clausters_core::threads::set_spawner(spawn);
    clausters_core::files::set_bridge(file);
}

/// **What wakes host `host`'s loop** when one of its applications has said
/// something, from the application's thread: the page posts it to the main
/// thread, where the loop takes the host's effects.
pub(super) fn wake(host: u32) -> std::sync::Arc<dyn Fn() + Send + Sync> {
    std::sync::Arc::new(move || page_said(host))
}

/// One call into the page's storage, made from a worker.
fn file(request: &clausters_core::files::Request) -> io::Result<()> {
    if !WORKER.with(|w| w.get()) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "a page's main thread touches no file: it cannot wait for one",
        ));
    }
    page_file(request as *const _ as u32);
    Ok(())
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
    WORKER.with(|w| w.set(true));
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

/// What the suite's check of a page's files left: 0 until it ends, 1 when
/// every step held, and 2 with the first that did not in [`apps_files_failed`].
static FILES_CHECKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

static FILES_FAILED: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// **The suite's check of a page's files** (`clients/web/tests/apps.html`):
/// a thread makes a folder, writes a file, reads part of it back from where
/// it sought, lists, moves and removes, through `clausters_core::files` --
/// and the main thread is refused one.
#[wasm_bindgen]
pub fn apps_files_check() -> Result<(), JsError> {
    use clausters_core::files;
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::Ordering;
    if files::create_dir_all("clausters/check").is_ok() {
        return Err(JsError::new("the main thread was let touch a file"));
    }
    FILES_CHECKED.store(0, Ordering::Release);
    clausters_core::threads::spawn("files-check", || {
        let steps = || -> Result<(), String> {
            let fail = |what: &str, e: std::io::Error| format!("{what}: {e}");
            let _ = files::remove_dir_all("clausters/check");
            files::create_dir_all("clausters/check/a").map_err(|e| fail("create_dir_all", e))?;
            files::write("clausters/check/a/take.bin", b"0123456789")
                .map_err(|e| fail("write", e))?;
            let mut file =
                files::File::open("clausters/check/a/take.bin").map_err(|e| fail("open", e))?;
            if file.len().map_err(|e| fail("len", e))? != 10 {
                return Err("len: not 10".into());
            }
            file.seek(SeekFrom::Start(4)).map_err(|e| fail("seek", e))?;
            let mut four = [0u8; 4];
            file.read_exact(&mut four).map_err(|e| fail("read", e))?;
            if &four != b"4567" {
                return Err(format!("read: {four:?}"));
            }
            drop(file);
            let meta =
                files::metadata("clausters/check/a/take.bin").map_err(|e| fail("metadata", e))?;
            if meta.len != 10 || meta.is_dir {
                return Err(format!("metadata: {meta:?}"));
            }
            files::rename("clausters/check/a/take.bin", "clausters/check/moved.bin")
                .map_err(|e| fail("rename", e))?;
            if files::exists("clausters/check/a/take.bin") {
                return Err("rename: the file is still where it was".into());
            }
            let mut names: Vec<String> = files::read_dir("clausters/check")
                .map_err(|e| fail("read_dir", e))?
                .into_iter()
                .map(|e| format!("{}{}", e.name, if e.is_dir { "/" } else { "" }))
                .collect();
            names.sort();
            if names != ["a/", "moved.bin"] {
                return Err(format!("read_dir: {names:?}"));
            }
            if files::read("clausters/check/moved.bin").map_err(|e| fail("read", e))?
                != b"0123456789"
            {
                return Err("read: not what was written".into());
            }
            match files::File::open("clausters/check/none.bin") {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                other => return Err(format!("a missing file: {other:?}")),
            }
            files::remove_dir_all("clausters/check").map_err(|e| fail("remove_dir_all", e))?;
            if files::exists("clausters/check") {
                return Err("remove_dir_all: still there".into());
            }
            Ok(())
        };
        match steps() {
            Ok(()) => FILES_CHECKED.store(1, Ordering::Release),
            Err(why) => {
                if let Ok(mut failed) = FILES_FAILED.lock() {
                    *failed = why;
                }
                FILES_CHECKED.store(2, Ordering::Release);
            }
        }
    })
    .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(())
}

/// What the files check did: 0, 1 or 2 ([`apps_files_check`]).
#[wasm_bindgen]
pub fn apps_files_checked() -> u32 {
    FILES_CHECKED.load(std::sync::atomic::Ordering::Acquire)
}

/// The step the files check failed at, and why.
#[wasm_bindgen]
pub fn apps_files_failed() -> String {
    FILES_FAILED
        .try_lock()
        .map(|f| f.clone())
        .unwrap_or_default()
}

/// What the suite's check of a working folder did: 0 while it runs, 1 while
/// it holds its folder, 3 once it let it go, 2 when a step failed.
static FOLDER_CHECKED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

static FOLDER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

thread_local! {
    /// The thread the folder check spawned, to wake.
    static FOLDER_CHECK: std::cell::RefCell<Option<clausters_core::threads::Spawned>> =
        const { std::cell::RefCell::new(None) };
}

/// **The suite's check of a page's working folder**: a thread finds a folder
/// left by no tab, makes a working folder (`clausters_core::workspace`),
/// which sweeps the left one and holds its own by a Web Lock, and holds it
/// until [`apps_folder_check_wake`] wakes it to let it go.
#[wasm_bindgen]
pub fn apps_folder_check() -> Result<(), JsError> {
    use std::sync::atomic::Ordering;
    FOLDER_CHECKED.store(0, Ordering::Release);
    let spawned = clausters_core::threads::spawn("folder-check", || {
        let held = (|| -> Result<clausters_core::workspace::Workspace, String> {
            clausters_core::files::create_dir_all("clausters/work/check-left")
                .map_err(|e| format!("a folder left behind: {e}"))?;
            let held = clausters_core::workspace::Workspace::create("check")
                .map_err(|e| format!("the working folder: {e}"))?;
            if clausters_core::files::exists("clausters/work/check-left") {
                return Err("the folder no tab holds was not swept".into());
            }
            Ok(held)
        })();
        match held {
            Ok(held) => {
                if let Ok(mut folder) = FOLDER.lock() {
                    *folder = held.path().to_string_lossy().into_owned();
                }
                FOLDER_CHECKED.store(1, Ordering::Release);
                while FOLDER_CHECKED.load(Ordering::Acquire) == 1 {
                    std::thread::park();
                }
                drop(held);
                FOLDER_CHECKED.store(3, Ordering::Release);
            }
            Err(why) => {
                if let Ok(mut folder) = FOLDER.lock() {
                    *folder = why;
                }
                FOLDER_CHECKED.store(2, Ordering::Release);
            }
        }
    })
    .map_err(|e| JsError::new(&e.to_string()))?;
    FOLDER_CHECK.with(|c| *c.borrow_mut() = Some(spawned));
    Ok(())
}

/// Wakes the folder check's thread to let its folder go.
#[wasm_bindgen]
pub fn apps_folder_check_wake() {
    use std::sync::atomic::Ordering;
    let _ = FOLDER_CHECKED.compare_exchange(1, 4, Ordering::AcqRel, Ordering::Acquire);
    FOLDER_CHECK.with(|c| {
        if let Some(spawned) = c.borrow().as_ref() {
            spawned.unpark();
        }
    });
}

/// What the folder check did ([`apps_folder_check`]).
#[wasm_bindgen]
pub fn apps_folder_checked() -> u32 {
    FOLDER_CHECKED.load(std::sync::atomic::Ordering::Acquire)
}

/// The folder the check holds, or why it failed.
#[wasm_bindgen]
pub fn apps_folder() -> String {
    FOLDER.try_lock().map(|f| f.clone()).unwrap_or_default()
}
