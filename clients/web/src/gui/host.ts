// `GuiHost` — the client object that drives a GUI host (mirrors
// `clausters/gui/host.py`). The page's *own* host is `./page.ts`.
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

import { decodePacket, encodeMessage, oscArg } from "../base/osc.ts";
import type { MsgArg, OscMessage } from "../base/osc.ts";
import { encodeImmediateBundle } from "../base/osc.ts";
import type { Connection } from "../base/connection.ts";
import { WsConnection } from "../base/connection.ts";
import { ReplyTimeout } from "../errors.ts";
import { toJson, view, View } from "./guidef.ts";
import type { Source } from "./guidef.ts";
import type { GuiNode } from "./guidef.ts";
import { GuiIdAllocator } from "./ids.ts";
import type { IdShare } from "../base/core.ts";
import { WidgetHandle, WindowHandle } from "./handle.ts";
import type { EventArgs } from "./handle.ts";
import { canvasIn, guiHost, newCanvas, pageGuiConnection } from "./page.ts";
import { ambientHost, setAmbientHost } from "./ambient.ts";
import type { ClaustersGui } from "./page.ts";

// The page's own host — the singleton, its canvases and the carrier over it —
// lives in `./page.ts` so the component run time can load it without this
// module and the GuiDef builders behind it. Re-exported here, where callers
// have always found it.
export { guiHost, newGuiHost, pageGuiConnection } from "./page.ts";
export type { ClaustersGui, EventListener } from "./page.ts";

/**
 * The GUI host's default OSC port, UDP and TCP alike — clear of the audio
 * server's family (57110/57120).
 */
export const DEFAULT_PORT = 57210;

/**
 * The GUI host's default **WebSocket** port (`clausters-gui --ws`), the only
 * network carrier a browser can use.
 */
export const DEFAULT_WS_PORT = 57220;

/**
 * A widget's state as `/gui_info` reports it. An **empty** `type` means the
 * host has no such widget — it answers either way, the way the audio server
 * replies even on a miss.
 */
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

/**
 * A GuiDef property value: a scalar, or an array/table that rides as its JSON
 * string (an OSC key/value is a scalar, so that is how the wire carries one).
 */
export type PropValue = number | string | boolean | readonly unknown[] | Record<string, unknown>;

/**
 * A prop name as the wire wants it. The builders spell each one out
 * (`["window_ms", windowMs]`) because they also decide what to drop; a live
 * `set` takes a whole bag at once and has nothing to spell out, so the
 * conversion is mechanical here. Wire names are snake_case throughout
 * (`docs/gui-protocol.md`), which is what makes the round trip safe: a name
 * already in that form has no capital to convert.
 */
const wireProp = (name: string): string =>
    name.replace(/[A-Z]/g, (c) => `_${c.toLowerCase()}`);

/**
 * A connection to a running GUI host — the object that sends the GuiDefs and
 * reads the widgets back.
 */
/**
 * The interface events a widget reports — what the **hand** did, as against
 * what the widget is worth. They arrive as a one-string payload and are the
 * only such payload, which is what tells them from a value.
 */
export const INTERFACE_EVENTS: readonly string[] = ["press", "release", "click"];

