//! C ABI over [`clausters_core`] -- the language-agnostic surface for client
//! bindings.
//!
//! Same contract as the server's embed ABI (`clausters::embed`): only flat
//! data crosses -- `f32`/`f64`/integers and pointer+length arrays, never a
//! library type. A thin per-language wrapper (Python `ctypes` now, JS N-API or
//! wasm later) sits on top. Check [`clausters_core_abi_version`] first.
//!
//! Scope: the numeric builtins, the seeded RNG and the timing/sample-conversion
//! scalars, the **document** surface ([`clausters_document_apply`] -- one
//! implementation of what an edit means, bound by every client rather than
//! re-derived per language), plus a **WebSocket client transport** (`clausters_ws_*`, in
//! [`ws`]) -- the carrier a browser-less binding uses to reach a `--ws` server,
//! sharing the server's WebSocket implementation (`tungstenite`) instead of
//! re-implementing the framing per language. OSC bundle assembly stays in
//! `clausters_core::osc` (Rust-tested).
//!
//! Two optional features widen that surface, both off by default: `notation`
//! adds the notation layer's pure half (see `clausters_core::notation`), and
//! `verovio` adds the engraver and the editable score on top of it -- the one
//! that links libverovio, so it stays opt-in the way the Faust family does in
//! the server.

use clausters_core::clocksync::SampleClockModel;
use std::sync::Mutex;

use clausters_core::peaks::{self, MultiPyramid, Pyramid};
use clausters_core::rng::{Rng, WhiteNoise};
use clausters_core::tempoclock::{self, Scheduler};
use clausters_core::window::Window;

mod apps;
mod builtins;
mod bundle;
mod clocksync;
mod document;
mod editing;
mod envshape;
mod event;
mod history;
mod measure;
#[cfg(feature = "notation")]
pub mod notation;
mod out;
mod patch;
mod plugin;
mod registry;
mod rng;
mod scale;
mod sched;
pub mod shm;
mod tempomap;
mod time;
mod widgetids;
pub mod ws;

// Every module's `extern "C"` items are re-exported here. The C symbols do not
// care which file declares them (`no_mangle` names are flat), but a Rust caller
// -- this crate's own `notation` module, a doc link, a binding that links the
// crate -- keeps naming them `clausters_ffi::...`.
pub use builtins::*;
pub use bundle::*;
pub use clocksync::*;
pub use document::*;
pub use event::*;
pub use history::*;
pub use measure::*;
pub use patch::*;
pub use plugin::*;
pub use registry::*;
pub use rng::*;
pub use scale::*;
pub use sched::*;
pub use time::*;

/// The C ABI version of this surface. Bump on any incompatible change; what
/// each version changed is recorded in `docs/bindings.md` ("What each core
/// ABI version changed").
pub const CORE_ABI_VERSION: u32 = 95;

/// Returns [`CORE_ABI_VERSION`]; call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_abi_version() -> u32 {
    CORE_ABI_VERSION
}

/// The session format this build writes -- [`clausters_document::session::FORMAT`].
///
/// A client **carries** this number rather than knowing it. It was restated in
/// both clients until 2026-09-12, and by then it had already drifted: the crate
/// moved to 2 for a source whose samples are spans of other sources, and both
/// clients went on stamping 1 onto files that could contain one. The session
/// module's own rule is that the shape lives once beside the tree it carries,
/// and the number is part of the shape.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_session_format() -> u32 {
    clausters_document::session::FORMAT
}

/// **A session written in an older format, as this build writes it** --
/// [`clausters_document::session::migrate`] over the session's JSON. A session
/// already at the current format comes back unchanged, and text that is not
/// JSON answers `0`.
///
/// Sizes with a null `out` and fills with a second call.
///
/// # Safety
/// `session` must be null or readable for `session_len` bytes, and `out` null
/// or writable for `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clausters_session_migrate(
    session: *const u8,
    session_len: usize,
    out: *mut u8,
    out_cap: usize,
) -> usize {
    // SAFETY: forwarded from this function's own contract.
    let Some(text) = (unsafe { crate::out::text(session, session_len) }) else {
        return 0;
    };
    let Ok(written) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    let answer = clausters_document::session::migrate(written).to_string();
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::out::fill(answer.as_bytes(), out, out_cap) }
}
