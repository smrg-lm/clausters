/* tslint:disable */
/* eslint-disable */

/**
 * The live engine in pulled mode: a 1:1 JS face over
 * `clausters::embed::ClaustersHeadless` (which owns all the logic and is
 * exercised by the native `tests/headless.rs` suite). The AudioWorklet
 * processor drives it: OSC packets in over `send`, one `process` per render
 * quantum, replies drained with `poll`.
 */
export class WebServer {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * The engine block size in frames (the granularity `process` needs).
     */
    block_frames(): number;
    /**
     * Begins a **staged** load and returns its ticket: the destination is
     * allocated, no samples are copied.
     *
     * [`buffer_load`](Self::buffer_load) copies the whole take in one call,
     * on this thread — which is the AudioWorklet's, the one that owes the next
     * quantum. Measured natively, a five-minute stereo take is some fourteen
     * times the quantum's budget (`examples/measure_turn.rs`), so a long take
     * is loaded in runs instead: `begin`, `chunk` as often as the caller
     * likes, `end`. Nothing is visible under `index` until `end`.
     */
    bufferLoadBegin(index: number, channels: number, sample_rate: number, frames: number): number;
    /**
     * Discards a staged load without installing it.
     */
    bufferLoadCancel(ticket: number): void;
    /**
     * Copies one run of interleaved samples into a staged load, at flat
     * sample offset `at`. Costs what it copies: the caller picks the run, and
     * therefore the deadline it fits in.
     */
    bufferLoadChunk(ticket: number, at: number, data: Float32Array): void;
    /**
     * Installs a staged load: one pointer swap, the samples being already in.
     */
    bufferLoadEnd(ticket: number): void;
    /**
     * Installs host-decoded samples as buffer `index` (the browser's
     * `/buffer_allocRead` replacement: fetch + `decodeAudioData`, then this).
     */
    buffer_load(index: number, channels: number, sample_rate: number, data: Float32Array): void;
    /**
     * The engine's sample counter (block-accurate; exact in an f64 for the
     * first 2^53 samples — thousands of years of audio).
     */
    clock(): number;
    /**
     * Data-plane control-bus read.
     */
    ctl_get(index: number): number;
    /**
     * Data-plane control-bus write (no command round trip).
     */
    ctl_set(index: number, value: number): void;
    /**
     * Hands the jobs the host does better over to it — reading a soundfile,
     * whose filesystem is the page's (OPFS, reachable only from a Worker) and
     * not the engine's. Call it once, at boot, if the page has a Worker to do
     * them; without it every job runs here, as before.
     */
    delegateJobs(): void;
    /**
     * Every open disk stream and what it wants right now, as JSON: an array of
     * `{id, direction: "in"|"out", path, channels, looping, format, samples}`.
     * `samples` is room to fill for an `in`, and samples waiting for an `out`.
     *
     * This is the whole interface between the graph and whatever is reading
     * files: the host walks it each turn, fills what is hungry with
     * [`disk_push`](Self::disk_push) and empties what is full with
     * [`disk_pull`](Self::disk_pull).
     */
    diskPoll(): string;
    /**
     * Pulls what a `DiskOut` stream has recorded, up to `max` samples.
     */
    diskPull(id: number, max: number): Float32Array;
    /**
     * Pushes interleaved frames into a `DiskIn` stream; returns how many
     * samples were taken. Fewer than offered means the ring filled and the
     * rest is the caller's to offer again.
     */
    diskPush(id: number, samples: Float32Array): number;
    /**
     * Tells a `DiskIn` stream how many channels its file turned out to have.
     *
     * Natively the UGen opens the file and knows on the spot; here reading is
     * asynchronous and belongs to another thread, so a stream is born
     * shapeless, reports `channels: 0` in [`disk_poll`](Self::disk_poll), and
     * plays silence until this arrives. Nothing is declared up front — a
     * declaration would be a call the other client has no counterpart for.
     */
    diskShape(id: number, channels: number): void;
    /**
     * Answers a delegated job: an empty `error` once the host has installed
     * the result through a staged load, otherwise the message the command
     * fails with. Emits the `/done` or `/fail` and unblocks the queue.
     */
    finishDelegated(ticket: number, error?: string | null): void;
    /**
     * Answers one compilation. On success `compute` and `init` are the table
     * slots the module's exports were appended at and `json` is the
     * compiler's own JSON verbatim; that emits `/done`. Pass `error` instead
     * to emit `/fail` with the compiler's message.
     *
     * **The slots are trusted.** They must belong to a module instantiated
     * against *this* engine's memory and table, with the shape its own JSON
     * declared; a wrong one writes into the engine's memory rather than
     * failing. Only the host that linked the module may call this.
     *
     * wasm32 only, for the same reason as
     * [`take_faust_jobs`](Self::take_faust_jobs).
     */
    finishFaust(ticket: number, compute: number, init: number, json?: string | null, error?: string | null): void;
    /**
     * How many frames one [`buffer_load_chunk`](Self::buffer_load_chunk)
     * should carry — the serving budget's number, read from the engine rather
     * than repeated in JavaScript.
     */
    installFrames(): number;
    /**
     * `unix_epoch`: Unix seconds at sample 0 (JS: `Date.now() / 1000`), the
     * anchor that lets wall-clocked clients' bundle timetags land on this
     * server's sample axis.
     */
    constructor(sample_rate: number, channels: number, unix_epoch: number);
    /**
     * One pending reply as `[peer, ...bytes]`, or `undefined`/`None` when none
     * is pending: the first byte group says **who the reply is for**, so the
     * page routes it to that client instead of handing every reply to all of
     * them.
     *
     * One `Vec` rather than a pair because the value crosses to JS: a tuple
     * would be a JS array holding a second typed array, which costs an extra
     * object per reply on the hottest path there is (every streamed bus
     * snapshot). The peer rides as a `u32` little-endian prefix instead, and
     * `readReply` in the loader unpacks it.
     */
    poll(): Uint8Array | undefined;
    /**
     * Renders into `out` (interleaved, a multiple of `block_frames() *
     * channels` samples): a serving turn before each engine block.
     */
    process(out: Float32Array): void;
    /**
     * Whether a `/server_quit` arrived; the page decides what closing means.
     */
    quit_requested(): boolean;
    /**
     * Pushes one complete OSC packet into the command ring, authored by
     * `peer`. `false` = momentarily full (backpressure): retry next quantum.
     *
     * A page holds **several** independent clients over this one engine — the
     * script and the GUI host, at least — and the server has to tell them
     * apart or their `/bus_stream` subscriptions overwrite each other. The tag
     * is the page's to assign; there is no handshake.
     */
    send(peer: number, packet: Uint8Array): boolean;
    /**
     * Sets the ceiling on the bus indices one `/bus_stream` subscription may
     * list — the page's half of the native `--max-stream-buses`, so an
     * in-page engine is configured on the same axis as a server process
     * (default 4096). A page whose document holds hundreds of live canvases
     * subscribes a bus per meter, and the number it may ask for should be its
     * own decision here as it is a server operator's there.
     *
     * What a client actually gets is this clamped by what the ring carries in
     * one reply, and `/server_query.reply` reports that number to it.
     */
    set_max_stream_buses(n: number): void;
    /**
     * The next job for the host, as JSON, or `undefined` if none is waiting:
     * `{ticket, index, kind: "allocRead", path, fileStart, numFrames,
     * channels}` for a read, and
     * `{ticket, index, kind: "write", path, sampleFormat, channels,
     * sampleRate, frames}` for a write. A delegated job blocks the buffer
     * queue behind it, so this hands out at most one at a time.
     *
     * A write's samples are **not** in here: the host pulls them a run at a
     * time with [`write_chunk`](Self::write_chunk).
     */
    takeDelegated(): string | undefined;
    /**
     * The Faust compilations waiting for this page's compiler, as a JSON
     * array (empty when there are none): `[{ticket, name, kind, def}]`, where
     * `kind` is `"source"`, `"boxes"` or `"signals"` — which of the three def
     * formats `def` is in.
     *
     * A page's Faust compiler is not a thread but the host: it compiles with
     * `libfaust-wasm` in its Worker, strips the emitted module's data section,
     * instantiates it against this engine's own memory and
     * `__indirect_function_table` with its math imports bound to this
     * engine's exports, and answers with [`finish_faust`](Self::finish_faust).
     * Until it does, the `/def_send faust` is simply still in flight.
     *
     * wasm32 only, unlike the rest of this shell: the compiler queue exists
     * only where the compiler is the host (see `clausters::faust`).
     */
    takeFaustJobs(): string;
    /**
     * One run of the outstanding write's samples, interleaved: frames
     * `at..at + frames` of the span the job declared, and an empty array once
     * the host has walked past its end.
     *
     * The payload leaves in runs because the thread handing it over owes the
     * next block — the same reason a long *load* arrives in runs. Size the run
     * from [`install_frames`](Self::install_frames).
     */
    writeChunk(at: number, frames: number): Float32Array;
}

