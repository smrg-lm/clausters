// The GUI host: the per-page singleton, the carrier over it, and `GuiHost` —
// the client object that drives a host (mirrors `clausters/gui/host.py`).
//
// The GUI host is a *sibling OSC front* of the audio server: the same
// encoding over the same carriers, only the vocabulary is `/gui_*`. So
// `GuiHost` sits on the very same `Connection` seam the audio `Server` does
// and never names a transport itself. Two carriers reach a host from a page:
//
// - `pageGuiConnection()` — the **in-page host**: the `clausters-gui` wasm
//   running on this page's canvas, reached through the `guiHost()` singleton's
//   binding bridge (`feed` in, `poll` out). No process, no socket.
// - `WsConnection.open("ws://host:57220")` — a **native** `clausters-gui --ws`
//   host, the browser's path into a desktop window.
//
// Keep the split the Python client keeps: building the GuiDef tree (see
// `./guidef.ts`) is host-agnostic; only this object talks to a host.
//
// **Nothing pumps.** Where the Python client drains the host's messages from
// the script's loop, the browser pushes: this object subscribes to the
// connection's reply stream once and routes `/gui_event`/`/gui_closed` to the
// handle callbacks as they arrive, while `query` resolves a promise. The
// "never block the clock" discipline of the reference client is here simply
// the language.

// The host's wasm glue is loaded **on demand** (inside `boot`), not at import
// time: `GuiHost` itself is plain OSC over a connection, so driving a native
// `--ws` host — from a page or from node — must not pull the browser bundle in.
import type { GuiBridge } from "../gui-host/clausters_gui.js";
import { server } from "../engine/server.ts";
import { decodePacket, encodeMessage, oscArg } from "../base/osc.ts";
import type { MsgArg, OscMessage } from "../base/osc.ts";
import type { Connection } from "../base/connection.ts";
import { WsConnection } from "../base/connection.ts";
import { ReplyTimeout } from "../errors.ts";
import { toJson } from "./guidef.ts";
import type { GuiNode } from "./guidef.ts";
import { GuiIdAllocator } from "./ids.ts";
import { WidgetHandle, WindowHandle } from "./handle.ts";
import type { EventArgs } from "./handle.ts";

export type EventListener = (packet: Uint8Array) => void;

/// The GUI host's default OSC port, UDP and TCP alike — clear of the audio
/// server's family (57110/57120).
export const DEFAULT_PORT = 57210;

/// The GUI host's default **WebSocket** port (`clausters-gui --ws`), the only
/// network carrier a browser can use.
export const DEFAULT_WS_PORT = 57220;

/// The page canvas' default size in device pixels, matching the host's own
/// default. A component sizes its canvas from its element box instead.
const DEFAULT_CANVAS = { width: 480, height: 420 };

/// The shared host surface: the raw binding bridge, the page-wide canvas
/// (re-parent it freely; the GPU context survives), and the outbound
/// `/gui_event`/`/gui_info`/`/gui_closed` stream as byte packets.
export interface ClaustersGui {
    bridge: GuiBridge;
    /// The page's default canvas — the one a page that does not make its own
    /// draws into. `attach` hands it to a def.
    canvas: HTMLCanvasElement;
    /// Gives a `window`-rooted def a canvas to draw into, before its
    /// `/gui_def` is fed. The host holds one canvas per def, so a document can
    /// show several at once; omit `canvas` to use the page's default one.
    attach(defId: number, canvas?: HTMLCanvasElement): void;
    addEvent(listener: EventListener): void;
    removeEvent(listener: EventListener): void;
}

let instance: Promise<ClaustersGui> | null = null;

/// The page's GUI host, booting it (and the engine) on first call.
///
/// One wasm GUI host serves the page, drawing one canvas per `window`-rooted
/// def. The first call initializes the wasm module, starts the host, makes the
/// page's default canvas (appended to `<body>`, where a page that makes none of
/// its own finds it), and wires the two singletons together **once**: engine
/// replies → `bridge.server_reply`, host outbound → `engine.send` (the in-page
/// server leg). Later calls get the same instance, so several components share
/// one host and one engine — the shared node/bus/buffer namespace.
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
        addEvent: (listener) => listeners.add(listener),
        removeEvent: (listener) => listeners.delete(listener),
    };
}

