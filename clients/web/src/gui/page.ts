// The page's GUI host: the singleton, its canvases, and the carrier over it.
//
// This is the wasm `clausters-gui` running on this page — the host itself, not
// a client of one. It is deliberately kept apart from `./host.ts` (the
// transport-agnostic `GuiHost` client object, which also drives a native
// `--ws` host and pulls the GuiDef builders in with it), because the component
// run time needs *only* this half: a page that mounts a bundle loads the
// engine, the host and the mount, and none of the authoring machinery. See
// `../runtime.ts`, and the module-graph test that holds the line.
//
// One host serves the page and draws **one canvas per `window`-rooted def**,
// so a document can show several at once — the desktop's window manager, with
// CSS in its place.

// The host's wasm glue is loaded **on demand** (inside `boot`), not at import
// time.
import type { GuiBridge } from "../gui-host/clausters_gui.js";
import { engine as engineInstance, server } from "../engine/server.ts";
import type { ClaustersServer } from "../engine/server.ts";
import { decodePacket } from "../base/osc.ts";
import type { Connection } from "../base/connection.ts";
import { canvasBox, onScaleChange } from "./canvasbox.ts";

// Measuring an element stayed a leaf so the notebook widget can use it
// without booting an engine; re-exported here, where callers expect it.
export { canvasBox, onScaleChange };
export type { CanvasBox } from "./canvasbox.ts";

export type EventListener = (packet: Uint8Array) => void;

/**
 * The page canvas' default size in device pixels, matching the host's own
 * default. A component sizes its canvas from its element box instead.
 */
const DEFAULT_CANVAS = { width: 480, height: 420 };

/**
 * The shared host surface: the raw binding bridge, the page-wide canvas
 * (re-parent it freely; the GPU context survives), and the outbound
 * `/gui_event`/`/gui_info`/`/gui_closed` stream as byte packets.
 */
export interface ClaustersGui {
    bridge: GuiBridge;
    /**
     * The page's default canvas — the one a page that does not make its own
     * draws into. `attach` hands it to a def.
     */
    canvas: HTMLCanvasElement;
    /**
     * Gives a `window`-rooted def a canvas to draw into, before its
     * `/gui_def` is fed. The host holds one canvas per def, so a document can
     * show several at once; omit `canvas` to use the page's default one.
     *
     * **Idempotent per def**: a def that already has a canvas keeps it, and a
     * second call is ignored. That is what lets a caller that owns where a
     * window draws — a notebook cell, a component — attach its own canvas
     * before the def is fed, without a carrier's default policy
     * (`pageGuiConnection`) taking it back.
     */
    attach(defId: number, canvas?: HTMLCanvasElement): void;
    /** Whether this def already has a canvas (`attach`'s memory). */
    attached(defId: number): boolean;
    /**
     * Gives up this def's canvas, so a later `attach` may give it another —
     * what closing a window does, and what a cell does when its output is
     * cleared.
     */
    detach(defId: number): void;
    /**
     * Binds a def's canvas to an element's box, so the drawing is as wide as
     * the **document** makes it — full width on a phone, whatever the layout
     * gives it on a desktop — instead of the host's fixed default.
     *
     * The canvas' backing store follows the element in device pixels (the
     * host never reads the DOM, so the page reports the size) and the host is
     * told on every change — of the element's box *and* of the display's scale,
     * which move independently. This is the same rule a `<clausters-bundle>`
     * component follows; a script that opens its own window calls it once,
     * after `open`, and gets the same behaviour:
     *
     * ```js
     * const win = host.open(tree);
     * const stop = (await guiHost()).fit(win.id, container);
     * ```
     *
     * Returns the disposer that stops observing (the canvas keeps its last
     * size). `canvas` defaults to the page's shared one.
     */
    fit(
        defId: number,
        element: Element,
        canvas?: HTMLCanvasElement,
    ): () => void;
    addEvent(listener: EventListener): void;
    removeEvent(listener: EventListener): void;
    /**
     * Hands one packet to this host's page listeners, as if the host had
     * emitted it.
     *
     * The event stream is a fan-out, and its source is not always this wasm
     * host: a page whose windows live in a **native** host — one reached over a
     * `--ws` socket — receives that host's `/gui_event`/`/gui_closed` on the
     * socket, and the elements on the page are listening here. This is where
     * those packets join, so an element hears a window closing wherever the
     * window was.
     */
    deliver(packet: Uint8Array): void;
    /**
     * The engine this host's audio leg is wired to — the page's under
     * `guiHost`, its own under `newGuiHost`. Exposed because a caller holding
     * an instance needs exactly this to open a `Server` on it, and asking for
     * it again by name would hand back the page's.
     *
     * `null` for a host built with `engine: null`: one whose audio leg is not
     * an in-page engine at all but a **native** server over its `--ws` port
     * (`bridge.connect_server(url)`), which is what a notebook cell drawing a
     * server that runs on the kernel's machine holds.
     */
    engine: ClaustersServer | null;
    /**
     * Releases this host: its wasm instance, its GPU device, its event drain.
     * The engine is **not** closed — a host is one client of it, and the page
     * or the `Session` that opened the engine is what stops it.
     *
     * Only an instance from `newGuiHost` is anyone's to close; the page's own
     * (`guiHost`) is shared page state. Closing one leaves every other host on
     * the page drawing, which is what makes several notebooks in one tab
     * possible.
     */
    close(): void;
}

