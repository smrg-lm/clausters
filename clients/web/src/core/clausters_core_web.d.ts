/* tslint:disable */
/* eslint-disable */

/**
 * One composition, held in Rust — the JS face of
 * [`clausters_document::Document`].
 */
export class Document {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Apply an edit. `apply(requestJson) -> outcomeJson`, the request carrying
     * `{ intent, against?, quant? }` and the result the outcome alone —
     * the document stays here and `snapshot` is how it leaves.
     *
     * One object rather than three arguments because the boundary is JSON
     * either way, and a request that grows a field then costs no signature.
     */
    apply(request: string): string;
    /**
     * Open a document from its JSON, or an empty composition from `undefined`.
     */
    constructor(json?: string | null);
    /**
     * Resolve a selection to the spans of samples underneath it.
     * `resolve(requestJson) -> resolvedJson`, the request carrying
     * `{ selection, framesPerBeat, inBeats? }`.
     */
    resolve(request: string): string;
    /**
     * The whole tree as JSON — for saving it, or for a caller that wants it.
     * The one call that still costs the size of the composition, and it is
     * asked for rather than paid on every edit.
     */
    snapshot(): string;
    /**
     * The monotonic version, bumped by every applied edit. Never zero.
     */
    readonly version: bigint;
}

/**
 * The undo history of one document, the JS face of
 * [`clausters_document::Log`].
 */
export class Log {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Apply an edit to `document` **and record it**, in one call: the inverse
     * has to be read out of the document before the edit lands, so applying
     * first and recording second would record the wrong thing.
     * `apply(document, requestJson) -> outcomeJson`, the request carrying
     * `{ intent, against?, quant?, label? }`.
     */
    apply(document: Document, request: string): string;
    /**
     * Forget everything, releasing what was spilled.
     */
    clear(): void;
    /**
     * A new log. `budget` is how many entries it keeps before the oldest falls
     * off and `spillAbove` how many `f32` values a sample payload must reach
     * before it leaves the log; either as 0 takes the crate's default.
     */
    constructor(budget: number, spill_above: number);
    /**
     * Record an entry the document cannot supply the inverse for — the
     * destructive case, whose overwritten samples are not in the tree. Applies
     * nothing. `record(requestJson)` with
     * `{ forward, backward, label?, coalesce? }`.
     */
    record(request: string): void;
    /**
     * Redo what was last undone, applying what it can to `document`. Returns
     * `{ remaining }` — the ordinary edits at the front are already applied,
     * and `remaining` holds the steps from the first one the crate cannot
     * perform onward, for the owner to re-run. `undefined` when there was
     * nothing to redo.
     */
    redo(document: Document): string | undefined;
    /**
     * Undo the last transaction, applying its inverses to `document`.
     * Returns `{ undone }`, or `undefined` when there was nothing to undo.
     */
    undo(document: Document): string | undefined;
    /**
     * Whether there is anything to redo.
     */
    readonly canRedo: boolean;
    /**
     * Whether there is anything to undo.
     */
    readonly canUndo: boolean;
    /**
     * Whether the log holds nothing — `len == 0`, spelled the way a JS
     * collection is read, as `JsScheduler` and `JsRegistry` already spell it.
     */
    readonly isEmpty: boolean;
    /**
     * How many entries the log holds.
     */
    readonly len: number;
    /**
     * What a redo would be called.
     */
    readonly redoLabel: string | undefined;
    /**
     * What an undo would be called, for a menu item.
     */
    readonly undoLabel: string | undefined;
}

/**
 * A built min/max peak pyramid, the JS face of
 * [`clausters_core::peaks::MultiPyramid`] — the summary a waveform view is
 * drawn from, so the drawing costs the width of the window rather than the
 * length of the buffer. Built (or filled from `/buffer_stream` reports) here
 * and handed to the GUI host, which draws it; the readers below answer **what
 * the cache is** — length, channels, bucket, levels — and never what it says,
 * which is a drawing's question.
 */