/// The in-page carrier: a `Connection` over the page's GUI-host singleton —
/// `feed` carries a packet in, the drained outbox carries the events back.
/// Closing detaches this connection's listeners; the host keeps running (it is
/// shared page state, not this connection's to stop).
///
/// A `/gui_def` sent over this carrier gets the page's default canvas attached
/// to it first, unless the caller already gave that def one. A `GuiHost` is
/// transport-agnostic — the same object drives a native `--ws` host, which has
/// windows rather than canvases — so the canvas policy belongs here, on the
/// carrier that *is* the page.
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

/// A widget's state as `/gui_info` reports it. An **empty** `type` means the
/// host has no such widget — it answers either way, the way the audio server
/// replies even on a miss.
export interface WidgetInfo {
    type: string;
    props: Record<string, number | string | boolean | null>;
}

interface Pending {
    match: (msg: OscMessage) => boolean;
    resolve: (msg: OscMessage) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
}

/// A GuiDef property value: a scalar, or an array/table that rides as its JSON
/// string (an OSC key/value is a scalar, so that is how the wire carries one).
export type PropValue = number | string | boolean | readonly unknown[] | Record<string, unknown>;

/// A connection to a running GUI host — the object that sends the GuiDefs and
/// reads the widgets back.
export class GuiHost {
    readonly connection: Connection;
    /// The one widget-id namespace for this host client — recycling, so a
    /// freed subtree's ids return to the pool. Windows and widgets share it.
    private readonly alloc = new GuiIdAllocator();
    /// The window ids opened through `open` and not yet closed.
    private readonly opened = new Set<number>();
    /// id → its child ids, for every widget this client defined — the subtree
    /// `free` walks to return the whole branch's ids to the pool.
    private readonly children = new Map<number, number[]>();
    private readonly onEventHandlers = new Map<number, (...args: EventArgs) => void>();
    private readonly onClosedHandlers = new Map<number, () => void>();
    private readonly handlers = new Set<(msg: OscMessage) => void>();
    private readonly pending = new Set<Pending>();
    private readonly listener: (packet: Uint8Array) => void;

    /// Drives the host behind `connection`. The core wasm must be loaded first
    /// (`await loadOsc()`), as for the audio `Server`.
    constructor(connection: Connection) {
        this.connection = connection;
        this.listener = (packet) => this.dispatch(packet);
        connection.addReply(this.listener);
    }

    /// A `GuiHost` driving **this page's** host (the `guiHost()` singleton) —
    /// the carrier that needs no process and no socket.
    static async page(): Promise<GuiHost> {
        return new GuiHost(await pageGuiConnection());
    }

    /// A `GuiHost` driving a **native** `clausters-gui --ws` host over a
    /// WebSocket (default `ws://127.0.0.1:57220`).
    static async connect(url = `ws://127.0.0.1:${DEFAULT_WS_PORT}`): Promise<GuiHost> {
        return new GuiHost(await WsConnection.open(url));
    }

    // ---- windows: open / close (the tree is a `window`-rooted GuiDef) ----

    /// A fresh id, unique across everything this host client names — windows
    /// and widgets share one recycling namespace, so a widget id never repeats
    /// across windows. A freed subtree's ids return to the pool.
    allocId(): number {
        return this.alloc.alloc();
    }

    /// Opens a window from a `window`-rooted GuiDef and returns its handle.
    ///
    /// A thin, id-managing wrapper over `define`: without an `id` one is
    /// assigned (and remembered, so `close`/`closeAll` free it); pass one to
    /// name the root yourself. Id-less **widgets** inside `tree` are assigned
    /// too, in place — see `define`. Any `blobs` ride along as they do there.
    open(
        tree: GuiNode,
        { id, blobs = [] }: { id?: number; blobs?: readonly Uint8Array[] } = {},
    ): WindowHandle {
        const wid = id ?? this.allocId();
        const handle = this.define(wid, tree, blobs);
        this.opened.add(wid);
        return handle;
    }

    /// `/gui_def <id> <json> [blob…]` — build a whole widget tree in one
    /// message, returning its `WindowHandle`. Trailing `blobs` (e.g. waveform
    /// samples from `samplesToBlob`) ride alongside the JSON and are
    /// referenced by index from a widget's `blob` property.
    ///
    /// Widgets built **without an id** get a fresh host-unique one here,
    /// written into the caller's object **in place** — so after
    /// `define`/`open` the widget you kept a reference to reads back as
    /// `widget.id`, ready for `set`/`bind`. Ids you picked are kept verbatim;
    /// they share one recycling namespace across every window on this host
    /// (allocation starts at 1000, so hand-picked ids below 1000 never collide
    /// with assigned ones). Any widget given a `name` is bound in the returned
    /// handle. Re-defining an existing id **redefines** it (the old subtree's
    /// ids return to the pool first, mirroring the host freeing it).
    define(id: number, tree: GuiNode, blobs: readonly Uint8Array[] = []): WindowHandle {
        if (this.children.has(id)) this.recycleSubtree(id, true);
        const names = new Map<string, number>();
        this.register(tree, id, names);
        this.send("/gui_def", ["i", id], toJson(tree), ...blobs);
        return new WindowHandle(this, id, names);
    }

