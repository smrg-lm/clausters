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
}

/**
 * The embed / IPC ABI version this build speaks (`clausters_abi_version`).
 */
export function abi_version(): number;

/**
 * The seed the last [`render`] on this thread used — how a caller gets back
 * to a take it liked. Separate from `render`'s return because the JS face
 * returns a bare `Float32Array`; a stats object is the shape to grow into if
 * the web client ever needs the frame, event and level counts too.
 */
export function last_render_seed(): bigint;

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
    readonly webserver_installFrames: (a: number) => number;
    readonly webserver_new: (a: number, b: number, c: number) => [number, number, number];
    readonly webserver_poll: (a: number) => [number, number];
    readonly webserver_process: (a: number, b: number, c: number, d: any) => [number, number];
    readonly webserver_quit_requested: (a: number) => number;
    readonly webserver_send: (a: number, b: number, c: number, d: number) => number;
    readonly webserver_set_max_stream_buses: (a: number, b: number) => void;
    readonly last_render_seed: () => bigint;
    readonly abi_version: () => number;
    readonly clausters_free_samples: (a: number, b: bigint) => void;
    readonly clausters_read_soundfile: (a: number, b: bigint, c: bigint, d: number, e: number, f: number, g: number, h: number) => number;
    readonly clausters_render: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
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