export class Pyramid {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Builds one pyramid per channel from interleaved `samples`.
     * `base_bucket` is the level-0 bucket size (256 is the usual choice:
     * ~0.8% of the source in cache for a floor of 256 samples per column).
     */
    static build(samples: Float32Array, channels: number, base_bucket: number): Pyramid;
    /**
     * **An empty pyramid of a given length** — the picture of a take that has
     * been allocated and not yet recorded into, ready to be filled by
     * [`Self::write_buckets`] as the reports arrive.
     *
     * Building one out of a buffer of silence instead would allocate the take
     * (230 MB for ten minutes of stereo) to summarize samples nobody wrote.
     */
    static empty(frames: number, channels: number, base_bucket: number): Pyramid;
    /**
     * Reads back a serialized cache (`toBytes`, or the file the GUI host maps
     * and the Python client writes). `undefined` when the bytes are not one.
     */
    static fromBytes(data: Uint8Array): Pyramid | undefined;
    /**
     * The cache's bytes, in the format every client reads: the mono layout
     * for a single channel and the multichannel one above it — the choice
     * the Python client's door makes, so the same samples serialize to the
     * same bytes whichever client reduced them. Both are read back by
     * `fromBytes` and by the GUI host.
     */
    toBytes(): Uint8Array;
    /**
     * Rewrites the part of the cache a **frame span** touches, from the
     * interleaved buffer as it now stands — what keeps an editor's overview
     * true after an edit without re-summarizing the take.
     *
     * `samples` is the whole buffer, not the span: a bucket at either edge of
     * it holds untouched samples too. Returns whether it applied — `false`,
     * changing nothing, when the buffer is not the one this cache describes,
     * which is an edit that changed the *length* and therefore a rebuild.
     */
    updateRange(samples: Float32Array, start: number, frames: number): boolean;
    /**
     * Folds a run of **already-summarized buckets** into this pyramid — the
     * receiving half of `/buffer_stream`, which is how a page follows a
     * recording it cannot map: the server sends the overview of what was
     * written (2 kB/s where the audio is 190) and this puts it in the
     * picture.
     *
     * `stats` is the reply's blob read as `f32`s, **bucket-major and
     * channel-minor**: for each bucket of `bucket` frames in order, for each
     * channel, `min`, `max` and mean square. `startFrame` is where the report
     * begins on the buffer's own sample axis. Returns whether it applied —
     * `false`, changing nothing, when the report is on another grid than this
     * cache (another bucket size, a start off a bucket boundary, or a run
     * that does not fit).
     */
    writeBuckets(start_frame: number, bucket: number, stats: Float32Array): boolean;
    readonly baseBucket: number;
    readonly channels: number;
    /**
     * Samples per channel — the length a view of this cache spans.
     */
    readonly frames: number;
    readonly numLevels: number;
}

/**
 * A registry of one finite id space, the JS face of
 * [`clausters_core::registry::Registry`].
 */
export class Registry {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Allocates `width` contiguous ids and returns the first, or `undefined`
     * when no such run is free. `width` 0 counts as 1.
     */
    alloc(width: number): number | undefined;
    /**
     * Releases everything back to the pool (a client reset).
     */
    clear(): void;
    /**
     * Whether `id` falls inside this registry's space (allocated or not) —
     * the filter for foreign `/node_end` ids.
     */
    contains(id: number): boolean;
    /**
     * Whether `id` is currently allocated.
     */
    isAllocated(id: number): boolean;
    /**
     * A bounded registry over `[base, base + capacity)`.
     */
    constructor(base: number, capacity: number);
    /**
     * Returns `width` ids starting at `first` to the pool. `true` when the
     * release was accepted; `false` leaves the map untouched (out of range,
     * or not currently allocated — a double release).
     */
    release(first: number, width: number): boolean;
    /**
     * The NRT/score registry: allocation never fails, ids ascend from `base`.
     */
    static unbounded(base: number): Registry;
    /**
     * The first id of the space.
     */
    readonly base: number;
    /**
     * The size of the id space; `undefined` when unbounded.
     */
    readonly capacity: number | undefined;
    /**
     * How many ids are currently allocated.
     */
    readonly inUse: number;
}

/**
 * A resumable seeded value stream, the JS face of
 * [`clausters_core::rng::Rng`].
 */