export class GuiHost {
    readonly connection: Connection;
    /**
     * The one widget-id namespace for this host client — recycling, so a
     * freed subtree's ids return to the pool. Windows and widgets share it.
     */
    private readonly alloc: GuiIdAllocator;
    /** The window ids opened through `open` and not yet closed. */
    private readonly opened = new Set<number>();
    /**
     * id → its child ids, for every widget this client defined — the subtree
     * `free` walks to return the whole branch's ids to the pool.
     */
    private readonly children = new Map<number, number[]>();
    /**
     * Window id → the handle handed out for it, so a redraw refreshes it
     * in place instead of orphaning the caller's copy.
     */
    private readonly handles = new Map<number, WindowHandle>();
    /**
     * The stamp of the last `/gui_event` seen — what `ack` answers. The host
     * numbers every edit it emits so an owner's reply can name which one it is
     * about; zero means nothing has arrived yet.
     */
    lastSeq = 0;
    /**
     * The document version the last `/gui_event` was made against — what the
     * host had been told when the hand let go. Zero means the host cannot say,
     * which is what an owner that never reports a version leaves it with, and
     * which an owner reads as *apply unchecked*.
     */
    lastVersion = 0;
    private readonly onEventHandlers = new Map<number, (...args: EventArgs) => void>();
    /**
     * Per widget, the interface-event callbacks by tag (`onPress`, `onRelease`,
     * `onClick`). Kept apart from {@link onEventHandlers} because they are a
     * different vocabulary, not a filter over the same one, and because both
     * may be registered on one widget at once.
     */
    private readonly onInterfaceHandlers = new Map<number, Map<string, () => void>>();
    private readonly onClosedHandlers = new Map<number, () => void>();
    private readonly handlers = new Set<(msg: OscMessage) => void>();
    private readonly pending = new Set<Pending>();
    private readonly listener: (packet: Uint8Array) => void;
    /**
     * The page surface this host draws on, when it is an in-page one — where a
     * def's canvas comes from. `null` for a host reached over a socket, which
     * has windows of its own and no document to mount into.
     */
    page: ClaustersGui | null = null;
    /** Window id → the disposer that stops following its element's box. */
    private readonly fitted = new Map<number, () => void>();
    /**
     * Widget id → the sources that widget draws, so a recycled widget stops
     * being one of the live ends a `Source.set` reaches.
     */
    private readonly sources = new Map<number, Source[]>();

    /**
     * Drives the host behind `connection`. The core wasm must be loaded first
     * (`await loadOsc()`), as for the audio `Server`.
     *
     * `share` takes one slice of the widget-id space instead of all of it,
     * for a host with more than one client naming widgets on it — the same
     * arrangement, and the same arithmetic, as the audio `Server`'s (see
     * `IdShare`).
     */
    constructor(connection: Connection, { share }: { share?: IdShare } = {}) {
        this.connection = connection;
        this.alloc = new GuiIdAllocator(undefined, undefined, share);
        this.listener = (packet) => this.dispatch(packet);
        connection.addReply(this.listener);
    }

    /**
     * A `GuiHost` driving **this page's** host (the `guiHost()` singleton) —
     * the carrier that needs no process and no socket. Pass an instance built
     * by `newGuiHost` to drive one of its own instead, which is how a
     * `Session` with its own engine gets a GUI leg wired to that engine.
     *
     * `share` splits the widget-id space with another client of the same
     * host; a `Session` passes its own share, so both of its legs are sliced
     * the same way.
     */
    static async page(
        target?: Promise<ClaustersGui> | ClaustersGui,
        { share, adoptAmbient = true }: { share?: IdShare; adoptAmbient?: boolean } = {},
    ): Promise<GuiHost> {
        const gui = await (target ?? guiHost());
        const host = new GuiHost(await pageGuiConnection(gui), { share });
        host.page = gui;
        if (adoptAmbient) host.adoptAmbient();
        return host;
    }

    /**
     * Register as the **ambient** host when none is, first-wins — so
     * `view(...).open()`, `plot` and `scope` land here instead of opening a
     * second host. The mirror of the audio `Server`'s default-session
     * adoption, and `stop` gives the registration up.
     */
    adoptAmbient(): this {
        if (ambientHost() === null) setAmbientHost(this);
        return this;
    }

    /**
     * A `GuiHost` driving a **native** `clausters-gui --ws` host over a
     * WebSocket (default `ws://127.0.0.1:57220`).
     */
    static async connect(
        url = `ws://127.0.0.1:${DEFAULT_WS_PORT}`,
        { share, adoptAmbient = true }: { share?: IdShare; adoptAmbient?: boolean } = {},
    ): Promise<GuiHost> {
        const host = new GuiHost(await WsConnection.open(url), { share });
        if (adoptAmbient) host.adoptAmbient();
        return host;
    }

