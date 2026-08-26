//! The page's Faust compiler: a queue, not a thread.
//!
//! Natively, `/def_send faust` hands the payload to a dedicated thread that
//! calls libfaust and posts a [`CompileResult`] back; the command loop drains
//! the results on its own schedule and answers `/done` or `/fail`. A page has
//! no libfaust and no thread here — the compiler is `libfaust-wasm` in the
//! engine's Worker — so this backend keeps the *same shape* and moves the
//! middle step outside: [`CompilerThread::submit`] parks the request, the host
//! drains [`CompilerThread::take_jobs`], compiles and instantiates the module
//! against the engine's memory, and answers with
//! [`CompilerThread::finish`]. Only then does a result appear in
//! [`CompilerThread::try_result`].
//!
//! This is the same delegation the browser engine already uses for soundfile
//! decoding (`server::nrt`), for the same reason: the audio thread owes a block
//! every 2.67 ms and cannot wait on anything. Nothing on the wire changes —
//! `/def_send faust` was always asynchronous, and the only observable
//! difference from a window is that the reply stops being `/fail`.
//!
//! What the host is expected to do between the two calls is written in
//! [`crate::faust::synth::FaustDef::link`].

use std::sync::Mutex;

use crate::faust::synth::{FaustDef, ParamSpec};
use crate::osc::ClientId;
use crate::osc::wake::Waker;

pub use crate::faust::CompilePayload;

/// Disk-cache work attached to a compile request. A page has no def store, so
/// nothing ever constructs one; the type exists so the command paths above are
/// one piece of code on both targets.
pub struct CacheJob {
    pub dir: std::path::PathBuf,
    pub restore: Option<()>,
}

pub struct CompileRequest {
    pub name: String,
    pub payload: CompilePayload,
    /// Who asked: the async reply goes back to this client.
    pub client: Option<ClientId>,
    pub cache: Option<Box<CacheJob>>,
}

pub struct CompileResult {
    pub name: String,
    pub client: Option<ClientId>,
    /// The linked def, or a human-readable compiler error destined for the
    /// `/fail` reply verbatim.
    pub outcome: Result<FaustDef, String>,
}

/// One parked compilation, as the host reads it.
pub struct CompileJob {
    pub ticket: u64,
    pub name: String,
    /// `"source"`, `"boxes"` or `"signals"` — which of the three def formats
    /// the payload is in, so the host calls the right libfaust entry point.
    pub kind: &'static str,
    pub def: String,
}

struct Inner {
    pending: Vec<(u64, CompileRequest)>,
    /// Handed out but not yet answered. A request stays here until the host
    /// calls `finish`, so a host that drops a job strands one def rather than
    /// losing the client that asked for it.
    outstanding: Vec<(u64, String, Option<ClientId>)>,
    results: Vec<CompileResult>,
    next_ticket: u64,
}

/// The page's stand-in for the compiler thread. Same surface, no thread.
pub struct CompilerThread {
    inner: Mutex<Inner>,
}

impl CompilerThread {
    /// `waker` is the socket poke a worker thread uses to end the command
    /// loop's blocking recv. There is no worker thread and no blocking recv in
    /// a page — the host pulls a serving turn before every block — so it is
    /// accepted and dropped, keeping one call site in `osc::server`.
    pub fn spawn(_waker: Option<Waker>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                pending: Vec::new(),
                outstanding: Vec::new(),
                results: Vec::new(),
                next_ticket: 1,
            }),
        }
    }

    /// Queues a compilation for the host to pick up. Never fails: unlike the
    /// native channel there is no thread that can have died.
    pub fn submit(&self, request: CompileRequest) -> Result<(), CompileRequest> {
        let mut inner = self.lock();
        let ticket = inner.next_ticket;
        inner.next_ticket += 1;
        inner.pending.push((ticket, request));
        Ok(())
    }

    /// Non-blocking: one finished compilation, if any.
    pub fn try_result(&self) -> Option<CompileResult> {
        let mut inner = self.lock();
        if inner.results.is_empty() {
            None
        } else {
            Some(inner.results.remove(0))
        }
    }

    /// Blocking with deadline — meaningless here, since nothing can arrive
    /// while this build is inside a call. Answers whatever is already there.
    pub fn recv_result_timeout(&self, _timeout: std::time::Duration) -> Option<CompileResult> {
        self.try_result()
    }

    /// Everything queued since the last call, moved to the outstanding list.
    pub fn take_jobs(&self) -> Vec<CompileJob> {
        let mut inner = self.lock();
        let taken: Vec<(u64, CompileRequest)> = std::mem::take(&mut inner.pending);
        let mut jobs = Vec::with_capacity(taken.len());
        for (ticket, req) in taken {
            jobs.push(CompileJob {
                ticket,
                name: req.name.clone(),
                kind: req.payload.kind(),
                def: req.payload.text().to_string(),
            });
            inner.outstanding.push((ticket, req.name, req.client));
        }
        jobs
    }

    /// Answers one outstanding job: the linked def, or the compiler's own
    /// error text. An unknown ticket is ignored — a host that answers twice is
    /// confused, not dangerous.
    pub fn finish(&self, ticket: u64, outcome: Result<FaustDef, String>) {
        let mut inner = self.lock();
        let Some(pos) = inner.outstanding.iter().position(|(t, ..)| *t == ticket) else {
            return;
        };
        let (_, name, client) = inner.outstanding.remove(pos);
        inner.results.push(CompileResult {
            name,
            client,
            outcome,
        });
    }

    /// How many compilations are queued or waiting for the host: what the
    /// serving turn reports as backlog, and what a `/server_sync` waits out.
    pub fn backlog(&self) -> usize {
        let inner = self.lock();
        inner.pending.len() + inner.outstanding.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What the host reports back for a successful compilation: where the module's
/// entry points landed in the engine's table, and the shape the compiler's own
/// JSON declared.
#[derive(serde::Deserialize)]
pub struct LinkedModule {
    /// Table slot of the module's `compute` export.
    pub compute: u32,
    /// Table slot of its `init` export.
    pub init: u32,
    /// Byte size of one DSP struct (`"size"` in the Faust JSON).
    pub size: usize,
    pub inputs: usize,
    pub outputs: usize,
    #[serde(default)]
    pub params: Vec<LinkedParam>,
}

/// One UI parameter, as the Faust JSON declares it.
#[derive(serde::Deserialize)]
pub struct LinkedParam {
    pub name: String,
    /// Byte offset of the zone inside the DSP struct (`"index"`).
    pub index: usize,
    #[serde(default)]
    pub init: f32,
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub max: f32,
    #[serde(default)]
    pub step: f32,
}

impl LinkedModule {
    /// Turns the host's report into a def. Sound checking lives in
    /// [`FaustDef::link`]; this only unpacks.
    ///
    /// # Safety
    /// The caller must have instantiated the module described here against the
    /// engine's own memory and table — see [`FaustDef::link`].
    pub unsafe fn into_def(self) -> Result<FaustDef, String> {
        let offsets = self.params.iter().map(|p| p.index).collect();
        let params = self
            .params
            .into_iter()
            .map(|p| ParamSpec {
                name: p.name,
                init: p.init,
                min: p.min,
                max: p.max,
                step: p.step,
            })
            .collect();
        unsafe {
            FaustDef::link(
                self.compute,
                self.init,
                self.size,
                offsets,
                params,
                self.inputs,
                self.outputs,
            )
        }
    }
}