export class Rng {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * The stream for `seed` (splitmix64-mixed, never zero) — the same seeding
     * as the server's `WhiteNoise`.
     */
    constructor(seed: number);
    /**
     * Uniform integer in `[0, n)`; 0 when `n` is 0.
     */
    nextBelow(n: number): number;
    /**
     * Uniform in `[0, 1)` with 53-bit resolution.
     */
    nextF64(): number;
    /**
     * A child stream seeded from this one's next word: deterministic
     * derivation, so seeding a root reproduces every stream created under it,
     * in creation order.
     */
    spawn(): Rng;
    /**
     * Uniform in `[lo, hi)` (degenerate to `lo` when `hi <= lo`).
     */
    uniform(lo: number, hi: number): number;
}

/**
 * The local-time ↔ sample regression, the JS face of
 * [`clausters_core::clocksync::SampleClockModel`].
 */
export class SampleClockModel {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Records one `/clock_query` observation: the local time it was taken at, the
     * server's counter, and the rate the server reported (0 keeps the
     * current one).
     */
    addAnchor(t_local: number, sample: number, rate: number): void;
    /**
     * The local time at which a server sample falls.
     */
    localTimeOf(sample: number): number;
    /**
     * A model seeded with the `nominal_rate`, keeping the last `window`
     * anchors.
     */
    constructor(nominal_rate: number, window: number);
    /**
     * The server's sample at a local time.
     */
    sampleAt(t_local: number): number;
    /**
     * The measured drift between the two clocks, in parts per million.
     */
    readonly driftPpm: number;
    readonly isEmpty: boolean;
    /**
     * How many anchors the model currently holds.
     */
    readonly len: number;
    /**
     * The measured sample rate (samples per local second).
     */
    readonly rate: number;
    /**
     * The local-time span the held anchors cover.
     */
    readonly span: number;
}

/**
 * The beat-ordered scheduling queue, the JS face of
 * [`clausters_core::tempoclock::Scheduler`]. It holds `(time, id)` pairs and
 * nothing else: the language side maps each id back to the routine it queued,
 * which is what keeps the coroutine driver in the language.
 */
export class Scheduler {
    free(): void;
    [Symbol.dispose](): void;
    clear(): void;
    constructor();
    /**
     * The earliest queued time, or `undefined` when the queue is empty.
     */
    peekTime(): number | undefined;
    /**
     * Pops the earliest entry when it is due at `now`, as `[time, id]`.
     */
    popDue(now: number): Float64Array | undefined;
    /**
     * Queues `id` at `time` (beats). Equal times keep insertion order.
     */
    push(time: number, id: number): void;
    /**
     * Drops every entry queued under `id`; returns how many went.
     */
    remove(id: number): number;
    readonly isEmpty: boolean;
    readonly len: number;
}

/**
 * A loaded score, held open in Rust so it can be edited and re-engraved — the
 * JS face of [`clausters_core::notation::Score`].
 *
 * The same object the Python client holds over the C ABI, running the same
 * state machine: a page that transposes a note and one that transposes it in a
 * window take the identical sequence of calls to verovio.
 */
export class Score {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Apply one **model** operation as a single undo step, and re-engrave.
     *
     * This is the edit path, and the reason it is not the engraver's editor:
     * there is one implementation of what an edit to a score means, and it is
     * vocabulary this package already binds. Returns whether it was applied;
     * a refusal leaves both the page and the model as they were.
     */
    apply(op: string): boolean;
    /**
     * This score engraved into a page: the display list the host draws, the
     * cursor track a playhead follows, and the notes that sound.
     */
    displayList(page: number): string;
    /**
     * One raw editor action (`set`, `insert`, `delete`, …) as a single undo
     * step, `param` being its parameter object as JSON.
     */
    edit(action: string, param: string): boolean;
    /**
     * The score as MEI, ids and all — what to persist, and what an undo step
     * is made of.
     */
    mei(): string;
    /**
     * Load `data` (any format the engraver auto-detects) on `engraver`, or
     * throw when it could not be read.
     *
     * Configuring the engraver — its resource path, its options — happens on
     * the JS side before this, exactly as the native binding configures its
     * toolkit before handing it over.
     */
    constructor(engraver: object, data: string);
    /**
     * Step forward again after an undo; `false` when there is nothing to redo.
     */
    redo(): boolean;
    /**
     * The open score as the **model**.
     *
     * Throws when the document could not be read into one — a state and not a
     * failure, since the page still draws and still plays and only the model's
     * verbs are unavailable on it.
     */
    sheet(): string;
    /**
     * Move a note by `steps` diatonic steps along the staff, as one undo step.
     * The relative form: reach for it only when the delta is what you have.
     */
    transpose(element_id: string, steps: number): boolean;
    /**
     * Move a note **to** a diatonic staff position, as one undo step — the
     * shape an edit travels in, so a resend cannot move the note twice.
     */
    transposeTo(element_id: string, position: number, page: number): boolean;
    /**
     * Step back one edit; `false` when there is nothing to undo.
     */
    undo(): boolean;
    /**
     * Whether there is an undone edit to step forward into.
     */
    readonly canRedo: boolean;
    /**
     * Whether there is an edit to step back over.
     */
    readonly canUndo: boolean;
}