    // ---- windows: open / close (the tree is a `window`-rooted GuiDef) ----

    /**
     * A fresh id, unique across everything this host client names — windows
     * and widgets share one recycling namespace, so a widget id never repeats
     * across windows. A freed subtree's ids return to the pool.
     */
    allocId(): number {
        return this.alloc.alloc();
    }

    /**
     * Opens a window from a `window`-rooted GuiDef and returns its handle.
     *
     * A thin, id-managing wrapper over `define`: without an `id` one is
     * assigned (and remembered, so `close`/`closeAll` free it); pass one to
     * name the root yourself. Id-less **widgets** inside `tree` are assigned
     * too, in place — see `define`. Any `blobs` ride along as they do there.
     */
    open(
        tree: GuiNode,
        {
            id,
            blobs = [],
            element,
        }: { id?: number; blobs?: readonly Uint8Array[]; element?: Element | null } = {},
    ): WindowHandle {
        const wid = id ?? this.allocId();
        // The wire opens an OS window for a `window`-rooted def and nothing
        // else, so a root that is not one is framed here: any node opens, and
        // the frame hugs whatever it holds rather than padding it out.
        const root = tree.type === "window" ? tree : view({ hug: true }, tree);
        if (element != null && this.page === null) {
            throw new Error(
                "this host has windows of its own, not a document to mount " +
                    "into — an element is only meaningful for a host on this page",
            );
        }
        let canvas: HTMLCanvasElement | undefined;
        if (this.page !== null) {
            // A page has no window manager, so *a view with no parent is a
            // window* finishes here: with an element, the view takes that
            // element's box; with none, it gets a canvas of its own. Either
            // way it is this view's canvas and not a page-wide one, which is
            // what lets a document hold as many as it opens.
            canvas = element != null ? canvasIn(element) : newCanvas();
            // Before the `/gui_def`: the carrier attaches the page's fallback
            // canvas to a def fed without one, and `attach` is idempotent, so
            // the canvas chosen here is the one the def keeps.
            this.page.attach(wid, canvas);
        }
        const handle = this.define(wid, root, blobs);
        handle.canvas = canvas ?? null;
        this.opened.add(wid);
        if (canvas !== undefined && element != null && this.page !== null) {
            this.fitted.get(wid)?.();
            // The host never reads the DOM, so the page reports the box — and
            // keeps reporting it, since an element's size and the display's
            // scale move independently.
            this.fitted.set(wid, this.page.fit(wid, element, canvas));
        }
        return handle;
    }

