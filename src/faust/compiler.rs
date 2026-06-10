//! Dedicated Faust compiler thread.
//!
//! JIT compilation takes ~10 ms per def (measured in F0) and the libfaust
//! lib context is global and not thread-safe, so all compilation runs on one
//! dedicated thread that serializes requests naturally. The network thread
//! submits [`CompileRequest`]s and drains [`CompileResult`]s on its own
//! schedule (after each packet / on its GC tick), then sends the async
//! `/done`/`/fail` reply to the requesting client.

use std::ffi::{CStr, CString, c_char};
use std::net::SocketAddr;
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::faust::factory::FaustFactory;
use crate::faust::ffi;

pub struct CompileRequest {
    pub name: String,
    /// Faust source code (F1). F2 replaces this with the JSON box graph.
    pub source: String,
    /// Who asked: the async reply goes back to this client.
    pub client: SocketAddr,
}

pub struct CompileResult {
    pub name: String,
    pub client: SocketAddr,
    /// The compiled factory, or a human-readable compiler error destined for
    /// the `/fail` reply verbatim.
    pub outcome: Result<FaustFactory, String>,
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
                    let outcome = compile(&req.name, &req.source);
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

/// Runs on the compiler thread. Compiles with default options: `-single`
/// (FAUSTFLOAT = f32, matching our buses) and maximum LLVM optimization.
fn compile(name: &str, source: &str) -> Result<FaustFactory, String> {
    let name_c = CString::new(name).map_err(|_| "NUL byte in name".to_string())?;
    let source_c = CString::new(source).map_err(|_| "NUL byte in source".to_string())?;
    let target = CString::new("").unwrap(); // current machine
    let mut error_msg = [0 as c_char; ffi::ERROR_MSG_SIZE];

    let _guard = ffi_lock();
    let ptr = unsafe {
        ffi::createCDSPFactoryFromString(
            name_c.as_ptr(),
            source_c.as_ptr(),
            0,
            std::ptr::null(),
            target.as_ptr(),
            error_msg.as_mut_ptr(),
            -1,
        )
    };
    match unsafe { FaustFactory::from_raw(ptr) } {
        Some(factory) => Ok(factory),
        None => {
            let msg = unsafe { CStr::from_ptr(error_msg.as_ptr()) };
            Err(msg.to_string_lossy().trim().to_string())
        }
    }
}