/**
 * The 0-based bar index `beats` falls in on a grid of `quant` beats per bar.
 */
export function bar(beats: number, quant: number): number;

/**
 * The beat within its bar, in `[0, quant)`.
 */
export function beat_in_bar(beats: number, quant: number): number;

/**
 * Seconds at `beats` for the affine clock `(tempo, base_beats, base_seconds)`.
 */
export function beats_to_secs(tempo: number, base_beats: number, base_seconds: number, beats: number): number;

/**
 * JS face: one binary builtin by name (`"add"`, `"pow"`, `"clip2"`, ...).
 */
export function binary(op: string, a: number, b: number): number;

/**
 * JS face: what one instance of a bundle needs allocated.
 * `bundle_requirements(requestJson) -> requirementsJson`, the request holding
 * the manifest and — for a bundle written before the contract, whose widget
 * ids are whatever its author picked — the template its id block is measured
 * from.
 */
export function bundle_requirements(request: string): string;

/**
 * JS face: one mounted instance, from the allocation the page just made.
 * `bundle_resolve(requestJson) -> resolvedJson`, the request carrying the
 * manifest, the template, the allocation and the supplied parameters
 * (`{ attributes, preset }`) in one object.
 */
export function bundle_resolve(request: string): string;

/**
 * JS face: the writers' pre-flight — the mount dry-run over the declared
 * defaults, plus the no-holes check on every def payload.
 * `bundle_validate(requestJson)`, throwing on the first problem.
 */
export function bundle_validate(request: string): void;

/**
 * JS face: the **peak and RMS** of one channel of an interleaved buffer, as
 * `[peak, rms]` — what a render reports back about what it produced. The
 * stride walk measures a render without deinterleaving it first, so a page
 * reads the same two numbers the server and the Python client report.
 *
 * An empty pair for a channel the buffer does not have.
 */
export function channel_stats(samples: Float32Array, channels: number, channel: number): Float32Array;

/**
 * JS face: the stereo **correlation** (Pearson's r) of two equal-length
 * channels, in `[-1, 1]`. `undefined` when it is undefined — a length
 * mismatch, an empty pair, or a constant channel.
 */
export function correlation(left: Float32Array, right: Float32Array): number | undefined;

/**
 * JS face: scale degree → MIDI note number in the pitch space
 * `octave`/`root`, with floored octave wrapping (sclang semantics). An empty
 * `scale` yields middle C.
 */
export function degree_to_midinote(degree: number, octave: number, root: number, scale: Float32Array): number;

/**
 * The engraver's options for one page, as the JSON object it is configured
 * with: `scale` (staff size), `pageWidth` (the page units a score wraps into
 * systems at) and an optional JSON object merged over them.
 *
 * A page configures its engraver through this rather than through a table of
 * its own, for the same reason the score model is shared: two clients that
 * configure verovio differently draw the same score two ways, and then no
 * display list from one is comparable with one from the other.
 */
export function engraveOptions(scale: number, page_width: number, extra?: string | null): string;

/**
 * JS face: the `[audio, control]` bus widths GraphDef instances reserve at
 * the top of each bus space (before clamping to a smaller configured count).
 */
export function graph_bus_reserved(): Uint32Array;

/**
 * The default interpretation, as JSON — every number the reading depends on,
 * and the value an override starts from.
 *
 * The parity surface for the reading, as `sheetOps` is for the verbs: the
 * interpretation crosses inside a payload, so nothing structural notices when
 * one client's idea of `mf` drifts from the other's.
 */
export function interpretation(): string;

/**
 * JS face: the **Lissajous / goniometer** projection of a stereo pair, as
 * interleaved `[x, y]` pairs (`x` = side, `y` = mid) — one pair per input
 * frame. An empty array when the two channels differ in length.
 */