    /// Instantiates a **persisted** GuiDef by name (`/gui_load`) — the host
    /// replays it as its saved `/gui_def`. The tree is the host's, so this
    /// client neither allocates its ids nor resolves its names.
    load(name: string): void {
        this.send("/gui_load", name);
    }

    /// Walks `node` (whose id is `nodeId`): assigns a fresh id to every id-less
    /// descendant **in place**, records each id's children (the subtree `free`
    /// recycles), and collects name → id. The root carries no id in the tree —
    /// it is the `/gui_def` argument — so its id is passed in.
    private register(node: GuiNode, nodeId: number, names: Map<string, number>): void {
        if (typeof node.name === "string" && node.name) names.set(node.name, nodeId);
        const childIds: number[] = [];
        for (const child of node.children ?? []) {
            child.id ??= this.allocId();
            childIds.push(child.id);
            this.register(child, child.id, names);
        }
        this.children.set(nodeId, childIds);
    }

    /// Returns `id`'s subtree ids to the pool and forgets its child map and
    /// handlers. With `keepRoot` the root id stays allocated (a redefine
    /// reuses it); a hand-picked id below the base was never allocated, so the
    /// pool ignores it.
    private recycleSubtree(id: number, keepRoot: boolean): void {
        for (const child of this.children.get(id) ?? []) {
            this.recycleSubtree(child, false);
        }
        this.children.delete(id);
        this.onEventHandlers.delete(id);
        if (keepRoot) return;
        this.onClosedHandlers.delete(id);
        this.alloc.free(id);
    }

    /// Closes a window opened with `open` (or any widget subtree): `/gui_free`
    /// frees the subtree and, for a `window` root, its window.
    close(id: number): void {
        this.free(id);
        this.opened.delete(id);
    }

    /// Closes every window still open through `open`.
    closeAll(): void {
        for (const id of [...this.opened]) this.close(id);
    }

    /// `/gui_set <id> <k> <v> …` — update one live widget. A value that is
    /// logically an array or a table (a curve's break-points, a theme) rides
    /// as its **JSON string**, since an OSC key/value is a scalar.
    set(id: number, props: Record<string, PropValue>): void {
        const args: MsgArg[] = [];
        for (const [key, value] of Object.entries(props)) {
            args.push(key, typeof value === "object" && value !== null
                ? JSON.stringify(value)
                : value);
        }
        this.send("/gui_set", ["i", id], ...args);
    }

    /// `/gui_free <id>` — free a widget and its subtree, returning its ids to
    /// the pool (the client-side mirror of the host freeing the subtree).
    free(id: number): void {
        this.send("/gui_free", ["i", id]);
        this.recycleSubtree(id, false);
    }

    /// `/gui_bind <id> "server" <address> <prefix…>` — forward this widget's
    /// value **straight to the audio server**, bypassing this script.
    ///
    /// On every change the host sends `address` (an OSC path like `/n_set` or
    /// `/c_set`) with the fixed `prefix` arguments followed by the widget's
    /// value — `bind(id, "/n_set", node.id, "freq")` makes the widget send
    /// `/n_set <node> freq <value>` itself, so the control responds with no
    /// round trip through the page's script (the low-latency path). A bound
    /// widget stops emitting `/gui_event`; `unbind` restores it. The host must
    /// have a server leg for the value to arrive — in the browser that is the
    /// in-page engine (wired by `guiHost()`) or a `--ws` server.
    bind(id: number, address: string, ...prefix: (number | string)[]): void {
        this.send("/gui_bind", ["i", id], "server", address, ...prefix);
    }

    /// `/gui_bind <id>` (no target) — remove a widget's binding, so its value
    /// flows back to this script as `/gui_event` again.
    unbind(id: number): void {
        this.send("/gui_bind", ["i", id]);
    }

