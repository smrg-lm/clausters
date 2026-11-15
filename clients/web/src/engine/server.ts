// The page's audio server: one by default, more when a caller asks.
//
// A page has one audio engine, however many components sit on it: the first
// `server()` call boots the AudioWorklet engine (AudioContext + worklet +
// wasm server, via the engine bundle's loader) and every later call -- another
// component, a REPL, the TS client -- gets the same instance. The raw engine
// handle exposes a single `onReply` slot, so the shared one owns it and fans
// replies out to any number of listeners; everything else passes through.
//
// Sharing is the default, not a limit of the page: `engine()` boots a separate
// one for a caller that must not share a node, bus and buffer space with the
// rest of the document. Nothing here was ever page-global except that memo --
// `bootClausters` builds its own AudioContext and worklet per call -- which is
// the same shape the GUI host has, where the instance and not the page is what
// owns an id space.
//
// The AudioContext starts suspended under the autoplay policy: call `resume()`
// from a user gesture (the `<clausters-*>` elements' power affordance does).

import { bootClausters } from "./loader.ts";

/**
 * The client tag a caller gets when it never claims one -- the single client a
 * segment has always had (`ipc::DEFAULT_PEER`). Every page with one client
 * stays exactly as it was.
 */
export const DEFAULT_PEER = 0;

/**
 * Listen to **every** reply this engine produces, whichever client it is for.
 *
 * Replies are addressed, so an ordinary listener hears only its own client's --
 * which is the point, and what keeps two clients over one engine from reading
 * each other's streams. An observer is the case that is not ordinary: a test
 * asserting that the GUI host's meters are streaming, a debug tap logging the
 * wire. Those get a door of their own rather than reaching into another
 * client's internals, and it is a *read* door: there is no `send` under this.
 */
export const ANY_PEER = -1;
import type { BootOptions, ClockAnchor } from "./loader.ts";

export type ReplyListener = (packet: Uint8Array) => void;

/**
 * The shared engine surface: raw OSC bytes in, fanned-out replies back --
 * what the connection seam (`base/connection.ts`) and any REPL build on.
 */
export interface ClaustersServer {
    context: AudioContext;
    node: AudioWorkletNode;
    /**
     * One complete OSC packet to the engine (bytes are transferred), from the
     * client `peer` -- `DEFAULT_PEER` when the caller has not claimed one.
     */
    send(bytes: Uint8Array, peer?: number): void;
    /**
     * Subscribe to the engine's replies **for one client**. The server keeps a
     * subscription and a reply queue per client, so a page holding several
     * (its script and its GUI host) claims a tag each with `claimPeer` and
     * listens under it; a listener registered without one hears the default
     * client's replies, which is every page that has only one. `ANY_PEER`
     * hears all of them -- the observer door, for tests and debug taps.
     */
    addReply(listener: ReplyListener, peer?: number): void;
    removeReply(listener: ReplyListener, peer?: number): void;
    /**
     * A client tag nobody else in this page is using -- what a second
     * independent client over this one engine needs so its `/bus_stream`
     * subscription is its own. See `docs/ipc.md`.
     */
    claimPeer(): number;
    /**
     * This engine for a client in **another realm** -- a worker, which has no
     * `AudioContext` and cannot reach this one: a peer of its own and a port
     * that goes to the engine directly. Transfer `link.port` to the worker,
     * which opens a session over it (`Session.live(port)`); {@link
     * EngineLink.close} lets the client go, which is what a page does after
     * terminating that worker.
     *
     * The engine stays this page's: the worker attaches to it, starts and
     * stops nothing, and is told when the engine closes.
     */
    link(): EngineLink;
    clock(): Promise<number>;
    /**
     * The engine's clock paired with the context's frame counter, both read
     * in the same instant -- what a sample-locked client anchors to.
     */
    clockAnchor(): Promise<ClockAnchor>;
    /**
     * Installs host-decoded samples as buffer `index` (the browser's
     * /buffer_allocRead); `samples` is interleaved and transferred.
     */
    bufferLoad(
        index: number,
        channels: number,
        sampleRate: number,
        samples: Float32Array,
    ): Promise<number>;
    resume(): Promise<void>;
    suspend(): Promise<void>;
    /**
     * Releases this engine: the `AudioContext` and with it the worklet, the
     * audio device and the browser's per-page context slot. Nothing restarts
     * it. The sibling of `GuiBridge.close()` on the GUI side, and for the
     * same reason -- an instance that outlives its purpose otherwise keeps
     * rendering. The page's shared engine (`server()`) is never closed by
     * anything that merely uses it.
     */
    close(): Promise<void>;
}

/**
 * A client of an engine in another realm, as the engine's page holds it
 * ({@link ClaustersServer.link}).
 */
export interface EngineLink {
    /** The client's end: transfer it to the worker that runs the client. */
    readonly port: MessagePort;
    /** The client's tag on the engine. */
    readonly peer: number;
    /**
     * Lets the client go: the engine forgets its port and the client hears
     * that the engine is gone. Idempotent.
     */
    close(): void;
}

/**
 * What a page says to a client in another realm over a link's port -- a link
 * to an engine ({@link ClaustersServer.link}) or to a GUI host
 * (`ClaustersGui.link`) -- once the client has asked with `{ type: "open" }`:
 * the port the client's packets travel on, then, when it goes, that the other
 * end is gone.
 */
export type LinkMessage =
    | { type: "port"; port: MessagePort; peer: number }
    | { type: "gone" };

let instance: Promise<ClaustersServer> | null = null;