export function lissajous(left: Float32Array, right: Float32Array): Float32Array;

/**
 * JS face: one range map by name (`"linlin"`, `"linexp"`, `"lincurve"`, ...),
 * with `clip` naming what an out-of-range input is trimmed to (`"minmax"`,
 * `"min"`, `"max"`, `"none"`). `curve` is read only by the bent pair and the
 * input bounds only by the maps that have an input range.
 */
export function map(op: string, clip: string, x: number, in_lo: number, in_hi: number, out_lo: number, out_hi: number, curve: number): number;

/**
 * Read an MEI document into the score model.
 *
 * The other return path, and not the one {@link sheetPerform} is: that turns a
 * model into sound, this turns a *document* into a model. A score opened from
 * typed text is a document and nothing else until this reads one, which is why
 * the model's verbs cannot touch it until then. There is one input format rather than
 * four, because the engraver normalizes whatever it loaded to MEI.
 */
export function meiToSheet(mei: string): string;

/**
 * MIDI 2.0 Clip File (SMF2CLIP) bytes from the same arguments, carrying note
 * velocities at 16-bit resolution.
 *
 * JS face: `midiWriteClip(Uint32Array, Uint8Array, ppq) -> Uint8Array`.
 */
export function midiWriteClip(ticks: Uint32Array, msgs: Uint8Array, ppq: number): Uint8Array;

/**
 * Type-0 Standard MIDI File bytes from `n` events at `ppq` ticks per quarter
 * note.
 *
 * JS face: `midiWriteSmf(Uint32Array, Uint8Array, ppq) -> Uint8Array`.
 */
export function midiWriteSmf(ticks: Uint32Array, msgs: Uint8Array, ppq: number): Uint8Array;

/**
 * JS face: the boot-derived node-id partition for a node table of
 * `max_nodes` slots — `{clientBase, clientCapacity, autoBase, autoCapacity,
 * midiBase, midiCapacity}`, the same formula the server applies.
 */
export function node_id_partition(max_nodes: number): object;

/**
 * JS face: `osc_decode_packet(bytes) -> [{addr, args}, ...]`, bundles
 * flattened, args as plain JS values (numbers/strings/`Uint8Array`/bool/
 * null).
 */
export function osc_decode_packet(bytes: Uint8Array): Array<any>;

/**
 * JS face: `osc_decode_packet_timed(bytes) -> [{addr, args, time}, ...]` —
 * [`osc_decode_packet`] plus the containing bundle's time, in Unix seconds
 * (`null` for an immediate bundle or a bare message). What the responder
 * layer reads, so a handler is given the same `time` the Python client hands
 * its own.
 */
export function osc_decode_packet_timed(bytes: Uint8Array): Array<any>;

/**
 * JS face: a bundle stamped at `unix_secs` (the wall clock the server reads
 * as an NTP timetag) → `Uint8Array`.
 */
export function osc_encode_bundle(unix_secs: number, messages: Array<any>): Uint8Array;

/**
 * JS face: a bundle with the *immediate* timetag → `Uint8Array`. What rides
 * inside `/sched_at`, whose own absolute sample carries the time.
 */
export function osc_encode_immediate_bundle(messages: Array<any>): Uint8Array;

/**
 * JS face: `osc_encode_message(addr, [[tag, value], ...]) -> Uint8Array`.
 * Tags: `"i"` int32, `"h"` int64 (number or BigInt), `"f"` float32, `"d"`
 * float64, `"s"` string, `"b"` blob (`Uint8Array`).
 */
export function osc_encode_message(addr: string, args: Array<any>): Uint8Array;

/**
 * JS face: a bundle stamped at `secs` **from the start of a render** → the
 * bundle an NRT score is made of. The same packing as [`osc_encode_bundle`]
 * on a different epoch: a score's time is not a wall clock, so nothing is
 * added to it (`clausters_core::osc::pack_timetag`, the rule every client
 * shares — the Python client reaches it through `clausters_core_ntp_timetag`
 * and assembles the bundle itself).
 */
export function osc_encode_score_bundle(secs: number, messages: Array<any>): Uint8Array;

