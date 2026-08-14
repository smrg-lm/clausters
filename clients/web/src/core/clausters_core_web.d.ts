/* tslint:disable */
/* eslint-disable */

/**
 * The undo history of one document, the JS face of
 * [`clausters_document::Log`].
 */
export class Log {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Apply an edit **and record it**, in one call: the inverse has to be read
     * out of the document before the edit lands, so applying first and
     * recording second would record the wrong thing.
     * `apply(requestJson) -> resultJson`, the request carrying
     * `{ document, intent, against?, quant?, label? }`.
     */
    apply(request: string): string;
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
     * Redo what was last undone, applying what it can. Returns
     * `{ document, remaining }` — the ordinary edits at the front are already
     * applied, and `remaining` holds the steps from the first one the crate
     * cannot perform onward, for the owner to re-run. `undefined` when there
     * was nothing to redo.
     */
    redo(document: string): string | undefined;
    /**
     * Undo the last transaction, applying its inverses to `documentJson`.
     * Returns `{ document, undone }`, or `undefined` when there was nothing to
     * undo.
     */
    undo(document: string): string | undefined;
    /**
     * Whether there is anything to redo.
     */
    readonly canRedo: boolean;
    /**
     * Whether there is anything to undo.
     */
    readonly canUndo: boolean;
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
 * [`clausters_core::peaks::MultiPyramid`] — what a navigable waveform reads
 * so a view costs the width of the window rather than the length of the
 * buffer. Built once from the samples, then queried per frame.
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
     * One column: the `[min, max]` of channel `ch` over `[s0, s1)` at
     * `level`. `undefined` for an unknown channel or an empty level.
     */
    column(ch: number, level: number, s0: number, s1: number): Float32Array | undefined;
    /**
     * A whole pixel row in one crossing: `width` columns spanning
     * `[s0, s1)` of channel `ch`, as interleaved `[min, max]` pairs, read at
     * the level `s1 - s0` and `width` imply. This is the door a view calls
     * every frame — never one column per call, and never a resolution finer
     * than the screen. An empty array for an unknown channel or a
     * degenerate span.
     */
    columns(ch: number, s0: number, s1: number, width: number): Float32Array;
    /**
     * Reads back a serialized cache (`toBytes`, or the file the GUI host maps
     * and the Python client writes). `undefined` when the bytes are not one.
     */
    static fromBytes(data: Uint8Array): Pyramid | undefined;
    /**
     * The bucket size (source samples per entry) of `level`, or `undefined`.
     */
    levelBucket(level: number): number | undefined;
    /**
     * The level whose buckets match `samples_per_px` — the finest one that
     * still aggregates about a bucket per pixel column.
     */
    levelFor(samples_per_px: number): number;
    /**
     * The cache's bytes, in the format every client reads: the mono layout
     * for a single channel and the multichannel one above it — the choice
     * the Python client's door makes, so the same samples serialize to the
     * same bytes whichever client reduced them. Both are read back by
     * `fromBytes` and by the GUI host.
     */
    toBytes(): Uint8Array;
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
 * JS face: apply an edit. `documentApply(requestJson) -> resultJson`, the
 * request carrying `{ document, intent, against?, quant? }` and the result
 * `{ document, outcome }`.
 *
 * One object rather than four arguments because the boundary is JSON either
 * way, and a request that grows a field then costs no signature.
 */
export function document_apply(request: string): string;

/**
 * JS face: resolve a selection to the spans of material underneath it.
 * `documentResolve(requestJson) -> resolvedJson`, the request carrying
 * `{ document, selection, framesPerBeat, inBeats? }`.
 */
export function document_resolve(request: string): string;

/**
 * JS face: the `[audio, control]` bus widths GraphDef instances reserve at
 * the top of each bus space (before clamping to a smaller configured count).
 */
export function graph_bus_reserved(): Uint32Array;

/**
 * JS face: the **Lissajous / goniometer** projection of a stereo pair, as
 * interleaved `[x, y]` pairs (`x` = side, `y` = mid) — one pair per input
 * frame. An empty array when the two channels differ in length.
 */
export function lissajous(left: Float32Array, right: Float32Array): Float32Array;

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
 * JS face: the triggered window's start inside `raw`, as `[start, locked]`
 * (`locked` 1 = the trigger fired, 0 = free-running on the newest window).
 */
export function oscil_align(raw: Float32Array, display: number, level: number): Float64Array;

/**
 * JS face: the display window in samples for `window_ms` at `sample_rate`.
 */
export function oscil_display_frames(window_ms: number, sample_rate: number): number;

/**
 * JS face: how many raw tap samples one display window needs — the window
 * plus the trigger's search slack. What a `/bus_tapStream` subscription asks for.
 */
export function oscil_raw_frames(display: number): number;

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
 * JS face: one spectrum frame — `samples` windowed, transformed and scaled to
 * decibels, `fft_size / 2` bins. `wintype` is the shared window code (`-1`
 * rectangular, `0` Hann — the display default —, `1` sine, `2` Welch, `3`
 * Hamming, `4` Blackman). An empty array when `fft_size` is not a supported
 * power of two. The per-frame half of a spectrum view; the smoothing and
 * peak-hold across frames belong to whoever draws.
 */
export function spectrum_db(samples: Float32Array, fft_size: number, wintype: number): Float32Array;

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

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_log_free: (a: number, b: number) => void;
    readonly __wbg_pyramid_free: (a: number, b: number) => void;
    readonly __wbg_registry_free: (a: number, b: number) => void;
    readonly __wbg_rng_free: (a: number, b: number) => void;
    readonly __wbg_sampleclockmodel_free: (a: number, b: number) => void;
    readonly __wbg_scheduler_free: (a: number, b: number) => void;
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
    readonly document_apply: (a: number, b: number) => [number, number, number, number];
    readonly document_resolve: (a: number, b: number) => [number, number, number, number];
    readonly graph_bus_reserved: () => [number, number];
    readonly lissajous: (a: number, b: number, c: number, d: number) => [number, number];
    readonly log_apply: (a: number, b: number, c: number) => [number, number, number, number];
    readonly log_canRedo: (a: number) => number;
    readonly log_canUndo: (a: number) => number;
    readonly log_clear: (a: number) => void;
    readonly log_len: (a: number) => number;
    readonly log_new: (a: number, b: number) => number;
    readonly log_record: (a: number, b: number, c: number) => [number, number];
    readonly log_redo: (a: number, b: number, c: number) => [number, number, number, number];
    readonly log_redoLabel: (a: number) => [number, number];
    readonly log_undo: (a: number, b: number, c: number) => [number, number, number, number];
    readonly log_undoLabel: (a: number) => [number, number];
    readonly node_id_partition: (a: number) => [number, number, number];
    readonly osc_decode_packet: (a: number, b: number) => [number, number, number];
    readonly osc_decode_packet_timed: (a: number, b: number) => [number, number, number];
    readonly osc_encode_bundle: (a: number, b: any) => [number, number, number, number];
    readonly osc_encode_immediate_bundle: (a: any) => [number, number, number, number];
    readonly osc_encode_message: (a: number, b: number, c: any) => [number, number, number, number];
    readonly osc_encode_score_bundle: (a: number, b: any) => [number, number, number, number];
    readonly oscil_align: (a: number, b: number, c: number, d: number) => [number, number];
    readonly oscil_raw_frames: (a: number) => number;
    readonly pyramid_baseBucket: (a: number) => number;
    readonly pyramid_build: (a: number, b: number, c: number, d: number) => number;
    readonly pyramid_channels: (a: number) => number;
    readonly pyramid_column: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly pyramid_columns: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly pyramid_frames: (a: number) => number;
    readonly pyramid_fromBytes: (a: number, b: number) => number;
    readonly pyramid_levelBucket: (a: number, b: number) => number;
    readonly pyramid_levelFor: (a: number, b: number) => number;
    readonly pyramid_numLevels: (a: number) => number;
    readonly pyramid_toBytes: (a: number) => [number, number];
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
    readonly secs_to_beats: (a: number, b: number, c: number, d: number) => number;
    readonly secs_to_samples: (a: number, b: number) => number;
    readonly spectrum_db: (a: number, b: number, c: number, d: number) => [number, number];
    readonly unary: (a: number, b: number, c: number) => [number, number, number];
    readonly unix_to_ntp: (a: number) => bigint;
    readonly unix_to_sample: (a: number, b: number, c: number, d: number) => number;
    readonly oscil_display_frames: (a: number, b: number) => number;
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