    /// `/gui_query <id>` → the `/gui_info` reply. Rejects with `ReplyTimeout`
    /// if the host does not answer; an **empty** `type` means no such widget.
    async query(id: number, timeout = 5.0): Promise<WidgetInfo> {
        const reply = this.awaitReply(
            (msg) => msg.addr === "/gui_info" && Number(msg.args[0]) === id,
            timeout,
            `/gui_info for widget ${id}`,
        );
        this.send("/gui_query", ["i", id]);
        const args = (await reply).args;
        const props: WidgetInfo["props"] = {};
        for (let i = 2; i + 1 < args.length; i += 2) {
            props[String(args[i])] = args[i + 1] as WidgetInfo["props"][string];
        }
        return { type: String(args[1] ?? ""), props };
    }

    /// A `WidgetHandle` for an id this client did not build (a widget of a
    /// `load`-ed def, or one whose id you picked elsewhere).
    widget(id: number): WidgetHandle {
        return new WidgetHandle(this, id);
    }

    // ---- the inbound stream ----

    /// Registers (or, with `null`, clears) the `WidgetHandle.onEvent` callback
    /// for a widget id. Called through the handles; public so a script holding
    /// a bare id can reach it too.
    setEventHandler(id: number, handler: ((...args: EventArgs) => void) | null): void {
        if (handler === null) this.onEventHandlers.delete(id);
        else this.onEventHandlers.set(id, handler);
    }

    /// Registers (or clears) the `WindowHandle.onClosed` callback for a window.
    setClosedHandler(id: number, handler: (() => void) | null): void {
        if (handler === null) this.onClosedHandlers.delete(id);
        else this.onClosedHandlers.set(id, handler);
    }

    /// Subscribes to every decoded inbound message (`/gui_event`,
    /// `/gui_closed`, `/gui_info`); returns the unsubscribe. The seam a
    /// responder layer builds on — the per-widget callbacks are the ordinary
    /// way in.
    onMessage(handler: (msg: OscMessage) => void): () => void {
        this.handlers.add(handler);
        return () => this.handlers.delete(handler);
    }

    /// Resolves with the first inbound message `match` accepts, or rejects
    /// with `ReplyTimeout`. Registered *before* whatever send provokes it, so
    /// a fast host cannot outrun it.
    private awaitReply(
        match: (msg: OscMessage) => boolean,
        timeout: number,
        what: string,
    ): Promise<OscMessage> {
        return new Promise((resolve, reject) => {
            const entry: Pending = {
                match,
                resolve,
                reject,
                timer: setTimeout(() => {
                    this.pending.delete(entry);
                    reject(new ReplyTimeout(`no ${what} within ${timeout}s`));
                }, timeout * 1000),
            };
            this.pending.add(entry);
        });
    }

    private dispatch(packet: Uint8Array): void {
        let messages: OscMessage[];
        try {
            messages = decodePacket(packet);
        } catch (error) {
            console.warn(`clausters: undecodable host packet: ${String(error)}`);
            return;
        }
        for (const msg of messages) {
            for (const p of [...this.pending]) {
                if (p.match(msg)) {
                    this.pending.delete(p);
                    clearTimeout(p.timer);
                    p.resolve(msg);
                }
            }
            this.route(msg);
            for (const handler of [...this.handlers]) handler(msg);
        }
    }

    /// Routes one inbound message to the handle callback registered for its id
    /// (`onEvent` for `/gui_event`, `onClosed` for `/gui_closed`). A
    /// `/gui_closed` also drops the window from the open set.
    private route(msg: OscMessage): void {
        if (msg.addr === "/gui_event" && msg.args.length > 0) {
            const handler = this.onEventHandlers.get(Number(msg.args[0]));
            if (handler) handler(...(msg.args.slice(1) as EventArgs));
        } else if (msg.addr === "/gui_closed" && msg.args.length > 0) {
            const id = Number(msg.args[0]);
            this.opened.delete(id);
            this.onClosedHandlers.get(id)?.();
        }
    }

    /// Sends one `/gui_*` message. Arguments are tagged by position where the
    /// protocol fixes the type (an id is an int) and by inference otherwise.
    private send(addr: string, ...args: MsgArg[]): void {
        this.connection.send(encodeMessage(addr, args.map(oscArg)));
    }

    /// Detaches this client from its connection (the connection itself, and
    /// any shared in-page host, keep running) — `close` is the *window* verb
    /// here, as it is in the protocol. Pending queries reject.
    stop(): void {
        this.connection.removeReply(this.listener);
        for (const p of this.pending) {
            clearTimeout(p.timer);
            p.reject(new ReplyTimeout("the host client was closed"));
        }
        this.pending.clear();
        this.handlers.clear();
    }
}