    /**
     * `/gui_def <id> <json> [blob…]` — build a whole widget tree in one
     * message, returning its `WindowHandle`. Trailing `blobs` (e.g. waveform
     * samples from `samplesToBlob`) ride alongside the JSON and are
     * referenced by index from a widget's `blob` property.
     *
     * Widgets built **without an id** get a fresh host-unique one here,
     * written into the caller's object **in place** — so after
     * `define`/`open` the widget you kept a reference to reads back as
     * `widget.id`, ready for `set`/`bind`. Ids you picked are kept verbatim;
     * they share one recycling namespace across every window on this host
     * (allocation starts at 1000, so hand-picked ids below 1000 never collide
     * with assigned ones). Any widget given a `name` is bound in the returned
     * handle. Re-defining an existing id **redefines** it (the old subtree's
     * ids return to the pool first, mirroring the host freeing it).
     */
    define(id: number, tree: GuiNode, blobs: readonly Uint8Array[] = []): WindowHandle {
        const previous = this.handles.get(id);
        const inherited = new Map<string, (...args: EventArgs) => void>();
        const inheritedHand = new Map<string, Map<string, () => void>>();
        const rootHandler = this.onEventHandlers.get(id);
        const rootHand = this.onInterfaceHandlers.get(id);
        if (this.children.has(id)) {
            if (previous !== undefined) {
                for (const name of previous.widgetNames()) {
                    const wid = previous.widget(name).id;
                    const func = this.onEventHandlers.get(wid);
                    if (func !== undefined) inherited.set(name, func);
                    // The interface handlers travel by name for the same
                    // reason: `onClick` is a callback of the widget the name
                    // points at, and a redrawn window that dropped them would
                    // look like a button that stopped working.
                    const hand = this.onInterfaceHandlers.get(wid);
                    if (hand !== undefined) inheritedHand.set(name, hand);
                }
            }
            this.recycleSubtree(id, true);
        }
        const names = new Map<string, number>();
        const controls = new Map<number, string>();
        const extra: Uint8Array[] = [];
        const document = this.stamp(tree, id, names, controls, extra, blobs.length);
        // A redraw takes fresh ids from the pool, so a handler kept under the
        // old id would be orphaned -- or fire for whatever widget inherited
        // that number. A callback belongs to the widget the *name* points at.
        if (rootHandler !== undefined) this.onEventHandlers.set(id, rootHandler);
        if (rootHand !== undefined) this.onInterfaceHandlers.set(id, rootHand);
        for (const [name, func] of inherited) {
            const wid = names.get(name);
            if (wid !== undefined) this.onEventHandlers.set(wid, func);
        }
        for (const [name, hand] of inheritedHand) {
            const wid = names.get(name);
            if (wid !== undefined) this.onInterfaceHandlers.set(wid, hand);
        }
        this.send("/gui_def", ["i", id], toJson(document), ...blobs, ...extra);
        if (previous !== undefined) {
            previous.refreshNames(names, controls);
            return previous;
        }
        const handle = new WindowHandle(this, id, names, controls);
        this.handles.set(id, handle);
        return handle;
    }

    /**
     * Instantiates a **persisted** GuiDef by name (`/gui_load`) — the host
     * replays it as its saved `/gui_def`. The tree is the host's, so this
     * client neither allocates its ids nor resolves its names.
     */
    load(name: string): void {
        this.send("/gui_load", name);
    }

    /**
     * `/gui_font <blob>` — draw text with this typeface from now on.
     *
     * `face` is a raw TrueType/OpenType file (the host's rasterizer does not
     * decompress WOFF2, so a Google Fonts CSS URL is not one), served with CORS
     * if it comes from another origin — a CSS `@font-face` cannot serve here,
     * since the host draws into a canvas and never reads the document's fonts.
     * A face is a property of the **host**, not of a window, so the call
     * carries no id and every window it has open — and every one it opens
     * later — draws with it.
     *
     * Loading one **relayouts nothing**: the size table never followed the
     * typeface, so the same tree comes up the same size before and after and a
     * face may be handed over at any point. What changes is that `textSize`
     * becomes continuous rather than quantized to half-steps of the cell, which
     * a bitmap glyph's own pixels require.
     *
     * A host built without a rasterizer logs and keeps drawing with its
     * embedded bitmap face — which is what it also does with bytes it cannot
     * read. Neither is an error here: the bitmap face is the floor every build
     * draws on.
     */
    font(face: Uint8Array): void {
        this.send("/gui_font", face);
    }

