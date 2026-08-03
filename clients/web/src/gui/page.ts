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
     */
    attach(defId: number, canvas?: HTMLCanvasElement): void;
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
     * The engine this host's audio leg is wired to — the page's under
     * `guiHost`, its own under `newGuiHost`. Exposed because a caller holding
     * an instance needs exactly this to open a `Server` on it, and asking for
     * it again by name would hand back the page's.
     */
    engine: ClaustersServer;
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

    // This host's server leg, wired once.
    engine.addReply((bytes) => bridge.server_reply(bytes));
    bridge.connect_page((bytes: Uint8Array) => engine.send(bytes));

    // Drain the host's outbound events to the page's listeners.
    const listeners = new Set<EventListener>();
    setInterval(() => {
        let packet: Uint8Array | undefined;
        while ((packet = bridge.poll()) !== undefined) {
            for (const listener of [...listeners]) listener(packet);
        }
    }, 33);

    return {
        bridge,
        canvas,
        engine,
        attach: (defId, element) => bridge.attach(defId, element ?? canvas),
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
    const attached = new Set<number>();
    return {
        send: (packet) => {
            for (const { addr, args } of decodePacket(packet)) {
                if (addr !== "/gui_def" || typeof args[0] !== "number") continue;
                if (!attached.has(args[0])) {
                    attached.add(args[0]);
                    gui.attach(args[0]);
                }
            }
            gui.bridge.feed(packet);
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
