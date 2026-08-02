/* tslint:disable */
/* eslint-disable */

/**
 * The binding surface JS holds: feed OSC packets / GuiDefs in, drain events out,
 * and connect the audio-server WebSocket. It reaches the running app through the
 * event-loop proxy and shares the outbox queue.
 */
export class GuiBridge {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Gives one `window`-rooted def its own `<canvas>`, which the caller
     * created and the document places.
     *
     * This is the browser's answer to the desktop's window manager: on the
     * desktop `clausters-gui` opens a window per def and the system places it;
     * in a tab the canvas is an element and **the document places it** — CSS,
     * the order of the markup. Attach before feeding the def's `/gui_def`, so
     * the first frame draws into the right surface. Attaching a def that
     * already has a canvas replaces it.
     *
     * A page that never calls this still works: a `/gui_def` with no canvas
     * gets one appended to `<body>`, the older single-canvas posture.
     */
    attach(def_id: number, canvas: HTMLCanvasElement): void;
    /**
     * Attaches the host's audio-server leg to the **in-page engine**: every
     * outbound OSC packet (bound-widget values, `/bus_stream`/`/bus_tapStream`
     * subscriptions, buffer fetches, `/clock_query`) is handed to `send` as a
     * `Uint8Array`; the page forwards it to the engine and feeds the engine's
     * replies back through [`server_reply`](Self::server_reply).
     */
    connect_page(send: Function): void;
    /**
     * Attaches the host's audio-server leg to a `--ws` server `url`, so a bound
     * widget forwards straight to it (the bypass path, in the browser).
     */
    connect_server(url: string): void;
    /**
     * Convenience: build and feed a `/gui_def <id> <json>` from a GuiDef JSON
     * string — the same JSON the Python builders emit, so a page needs no OSC
     * encoder of its own.
     */
    def(id: number, json: string): void;
    /**
     * Frees a def's canvas: its GPU surface and every derived resource go. The
     * `<canvas>` element itself is the page's, to remove or reuse.
     */
    detach(def_id: number): void;
    /**
     * Feeds one raw OSC packet (e.g. a `/gui_def`/`/gui_set`/`/gui_bind`) to the
     * host, exactly as the WS wire format delivers it (one packet per call).
     */
    feed(packet: Uint8Array): void;
    /**
     * Overlays the host's size metrics from a JSON object of
     * `{"role": number}` entries — the browser form of the native
     * `[gui.metrics]` config table, the reserved `scale` density key included.
     * A partial object is fine; unknown roles or unusable numbers are logged
     * and skipped.
     */
    metrics(json: string): void;
    /**
     * Pops the next outbound OSC packet (`/gui_event`/`/gui_closed`/`/gui_info`)
     * for the page to decode, or `undefined` when the queue is empty.
     */
    poll(): Uint8Array | undefined;
    /**
     * Sizes a canvas in **device pixels**, with the **scale** those pixels were
     * measured at — a component's `ResizeObserver` box times
     * `devicePixelRatio`, and that ratio. The host never reads the DOM: the
     * element owns its box and reports the pixels.
     *
     * Both halves are needed and neither substitutes for the other. The
     * backing store is device pixels, so the surface takes the product; the
     * widget sizes a GuiDef declares are **logical**, so resolving them takes
     * the ratio — and a product cannot be un-multiplied. A page that already
     * scales its box by `devicePixelRatio` passes the same ratio here.
     */
    resize(def_id: number, width: number, height: number, scale: number): void;
    /**
     * Feeds one reply packet from the in-page engine (a streamed `/bus_stream.reply`, a
     * `/bus_tapStream.reply`, a `/buffer_query.reply`/`/buffer_getRange.reply`, a `/clock_query.reply`) into the host —
     * the inbound half of [`connect_page`](Self::connect_page), the same
     * dispatch the WS leg's `onmessage` uses.
     */
    server_reply(packet: Uint8Array): void;
    /**
     * Tells the host whether a canvas is in the viewport (a component's
     * `IntersectionObserver`).
     *
     * A hidden canvas is skipped on the tick and its buses leave the
     * `/bus_stream`/`/bus_tapStream` sets — a document can hold fifty canvases with
     * three in view, and neither this host nor the server should be working
     * for the other forty-seven.
     */
    set_visible(def_id: number, visible: boolean): void;
    /**
     * Overlays the host's color theme from a JSON object of
     * `{"role": "#rrggbb[aa]"}` entries — the browser form of the native
     * `[gui.theme]` config table. A partial object is fine; unknown roles or
     * bad colors are logged and skipped.
     */
    theme(json: string): void;
}

/**
 * The ordered boot packets of a persisted bundle, for the page to send to the
 * in-page engine: `synthdefs`/`graphdefs` are arrays of `Uint8Array` (each
 * file's bytes verbatim), `boot_json` the optional `boot.json` text,
 * `guidef_tree` the GuiDef tree JSON (its root `boot` messages run last).
 * Returns an array of `Uint8Array` packets ending in `/server_sync sync_id+1` — the
 * page knows the bundle is up when `/server_sync.reply sync_id+1` comes back. The
 * ordering/encoding logic lives in the platform-agnostic `host::bundle`
 * module, natively unit-tested.
 */
export function bundle_boot_packets(synthdefs: Array<any>, graphdefs: Array<any>, boot_json: string | null | undefined, guidef_tree: string, sync_id: number): Array<any>;

/**
 * The wasm entry point: build the event loop, spawn the app on the browser's
 * animation-frame loop (returns immediately, nothing blocks the main thread),
 * and hand the page a [`GuiBridge`] to drive it.
 */
export function start(): GuiBridge;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_guibridge_free: (a: number, b: number) => void;
    readonly bundle_boot_packets: (a: any, b: any, c: number, d: number, e: number, f: number, g: number) => any;
    readonly guibridge_attach: (a: number, b: number, c: any) => void;
    readonly guibridge_connect_page: (a: number, b: any) => void;
    readonly guibridge_connect_server: (a: number, b: number, c: number) => void;
    readonly guibridge_def: (a: number, b: number, c: number, d: number) => void;
    readonly guibridge_detach: (a: number, b: number) => void;
    readonly guibridge_feed: (a: number, b: number, c: number) => void;
    readonly guibridge_metrics: (a: number, b: number, c: number) => void;
    readonly guibridge_poll: (a: number) => [number, number];
    readonly guibridge_resize: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly guibridge_server_reply: (a: number, b: number, c: number) => void;
    readonly guibridge_set_visible: (a: number, b: number, c: number) => void;
    readonly guibridge_theme: (a: number, b: number, c: number) => void;
    readonly start: () => number;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue__core_9b3796e30d99ddb7___result__Result_____wasm_bindgen_ac0f9960a408a037___JsError___true_: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___js_sys_680697122f8f483d___Array__web_sys_80b4f796a3c8b06___features__gen_ResizeObserver__ResizeObserver______true_: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1_: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true_: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__4: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__5: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__6: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__7: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___web_sys_80b4f796a3c8b06___features__gen_MessageEvent__MessageEvent______true_: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__9: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__10: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke___wasm_bindgen_ac0f9960a408a037___JsValue______true__1__11: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_ac0f9960a408a037___convert__closures_____invoke_______true_: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
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