    /**
     * Walks `node` (whose id is `nodeId`) and returns **a copy** carrying the
     * ids: every id-less descendant gets a fresh one, each id's children are
     * recorded (the subtree `free` recycles), and name → id is collected. The
     * root carries no id in the tree — it is the `/gui_def` argument — so its
     * id is passed in.
     *
     * It copies rather than writing into the caller's tree because **an id
     * names a live widget and a view is a definition**: one view opens as many
     * times as you like, each window with ids of its own. Copying is also what
     * makes the same subtree nested twice work — node identity never enters,
     * so the two places get two id runs and the host is not told to build one
     * widget twice.
     *
     * A duplicate `name` is refused here as well as where a view is built: a
     * hand-written object literal reaches this walk without passing a builder,
     * and a shadowed widget would draw and be unreachable.
     */
    private stamp(
        node: GuiNode,
        nodeId: number,
        names: Map<string, number>,
        controls: Map<number, string>,
        blobs: Uint8Array[],
        blobBase: number,
    ): GuiNode {
        if (typeof node.name === "string" && node.name) {
            if (names.has(node.name)) {
                throw new Error(
                    `duplicate widget name "${node.name}" in one tree — a name is ` +
                        "how this client addresses a widget, so two widgets cannot " +
                        "share one.",
                );
            }
            names.set(node.name, nodeId);
        }
        const control = node instanceof View ? node.control : null;
        if (control !== null && control !== undefined) {
            controls.set(nodeId, (control as { name: string }).name);
        }
        const out: GuiNode = { type: node.type };
        for (const [key, value] of Object.entries(node)) {
            if (key !== "children") out[key] = value;
        }
        if (node instanceof View) {
            for (const { source } of node.sources) {
                source.addLive(this, nodeId);
                const held = this.sources.get(nodeId);
                if (held === undefined) this.sources.set(nodeId, [source]);
                else held.push(source);
                // A blob source rides its bytes beside the JSON, and the index
                // is where they land in *this* message — which is why nobody
                // has to keep `blob: 0` in step with the open call by hand.
                const bytes = source.bytes;
                if (bytes !== null) {
                    out.blob = blobBase + blobs.length;
                    blobs.push(bytes);
                }
            }
        }
        const childIds: number[] = [];
        const stamped: GuiNode[] = [];
        for (const child of node.children ?? []) {
            const cid = child.id ?? this.allocId();
            childIds.push(cid);
            const sub = this.stamp(child, cid, names, controls, blobs, blobBase);
            sub.id = cid;
            stamped.push(sub);
        }
        if (stamped.length > 0) out.children = stamped;
        this.children.set(nodeId, childIds);
        return out;
    }

    /**
     * Returns `id`'s subtree ids to the pool and forgets its child map and
     * handlers. With `keepRoot` the root id stays allocated (a redefine
     * reuses it); a hand-picked id below the base was never allocated, so the
     * pool ignores it.
     */
    private recycleSubtree(id: number, keepRoot: boolean): void {
        for (const child of this.children.get(id) ?? []) {
            this.recycleSubtree(child, false);
        }
        for (const held of this.sources.get(id) ?? []) held.dropLive(this, id);
        this.sources.delete(id);
        this.children.delete(id);
        this.onEventHandlers.delete(id);
        this.onInterfaceHandlers.delete(id);
        if (keepRoot) return;
        this.onClosedHandlers.delete(id);
        this.handles.delete(id);
        this.alloc.free(id);
    }

    /**
     * Closes a window opened with `open` (or any widget subtree): `/gui_free`
     * frees the subtree and, for a `window` root, its window.
     */
    close(id: number): void {
        this.fitted.get(id)?.();
        this.fitted.delete(id);
        this.free(id);
        this.opened.delete(id);
    }

    /** Closes every window still open through `open`. */
    closeAll(): void {
        for (const id of [...this.opened]) this.close(id);
    }

    /**
     * `/gui_ack <seq> <docVersion> [<source> <generation>…] [<reason>]` — answer
     * the edits the host emitted, up to `seq`.
     *
     * The reply `/gui_event` never had. Without it the host cannot tell an edit
     * the owner **refused** from one it took, so it goes on drawing what the
     * hand did — and cannot tell which of two gestures in flight an answer
     * belongs to.
     *
     * There is no success flag, because there is nothing to branch on: the
     * values the owner decided ride as ordinary `set` calls **in the same
     * bundle** (see `push`), and *applied*, *applied transformed* and *refused*
     * are the same message — a refusal is simply the previous value pushed
     * back. Send it **always**, including when nothing changed.
     */
    ack(
        seq: number,
        docVersion = 0,
        generations: readonly (readonly [number, number])[] = [],
        reason?: string,
    ): void {
        this.send("/gui_ack", ...ackArgs(seq, docVersion, generations, reason));
    }