/**
 * The patcher's **cord→bus pass**: a directed patch (`{boxes, cords}`) in, the
 * buses and wired members it compiles to out, both as JSON.
 *
 * One bus per connected net, its writers summing, and a bad cord — reversed,
 * rate-mismatched, out of range — comes back as `{"error": …}` naming it. The
 * same door the C ABI opens as `clausters_core_patch_compile`: a patcher is a
 * model with one compilation, and a second implementation of it in TypeScript
 * would be a second answer to "what does this cord mean".
 */
export function patchCompile(patch: string): string;

/**
 * Beats to wait so a routine starts on the next `quant` boundary of the grid
 * (`quant <= 0` → now). The snapping rule every client shares.
 */
export function quant_delay(pos: number, quant: number): number;

/**
 * Sample count → seconds at `sample_rate`.
 */
export function samples_to_secs(samples: number, sample_rate: number): number;

/**
 * Beats at `secs` for the affine clock `(tempo, base_beats, base_seconds)`.
 */
export function secs_to_beats(tempo: number, base_beats: number, base_seconds: number, secs: number): number;

/**
 * Seconds → sample count at `sample_rate` (ties to even).
 */
export function secs_to_samples(secs: number, sample_rate: number): number;

/**
 * Apply one operation to a score model, both as JSON, returning the new model.
 *
 * **One export for every operation there will ever be**: the verb and its
 * parameters are inside `op` (`{"op": "transpose", "semitones": 2}`), so a new
 * operation costs nothing here. What the C ABI answers in an envelope
 * (`{"ok": …}` / `{"error": …}`) this **throws** instead, which is the same
 * behaviour in the shape a page expects — and the reason the refusal reaches
 * the caller either way, since a refused operation has to say why.
 *
 * A refused operation changes nothing: the model crossed by value, so the
 * caller still holds what it sent.
 */
export function sheetApply(sheet: string, op: string): string;

/**
 * Every operation this core knows, as JSON — the verb and the parameters each
 * takes.
 *
 * The parity surface the binding table cannot provide: operations cross as
 * data, so nothing fails when one client grows a verb the other lacks. This
 * package is contrasted against this list, as the Python client is.
 */
export function sheetOps(): string;

/**
 * Read a score model into the notes it **sounds**, under `interp`.
 *
 * Each note carries two lengths — `dur`, what is written, and `sustain`, what
 * is heard — because an honoured articulation makes them different numbers and
 * collapsing them would move every attack after a staccato. It also names the
 * `staff` and `voice` it was written on, which is what a caller binds an
 * instrument to: the notation does not say what plays it.
 *
 * `interp` may be `""` or `"{}"` for the default reading, and any field left
 * out keeps its default. What the default *is* comes back from
 * [`interpretation`], so this package writes none of those numbers down.
 */
export function sheetPerform(sheet: string, interp: string): string;

/**
 * Write a score model out as MEI.
 *
 * Throws with the emitter's own reason when the model holds something MEI
 * cannot be written for yet — a duration that is not an exact note value, an
 * accidental past a double, or polyphony — each saying which it is, so a
 * caller knows whether it is wrong or early.
 */
export function sheetToMei(sheet: string): string;

/**
 * Walk a verovio SVG into a `score` display list, as JSON.
 *
 * The one-shot path: a page that only draws a score engraves once and walks
 * the SVG, with no document held open. A malformed SVG yields an empty display
 * list rather than an error, as the C ABI's twin does.
 */
export function svgToDisplayList(svg: string): string;

/**
 * JS face: one unary builtin by name (`"midicps"`, `"cpsmidi"`, `"dbamp"`,
 * ...), computed in `f32` exactly as the server's UGens compute it.
 */
export function unary(op: string, x: number): number;

/**
 * A Unix timestamp → the 64 NTP timetag bits, as a `BigInt` (the wire value
 * is a full 64-bit word; JS numbers would lose its low bits).
 */
export function unix_to_ntp(unix_secs: number): bigint;

/**
 * A Unix timestamp → the server's absolute sample, through a `/clock_query` anchor
 * (`anchor_unix`, `anchor_sample`) and the measured `rate`.
 */
export function unix_to_sample(unix_secs: number, anchor_unix: number, anchor_sample: number, rate: number): number;