let instance: Promise<ClaustersGui> | null = null;

/**
 * The page's GUI host, booting it (and the page's engine) on first call.
 *
 * **The page's, not the page's only one.** This is the host a document wants by
 * default — its components belong to one mix, so they meet in one node, bus,
 * buffer and widget namespace — and it comes with the page's default canvas,
 * appended to `<body>` where a page that makes none of its own finds it. Later
 * calls get the same instance.
 *
 * A caller that wants an *independent* client uses `newGuiHost`, which is to
 * this what `engine` is to `server`.
 */
export function guiHost(): Promise<ClaustersGui> {
    instance ??= boot();
    return instance;
}

/**
 * A GUI host of one's own, sharing the page with whatever else is on it.
 *
 * The instance counterpart of `guiHost`, as `engine` is of `server`. What a
 * page holds one of is the windowing event loop, not the host: the loop drives
 * any number of windows, so instances share it and nothing else. Two of them
 * may hold the very same window and widget ids without seeing each other,
 * which is the only arrangement that works for clients that allocate ids
 * independently and have no channel to agree on a range over — notebooks open
 * in one JupyterLab tab, isolated demos side by side.
 *
 * Two differences from `guiHost`, both because this host is not the page's:
 * it appends no canvas (the caller owns where its windows draw, and passes one
 * to `attach`), and it takes the engine to wire its audio leg to — its own by
 * default, since an independent client wants an independent node space.
 *
 * A second host costs neither a download nor a GPU device; a second engine is
 * a second `AudioContext`, and browsers cap those (Chrome at six). Release one
 * with `bridge.close()`.
 */
export async function newGuiHost(
    options: { engine?: ClaustersServer | null; wasm?: BufferSource } = {},
): Promise<ClaustersGui> {
    // `undefined` means "one of my own"; `null` means "none, I will wire the
    // audio leg myself" — the two are not the same wish, and only the explicit
    // null keeps this from starting an AudioContext the caller does not want.
    const audio = options.engine === null
        ? null
        : options.engine ?? await engineInstance();
    return boot(audio, options.wasm);
}

