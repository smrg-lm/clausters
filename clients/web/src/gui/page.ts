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
import { server } from "../engine/server.ts";
import { decodePacket } from "../base/osc.ts";
import type { Connection } from "../base/connection.ts";

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
     * told on every change. This is the same rule a `<clausters-bundle>`
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
}

/**
 * One element box measured for a canvas: its size in **device pixels** (floored
 * at 1, so a hidden element never asks for a zero-sized surface) and the
 * `devicePixelRatio` those pixels were measured at, kept **separately**.
 *
 * The two are not interchangeable. A canvas' backing store is device pixels, so
 * the surface takes `width`/`height`; the sizes a GuiDef declares are logical,
 * so resolving them takes `scale` — and the product alone cannot be
 * un-multiplied. Reporting both is what lets the host draw a 28-pixel strip as
 * 28 *apparent* pixels on any display, while never reading the DOM itself.
 */
export interface CanvasBox {
    /** The backing-store width, in device pixels. */
    width: number;
    /** The backing-store height, in device pixels. */
    height: number;
    /** The device-pixel ratio the box was measured at (the host's UI scale). */
    scale: number;
}

/** Measures `element` for a canvas (see {@link CanvasBox}). */
export function canvasBox(element: Element): CanvasBox {
    const scale = globalThis.devicePixelRatio || 1;
    const box = element.getBoundingClientRect();
    return {
        width: Math.max(1, Math.round(box.width * scale)),
        height: Math.max(1, Math.round(box.height * scale)),
        scale,
    };
}

let instance: Promise<ClaustersGui> | null = null;

/**
 * The page's GUI host, booting it (and the engine) on first call.
 *
 * One wasm GUI host serves the page, drawing one canvas per `window`-rooted
 * def. The first call initializes the wasm module, starts the host, makes the
 * page's default canvas (appended to `<body>`, where a page that makes none of
 * its own finds it), and wires the two singletons together **once**: engine
 * replies → `bridge.server_reply`, host outbound → `engine.send` (the in-page
 * server leg). Later calls get the same instance, so several components share
 * one host and one engine — the shared node/bus/buffer namespace.
 */
export function guiHost(): Promise<ClaustersGui> {
    instance ??= boot();
    return instance;
}

async function boot(): Promise<ClaustersGui> {
    const { default: init, start } = await import("../gui-host/clausters_gui.js");
    const engine = await server();
    await init();
    const bridge = start();

    // The page makes the canvas and hands it over, rather than waiting for one
    // to be appended and grabbing it: that is the ownership a document has, and
    // the only way several canvases can exist at once. This one is the page's
    // default, in <body> where the older single-canvas pages expect it; a
    // component supplies its own to `attach`.
    const canvas = document.createElement("canvas");
    canvas.width = DEFAULT_CANVAS.width;
    canvas.height = DEFAULT_CANVAS.height;
    canvas.style.display = "block";
    document.body.append(canvas);

    // The in-page server leg, wired once for the whole page.
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
            return () => observer.disconnect();
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
 * A `/gui_def` sent over this carrier gets the page's default canvas attached
 * to it first, unless the caller already gave that def one. A `GuiHost` is
 * transport-agnostic — the same object drives a native `--ws` host, which has
 * windows rather than canvases — so the canvas policy belongs here, on the
 * carrier that *is* the page.
 */
export async function pageGuiConnection(): Promise<Connection> {
    const gui = await guiHost();
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
