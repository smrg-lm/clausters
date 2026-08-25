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

// Measuring an element stayed a leaf so an embedder can use it
// without booting an engine; re-exported here, where callers expect it.
export { canvasBox, onScaleChange };
export type { CanvasBox } from "./canvasbox.ts";

/**
 * Where a page draws: the document element a view is opened into.
 *
 * The one browser-only argument the API takes, and it is named here — in the
 * module that owns everything the DOM is — rather than spelled `Element` at
 * each door, so a reader can see at a glance which arguments are the page's.
 * The Python client's counterpart verbs take no such thing: a script gets an
 * OS window, and so does a host reached over a socket, which refuses one.
 */
export type Stage = Element;

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
     * The page's **fallback** canvas: what a def fed straight through the
     * binding surface draws on, with nobody having said where. `attach` hands
     * it to a def given no canvas of its own, and it is appended to `<body>`
     * the first time that happens — so a page whose views all name their own
     * place never finds an empty canvas in the document.
     *
     * A view opened through `GuiHost.open` does **not** use it: it gets a
     * canvas of its own, which is where `WindowHandle.canvas` points.
     */
    canvas: HTMLCanvasElement;
    /**
     * Gives a `window`-rooted def a canvas to draw into, before its
     * `/gui_def` is fed. The host holds one canvas per def, so a document can
     * show several at once; omit `canvas` to use the page's fallback one.
     *
     * **Idempotent per def**: a def that already has a canvas keeps it, and a
     * second call is ignored. That is what lets a caller that owns where a
     * window draws — a component, an embedder — attach its own canvas before
     * the def is fed, without a carrier's default policy
     * (`pageGuiConnection`) taking it back.
     */
    attach(defId: number, canvas?: HTMLCanvasElement): void;
    /**
     * Gives up this def's canvas, so a later `attach` may give it another —
     * what closing a window does, and what a component does when it is
     * removed from the page.
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
     * `GuiHost.open` calls this for a view given an element, so a script
     * normally never does:
     *
     * ```js
     * const win = host.open(tree, { element: container });
     * ```
     *
     * Returns the disposer that stops observing (the canvas keeps its last
     * size). `canvas` defaults to the page's fallback one.
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
     */
    engine: ClaustersServer;
    /**
     * Releases this host: its wasm instance, its GPU device, its event drain.
     * The engine is **not** closed — a host is one client of it, and the page
     * or the `Session` that opened the engine is what stops it.
     *
     * Only an instance from `newGuiHost` is anyone's to close; the page's own
     * (`guiHost`) is shared page state. Closing one leaves every other host on
     * the page drawing.
     */
    close(): void;
}

/**
 * The canvas a view mounted into `element` draws on: the element itself when
 * it *is* a canvas, else one this module made inside it before, else a fresh
 * one appended to it.
 *
 * A page names the box it wants a view in — a `<div>` its layout sizes — and
 * the canvas is an implementation detail of drawing into that box, so nobody
 * has to make one. Memoized on the element so re-opening into the same box
 * keeps its GPU surface instead of stacking canvases.
 */
export function canvasIn(element: Element): HTMLCanvasElement {
    if (element instanceof HTMLCanvasElement) return element;
    const held = mounted.get(element);
    if (held !== undefined && held.isConnected) return held;
    const canvas = document.createElement("canvas");
    canvas.style.display = "block";
    canvas.style.width = "100%";
    canvas.style.height = "100%";
    element.append(canvas);
    mounted.set(element, canvas);
    return canvas;
}

/** element → the canvas `canvasIn` made in it. */
const mounted = new WeakMap<Element, HTMLCanvasElement>();

/**
 * A canvas of a view's own, appended to the document — what a view opened with
 * **no element** draws on.
 *
 * *A view with no parent is a window* is the rule the reference client settled;
 * a page has no window, so the sentence finishes here: a view with no element
 * is a canvas. That is what makes several canvases in one document fall out of
 * opening several views, rather than being a feature — the host has kept one
 * surface per `window`-rooted def since W4, and this is the client side finally
 * asking for them.
 */
export function newCanvas(): HTMLCanvasElement {
    const canvas = document.createElement("canvas");
    canvas.width = DEFAULT_CANVAS.width;
    canvas.height = DEFAULT_CANVAS.height;
    canvas.style.display = "block";
    document.body.append(canvas);
    return canvas;
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
 * independently and have no channel to agree on a range over — isolated
 * demos side by side, an editor beside a player.
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
    options: { engine?: ClaustersServer } = {},
): Promise<ClaustersGui> {
    return boot(options.engine ?? await engineInstance());
}

async function boot(audio?: ClaustersServer): Promise<ClaustersGui> {
    const { default: init, start } = await import("../gui-host/clausters_gui.js");
    const engine = audio ?? await server();
    await init();
    const bridge = start();

    // The page makes the canvas and hands it over, rather than waiting for one
    // to be appended and grabbing it: that is the ownership a document has, and
    // the only way several canvases can exist at once. This one is the page's
    // **fallback** — what a def fed straight through the binding surface draws
    // on, with nobody having said where. A view opened through `GuiHost.open`
    // gets a canvas of its own instead (`newCanvas`), and a component supplies
    // one to `attach`.
    //
    // It is appended **when it is first used**, not here: a page whose views
    // all name their own place must not find an empty canvas in <body> that
    // nothing ever draws on. `newGuiHost` never appends at all — that instance
    // belongs to whoever asked for it, and putting a canvas in <body> on their
    // behalf would put it somewhere they did not choose.
    const canvas = document.createElement("canvas");
    canvas.width = DEFAULT_CANVAS.width;
    canvas.height = DEFAULT_CANVAS.height;
    canvas.style.display = "block";
    const useFallback = () => {
        if (audio === undefined && !canvas.isConnected) document.body.append(canvas);
        return canvas;
    };

    // This host's server leg, wired once — and under a client tag of its own.
    // The host is a *second* client of this engine beside the page's script,
    // and the server keeps one `/bus_stream` subscription per client: sharing a
    // tag is what used to make the two take the stream from each other, leaving
    // the host's meters frozen until a widget was added or removed.
    //
    const peer = engine.claimPeer();
    engine.addReply((bytes) => bridge.server_reply(bytes), peer);
    bridge.connect_page((bytes: Uint8Array) => engine.send(bytes, peer));

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
            bridge.attach(defId, element ?? useFallback());
        },
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
            const surface = target ?? useFallback();
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
