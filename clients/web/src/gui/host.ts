// The per-page GUI-host singleton, wired to the engine singleton.
//
// One wasm GUI host serves the page (the host shows one window-rooted def at
// a time on one canvas — the browser front's shape). The first `guiHost()`
// call initializes the wasm module, starts the host, captures the canvas
// winit appends to <body> (so an element can adopt it into its shadow DOM),
// and wires the two singletons together **once**: engine replies →
// `bridge.server_reply`, host outbound → `engine.send` (the in-page
// `ServerLink::Page` leg). Later calls get the same instance, so several
// components share one host and one engine — the shared node/bus/buffer
// namespace.

import init, { start, GuiBridge } from "../gui-host/clausters_gui.js";
import { server } from "../engine/server.ts";

export type EventListener = (packet: Uint8Array) => void;

/// The shared host surface: the raw binding bridge, the page-wide canvas
/// (re-parent it freely; the GPU context survives), and the outbound
/// `/gui_event`/`/gui_info`/`/gui_closed` stream as byte packets.
export interface ClaustersGui {
    bridge: GuiBridge;
    canvas: HTMLCanvasElement;
    addEvent(listener: EventListener): void;
    removeEvent(listener: EventListener): void;
}

let instance: Promise<ClaustersGui> | null = null;

/// The page's GUI host, booting it (and the engine) on first call.
export function guiHost(): Promise<ClaustersGui> {
    instance ??= boot();
    return instance;
}

async function boot(): Promise<ClaustersGui> {
    const engine = await server();
    const before = new Set(document.querySelectorAll("body > canvas"));
    await init();
    const bridge = start();

    // winit appends the host's canvas to <body> asynchronously (on its first
    // animation frame); wait for it so callers can re-parent it.
    const canvas = await new Promise<HTMLCanvasElement>((resolve, reject) => {
        const t0 = performance.now();
        const look = () => {
            for (const c of document.querySelectorAll("body > canvas")) {
                if (!before.has(c)) return resolve(c as HTMLCanvasElement);
            }
            if (performance.now() - t0 > 5000) {
                return reject(new Error("the GUI host's canvas never appeared"));
            }
            requestAnimationFrame(look);
        };
        look();
    });

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
        addEvent: (listener) => listeners.add(listener),
        removeEvent: (listener) => listeners.delete(listener),
    };
}