    /**
     * The state the owner decided, plus the acknowledgement, as **one bundle**.
     *
     * The acknowledgement goes last, after the values, and the whole thing is
     * one packet: the host processes a bundle's messages in order as a unit, so
     * it never sees a stamp retire an edit before the state that edit produced
     * has arrived.
     */
    push(
        seq: number,
        sets: readonly (readonly [number, Record<string, PropValue>])[],
        docVersion = 0,
        generations: readonly (readonly [number, number])[] = [],
        reason?: string,
    ): void {
        const messages: [string, MsgArg[]][] = sets.map(([id, props]) => [
            "/gui_set",
            [["i", id] as MsgArg, ...setArgs(props as Record<string, PropValue>)],
        ]);
        messages.push([
            "/gui_ack",
            ackArgs(seq, docVersion, generations, reason),
        ]);
        this.connection.send(
            encodeImmediateBundle(
                messages.map(([addr, args]) => ({ addr, args: args.map(oscArg) })),
            ),
        );
    }

    /**
     * `/gui_set <id> <k> <v> …` — update one live widget. A value that is
     * logically an array or a table (a curve's break-points, a theme) rides
     * as its **JSON string**, since an OSC key/value is a scalar.
     *
     * Prop names are written the way the builders take them and go out the
     * way the wire wants them (`windowMs` → `window_ms`), which is the
     * package's standing rule — the options are TypeScript's, the props are
     * the wire's. A name already in wire form passes through untouched, so
     * `set({ ruler: "off" })` is what it always was.
     */
    set(id: number, props: Record<string, PropValue>): void {
        this.send("/gui_set", ["i", id], ...setArgs(props));
    }

    /**
     * `/gui_set <id> focus 1` — point the keyboard at this widget (`on: false`
     * gives the focus up).
     *
     * The focused widget is the only one keys reach, and there is one focus per
     * host. The user moves it by clicking or with Tab; this is the page's way,
     * for a field that should be ready to type into the moment its window
     * opens. A widget that reads no keyboard refuses it, and the move is
     * reported back as a `"focus"` event on both the widget that gained it and
     * the one that lost it.
     */
    focus(id: number, on = true): void {
        this.set(id, { focus: on ? 1 : 0 });
    }

    /**
     * `/gui_free <id>` — free a widget and its subtree, returning its ids to
     * the pool (the client-side mirror of the host freeing the subtree).
     */
    free(id: number): void {
        this.send("/gui_free", ["i", id]);
        this.recycleSubtree(id, false);
    }

    /**
     * `/gui_bind <id> "server" <address> <prefix…>` — forward this widget's
     * value **straight to the audio server**, bypassing this script.
     *
     * On every change the host sends `address` (an OSC path like `/node_set` or
     * `/bus_set`) with the fixed `prefix` arguments followed by the widget's
     * value — `bind(id, "/node_set", node.id, "freq")` makes the widget send
     * `/node_set <node> freq <value>` itself, so the control responds with no
     * round trip through the page's script (the low-latency path). A bound
     * widget stops emitting `/gui_event`; `unbind` restores it. The host must
     * have a server leg for the value to arrive — in the browser that is the
     * in-page engine (wired by `guiHost()`) or a `--ws` server.
     */
    bind(id: number, address: string, ...prefix: (number | string)[]): void {
        this.send("/gui_bind", ["i", id], "server", address, ...prefix);
    }