async function boot(
    audio?: ClaustersServer | null,
    wasm?: BufferSource,
): Promise<ClaustersGui> {
    const { default: init, start } = await import("../gui-host/clausters_gui.js");
    const engine = audio === null ? null : audio ?? await server();
    // With no bytes, wasm-bindgen locates the `.wasm` next to its glue. That
    // is right for a served page and impossible for a caller whose modules
    // came over a wire and live at blob URLs, where "next to" names nothing —
    // so those pass the bytes they already have.
    await init(wasm === undefined ? undefined : { module_or_path: wasm });
    const bridge = start();

    // The page makes the canvas and hands it over, rather than waiting for one
    // to be appended and grabbing it: that is the ownership a document has, and
    // the only way several canvases can exist at once. This one is the page's
    // default, in <body> where the older single-canvas pages expect it; a
    // component supplies its own to `attach`.
    //
    // Only the page's host gets one. An instance from `newGuiHost` belongs to
    // whoever asked for it, and appending a canvas to <body> on their behalf
    // would put it somewhere they did not choose.
    const canvas = document.createElement("canvas");
    canvas.width = DEFAULT_CANVAS.width;
    canvas.height = DEFAULT_CANVAS.height;
    canvas.style.display = "block";
    if (audio === undefined) document.body.append(canvas);

    // This host's server leg, wired once — and under a client tag of its own.
    // The host is a *second* client of this engine beside the page's script,
    // and the server keeps one `/bus_stream` subscription per client: sharing a
    // tag is what used to make the two take the stream from each other, leaving
    // the host's meters frozen until a widget was added or removed.
    //
    // A host with no engine has no leg to wire here: its caller connects it to
    // a native server instead, which is the one other thing that end can be.
    if (engine !== null) {
        const peer = engine.claimPeer();
        engine.addReply((bytes) => bridge.server_reply(bytes), peer);
        bridge.connect_page((bytes: Uint8Array) => engine.send(bytes, peer));
    }

    // Drain the host's outbound events to the page's listeners.
    const listeners = new Set<EventListener>();
    const deliver = (packet: Uint8Array) => {
        for (const listener of [...listeners]) listener(packet);
    };
    const drain = setInterval(() => {
        let packet: Uint8Array | undefined;
        while ((packet = bridge.poll()) !== undefined) deliver(packet);
    }, 33);

    // Which defs already have a canvas. It lives here rather than in whoever
    // calls `attach`, because the question is about the *host* — two carriers
    // over one host each keeping their own answer is how a def gets attached
    // twice, the second canvas taking the window off the first.
    const canvases = new Set<number>();

    return {
        bridge,
        canvas,
        engine,
        attach: (defId, element) => {
            if (canvases.has(defId)) return;
            canvases.add(defId);
            bridge.attach(defId, element ?? canvas);
        },
        attached: (defId) => canvases.has(defId),
        detach: (defId) => {
            if (!canvases.delete(defId)) return;
            bridge.detach(defId);
        },
        close: () => {
            clearInterval(drain);
            listeners.clear();
            canvases.clear();
            bridge.close();
        },
        fit: (defId, element, target) => {
            const surface = target ?? canvas;
            const apply = () => {
                const { width, height, scale } = canvasBox(element);
                surface.width = width;
                surface.height = height;
                bridge.resize(defId, width, height, scale);
            };
            apply();
            const observer = new ResizeObserver(apply);
            observer.observe(element);
            // Two triggers, because the box and the density move
            // independently: a layout change, and a change of display scale
            // that leaves the CSS box untouched.
            const unwatch = onScaleChange(apply);
            return () => {
                observer.disconnect();
                unwatch();
            };
        },
        addEvent: (listener) => listeners.add(listener),
        removeEvent: (listener) => listeners.delete(listener),
        deliver,
    };
}

/**
 * The in-page carrier: a `Connection` over the page's GUI-host singleton —
 * `feed` carries a packet in, the drained outbox carries the events back.
 * Closing detaches this connection's listeners; the host keeps running (it is
 * shared page state, not this connection's to stop).
 *
 * A `/gui_def` sent over this carrier gets a canvas attached to it first,
 * unless the caller already gave that def one. A `GuiHost` is
 * transport-agnostic — the same object drives a native `--ws` host, which has
 * windows rather than canvases — so the canvas policy belongs here, on the
 * carrier that *is* the page.
 *
 * Defaults to the page's host, which is what a page wants. Pass one built by
 * `newGuiHost` to carry a client over a host of its own — a `Session` that
 * holds its own engine wires its GUI leg to a host wired to that engine, so
 * a bound widget reaches its session's server and not the page's.
 */
export async function pageGuiConnection(
    target?: Promise<ClaustersGui> | ClaustersGui,
): Promise<Connection> {
    const gui = await (target ?? guiHost());
    const mine = new Set<EventListener>();
    return {
        send: (packet) => {
            const freed: number[] = [];
            for (const { addr, args } of decodePacket(packet)) {
                if (typeof args[0] !== "number") continue;
                // Idempotent on the host, so a def whose canvas the caller
                // already chose keeps it: this is the *default* policy, not an
                // override of one.
                if (addr === "/gui_def") gui.attach(args[0]);
                else if (addr === "/gui_free") freed.push(args[0]);
            }
            gui.bridge.feed(packet);
            // After the feed, not before: the host frees the window on the
            // packet, and taking its surface away first would be pulling the
            // canvas out from under the thing still being freed. A surface
            // left attached to a freed def holds its last frame — a picture of
            // a window that no longer exists.
            for (const id of freed) gui.detach(id);
        },
        addReply: (listener) => {
            mine.add(listener);
            gui.addEvent(listener);
        },
        removeReply: (listener) => {
            mine.delete(listener);
            gui.removeEvent(listener);
        },
        close: () => {
            for (const listener of mine) gui.removeEvent(listener);
            mine.clear();
        },
    };
}
