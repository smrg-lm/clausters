//! Dedicated Faust compiler thread.
//!
//! JIT compilation takes ~10 ms per def (measured in F0) and the libfaust
//! lib context is global and not thread-safe, so all compilation runs on one
//! dedicated thread that serializes requests naturally. The network thread
//! submits [`CompileRequest`]s and drains [`CompileResult`]s on its own
//! schedule (after each packet / on its GC tick), then sends the async
//! `/done`/`/fail` reply to the requesting client.

use crate::osc::ClientId;
use std::ffi::{CStr, CString, c_char, c_int};
use std::path::PathBuf;
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::faust::boxes;
use crate::faust::cache::{self, FaustRecord};
use crate::faust::factory::FaustFactory;
use crate::faust::ffi;
use crate::faust::signals;
use crate::faust::synth::FaustDef;

/// What `/def_send faust` carries: one of the three def formats.
pub enum CompilePayload {
    /// Raw Faust source code (F1), compiled with
    /// `createCDSPFactoryFromString`.
    Source(String),
    /// JSON box graph (F2), mapped to Box API calls (see [`boxes`]) and
    /// compiled with `createCDSPFactoryFromBoxes`.
    Json(String),
    /// JSON signal tree, mapped to Signal API calls (see
    /// [`crate::faust::signals`]) and compiled with
    /// `createCDSPFactoryFromSignals`.
    Signal(String),
}

impl CompilePayload {
    /// Classifies a `/def_send faust` def string: raw Faust source unless it starts
    /// with `{`, then a signal tree if the JSON object has a top-level
    /// `"signals"` key, otherwise a box tree. The sniff is unambiguous —
    /// Faust source never starts with `{`, and a box def's root is a single
    /// box node (`{"op": …}`), never an object keyed by `"signals"`.
    pub fn classify(def: String) -> Self {
        if !def.trim_start().starts_with('{') {
            return Self::Source(def);
        }
        let is_signal = serde_json::from_str::<serde_json::Value>(&def)
            .ok()
            .and_then(|v| v.as_object().map(|o| o.contains_key("signals")))
            .unwrap_or(false);
        if is_signal {
            Self::Signal(def)
        } else {
            Self::Json(def)
        }
    }
}

/// Disk-cache work attached to a compile request (see
/// [`crate::server::defstore`]). Present only when persistence is enabled.
pub struct CacheJob {
    /// `<data>/defs/faustdefs`, where the record and bitcode live.
    pub dir: PathBuf,
    /// On a startup reload, the record to restore: the thread tries its
    /// bitcode first and recompiles only on a miss. `None` for a live
    /// `/def_send faust`, which always compiles fresh and then (re)writes the cache.
    pub restore: Option<FaustRecord>,
}

pub struct CompileRequest {
    pub name: String,
    pub payload: CompilePayload,
    /// Who asked: the async reply goes back to this client. `None` for an
    /// internal startup reload, which has no requester to answer.
    pub client: Option<ClientId>,
    /// Bitcode cache read/write, when persistence is on. Boxed so a bounced
    /// [`CompilerThread::submit`] error stays small.
    pub cache: Option<Box<CacheJob>>,
}

