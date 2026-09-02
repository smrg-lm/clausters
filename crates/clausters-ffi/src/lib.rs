//! C ABI over [`clausters_core`] — the language-agnostic surface for client
//! bindings.
//!
//! Same contract as the server's embed ABI (`clausters::embed`): only flat
//! data crosses — `f32`/`f64`/integers and pointer+length arrays, never a
//! library type. A thin per-language wrapper (Python `ctypes` now, JS N-API or
//! wasm later) sits on top. Check [`clausters_core_abi_version`] first.
//!
//! Scope: the numeric builtins, the seeded RNG and the timing/sample-conversion
//! scalars, the **document** surface ([`clausters_document_apply`] — one
//! implementation of what an edit means, bound by every client rather than
//! re-derived per language), plus a **WebSocket client transport** (`clausters_ws_*`, in
//! [`ws`]) — the carrier a browser-less binding uses to reach a `--ws` server,
//! sharing the server's WebSocket implementation (`tungstenite`) instead of
//! re-implementing the framing per language. OSC bundle assembly stays in
//! `clausters_core::osc` (Rust-tested).
//!
//! Two optional features widen that surface, both off by default: `notation`
//! adds the notation layer's pure half (see `clausters_core::notation`), and
//! `verovio` adds the engraver and the editable score on top of it — the one
//! that links libverovio, so it stays opt-in the way the Faust family does in
//! the server.

use clausters_core::clocksync::SampleClockModel;
use std::sync::Mutex;

use clausters_core::peaks::{self, MultiPyramid, Pyramid};
use clausters_core::rng::{Rng, WhiteNoise};
use clausters_core::tempoclock::{self, Scheduler};
use clausters_core::window::Window;

mod builtins;
mod bundle;
mod clocksync;
mod document;
mod history;
mod measure;
#[cfg(feature = "notation")]
pub mod notation;
mod patch;
mod registry;
mod rng;
mod scale;
mod sched;
pub mod shm;
mod tempomap;
mod time;
pub mod ws;

// Every module's `extern "C"` items are re-exported here. The C symbols do not
// care which file declares them (`no_mangle` names are flat), but a Rust caller
// -- this crate's own `notation` module, a doc link, a binding that links the
// crate -- keeps naming them `clausters_ffi::…`.
pub use builtins::*;
pub use bundle::*;
pub use clocksync::*;
pub use document::*;
pub use history::*;
pub use measure::*;
pub use patch::*;
pub use registry::*;
pub use rng::*;
pub use scale::*;
pub use sched::*;
pub use time::*;

