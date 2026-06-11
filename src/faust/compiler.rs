//! Dedicated Faust compiler thread.
//!
//! JIT compilation takes ~10 ms per def (measured in F0) and the libfaust
//! lib context is global and not thread-safe, so all compilation runs on one
//! dedicated thread that serializes requests naturally. The network thread
//! submits [`CompileRequest`]s and drains [`CompileResult`]s on its own
//! schedule (after each packet / on its GC tick), then sends the async
//! `/done`/`/fail` reply to the requesting client.

use std::ffi::{CStr, CString, c_char, c_int};
use std::net::SocketAddr;
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::faust::boxes;
use crate::faust::factory::FaustFactory;
use crate::faust::ffi;
use crate::faust::synth::FaustDef;

/// What `/d_faust` carries: either of the two def formats.
pub enum CompilePayload {
    /// Raw Faust source code (F1), compiled with
    /// `createCDSPFactoryFromString`.
    Source(String),
    /// JSON box graph (F2), mapped to Box API calls (see [`boxes`]) and
    /// compiled with `createCDSPFactoryFromBoxes`.
    Json(String),
}

pub struct CompileRequest {
    pub name: String,
    pub payload: CompilePayload,
    /// Who asked: the async reply goes back to this client.
    pub client: SocketAddr,
}

pub struct CompileResult {
    pub name: String,
    pub client: SocketAddr,
    /// The compiled def (factory + probed parameters), or a human-readable
    /// compiler error destined for the `/fail` reply verbatim.
    pub outcome: Result<FaustDef, String>,
}

pub struct CompilerThread {
    /// `Option` so `Drop` can close the channel before joining.
    requests: Option<mpsc::Sender<CompileRequest>>,
    results: mpsc::Receiver<CompileResult>,
    handle: Option<JoinHandle<()>>,
}

impl CompilerThread {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<CompileRequest>();
        let (res_tx, res_rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("faust-compiler".into())
            .spawn(move || {
                while let Ok(req) = req_rx.recv() {
                    let outcome = compile(&req.name, &req.payload);
                    let result = CompileResult {
                        name: req.name,
                        client: req.client,
                        outcome,
                    };
                    if res_tx.send(result).is_err() {
                        break; // receiver gone: we are shutting down
                    }
                }
            })
            .expect("failed to spawn the faust compiler thread");
        Self {
            requests: Some(req_tx),
            results: res_rx,
            handle: Some(handle),
        }
    }

    /// Queues a compilation. Fails only if the compiler thread died.
    pub fn submit(&self, request: CompileRequest) -> Result<(), CompileRequest> {
        match &self.requests {
            Some(tx) => tx.send(request).map_err(|e| e.0),
            None => Err(request),
        }
    }

    /// Non-blocking: one finished compilation, if any.
    pub fn try_result(&self) -> Option<CompileResult> {
        self.results.try_recv().ok()
    }

    /// Blocking with deadline; for tests, which must wait explicitly instead
    /// of sleeping.
    pub fn recv_result_timeout(&self, timeout: Duration) -> Option<CompileResult> {
        self.results.recv_timeout(timeout).ok()
    }
}

