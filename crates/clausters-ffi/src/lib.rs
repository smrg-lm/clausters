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
/// audit pass -- the `clausters_sched_*` beat queue, the `clausters_clocksync_*`
/// sample-clock model, the `clausters_rng_*` value stream, NTP timetag packing,
/// `quant_delay` and `degree_to_midinote` -- so no value/time logic remains
/// per-language; v6 `clausters_rng_next_u64` (child-stream seed derivation for
/// the per-routine random context); v7 the `clausters_core_correlation` /
/// `clausters_core_lissajous` stereo-field measurements (shared with the GUI
/// phasescope so a headless client reads the identical numbers); v8 the
/// `clausters_core_peaks_multi_*` multichannel peak-pyramid cache (one cache
/// resource per buffer, all channels -- the editor-grade waveform's format);
/// v9 the ruler/axis scalars -- `clausters_core_hz_to_mel`/`_mel_to_hz`/
/// `_hz_to_bark`/`_bark_to_hz` (perceptual frequency scales, shared with the
/// GUI spectrogram axis) and `clausters_core_bar`/`_beat_in_bar` (the bar:beat
/// read of a quant grid, the display complement of `quant_delay`); v10 the
/// `clausters_registry_*` finite-resource id registry (node ids, buses,
/// buffers -- every client's allocator and the server's reserved ranges share
/// the one occupancy-map model, internally locked per handle); v11 the
/// `clausters_core_patch_compile` cord->bus pass (a directed patch JSON in, its
/// GraphDef wiring JSON out -- the GUI patcher's translation, shared so every
/// client compiles a patch identically); v12 the notation surface
/// (feature-gated, see `clausters_core::notation`) -- the pure
/// `clausters_core_svg_to_display_list` and `clausters_core_voice_to_mei`,
/// plus, behind `verovio`, the editable
/// `clausters_score_*` handle, so a client binds the notation layer instead of
/// reimplementing it; v13 the `clausters_core_bundle_*` component-bundle pass
/// (a manifest's requirements, one mounted instance's resolution, and the
/// writers' pre-flight -- shared so a bundle authored in any language mounts
/// identically in a tab, on the desktop and over loopback); v14
/// `clausters_core_stats`, the peak/RMS of one channel of an interleaved
/// buffer (what a render reports back, so no client writes the loop); v15 the
/// document surface -- `clausters_document_apply` and
/// `clausters_document_resolve` -- which is how every client binds one
/// implementation of what an edit *means* instead of three: the document and
/// the intent cross by value and the new document comes back, rather than each
/// client holding handles into a Rust object graph; v16 the undo log
/// (`clausters_log_*`), which crosses as a **handle** where the document
/// crosses by value -- a bulk inverse leaves the log on purpose, so sending one
/// by value would carry every spilled span on every call, which is the cost
/// spilling exists to avoid. (v32 renamed these `clausters_history_*`; see
/// below.) **v19 is a format rather than a symbol**: the peak
/// cache the `clausters_core_peaks_*` builders emit is CLPK v3, which carries a
/// mean square beside each bucket's min/max, so a cache built by this surface is
/// longer than a v18 one and a reader that predates it cannot parse it (the
/// converse holds: v1 and v2 caches still load). v21 the shared-memory segment
/// (`clausters_core_shm_*`): a peer maps the file in its own language and asks
/// here for every offset and count, for the directory's seqlock, for the ring
/// framing and for a region file's name -- the numbers a binding used to
/// transcribe, which is how one of them came to declare 1024 control buses
/// against a server that had 16 384. **v21 also carries**
/// `clausters_core_peaks_multi_write_buckets`, the receiving half of
/// `/buffer_stream`: a run of buckets somebody else measured, folded into a
/// cache in place, so a client that cannot map the memory a recording is
/// filling still draws it. **v23 the score model** -- `clausters_core_sheet_apply`,
/// `clausters_core_sheet_to_mei` and `clausters_core_sheet_ops`: notation as
/// data a client holds and operations as data it sends, so one implementation
/// of what an edit to a score *means* serves every client and, more to the
/// point, serves a standalone host that has no client language in the process
/// at all. It is the same by-value shape the document surface took at v15, for
/// the same reason. The verbs are **not** symbols -- they ride inside the
/// payload -- so the catalog is what says which exist, and adding one moves
/// nothing here. **v24 the interpreter** -- `clausters_core_sheet_perform` and
/// `clausters_core_interpretation`: the path back out of the score, reading
/// what the symbols *mean* into sounding notes, and the default reading a
/// caller starts from when it wants another one. Two symbols rather than one
/// because an override has to be able to read the defaults before editing them,
/// and a client that wrote those numbers down for itself would play the same
/// score at a different amplitude than the other client does. **v25 the
/// reader** -- `clausters_core_mei_to_sheet`: a *document* back into the model,
/// which is the other return path and the one that makes a score opened from
/// typed text editable at all. One symbol for every notation format there is,
/// because the engraver normalizes whatever it loaded to MEI before this sees
/// it. **v26 the score's edit path** -- `clausters_score_apply` and
/// `clausters_score_sheet`: an open document is edited through the *model's*
/// verbs rather than through the engraver's editor, so there is one
/// implementation of what an edit to a score means and a standalone host
/// performs the same one. The engraver's editor stays as the escape hatch for a
/// document that has no model behind it. **v27 which item a page element is** --
/// `clausters_core_item_id`: the step between a selection on the page and a
/// model verb, answered by the emitter that spelled the element rather than by
/// each client working the spelling out again. **v28 a take's length is
/// seconds** -- `clausters_document_resolve` takes `frames_per_second` beside
/// `frames_per_beat`, because the document now measures a placement in beats
/// and what it places in the unit of that element's own data, so one ratio can no
/// longer answer both questions. **v29 the multitrack's time map** --
/// `clausters_tempomap_*`: a beat is a logical coordinate and the tempo that
/// turns it into a second can change along the multitrack, so the conversion stops
/// being a scalar and becomes an integral. Additive: the affine functions stay
/// exactly as they were, and a one-segment map computes their expression.
/// **v30 a tempo curve has a shape** -- `clausters_tempomap_segment` writes
/// **seven** `f64` instead of six (the seventh is the curvature), and
/// `clausters_tempomap_shaped`/`_env` write a shaped ramp and a whole finite
/// envelope. Breaking rather than additive: the segment payload widened, and
/// its fourth number is now an envelope shape number rather than a flag, so a
/// reader that does not know shape 2 or 5 misreads a segment it can see.
///
/// **v31 the map is a value** -- `clausters_tempomap_version`, `_dump` and
/// `_load`: the edit counter a holder of a *shared* map compares, and the map
/// written out as its breakpoints and read back through the ordinary writers.
/// Additive, and the counter still moves: the ctypes binding declares every
/// symbol eagerly, so a staged library missing these fails at load -- with
/// *"speaks ABI v30, this binding v31"* rather than an `AttributeError` on a
/// name nobody was looking at.
///
/// **v32 the log became a history**, and the rename is the honest part of a
/// breaking change: the handle no longer holds one document's undo but one
/// *editing context* -- the structures registered in it and one ordered pile
/// over them -- so `clausters_log_*` became `clausters_history_*`, with
/// `clausters_history_register` minting a structure's identity and every call
/// that names one taking it. `undo` and `redo` lost their document argument and
/// apply nothing: a history holds structures this surface cannot reach, so they
/// hand back each payload with the structure it belongs to (`{"inverses": ...}`,
/// `{"edits": ..., "remaining": ...}`) and the caller applies them through whatever
/// door each domain has. `record` gained the coalesce **key**, because "the
/// same thing done the same way" is a sentence in a vocabulary the pile does
/// not read.
///
/// **v33 an entry is a transaction.** `clausters_history_record` takes the
/// whole entry as one JSON request -- a label, a coalesce flag and a list of
/// legs, each naming its structure -- because a gesture may touch more than one
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
/// defers the free -- undoing a deletion has to be able to give the data back --
/// with `clausters_history_released` saying when the last entry holding it has
/// retired. And the save mark is the pile's: `clausters_history_mark_saved`,
/// `_dirty` and `_saved_reachable`.
/// **v35 the vocabularies are named once.** `clausters_domain_coalesce_key`
/// answers the coalesce sentence for any domain the crate speaks -- the
/// arrangement, a curve, a span of samples, a timeline -- because a caller
/// recording its own entry has to state a key the pile cannot compute, and
/// spelling four vocabularies' rules again in ctypes and again in TypeScript is
/// the divergence `clausters_document_coalesce_key` was given a door to
/// prevent. Additive, and the counter still moves for the reason v31 states:
/// the ctypes binding declares every symbol eagerly, so a staged library
/// missing this one fails at load with a version mismatch rather than an
/// `AttributeError` on a name nobody was looking at. One behaviour moved with
/// it and is not additive: an **empty** `writesamples` -- the inverse the
/// document can state for a destructive edit -- now bumps the source's
/// generation instead of applying as a no-op, so a reader's copy is marked
/// stale by an undo as it is by the edit.
/// **v36 a domain inverts its own edits.** `clausters_domain_edit` applies a
/// payload to a structure held as its own state -- a curve's points, a
/// timeline's events -- and answers with what it now is *and* the payload that
/// puts it back, both in one call because the inverse has to be read before the
/// edit lands. It is the other half of v35's argument: the coalesce sentence
/// and the inverse are the same vocabulary's rule, and a client computing the
/// second itself is the divergence the first was given a door to prevent. Two
/// domains are deliberately not served: the arrangement's tree, which needs a
/// version to check against and a grid to snap to and has
/// `clausters_document_apply` of its own, and a span of samples, whose frames
/// live in a buffer rather than in a value. Additive, and the counter moves for
/// v31's reason.
///
/// **v37 a score can be put back.** `clausters_score_load` (`JsScore.load` in a
/// page) loads a MEI state into an engraved score and clears the shared layer's
/// own stack, which is what an undo of a page needs once the history is the
/// editing context's rather than the engraver's: one score, one order.
///
/// **v38 a curve's axis is not the view's to invent.**
/// `clausters_core_curve_axis` answers what a break-point curve is *drawn*
/// against -- its range with a tenth of headroom, and, given the axis already in
/// hand, only widened where the data stopped fitting inside it. It is a drawing
/// rule rather than a signal one, and it is here for the reason the project
/// gives for all of them: it was written twice, once per client, and was about
/// to be written a third time for the standalone curve editor. Recomputed per
/// redraw it makes an edit rescale the picture, so the two clients agreeing
/// about it is the difference between one curve drawn one way and one curve
/// drawn two. Additive, and the counter moves for v31's reason.
///
/// **v39 a widget id names what it draws.** `clausters_widgetids_*` is the GUI
/// namespace with two doors over one occupancy map: the anonymous lease a
/// hand-built tree takes, and a **keyed** id asked for by naming the structure,
/// the role and which one it is -- the same name getting the same number for as
/// long as it keeps being drawn. It is here rather than in each client because
/// a leased id is what makes an edit-back in flight across a redraw land on the
/// wrong widget, and a table written twice would agree about that in one
/// language and not the other. `_begin`/`_retire` are the draw cycle: what was
/// not asked for is taken back, ascending, so two clients free the same widgets
/// in the same order. Additive, and the counter moves for v31's reason.
///
/// **v40 a redraw is the difference.** `clausters_gui_difference` answers what
/// to send so a host drawing one picture draws another: one `/gui_set` per
/// widget whose props moved, or the word that says the shape changed and the
/// tree has to go whole. A redefine frees the old subtree, so it takes every
/// widget's screen state with it and drops what the host had pending -- and a
/// walk deciding when that is necessary is a rule, not a convenience, which is
/// why it is here rather than once per client. Additive, and the counter moves
/// for v31's reason.
///
/// **v41 a change of shape costs a subtree, not the window.**
/// `clausters_gui_difference` answers `{whole, redefine, sets}` where it
/// answered `{define}` or `{sets}`: `/gui_def` names any widget, so a clip that
/// appeared in one lane is that lane's definition and every other lane keeps
/// the zoom, the scroll and the selection it had. **Not additive** -- the same
/// symbol answers a different document -- which is exactly why the counter has
/// to move: a staged library one version behind would otherwise be read as
/// saying "nothing changed" for every redraw.
/// **v42 the difference goes, because the host reconciles.**
/// `clausters_gui_difference` is removed. It answered *what to send so a host
/// drawing one picture draws another*, which is only a question a caller
/// holding a copy of the host's picture can ask -- and no caller can hold one:
/// the host mutates on its own (a drag writes an offset per frame, a wheel
/// writes a window, a marquee writes a mark) and screen state is reported by
/// nothing, correctly. A `/gui_def` now means *make it look like this*: the
/// host walks the tree it was handed beside the tree it draws, matches widget
/// to widget by the id that names what it draws, and keeps what is its own. So
/// the decision moved to the only place that has the true copy, and it did not
/// move as this symbol -- it is a comparison of a document with a **widget
/// tree**, not of two documents. **Removing a symbol**, so the counter moves and
/// a caller of it fails to link rather than diffing against a picture nobody is
/// drawing.
/// **v43 a multitrack's picture is the crate's.** `clausters_multitrack_picture`
/// answers the rows and boxes a multitrack draws as, and
/// `clausters_multitrack_read` answers what a report of those boxes *means* in
/// the multitrack's own vocabulary. Both are in beats and seconds, because the shape
/// is the format's and the time is `tempomap`'s; a caller crosses to its own
/// axis with the calls it already binds. **Additive**, and the reason the
/// counter moves at all is that a client which cannot find them has no way to
/// draw a multitrack without writing the mapping again -- which is the thing
/// they exist to prevent.
/// **v44 the routing table is the crate's.** `clausters_view_not_an_edit`
/// answers the `/gui_event` tags that are screen state rather than edits, which
/// each client held as its own literal list. **Additive**, and the counter
/// moves for the same reason v43 did: a client that cannot find it writes the
/// list again, which is the divergence the symbol exists to end.
/// **v45 the catalogue views are the crate's.** `clausters_view_props` answers
/// the widget a waveform, a curve or a roll is and what is on it, from the
/// facts a caller states -- one door with the kind named, so a view the crate
/// learns to draw needs no new symbol. **Additive**, and it moves the counter
/// because a client that cannot find it assembles the picture itself, which is
/// the third implementation this ends.
/// **v46 a history walk is one door.** `clausters_history_walk` takes the
/// direction and answers the legs already gathered per structure, replacing
/// `clausters_history_undo` and `clausters_history_redo`. **Removing symbols**,
/// so a caller of the old pair fails to link rather than keeping its own copy
/// of the two rules that moved: which side of an entry a direction reads, and
/// which legs a structure owns.
/// **v47 a multitrack's curves are in its picture, and a report of them is read
/// here.** `clausters_multitrack_picture` answers `curves` (a track's
/// automations, each a row of its own) and `layers` (a region's, each inside
/// its box) beside the rows and boxes, and `clausters_multitrack_read_points`
/// answers what a report of every curve *means* -- one `SetAutomation` per
/// curve whose break-points moved. **Additive**, and the counter moves for the
/// reason v43's pair did: a client that cannot find them writes the mapping
/// again in its own language.
/// **v48 a report of a multitrack's rows is read here too.**
/// `clausters_multitrack_read_rows` answers what a report of every row *means*
/// -- one `SetTracks` whatever changed, so adding a track, removing one with
/// its boxes, reordering the stack and moving a fader are one verb and one
/// entry. **Additive**, and the counter moves for the reason the other two
/// readers did: the client that could not find it was writing the mixer's half
/// of this mapping in its own language already, and the half it did not have
/// (a track added, a track gone) is exactly where two clients drift.
/// **v50 a projection is bound, not re-derived.** `clausters_editing_*` is the
/// first door onto `clausters-editing`: what an editable structure owes its
/// three endpoints -- the props a host draws it with, and in time the payloads
/// an edit becomes and the operations that make a server sound it. The first of
/// them is a break-point curve's props, which is deliberately the smallest
/// payload there is: what it establishes is that a *projection* crosses here at
/// all, beside the *rules* that already did. A projection written once per
/// client is one no compiler and no test reads against its twin, and that is
/// where a page and a script come to draw one curve two ways. **Additive**, and
/// the counter moves for v31's reason.
/// **v51 a multitrack's props are one answer.** `clausters_editing_multitrack_props`
/// hands back the rows, the boxes, the automations over both, their
/// break-points, which are hidden and which boxes loop -- everything a multitrack
/// has from the document alone. It was written three times before it was
/// written here, and the third was already in Rust: a standalone host draws the
/// same picture with no client in the process, so `clients/gui` linked its own
/// copy of the same sextuple, the same septuple and the same row height.
/// **Additive**, and the counter moves for v31's reason.
/// **v52 a gesture is read once, for every domain there is.**
/// `clausters_editing_intake` is the second projection: a tag and a flat list
/// of values become payloads in a structure's own vocabulary -- a curve's
/// points, a stroke over samples, a roll's two lanes, a multitrack's boxes, rows and
/// break-points. There were sixteen small readers before this, eight per
/// language, each able to disagree with its twin about what a septuple means;
/// it is **one** door for all four because a host reports every gesture the
/// same way, which is also what keeps a client from quietly growing a fifth
/// vocabulary. **Additive**, and the counter moves for v31's reason.
/// **v53 what is sounding is one reconciler.** `clausters_editing_instance_*`
/// is the third projection and the one with state: it holds what was made of
/// the last plan and answers the **difference** as a list of operations --
/// send this def, add this slot, set these ports, free that node. It opens no
/// socket, awaits nothing and allocates nothing, which is what makes it
/// testable with no server in the room and identical under NRT; an operation
/// names what it acts on by a handle rather than by a node id, a bus index or a
/// buffer number, because those are a running session's facts and the client's
/// to allocate. A handle rather than a function, for the same reason
/// `clausters_history_*` is one. **Additive**, and the counter moves for v31's
/// reason.
/// **v54 the conversation is one algorithm.** `clausters_editing_conversation_*`
/// is the protocol every editor speaks whatever it edits: what a message from
/// the host *is* (a close, a history step, an edit made against a picture that
/// is gone, or an edit to route), and what to answer it with. The floor and the
/// staleness rule are in it, which is where the version defects were, and the
/// envelope crosses rather than the payload -- what a report means already
/// crosses once through `clausters_editing_intake`. Pure, both of them: the
/// conversation's whole state is two integers, so a client keeps the pair and
/// hands it back. `clausters_editing_multitrack_names` goes with them, the
/// minting correction's half that is a fact about the multitrack. **Additive**, and
/// the counter moves for v31's reason.
/// **v55 the superseded readers go, and the format is askable.** The four
/// doors `O26` and `O27` replaced -- `clausters_multitrack_picture` and the
/// three `clausters_multitrack_read*` -- are **removed**: the view is
/// `clausters_editing_multitrack_props` and the reading is
/// `clausters_editing_intake`, both clients have called those since `O29`, and
/// a door that answers half of what its replacement answers is a door that
/// invites the wrong call. `clausters_session_format` arrives in their place,
/// so a client can ask what format the crate writes instead of only knowing.
/// **Breaking**: a caller of the four is a caller of symbols that no longer
/// exist.
/// **v56 an editor's window is one composition.** `clausters_apps_multitrack_*`
/// is the first door of the applications crate: the window the multitrack
/// editor opens -- the time ruler above the multitrack, the multitrack, the transport row
/// -- and the props any widget of it is corrected with. Each client composed it
/// for itself and the standalone host composed a third, which had no ruler and
/// no transport at all. **Additive**, and the counter moves for v31's reason.
/// **v57 the editor is a handle.** `clausters_apps_multitrack_editor_*` holds
/// the multitrack editor between messages -- the conversation's floor, the
/// window's ids, the cursor -- and answers every turn: a gesture read and
/// applied with its inverse, the entry to record, the acknowledgement and the
/// corrections. Its verbs cross through one JSON door (`_call`), and it composes
/// the window itself, so the two stateless v56 doors are **removed** --
/// **breaking** for a caller of those two.
/// **v58 steps are carried out once.** `clausters_editing_runner_*` holds the
/// queue a playback's answers are walked through: the messages that may go out
/// now, the one awaited last, and what a reply releases. Each client walked
/// the steps its own way and the standalone host had a third walk, which could
/// not order a join's reads before its stitch. **Additive**, and the counter
/// moves for v31's reason.
/// **v59 one transport.** `clausters_editing_playback_new` takes only `chunk`:
/// the playback makes the transport's group at the top, binds it and makes the
/// multitrack inside it, the same for every endpoint. A client used to bind the
/// multitrack's own graph and the GUI host a group of its own, and said which by
/// passing `target` and `bind_transport`. **Breaking**: a caller of the
/// three-argument door is a caller of a signature that no longer exists.
/// **v60 a session is loaded once.** `clausters_editing_load` plans the reads
/// and the stitches that put a saved session's sources into a server, as steps
/// for the runner. Only the GUI host could open a session and sound it; a
/// client loaded each take by hand and could not load a join at all.
/// **Additive**, and the counter moves for v31's reason.
/// **v61 the samples editor's window is the crate's.**
/// `clausters_apps_samples_editor_*` holds a take, the measures its picture
/// stacks and the window's chrome, and composes and corrects the window; each
/// client composed it for itself. `clausters_apps_samples_measures` checks a
/// measure stack. **Additive**, and the counter moves for v31's reason.
/// **v62 one undo order across applications.** `clausters_apps_editing_*` holds
/// an editing context: the history, the version and the editors opened in it,
/// whose turns and history steps it takes. A verb runs once across its sizing
/// and filling calls, since a history is not copied for a sizing pass.
/// **Additive**, and the counter moves for v31's reason.
/// **v63 a take's step is its writes.** `clausters_apps_editing_new` takes no
/// argument: a step hands a take's writes back as payloads, which the member
/// turns into steps with the bound of the server the take is on, so a context
/// carries no bound of its own. An open answers the structure, and `record`
/// takes `coalesce` and answers the version. **Breaking** for a caller of the
/// one-argument door.
/// **v64 an editor is only a member.** `clausters_apps_multitrack_editor_*` and
/// `clausters_apps_samples_editor_*` are **removed**: both clients open their
/// editors in an editing context (`clausters_apps_editing_*`), which holds the
/// history they share. **Breaking** for a caller of the editor handles.
/// **v65 true peak.** `clausters_core_true_peak` measures the *reconstructed*
/// peak of one channel of an interleaved buffer -- the ITU-R BS.1770-4 Annex 2
/// filter at 4×, which is what makes a reading dBTP -- where
/// `clausters_core_stats` reports the largest sample. **Additive**, and the
/// counter moves for v31's reason.
/// **v66 loudness.** `clausters_core_loudness` measures an interleaved buffer
/// as ITU-R BS.1770 and EBU R 128 do: the gated integrated loudness, the
/// loudness range and the maximum momentary and short-term loudness.
/// **Additive**, and the counter moves for v31's reason.
/// **v67 the multitrack is in seconds.** A region, a fade, a curve point, a
/// marker and a span are seconds, so nothing that plans, draws or reads a
/// multitrack takes a tempo: `clausters_multitrack_plan`,
/// `clausters_editing_multitrack_props` and
/// `clausters_editing_instance_reconcile` lose `default_bpm`, a playback's
/// `locate` and `cue` take seconds, `clausters_editing_playback_beats_to_samples`
/// and `_samples_to_beats` become `_secs_to_samples` and `_samples_to_secs`, and
/// `clausters_editing_default_bpm` becomes `clausters_editing_default_tempo`, in
/// beats per second, which only a ruler reads. `clausters_session_migrate` reads
/// a session written in an older format as this one writes it. **Breaking**.
/// **v68 the structure is named, and a refusal says why.** The editing request
/// and outcome key `piece` is `multitrack`, the instance handle a playback
/// makes is `"multitrack"`, the mixer's graph is `clausters.multitrack.N`, and
/// the multitrack window's transport controls are `transport_rewind`,
/// `transport_play`, `transport_stop` and `transport_clock`.
/// `clausters_view_props` answers `{"error": reason}` for a kind it does not
/// draw or facts that will not read, where it answered nothing -- which both
/// clients stamped into a widget with nothing on it. **Breaking**.
pub const CORE_ABI_VERSION: u32 = 68;

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
    let Some(text) = (unsafe { crate::document::text(session, session_len) }) else {
        return 0;
    };
    let Ok(written) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    let answer = clausters_document::session::migrate(written).to_string();
    // SAFETY: forwarded from this function's own contract. A pure read.
    unsafe { crate::document::fill(answer.as_bytes(), out, out_cap, || {}) }
}