/**
 * The embed / IPC ABI version this build speaks (`clausters_abi_version`).
 */
export function abi_version(): number;

/**
 * The Faust defs a score sends, as a JSON array of
 * `{"name", "kind", "def"}` — the same three fields a live compile job
 * carries, so the host compiles them with the code it already has.
 *
 * The offline renderer cannot wait: it loads a def where it stands and time
 * does not advance until it has. A page's compiler is another scope and
 * answers later, so the page asks *this* before it renders, compiles and
 * links each def with [`link_faust`], and only then calls [`render`].
 *
 * The score is read here rather than in TypeScript on purpose: it is the same
 * reader the render itself uses, so a score the render understands and the
 * pre-pass does not cannot happen.
 */
export function faustJobs(score: Uint8Array): string;

/**
 * The seed the last [`render`] on this thread used — how a caller gets back
 * to a take it liked. Separate from `render`'s return because the JS face
 * returns a bare `Float32Array`; a stats object is the shape to grow into if
 * the web client ever needs the frame, event and level counts too.
 */
export function last_render_seed(): bigint;

/**
 * Adopts a def the host compiled and linked for a render still to come, under
 * the name the score sends it with. The next [`render`] whose score sends
 * that name finds it here.
 *
 * **The slots are trusted**, exactly as
 * [`WebServer::finish_faust`](WebServer::finish_faust)'s are: they must belong
 * to a module instantiated against *this* module's memory and table, with the
 * shape its own JSON declared.
 */