/**
 * The page's engine, booting it on first call. `options` (channels, an
 * existing AudioContext) only apply to that first call.
 *
 * This is the shared one, and sharing is what a page wants: every component on
 * it plays into the same mix. Use {@link engine} for the other case -- a caller
 * that needs an engine of its own rather than the page's.
 */
export function server(options: BootOptions = {}): Promise<ClaustersServer> {
    instance ??= boot(options);
    return instance;
}

/**
 * The page's engine **only if one is already up**, else `null` -- asked without
 * bringing one into being, which {@link server} cannot be.
 *
 * This is what `Server.attach()` reads. Attaching is for a server already
 * running, and it verifies rather than starting one: a handle pointing at
 * silence has to say so instead of quietly manufacturing the thing it was
 * meant to find. The reference client asks the same question of a port.
 */
export function pageEngineIfUp(): Promise<ClaustersServer> | null {
    return instance;
}

/**
 * A **separate** engine, not the page's.
 *
 * The default is one engine per page, as {@link server} gives, because
 * components on one page belong to one mix. But the count is a property of the
 * caller, not of the page: a document hosting several independent clients --
 * isolated demos side by side, an editor beside a player -- needs each to have its
 * own node ids, its own buses and its own buffers, and that is exactly what a
 * separate engine gives without partitioning anything.
 *
 * Each one is its own `AudioContext` and its own worklet, so they mix in the
 * browser rather than in the engine. Browsers cap concurrent `AudioContext`s
 * (Chrome at six), which bounds how many are worth having.
 */
export function engine(options: BootOptions = {}): Promise<ClaustersServer> {
    return boot(options);
}

async function boot(options: BootOptions): Promise<ClaustersServer> {
    const raw = await bootClausters(options);
    // Listeners are kept per client tag: a reply carries the tag of whoever
    // asked for it, so it reaches that client's listeners and nobody else's.
    const listeners = new Map<number, Set<ReplyListener>>();
    let nextPeer = DEFAULT_PEER;
    /** The one close, remembered -- see `close` below. */
    let closing: Promise<void> | null = null;
    /** The clients in other realms, each told when the engine goes. */
    const links = new Set<EngineLink>();
    // However the context closes -- `close` below or a caller closing it
    // directly -- a client in another realm cannot see it, so it is told.
    raw.context.addEventListener("statechange", () => {
        if (raw.context.state === "closed") for (const link of [...links]) link.close();
    });
    raw.onReply = (bytes, peer) => {
        const mine = listeners.get(peer);
        if (mine) for (const listener of [...mine]) listener(bytes);
        const observers = listeners.get(ANY_PEER);
        if (observers) for (const listener of [...observers]) listener(bytes);
    };
    raw.onError = (message) => console.error(`clausters engine: ${message}`);
    raw.onQuit = () => console.warn("clausters engine: /server_quit -- engine stopped");
    return {
        context: raw.context,
        node: raw.node,
        send: (bytes, peer = DEFAULT_PEER) => raw.send(bytes, peer),
        addReply: (listener, peer = DEFAULT_PEER) => {
            let mine = listeners.get(peer);
            if (!mine) listeners.set(peer, (mine = new Set()));
            mine.add(listener);
        },
        removeReply: (listener, peer = DEFAULT_PEER) => {
            listeners.get(peer)?.delete(listener);
        },
        claimPeer: () => ++nextPeer,
        link: () => {
            // Two channels: this page's to the client, which is the one port
            // the caller transfers, and the engine's own (the worklet's end
            // and the client's), which the first carries when the client asks
            // for it -- and only while the link is open, so a link closed
            // before anyone used it gives nothing.
            const peer = ++nextPeer;
            const control = new MessageChannel();
            let open = true;
            let lent = false;
            const link: EngineLink = {
                port: control.port2,
                peer,
                close: () => {
                    if (!open) return;
                    open = false;
                    links.delete(link);
                    if (lent) raw.release(peer);
                    // Not closed after it: a port closed in the same turn may
                    // drop what was just posted to it, and this is the one
                    // message the client cannot learn otherwise.
                    control.port1.postMessage({ type: "gone" } satisfies LinkMessage);
                },
            };
            control.port1.onmessage = (event: MessageEvent) => {
                if ((event.data as { type?: string }).type !== "open" || lent) return;
                if (!open) {
                    control.port1.postMessage({ type: "gone" } satisfies LinkMessage);
                    return;
                }
                lent = true;
                const engine = raw.lend(peer);
                control.port1.postMessage(
                    { type: "port", port: engine, peer } satisfies LinkMessage,
                    [engine],
                );
            };
            links.add(link);
            return link;
        },
        clock: () => raw.clock(),
        clockAnchor: () => raw.clockAnchor(),
        bufferLoad: (index, channels, sampleRate, samples) =>
            raw.bufferLoad(index, channels, sampleRate, samples),
        resume: () => raw.context.resume(),
        suspend: () => raw.context.suspend(),
        close: () => {
            // Idempotent on purpose, and idempotent against **overlap** rather
            // than only against a second call afterwards: a session closes the
            // server it owns (which quits the engine it booted) and then the
            // engine itself, without awaiting the first -- so the second call
            // arrives while `context.close()` is still in flight, when the
            // state is not yet `"closed"` and a state check waves it through.
            // An `AudioContext` refuses the second close outright ("Can't
            // close an AudioContext twice"), and since nobody awaited it, it
            // surfaced as an unhandled rejection in the page's console at the
            // end of an otherwise finished run. Remembering the promise is
            // what makes both callers wait on one close.
            closing ??= (async () => {
                if (raw.context.state === "closed") return;
                for (const link of [...links]) link.close();
                listeners.clear();
                raw.dispose();
                await raw.context.close();
            })();
            return closing;
        },
    };
}
