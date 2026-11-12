//! The shared core's JS door (the W track): a thin wasm-bindgen shell over
//! `clausters-core`, the browser sibling of `clausters-ffi`'s C door and the
//! core twin of `crates/clausters-web` (the whole-server shell).
//!
//! It exposes the OSC codec: the web client encodes and decodes through the
//! same `clausters_core::osc` the server and every other client use, so the
//! bytes are identical by construction (the parity vectors in
//! `clients/web/tests/` hold this against the Python client). It exposes the
//! **registry** -- the id-allocation model behind node ids, buses and buffers,
//! the same door `clausters-ffi` opens for Python. It exposes the sequencing
//! layer's core: the beat-ordered **queue**, the beat/second/sample
//! arithmetic, **bundle assembly** with a timetag, the seeded value stream,
//! the builtins and the pitch space, and the sample-clock model. And it exposes the
//! **data paths' analysis** -- the stereo-field measurements and the peak
//! pyramid -- so a page that reads buses, taps and buffers measures them with
//! the same functions the GUI host measures with. Only measurements: what a
//! *drawing* needs of them (a pixel row, a display window, a decibel curve)
//! stays in the host, which is the one thing that draws.
//! Always the same shape: the logic lives in `clausters-core`, natively tested; this
//! shell only converts values at the JS boundary, and only on the wasm target.
//!
//! The JS argument convention mirrors the tagged pairs the interim page codec
//! used: encode takes `[tag, value]` pairs (`"i"`/`"h"`/`"f"`/`"d"`/`"s"`/
//! `"b"`), preserving the int/float distinction explicitly; decode returns
//! plain values (`{addr, args}` per message, bundles flattened, timetags as
//! Unix seconds), which is what reply consumers want.

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
mod midi;
mod notation;
pub mod osc;
mod patch;
mod plugin;
mod registry;
mod rng;
mod scale;
mod sched;
mod tempomap;
mod time;
mod widgetids;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Runs when the module is instantiated: a panic is written to the console
/// with its message and location before the module traps. A panic in wasm
/// aborts and the module is dead after it, so nothing can be answered then;
/// what a page can still have is the reason, which without this is the bare
/// `unreachable` of the trap.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
fn report_panics() {
    console_error_panic_hook::set_once();
}

/// **The session format this build writes** -- the crate's `session::FORMAT`.
///
/// Both clients keep the number as a **literal**, because a `Session` is plain
/// data and writing one must not need an `await loadCore()`. What keeps the two
/// literals the crate's is a test in each client that asks this and compares --
/// the check that did not exist when the format moved to 2 and both clients
/// went on stamping 1 onto files that could carry a source whose samples are
/// spans of other sources.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sessionFormat)]
pub fn session_format() -> u32 {
    clausters_document::session::FORMAT
}

/// **A session written in an older format, as this build writes it** -- the
/// crate's `session::migrate` over the session's JSON text, or an empty string
/// for text that is not JSON. A session already current comes back unchanged.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = sessionMigrate)]
pub fn session_migrate(session: &str) -> String {
    serde_json::from_str::<serde_json::Value>(session)
        .map(|written| clausters_document::session::migrate(written).to_string())
        .unwrap_or_default()
}