pub struct CompileResult {
    pub name: String,
    pub client: Option<ClientId>,
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
                    let outcome = run_request(&req);
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
    /// The arguments every factory is created with:
    ///
    /// - `-I <dir>` for the Faust stdlib (`stdfaust.lib` and friends), so
    ///   both raw-source defs and `faust` fragments inside JSON can
    ///   `import()` it. The directory comes from `$FAUST_PREFIX/share/faust`,
    ///   falling back to `~/.local`, then `/usr/local` — same search order
    ///   as build.rs.
    /// - `-ftz 2`: the generated code flushes recursive variables below the
    ///   normal float range, so decaying tails cannot strand the audio
    ///   thread in slow subnormal math regardless of the host FPU mode (the
    ///   architecture-independent half of [`crate::dsp::denormals`]).
    pub(crate) fn defaults() -> Self {
        let mut storage = vec![CString::new("-ftz").unwrap(), CString::new("2").unwrap()];
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

/// The LLVM target the factory JITs for, as a Faust `triple:mcpu` string.
///
/// Empty — the default, and what a production server wants — means the host
/// machine: LLVM detects the CPU and emits code tuned for it. The
/// `CLAUSTERS_FAUST_TARGET` env var overrides it; CI sets a baseline CPU
/// (`:x86-64`) because virtualized runners can misreport their CPU features and
/// then SIGILL when the JIT emits host-tuned instructions the VM cannot run.
pub(crate) fn host_target() -> CString {
    let spec = std::env::var("CLAUSTERS_FAUST_TARGET").unwrap_or_default();
    CString::new(spec).unwrap_or_default()
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

/// Compiles a def synchronously on the calling thread: the compiler thread
/// uses this per request, and the NRT renderer calls it directly (it owns
/// the process, so hogging the thread for ~10 ms is fine). Serialized by
/// the process-wide FFI lock either way. Options: `-single` (FAUSTFLOAT =
/// f32, matching our buses), `-ftz 2` plus the stdlib include path
/// (`FaustArgs::defaults`) and maximum LLVM optimization; afterwards a
/// throwaway instance is probed for the def's parameters and I/O arity
/// (F3), so `/synth_new`/`/node_set` can resolve control names without touching
/// libfaust again.
///
/// Runs with the FPU in normal precision: the NRT renderer calls this from
/// its flush-to-zero render thread, and libfaust's front-end must not do
/// its double math (interval typing, constant folding) in FTZ/DAZ mode —
/// its interval assertions abort the process on a flushed bound (see
/// [`crate::dsp::denormals::normal_precision`]).
pub fn compile(name: &str, payload: &CompilePayload) -> Result<FaustDef, String> {
    crate::dsp::denormals::normal_precision(|| {
        let factory = match payload {
            CompilePayload::Source(source) => compile_source(name, source),
            CompilePayload::Json(json) => compile_json(name, json),
            CompilePayload::Signal(json) => compile_signal(name, json),
        }?;
        FaustDef::probe(factory)
    })
}

/// Runs one request: on a startup reload, tries the bitcode cache first
/// (skipping the Faust front-end); otherwise — and on any cache miss —
/// compiles from source. When persistence is on, a fresh compile (re)writes
/// the cache. The cache is non-authoritative: a miss is silent and always
/// recoverable.
fn run_request(req: &CompileRequest) -> Result<FaustDef, String> {
    // The cache path skips the Faust front-end but still runs LLVM's JIT,
    // whose own folding gets the same normal-precision bracket as compile().
    if let Some(job) = &req.cache
        && let Some(record) = &job.restore
        && let Ok(def) = crate::dsp::denormals::normal_precision(|| {
            cache::try_restore(record, &job.dir).and_then(FaustDef::probe)
        })
    {
        return Ok(def);
    }
    let def = compile(&req.name, &req.payload)?;
    if let Some(job) = &req.cache {
        cache::persist(def.factory(), &req.name, &req.payload, &job.dir);
    }
    Ok(def)
}

fn compile_source(name: &str, source: &str) -> Result<FaustFactory, String> {
    let name_c = CString::new(name).map_err(|_| "NUL byte in name".to_string())?;
    let source_c = CString::new(source).map_err(|_| "NUL byte in source".to_string())?;
    let target = host_target();
    let args = FaustArgs::defaults();
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
    let target = host_target();
    let args = FaustArgs::defaults();
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

/// JSON → Signal API (see [`crate::faust::signals`] for the schema). Mirrors
/// [`compile_json`] but builds a NULL-terminated output-signal vector and
/// calls `createCDSPFactoryFromSignals`.
fn compile_signal(name: &str, json: &str) -> Result<FaustFactory, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let name_c = CString::new(name).map_err(|_| "NUL byte in name".to_string())?;
    let target = host_target();
    let args = FaustArgs::defaults();
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];
    let mut cstrings = Vec::new();

    let ctx = LibContext::acquire();
    let ptr = match unsafe { signals::build_signals(&root, &mut cstrings) } {
        Ok(mut outputs) => {
            outputs.push(std::ptr::null_mut()); // NULL-terminated output array
            unsafe {
                ffi::createCDSPFactoryFromSignals(
                    name_c.as_ptr(),
                    outputs.as_mut_ptr(),
                    args.argc(),
                    args.argv(),
                    target.as_ptr(),
                    error_msg.as_mut_ptr(),
                    -1,
                )
            }
        }
        Err(e) => {
            drop(ctx);
            return Err(e);
        }
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