export function linkFaust(name: string, compute: number, init: number, json: string): void;

/**
 * JS face: `render(scoreBytes, sampleRate, channels, seed?) -> Float32Array`,
 * throwing a `JsError` with the render's message on failure. The seed the
 * render used is read back with [`last_render_seed`].
 */
export function render(score: Uint8Array, sample_rate: number, channels: number, seed?: bigint | null): Float32Array;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly clausters_abi_version: () => number;
    readonly __wbg_webserver_free: (a: number, b: number) => void;
    readonly __indirect_function_table: WebAssembly.Table;
    readonly faustJobs: (a: number, b: number) => [number, number, number, number];
    readonly linkFaust: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly render: (a: number, b: number, c: number, d: number, e: number, f: bigint) => [number, number, number, number];
    readonly webserver_block_frames: (a: number) => number;
    readonly webserver_bufferLoadBegin: (a: number, b: number, c: number, d: number, e: number) => [number, number, number];
    readonly webserver_bufferLoadCancel: (a: number, b: number) => void;
    readonly webserver_bufferLoadChunk: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly webserver_bufferLoadEnd: (a: number, b: number) => [number, number];
    readonly webserver_buffer_load: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly webserver_clock: (a: number) => number;
    readonly webserver_ctl_get: (a: number, b: number) => number;
    readonly webserver_ctl_set: (a: number, b: number, c: number) => void;
    readonly webserver_delegateJobs: (a: number) => void;
    readonly webserver_diskPoll: (a: number) => [number, number];
    readonly webserver_diskPull: (a: number, b: number, c: number) => [number, number];
    readonly webserver_diskPush: (a: number, b: number, c: number, d: number) => number;
    readonly webserver_diskShape: (a: number, b: number, c: number) => void;
    readonly webserver_finishDelegated: (a: number, b: number, c: number, d: number) => void;
    readonly webserver_finishFaust: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
    readonly webserver_installFrames: (a: number) => number;
    readonly webserver_new: (a: number, b: number, c: number) => [number, number, number];
    readonly webserver_poll: (a: number) => [number, number];
    readonly webserver_process: (a: number, b: number, c: number, d: any) => [number, number];
    readonly webserver_quit_requested: (a: number) => number;
    readonly webserver_send: (a: number, b: number, c: number, d: number) => number;
    readonly webserver_set_max_stream_buses: (a: number, b: number) => void;
    readonly webserver_takeDelegated: (a: number) => [number, number];
    readonly webserver_takeFaustJobs: (a: number) => [number, number];
    readonly webserver_writeChunk: (a: number, b: number, c: number) => [number, number];
    readonly last_render_seed: () => bigint;
    readonly abi_version: () => number;
    readonly _abs: (a: number) => number;
    readonly _acos: (a: number) => number;
    readonly _acosf: (a: number) => number;
    readonly _acosh: (a: number) => number;
    readonly _acoshf: (a: number) => number;
    readonly _asin: (a: number) => number;
    readonly _asinf: (a: number) => number;
    readonly _asinh: (a: number) => number;
    readonly _asinhf: (a: number) => number;
    readonly _atan: (a: number) => number;
    readonly _atan2: (a: number, b: number) => number;
    readonly _atan2f: (a: number, b: number) => number;
    readonly _atanf: (a: number) => number;
    readonly _atanh: (a: number) => number;
    readonly _atanhf: (a: number) => number;
    readonly _cos: (a: number) => number;
    readonly _cosf: (a: number) => number;
    readonly _cosh: (a: number) => number;
    readonly _coshf: (a: number) => number;
    readonly _exp: (a: number) => number;
    readonly _expf: (a: number) => number;
    readonly _fmod: (a: number, b: number) => number;
    readonly _fmodf: (a: number, b: number) => number;
    readonly _log: (a: number) => number;
    readonly _log10: (a: number) => number;
    readonly _log10f: (a: number) => number;
    readonly _logf: (a: number) => number;
    readonly _pow: (a: number, b: number) => number;
    readonly _powf: (a: number, b: number) => number;
    readonly _remainder: (a: number, b: number) => number;
    readonly _remainderf: (a: number, b: number) => number;
    readonly _round: (a: number) => number;
    readonly _roundf: (a: number) => number;
    readonly _sin: (a: number) => number;
    readonly _sinf: (a: number) => number;
    readonly _sinh: (a: number) => number;
    readonly _sinhf: (a: number) => number;
    readonly _tan: (a: number) => number;
    readonly _tanf: (a: number) => number;
    readonly _tanh: (a: number) => number;
    readonly _tanhf: (a: number) => number;
    readonly clausters_free_samples: (a: number, b: bigint) => void;
    readonly clausters_read_soundfile: (a: number, b: bigint, c: bigint, d: number, e: number, f: number, g: number, h: number) => number;
    readonly clausters_render: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
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