/// The C ABI version of this surface. Bump on any incompatible change. v2 added
/// the `clausters_ws_*` WebSocket client transport; v3 the `clausters_core_peaks_*`
/// peak-pyramid cache builder; v4 the `clausters_core_window` smoothing windows
/// (shared with the server's FFT chain for bit-identical analysis); v5 the seam
/// audit pass — the `clausters_sched_*` beat queue, the `clausters_clocksync_*`
/// sample-clock model, the `clausters_rng_*` value stream, NTP timetag packing,
/// `quant_delay` and `degree_to_midinote` — so no value/time logic remains
/// per-language; v6 `clausters_rng_next_u64` (child-stream seed derivation for
/// the per-routine random context); v7 the `clausters_core_correlation` /
/// `clausters_core_lissajous` stereo-field measurements (shared with the GUI
/// phasescope so a headless client reads the identical numbers); v8 the
/// `clausters_core_peaks_multi_*` multichannel peak-pyramid cache (one cache
/// resource per buffer, all channels — the editor-grade waveform's format);
/// v9 the ruler/axis scalars — `clausters_core_hz_to_mel`/`_mel_to_hz`/
/// `_hz_to_bark`/`_bark_to_hz` (perceptual frequency scales, shared with the
/// GUI spectrogram axis) and `clausters_core_bar`/`_beat_in_bar` (the bar:beat
/// read of a quant grid, the display complement of `quant_delay`); v10 the
/// `clausters_registry_*` finite-resource id registry (node ids, buses,
/// buffers — every client's allocator and the server's reserved ranges share
/// the one occupancy-map model, internally locked per handle); v11 the
/// `clausters_core_patch_compile` cord→bus pass (a directed patch JSON in, its
/// GraphDef wiring JSON out — the GUI patcher's translation, shared so every
/// client compiles a patch identically); v12 the notation surface
/// (feature-gated, see `clausters_core::notation`) — the pure
/// `clausters_core_svg_to_display_list` and `clausters_core_voice_to_mei`,
/// plus, behind `verovio`, the editable
/// `clausters_score_*` handle, so a client binds the notation layer instead of
/// reimplementing it; v13 the `clausters_core_bundle_*` component-bundle pass
/// (a manifest's requirements, one mounted instance's resolution, and the
/// writers' pre-flight — shared so a bundle authored in any language mounts
/// identically in a tab, on the desktop and over loopback); v14
/// `clausters_core_stats`, the peak/RMS of one channel of an interleaved
/// buffer (what a render reports back, so no client writes the loop); v15 the
/// document surface — `clausters_document_apply` and
/// `clausters_document_resolve` — which is how every client binds one
/// implementation of what an edit *means* instead of three: the document and
/// the intent cross by value and the new document comes back, rather than each
/// client holding handles into a Rust object graph; v16 the undo log
/// (`clausters_log_*`), which crosses as a **handle** where the document
/// crosses by value — a bulk inverse leaves the log on purpose, so sending one
/// by value would carry every spilled span on every call, which is the cost
/// spilling exists to avoid. (v32 renamed these `clausters_history_*`; see
/// below.) **v19 is a format rather than a symbol**: the peak
/// cache the `clausters_core_peaks_*` builders emit is CLPK v3, which carries a
/// mean square beside each bucket's min/max, so a cache built by this surface is
/// longer than a v18 one and a reader that predates it cannot parse it (the
/// converse holds: v1 and v2 caches still load). v21 the shared-memory segment
/// (`clausters_core_shm_*`): a peer maps the file in its own language and asks
/// here for every offset and count, for the directory's seqlock, for the ring
/// framing and for a region file's name — the numbers a binding used to
/// transcribe, which is how one of them came to declare 1024 control buses
/// against a server that had 16 384. **v21 also carries**
/// `clausters_core_peaks_multi_write_buckets`, the receiving half of
/// `/buffer_stream`: a run of buckets somebody else measured, folded into a
/// cache in place, so a client that cannot map the memory a recording is
/// filling still draws it. **v23 the score model** — `clausters_core_sheet_apply`,
/// `clausters_core_sheet_to_mei` and `clausters_core_sheet_ops`: notation as
/// data a client holds and operations as data it sends, so one implementation
/// of what an edit to a score *means* serves every client and, more to the
/// point, serves a standalone host that has no client language in the process
/// at all. It is the same by-value shape the document surface took at v15, for
/// the same reason. The verbs are **not** symbols — they ride inside the
/// payload — so the catalog is what says which exist, and adding one moves
/// nothing here. **v24 the interpreter** — `clausters_core_sheet_perform` and
/// `clausters_core_interpretation`: the path back out of the score, reading
/// what the symbols *mean* into sounding notes, and the default reading a
/// caller starts from when it wants another one. Two symbols rather than one
/// because an override has to be able to read the defaults before editing them,
/// and a client that wrote those numbers down for itself would play the same
/// score at a different amplitude than the other client does. **v25 the
/// reader** — `clausters_core_mei_to_sheet`: a *document* back into the model,
/// which is the other return path and the one that makes a score opened from
/// typed text editable at all. One symbol for every notation format there is,
/// because the engraver normalizes whatever it loaded to MEI before this sees
/// it. **v26 the score's edit path** — `clausters_score_apply` and
/// `clausters_score_sheet`: an open document is edited through the *model's*
/// verbs rather than through the engraver's editor, so there is one
/// implementation of what an edit to a score means and a standalone host
/// performs the same one. The engraver's editor stays as the escape hatch for a
/// document that has no model behind it. **v27 which item a page element is** —
/// `clausters_core_item_id`: the step between a selection on the page and a
/// model verb, answered by the emitter that spelled the element rather than by
/// each client working the spelling out again. **v28 a take's length is
/// seconds** — `clausters_document_resolve` takes `frames_per_second` beside
/// `frames_per_beat`, because the document now measures a placement in beats
/// and what it places in the unit of that element's own data, so one ratio can no
/// longer answer both questions. **v29 the piece's time map** —
/// `clausters_tempomap_*`: a beat is a logical coordinate and the tempo that
/// turns it into a second can change along the piece, so the conversion stops
/// being a scalar and becomes an integral. Additive: the affine functions stay
/// exactly as they were, and a one-segment map computes their expression.
/// **v30 a tempo curve has a shape** — `clausters_tempomap_segment` writes
/// **seven** `f64` instead of six (the seventh is the curvature), and
/// `clausters_tempomap_shaped`/`_env` write a shaped ramp and a whole finite
/// envelope. Breaking rather than additive: the segment payload widened, and
/// its fourth number is now an envelope shape number rather than a flag, so a
/// reader that does not know shape 2 or 5 misreads a segment it can see.
///
/// **v31 the map is a value** — `clausters_tempomap_version`, `_dump` and
/// `_load`: the edit counter a holder of a *shared* map compares, and the map
/// written out as its breakpoints and read back through the ordinary writers.
/// Additive, and the counter still moves: the ctypes binding declares every
/// symbol eagerly, so a staged library missing these fails at load — with
/// *"speaks ABI v30, this binding v31"* rather than an `AttributeError` on a
/// name nobody was looking at.
///
/// **v32 the log became a history**, and the rename is the honest part of a
/// breaking change: the handle no longer holds one document's undo but one
/// *editing context* — the structures registered in it and one ordered pile
/// over them — so `clausters_log_*` became `clausters_history_*`, with
/// `clausters_history_register` minting a structure's identity and every call
/// that names one taking it. `undo` and `redo` lost their document argument and
/// apply nothing: a history holds structures this surface cannot reach, so they
/// hand back each payload with the structure it belongs to (`{"inverses": …}`,
/// `{"edits": …, "remaining": …}`) and the caller applies them through whatever
/// door each domain has. `record` gained the coalesce **key**, because "the
/// same thing done the same way" is a sentence in a vocabulary the pile does
/// not read.
///
/// **v33 an entry is a transaction.** `clausters_history_record` takes the
/// whole entry as one JSON request — a label, a coalesce flag and a list of
/// legs, each naming its structure — because a gesture may touch more than one
/// structure and has to undo as one step, and a leg at a time would let half a
/// transaction land. `clausters_document_inverse` came with it: a caller
/// recording its own entry needs the inverse read *before* the edit lands, and
/// only the arrangement can state one for the arrangement.
///
/// **v34 what a history refuses to promise.** A leg may carry no `backward`:
/// an act with no inverse is recorded, marked, and walked past in both
/// directions, so `clausters_history_undo`/`_redo` now answer with the entry's
/// `label` and the `skipped` labels beside the payloads. Deleting a structure
/// is `clausters_history_forget`, which invalidates the entries naming it and
/// defers the free — undoing a deletion has to be able to give the data back —
/// with `clausters_history_released` saying when the last entry holding it has
/// retired. And the save mark is the pile's: `clausters_history_mark_saved`,
/// `_dirty` and `_saved_reachable`.
/// **v35 the vocabularies are named once.** `clausters_domain_coalesce_key`
/// answers the coalesce sentence for any domain the crate speaks — the
/// arrangement, a curve, a span of samples, a timeline — because a caller
/// recording its own entry has to state a key the pile cannot compute, and
/// spelling four vocabularies' rules again in ctypes and again in TypeScript is
/// the divergence `clausters_document_coalesce_key` was given a door to
/// prevent. Additive, and the counter still moves for the reason v31 states:
/// the ctypes binding declares every symbol eagerly, so a staged library
/// missing this one fails at load with a version mismatch rather than an
/// `AttributeError` on a name nobody was looking at. One behaviour moved with
/// it and is not additive: an **empty** `writesamples` — the inverse the
/// document can state for a destructive edit — now bumps the source's
/// generation instead of applying as a no-op, so a reader's copy is marked
/// stale by an undo as it is by the edit.
/// **v36 a domain inverts its own edits.** `clausters_domain_edit` applies a
/// payload to a structure held as its own state — a curve's points, a
/// timeline's events — and answers with what it now is *and* the payload that
/// puts it back, both in one call because the inverse has to be read before the
/// edit lands. It is the other half of v35's argument: the coalesce sentence
/// and the inverse are the same vocabulary's rule, and a client computing the
/// second itself is the divergence the first was given a door to prevent. Two
/// domains are deliberately not served: the arrangement's tree, which needs a
/// version to check against and a grid to snap to and has
/// `clausters_document_apply` of its own, and a span of samples, whose frames
/// live in a buffer rather than in a value. Additive, and the counter moves for
/// v31's reason.
pub const CORE_ABI_VERSION: u32 = 36;

/// Returns [`CORE_ABI_VERSION`]; call before anything else.
#[unsafe(no_mangle)]
pub extern "C" fn clausters_core_abi_version() -> u32 {
    CORE_ABI_VERSION
}