impl Drop for CompilerThread {
    fn drop(&mut self) {
        // Closing the request channel ends the thread's recv loop.
        self.requests.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// libfaust keeps global compiler state: concurrent compilations in one
/// process SIGSEGV (verified empirically — parallel test runs crashed). One
/// server has one compiler thread, but tests and embedders may hold several,
/// so the actual FFI call is serialized process-wide.
static COMPILE_LOCK: Mutex<()> = Mutex::new(());

/// Serializes any direct libfaust FFI use (box construction, factory
/// creation) against in-flight compilations. Hold it for the whole
/// `createLibContext`..`destroyLibContext` bracket.
pub fn ffi_lock() -> std::sync::MutexGuard<'static, ()> {
    COMPILE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Compiler arguments handed to libfaust as C `argc`/`argv`.
pub(crate) struct FaustArgs {
    storage: Vec<CString>,
    ptrs: Vec<*const c_char>,
}

impl FaustArgs {
    /// `-I <dir>` for the Faust stdlib (`stdfaust.lib` and friends), so both
    /// raw-source defs and `faust` fragments inside JSON can `import()` it.
    /// The directory comes from `$FAUST_PREFIX/share/faust`, falling back to
    /// `~/.local`, then `/usr/local` — same search order as build.rs.
    pub(crate) fn stdlib() -> Self {
        let mut storage = Vec::new();
        if let Some(dir) = stdlib_dir()
            && let Ok(dir_c) = CString::new(dir)
        {
            storage.push(CString::new("-I").unwrap());
            storage.push(dir_c);
        }
        // CStrings own their bytes on the heap: moving them (or the Vec)
        // does not invalidate these pointers.
        let ptrs = storage.iter().map(|s| s.as_ptr()).collect();
        Self { storage, ptrs }
    }

    pub(crate) fn argc(&self) -> c_int {
        self.storage.len() as c_int
    }

    pub(crate) fn argv(&self) -> *const *const c_char {
        if self.ptrs.is_empty() {
            std::ptr::null()
        } else {
            self.ptrs.as_ptr()
        }
    }
}

fn stdlib_dir() -> Option<String> {
    let mut prefixes = Vec::new();
    if let Ok(prefix) = std::env::var("FAUST_PREFIX") {
        prefixes.push(prefix);
    }
    if let Ok(home) = std::env::var("HOME") {
        prefixes.push(format!("{home}/.local"));
    }
    prefixes.push("/usr/local".into());
    prefixes
        .into_iter()
        .map(|prefix| format!("{prefix}/share/faust"))
        .find(|dir| std::path::Path::new(dir).exists())
}

/// Holds the global FFI lock with the libfaust context open; dropping it
/// destroys the context (before releasing the lock). Boxes built inside are
/// arena pointers that die with the context — only the factory survives.
struct LibContext {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl LibContext {
    fn acquire() -> Self {
        let lock = ffi_lock();
        unsafe { ffi::createLibContext() };
        Self { _lock: lock }
    }
}

impl Drop for LibContext {
    fn drop(&mut self) {
        unsafe { ffi::destroyLibContext() };
    }
}

/// Runs on the compiler thread. Compiles with default options — `-single`
/// (FAUSTFLOAT = f32, matching our buses) and maximum LLVM optimization —
/// then probes a throwaway instance for the def's parameters and I/O arity
/// (F3), so `/s_new`/`/n_set` can resolve control names without touching
/// libfaust again.
/// Compiles a def synchronously on the calling thread. The compiler thread
/// uses this per request; the NRT renderer calls it directly (it owns the
/// process, so hogging the thread for ~10 ms is fine). Serialized by the
/// process-wide FFI lock either way.
pub fn compile(name: &str, payload: &CompilePayload) -> Result<FaustDef, String> {
    let factory = match payload {
        CompilePayload::Source(source) => compile_source(name, source),
        CompilePayload::Json(json) => compile_json(name, json),
    }?;
    FaustDef::probe(factory)
}

fn compile_source(name: &str, source: &str) -> Result<FaustFactory, String> {
    let name_c = CString::new(name).map_err(|_| "NUL byte in name".to_string())?;
    let source_c = CString::new(source).map_err(|_| "NUL byte in source".to_string())?;
    let target = CString::new("").unwrap(); // current machine
    let args = FaustArgs::stdlib();
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];

    let _guard = ffi_lock();
    let ptr = unsafe {
        ffi::createCDSPFactoryFromString(
            name_c.as_ptr(),
            source_c.as_ptr(),
            args.argc(),
            args.argv(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        )
    };
    factory_or_error(ptr, &error_msg)
}

/// F2: JSON → Box API (see [`boxes`] for the schema). Validation errors come
/// back with the path of the offending JSON node; Faust's own errors
/// (arities, dangling inputs) come from the factory step, verbatim.
fn compile_json(name: &str, json: &str) -> Result<FaustFactory, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let name_c = CString::new(name).map_err(|_| "NUL byte in name".to_string())?;
    let target = CString::new("").unwrap();
    let args = FaustArgs::stdlib();
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];
    // Labels handed to libfaust stay alive until the factory exists.
    let mut cstrings = Vec::new();

    let ctx = LibContext::acquire();
    let process = unsafe { boxes::build_process(&root, &mut cstrings) }?;
    let ptr = unsafe {
        ffi::createCDSPFactoryFromBoxes(
            name_c.as_ptr(),
            process,
            args.argc(),
            args.argv(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        )
    };
    drop(ctx);
    factory_or_error(ptr, &error_msg)
}

fn factory_or_error(
    ptr: *mut ffi::llvm_dsp_factory,
    error_msg: &[c_char; ffi::ERROR_MSG_SIZE],
) -> Result<FaustFactory, String> {
    match unsafe { FaustFactory::from_raw(ptr) } {
        Some(factory) => Ok(factory),
        None => {
            let msg = unsafe { CStr::from_ptr(error_msg.as_ptr()) };
            Err(msg.to_string_lossy().trim().to_string())
        }
    }
}