    /**
     * `/gui_bind <id> "widget" <target> <prop>` — apply this widget's value to
     * **another widget's property**, with no round-trip through this script.
     *
     * On every change the host sets `prop` on widget `target` exactly as a
     * `set` would — `bindWidget(picker, pages, "index")` makes a menu flip a
     * `stack`'s page, a slider drive a plot's `max`, a curve write another
     * curve's `points` (an edit-back payload rides as the JSON string the prop
     * already takes). A bound widget stops emitting `/gui_event`; `unbind`
     * restores it.
     *
     * **A binding fires an apply, never another binding**: the target's own
     * binding does not fire from it, so two widgets bound to each other settle
     * instead of cascading.
     */
    bindWidget(id: number, target: number, prop: string): void {
        this.send("/gui_bind", ["i", id], "widget", ["i", target], prop);
    }

    /**
     * `/gui_bind <id>` (no target) — remove a widget's binding, so its value
     * flows back to this script as `/gui_event` again.
     */
    unbind(id: number): void {
        this.send("/gui_bind", ["i", id]);
    }

    /**
     * `/gui_query <id>` → the `/gui_info` reply. Rejects with `ReplyTimeout`
     * if the host does not answer; an **empty** `type` means no such widget.
     *
     * What the widget **is now**: the props it was defined with, with every
     * edit the user has made since laid over them — a dragged control's value,
     * a moved clip's `offset`/`dur`, an edited curve's `points` — so this is
     * how a page reads back what a gesture did without listening for the event
     * that announced it. Scalars only (the reply is flat OSC arguments); an
     * edited structure comes back as the JSON string its own `set` accepts.
     *
     * The default wait is the Python client's second: a host that is up
     * answers a query off its own event loop, and one that is not will not
     * answer in five either.
     */
    async query(id: number, timeout = 1.0): Promise<WidgetInfo> {
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

    /**
     * A `WidgetHandle` for an id this client did not build (a widget of a
     * `load`-ed def, or one whose id you picked elsewhere).
     */
    widget(id: number): WidgetHandle {
        return new WidgetHandle(this, id);
    }

    // ---- the inbound stream ----

    /**
     * Registers (or, with `null`, clears) the `WidgetHandle.onEvent` callback
     * for a widget id. Called through the handles; public so a script holding
     * a bare id can reach it too.
     */
    setEventHandler(id: number, handler: ((...args: EventArgs) => void) | null): void {
        if (handler === null) this.onEventHandlers.delete(id);
        else this.onEventHandlers.set(id, handler);
    }

    /**
     * Registers (or, with `null`, clears) an interface-event callback
     * (`WidgetHandle.onPress` / `onRelease` / `onClick`) for a widget id.
     */
    setInterfaceHandler(id: number, tag: string, handler: (() => void) | null): void {
        const table = this.onInterfaceHandlers.get(id);
        if (handler === null) {
            table?.delete(tag);
            if (table !== undefined && table.size === 0) this.onInterfaceHandlers.delete(id);
            return;
        }
        if (table === undefined) this.onInterfaceHandlers.set(id, new Map([[tag, handler]]));
        else table.set(tag, handler);
    }

    /** Registers (or clears) the `WindowHandle.onClosed` callback for a window. */
    setClosedHandler(id: number, handler: (() => void) | null): void {
        if (handler === null) this.onClosedHandlers.delete(id);
        else this.onClosedHandlers.set(id, handler);
    }

    /**
     * Subscribes to every decoded inbound message (`/gui_event`,
     * `/gui_closed`, `/gui_info`); returns the unsubscribe. The seam a
     * responder layer builds on — the per-widget callbacks are the ordinary
     * way in.
     */
    onMessage(handler: (msg: OscMessage) => void): () => void {
        this.handlers.add(handler);
        return () => this.handlers.delete(handler);
    }

    /**
     * Resolves with the first inbound message `match` accepts, or rejects
     * with `ReplyTimeout`. Registered *before* whatever send provokes it, so
     * a fast host cannot outrun it.
     */
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

    /**
     * Routes one inbound message to the handle callback registered for its id
     * (`onEvent` for `/gui_event`, `onClosed` for `/gui_closed`). A
     * `/gui_closed` also drops the window from the open set.
     */
    private route(msg: OscMessage): void {
        if (msg.addr === "/gui_event" && msg.args.length > 0) {
            // `<id> <seq> <version> <payload…>`: the stamp and the version the
            // edit was made against are the second and third arguments of every
            // event, before any tag, so one rule reads them all. A handler is
            // given the payload — those two are this client's bookkeeping, and
            // `ack` is what answers them.
            this.lastSeq = msg.args.length > 1 ? Number(msg.args[1]) : 0;
            this.lastVersion = msg.args.length > 2 ? Number(msg.args[2]) : 0;
            const wid = Number(msg.args[0]);
            const payload = msg.args.slice(3);
            // The interface events first, and they are *also* handed to
            // `onEvent`: that verb is the raw stream, so a script that reads
            // everything keeps reading everything.
            if (payload.length === 1 && typeof payload[0] === "string"
                && INTERFACE_EVENTS.includes(payload[0])) {
                this.onInterfaceHandlers.get(wid)?.get(payload[0])?.();
            }
            const handler = this.onEventHandlers.get(wid);
            if (handler) handler(...(payload as EventArgs));
        } else if (msg.addr === "/gui_closed" && msg.args.length > 0) {
            const id = Number(msg.args[0]);
            this.opened.delete(id);
            this.onClosedHandlers.get(id)?.();
        }
    }

    /**
     * Sends one `/gui_*` message. Arguments are tagged by position where the
     * protocol fixes the type (an id is an int) and by inference otherwise.
     */
    private send(addr: string, ...args: MsgArg[]): void {
        this.connection.send(encodeMessage(addr, args.map(oscArg)));
    }

    /**
     * Detaches this client from its connection (the connection itself, and
     * any shared in-page host, keep running) — `close` is the *window* verb
     * here, as it is in the protocol. Pending queries reject.
     */
    stop(): void {
        if (ambientHost() === this) setAmbientHost(null);
        for (const dispose of this.fitted.values()) dispose();
        this.fitted.clear();
        this.connection.removeReply(this.listener);
        for (const p of this.pending) {
            clearTimeout(p.timer);
            p.reject(new ReplyTimeout("the host client was closed"));
        }
        this.pending.clear();
        this.handlers.clear();
    }
}

/**
 * A `/gui_set`'s arguments for one props object — the same shaping `set` does,
 * factored out because `push` bundles several of them with an acknowledgement.
 */
function setArgs(props: Record<string, PropValue>): MsgArg[] {
    const args: MsgArg[] = [];
    for (const [key, value] of Object.entries(props)) {
        args.push(wireProp(key), wireValue(value));
    }
    return args;
}

/**
 * One `/gui_set` value as the wire takes it.
 *
 * A **structural** value has no OSC type at all and rides as its JSON string.
 * A **boolean** rides as `1`/`0`: OSC's own boolean tags carry no argument, so
 * a flag prop has always been an int there and the builders emit one —
 * `set({ fills: false })` is what a reader of `fills: true` in a builder will
 * write, so it has to mean the same thing.
 */
function wireValue(value: PropValue): MsgArg {
    if (typeof value === "boolean") return ["i", value ? 1 : 0];
    if (typeof value === "object" && value !== null) return JSON.stringify(value);
    return value;
}

/** A `/gui_ack`'s arguments: the stamp, the document version, the source
 * generations that moved, and an optional reason. */
function ackArgs(
    seq: number,
    docVersion: number,
    generations: readonly (readonly [number, number])[],
    reason?: string,
): MsgArg[] {
    const args: MsgArg[] = [["i", Math.trunc(seq)], ["i", Math.trunc(docVersion)]];
    for (const [source, generation] of generations) {
        args.push(["i", Math.trunc(source)], ["i", Math.trunc(generation)]);
    }
    if (reason !== undefined) args.push(reason);
    return args;
}