/**
 * Lay a **voice** — a JSON array of slots, `{"midis": [60], "ticks": 8}` per
 * note or chord and `{"ticks": 8}` per rest — out into barred, tied MEI.
 *
 * `meter` is `"num/den"`, `clef` a shape+line like `"G2"`, and `key` selects
 * the key signature and the sharp-vs-flat spelling. Reducing a client's own
 * sequencing data to that voice stays in the client, where the native types
 * are; this is the language-agnostic step below it, and the seam a richer
 * encoding extends for every client at once.
 */
export function voiceToMei(voice: string, meter: string, clef: string, key: string): string;

/**
 * Lift a **voice** — the v1 wire form, a JSON array of slots — into the score
 * model.
 *
 * The bridge a client crosses once: it reduces its own sequencing types to
 * slots, which reads client-native types and stays here, and everything above
 * that is the model. Ticks become exact durations and MIDI numbers become
 * spelled pitches in the world `key` implies.
 */
export function voiceToSheet(voice: string, meter: string, clef: string, key: string): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_document_free: (a: number, b: number) => void;
    readonly __wbg_log_free: (a: number, b: number) => void;
    readonly __wbg_pyramid_free: (a: number, b: number) => void;
    readonly __wbg_registry_free: (a: number, b: number) => void;
    readonly __wbg_rng_free: (a: number, b: number) => void;
    readonly __wbg_sampleclockmodel_free: (a: number, b: number) => void;
    readonly __wbg_scheduler_free: (a: number, b: number) => void;
    readonly __wbg_score_free: (a: number, b: number) => void;
    readonly bar: (a: number, b: number) => number;
    readonly beat_in_bar: (a: number, b: number) => number;
    readonly beats_to_secs: (a: number, b: number, c: number, d: number) => number;
    readonly binary: (a: number, b: number, c: number, d: number) => [number, number, number];
    readonly bundle_requirements: (a: number, b: number) => [number, number, number, number];
    readonly bundle_resolve: (a: number, b: number) => [number, number, number, number];
    readonly bundle_validate: (a: number, b: number) => [number, number];
    readonly channel_stats: (a: number, b: number, c: number, d: number) => [number, number];
    readonly correlation: (a: number, b: number, c: number, d: number) => number;
    readonly degree_to_midinote: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly document_apply: (a: number, b: number, c: number) => [number, number, number, number];
    readonly document_new: (a: number, b: number) => [number, number, number];
    readonly document_resolve: (a: number, b: number, c: number) => [number, number, number, number];
    readonly document_snapshot: (a: number) => [number, number, number, number];
    readonly document_version: (a: number) => bigint;
    readonly engraveOptions: (a: number, b: number, c: number, d: number) => [number, number];
    readonly graph_bus_reserved: () => [number, number];
    readonly interpretation: () => [number, number, number, number];
    readonly lissajous: (a: number, b: number, c: number, d: number) => [number, number];
    readonly log_apply: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly log_canRedo: (a: number) => number;
    readonly log_canUndo: (a: number) => number;
    readonly log_clear: (a: number) => void;
    readonly log_isEmpty: (a: number) => number;
    readonly log_len: (a: number) => number;
    readonly log_new: (a: number, b: number) => number;
    readonly log_record: (a: number, b: number, c: number) => [number, number];
    readonly log_redo: (a: number, b: number) => [number, number, number, number];
    readonly log_redoLabel: (a: number) => [number, number];
    readonly log_undo: (a: number, b: number) => [number, number, number, number];
    readonly log_undoLabel: (a: number) => [number, number];
    readonly map: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly meiToSheet: (a: number, b: number) => [number, number, number, number];
    readonly midiWriteClip: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly midiWriteSmf: (a: number, b: number, c: number, d: number, e: number) => [number, number, number, number];
    readonly node_id_partition: (a: number) => [number, number, number];
    readonly osc_decode_packet: (a: number, b: number) => [number, number, number];
    readonly osc_decode_packet_timed: (a: number, b: number) => [number, number, number];
    readonly osc_encode_bundle: (a: number, b: any) => [number, number, number, number];
    readonly osc_encode_immediate_bundle: (a: any) => [number, number, number, number];
    readonly osc_encode_message: (a: number, b: number, c: any) => [number, number, number, number];
    readonly osc_encode_score_bundle: (a: number, b: any) => [number, number, number, number];
    readonly patchCompile: (a: number, b: number) => [number, number, number, number];
    readonly pyramid_baseBucket: (a: number) => number;
    readonly pyramid_build: (a: number, b: number, c: number, d: number) => number;
    readonly pyramid_channels: (a: number) => number;
    readonly pyramid_empty: (a: number, b: number, c: number) => number;
    readonly pyramid_frames: (a: number) => number;
    readonly pyramid_fromBytes: (a: number, b: number) => number;
    readonly pyramid_numLevels: (a: number) => number;
    readonly pyramid_toBytes: (a: number) => [number, number];
    readonly pyramid_updateRange: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly pyramid_writeBuckets: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly quant_delay: (a: number, b: number) => number;
    readonly registry_alloc: (a: number, b: number) => [number, number];
    readonly registry_base: (a: number) => number;
    readonly registry_capacity: (a: number) => number;
    readonly registry_clear: (a: number) => void;
    readonly registry_contains: (a: number, b: number) => number;
    readonly registry_inUse: (a: number) => number;
    readonly registry_isAllocated: (a: number, b: number) => number;
    readonly registry_new: (a: number, b: number) => number;
    readonly registry_release: (a: number, b: number, c: number) => number;
    readonly registry_unbounded: (a: number) => number;
    readonly rng_new: (a: number) => number;
    readonly rng_nextBelow: (a: number, b: number) => number;
    readonly rng_nextF64: (a: number) => number;
    readonly rng_spawn: (a: number) => number;
    readonly rng_uniform: (a: number, b: number, c: number) => number;
    readonly sampleclockmodel_addAnchor: (a: number, b: number, c: number, d: number) => void;
    readonly sampleclockmodel_driftPpm: (a: number) => number;
    readonly sampleclockmodel_isEmpty: (a: number) => number;
    readonly sampleclockmodel_len: (a: number) => number;
    readonly sampleclockmodel_localTimeOf: (a: number, b: number) => number;
    readonly sampleclockmodel_new: (a: number, b: number) => number;
    readonly sampleclockmodel_rate: (a: number) => number;
    readonly sampleclockmodel_sampleAt: (a: number, b: number) => number;
    readonly sampleclockmodel_span: (a: number) => number;
    readonly samples_to_secs: (a: number, b: number) => number;
    readonly scheduler_clear: (a: number) => void;
    readonly scheduler_isEmpty: (a: number) => number;
    readonly scheduler_len: (a: number) => number;
    readonly scheduler_new: () => number;
    readonly scheduler_peekTime: (a: number) => [number, number];
    readonly scheduler_popDue: (a: number, b: number) => [number, number];
    readonly scheduler_push: (a: number, b: number, c: number) => void;
    readonly scheduler_remove: (a: number, b: number) => number;
    readonly score_apply: (a: number, b: number, c: number) => [number, number, number];
    readonly score_canRedo: (a: number) => number;
    readonly score_canUndo: (a: number) => number;
    readonly score_displayList: (a: number, b: number) => [number, number, number, number];
    readonly score_edit: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly score_mei: (a: number) => [number, number];
    readonly score_new: (a: any, b: number, c: number) => [number, number, number];
    readonly score_redo: (a: number) => number;
    readonly score_sheet: (a: number) => [number, number, number, number];
    readonly score_transpose: (a: number, b: number, c: number, d: number) => number;
    readonly score_transposeTo: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly score_undo: (a: number) => number;
    readonly secs_to_beats: (a: number, b: number, c: number, d: number) => number;
    readonly secs_to_samples: (a: number, b: number) => number;
    readonly sheetApply: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly sheetOps: () => [number, number, number, number];
    readonly sheetPerform: (a: number, b: number, c: number, d: number) => [number, number, number, number];
    readonly sheetToMei: (a: number, b: number) => [number, number, number, number];
    readonly svgToDisplayList: (a: number, b: number) => [number, number, number, number];
    readonly unary: (a: number, b: number, c: number) => [number, number, number];
    readonly unix_to_ntp: (a: number) => bigint;
    readonly unix_to_sample: (a: number, b: number, c: number, d: number) => number;
    readonly voiceToMei: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly voiceToSheet: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number, number, number];
    readonly clausters_midi_abi_version: () => number;
    readonly clausters_midi_free: (a: number, b: number) => void;
    readonly clausters_midi_write_clip: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly clausters_midi_write_smf: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
