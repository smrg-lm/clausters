// The audio server, driven over the W0 carrier seam (mirrors
// `clausters/defs/server/`).
//
// A `Server` is the only object that knows a connection: defs, nodes, buses
// and buffers are built transport-agnostically and reach the server through
// here. Which carrier is underneath — the in-page engine or a `--ws` native
// server — is `Connection`'s business and never named above it.
//
// **Everything that waits is a promise.** Where the Python client blocks a
// thread on a reply (`sync`, `add_synthdef(wait=True)`), this one returns a
// promise: the browser has one thread and the page must keep running. So the
// discipline the Python client states as "never block in a routine" is here
// simply `await`.
//
// **Argument typing.** A JS number is a double, with no int/float
// distinction, so this module tags by **position**, not by value: node ids,
// bus indices, buffer numbers and add actions go out as int32, control values
// as float32 — which is what each of them is. The free-form `sendMsg` infers
// (an integral number is an int32) and takes an explicit `[tag, value]` pair
// wherever that guess is wrong.
//
// **Where things live.** This module holds the `Server` itself — the
// connection, the allocators, the raw OSC paths, the request machinery and the
// server's own lifecycle. Beside it, `options` (the configuration it is sized
// from and the configuration it reports), `queries` (what a running server
// holds), `streams` (the subscriptions the server pushes) and `transport` (the
// shared beat grid, and the group it governs) — the same split the Python
// package makes, as mixins rather than collaborators precisely so no attribute
// path moves.
//
// The sample-clock tracking a `sampleTimebase()` sets up is not here either:
// it is `defs/clocksync.ts`, one class per carrier, as in the Python client.
// This handle only resolves which one and keeps it.

import {
    encodeBundle,
    encodeImmediateBundle,
    encodeMessage,
    oscArg,
    toBundle,
} from "../../base/osc.ts";
import type { MsgArg, OscMessage, TimedMessage } from "../../base/osc.ts";
import { ROOT_NODE_ID } from "../node.ts";
export type { TimedMessage } from "../../base/osc.ts";
import type { Connection } from "../../base/connection.ts";
import { pageConnection, ScoreConnection, WsConnection } from "../../base/connection.ts";
import { engine as ownEngine, pageEngineIfUp } from "../../engine/server.ts";
import type { ClaustersServer } from "../../engine/server.ts";
import { loadCore } from "../../base/core.ts";
import { OscReceiver } from "../../base/receiver.ts";
import type { OscHandler } from "../../base/receiver.ts";
import { OscFunc } from "../../responders.ts";
import type { RenderOptions, RenderStats } from "../../render.ts";
import { main } from "../../base/main.ts";
import { Moment } from "../../base/moment.ts";
import { MonotonicTimebase, SampleTimebase } from "../../base/timebase.ts";
import type { Timebase } from "../../base/timebase.ts";
import { sampleClockFor } from "../clocksync.ts";
import type { ServerSampleClock } from "../clocksync.ts";
import type { TempoClock } from "../../base/clock.ts";
import type { Event } from "../../seq/event.ts";
import { CommandError, ReplyTimeout, ServerError } from "../../errors.ts";
import { NodeIdAllocator } from "../node.ts";
import { WHOLE_SHARE } from "../../base/core.ts";
import type { IdShare } from "../../base/core.ts";
import { AudioBusAllocator, ControlBusAllocator } from "../bus.ts";
import { BufferAllocator } from "../buffer.ts";
import {
    DEFAULT_AUDIO_BUSES,
    DEFAULT_CONTROL_BUSES,
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_NODES,
    DEFAULT_TAPS,
} from "./options.ts";
import type { ServerSizing } from "./options.ts";
import { ServerQueries } from "./queries.ts";
import { ServerStreams } from "./streams.ts";
import { ServerTransport } from "./transport.ts";

// The package's public surface: `Server` plus what its configuration is made
// of. The names re-exported here are the ones the module answered to before it
// became a package, so importing from `defs/server` is unchanged.
export {
    DEFAULT_AUDIO_BUSES,
    DEFAULT_CONTROL_BUSES,
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_GRAPH_CHILDREN,
    DEFAULT_MAX_NODES,
    DEFAULT_MAX_UGEN_INPUTS,
    DEFAULT_SAMPLE_RATE,
    DEFAULT_TAP_FRAMES,
    DEFAULT_TAPS,
    formatServerInfo,
    formatServerStatus,
} from "./options.ts";
export type { ServerInfo, ServerSizing, ServerStatus } from "./options.ts";
import type { ServerStatus } from "./options.ts";
export { ServerQueries } from "./queries.ts";
export { ServerStreams } from "./streams.ts";
export { ServerTransport } from "./transport.ts";
export type { TransportGrid, TransportState } from "./transport.ts";

/**
 * A plain value a message argument may take, or an explicit `[tag, value]`
 * pair when the inferred type is wrong (the codec's own type — re-exported
 * here, where the commands take it).
 */
export type { MsgArg };

/** The handle's default reply timeout, in seconds. */
const DEFAULT_TIMEOUT = 5.0;

/** Where `clausters --ws` listens, when a handle names no address. */
const DEFAULT_WS_URL = "ws://127.0.0.1:57120";

/**
 * Where a handle's server is. The reference client's `transport`, with the
 * values a browser actually has: it can run the server in this tab, and it can
 * reach one over a WebSocket. It has no TCP and no UDP, which is why those two
 * names are absent rather than renamed.
 */
export type ServerTransportName = "page" | "ws";

/**
 * What {@link Server} is built with — the reference client's constructor, in
 * the shape a page takes it.
 */
export interface ServerOptions {
    /** Where the server is; `"page"` (the default) or `"ws"`. */
    transport?: ServerTransportName;
    /** The `clausters --ws` address, when `transport` is `"ws"`. */
    url?: string;
    /**
     * *Which* in-page engine, when it is not this page's own — the address of
     * a server in a tab, where the reference client uses a port. Read it off a
     * handle that booted one ({@link Server.engine}) to point a second handle
     * at the same server, which is what `attach` is for.
     */
    engine?: ClaustersServer;
    /**
     * A carrier built by hand, which wins over `transport` — the reference
     * client's `interface=`. This is how an offline handle is made
     * (`new Server({ connection: new ScoreConnection() })`, the page's
     * `Server(interface=OscNrtInterface())`) and how a handle is pointed at one
     * particular engine among several.
     */
    connection?: Connection;
    /** Seconds of scheduling headroom added to every timed send. */
    latency?: number;
    /** The server's capacities, when they are known without asking. */
    sizing?: Partial<ServerSizing>;
    /** The handle's own default reply wait, in seconds. */
    timeout?: number;
    /** This handle's slice of the id space, for a shared server. */
    share?: IdShare;
}

interface Pending {
    match: (msg: OscMessage) => boolean;
    resolve: (msg: OscMessage) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
}

/** The mixin surface, merged so `server.queryTree(...)` types as its own. */
export interface Server extends ServerQueries, ServerStreams, ServerTransport {}

/**
 * What {@link Server.boot} and {@link Server.attach} take: the same three
 * questions either way, since what differs between them is where the server
 * comes from and not what the handle does about it.
 */
export interface ServerBootOptions {
    /**
     * Resize this handle's allocators from the server's own capacities
     * ({@link Server.reconcile}). Worth leaving on: a server this page did not
     * bring up may have been started with other flags, and allocators sized
     * from the wrong numbers hand out node ids and buses it does not have.
     */
    reconcile?: boolean;
    /** Register for the server's pushes, which is what recycles node ids. */
    notify?: boolean;
    /** The wait for the opening round trips; the handle's own by default. */
    timeout?: number;
    /**
     * Make this the **default session's** server when there is none, so the
     * free-standing verbs (`play`, `render`, a bare `new Synth`) resolve it
     * with nothing else wired. First-wins: a server already adopted is not
     * displaced. The reference client's `adopt_default`.
     */
    adoptDefault?: boolean;
}

export class Server {
    // Where this handle's server is, and its address — the constructor's
    // `transport`/`url`. Private, and named apart from the option, because
    // `Server.transport()` is already the transport grid's query; the reference
    // client keeps its own `transport=` out of the instance for the same
    // reason, the argument choosing a carrier rather than naming a field.
    private readonly carrierKind: ServerTransportName;
    private readonly carrierUrl: string;
    private conn: Connection | null;
    private audio: ClaustersServer | null;

    /**
     * The in-page engine this handle talks to, once it has one — `null` over a
     * socket or a score, where there is no engine in this tab to name.
     *
     * This is a server's *address* in a page: hand it to a second handle
     * (`new Server({ engine: first.engine }).attach()`) to reach the same
     * server, the way the reference client hands a second handle the port.
     */
    get engine(): ClaustersServer | null {
        return this.audio;
    }
    private recv: OscReceiver | null = null;
    /** Whether `boot` brought up the engine under this handle, so `quit` ends it. */
    private ownsEngine = false;

    /**
     * The carrier this handle talks over.
     *
     * A handle opens it in `boot`/`attach` (or is handed one), so asking before
     * either says which of the two is missing rather than returning something
     * that reaches nothing.
     */
    get connection(): Connection {
        if (!this.conn) {
            throw new ServerError(
                "this handle has no carrier yet — boot() one, attach() to a " +
                    "server already running, or build it with an explicit " +
                    "`connection`.",
            );
        }
        return this.conn;
    }
    /**
     * The slice of the server's client id space this handle allocates from —
     * the whole of it unless a second client shares the server (`IdShare`).
     */
    readonly share: IdShare;
    /**
     * The sizes this client's allocators were built against — the constructor's
     * guess until {@link Server.reconcile} replaces it with the server's own
     * answer, which is why neither this nor the allocators under it is `readonly`.
     */
    sizing: ServerSizing;
    nodes: NodeIdAllocator;
    audioBuses: AudioBusAllocator;
    controlBuses: ControlBusAllocator;
    buffers: BufferAllocator;
    /**
     * Seconds added to every timed send — the scheduling headroom. Kept here
     * so the sequencing layer (a later milestone) has one place to read it.
     *
     * A **score** carrier sets it to 0 in the constructor: latency is lead
     * time against a real deadline, and an offline render has none.
     */
    latency = 0.05;

    /**
     * Whether this handle writes a score instead of sending to a server —
     * the carrier's `timeMode`, read where the difference shows: how a bundle
     * is stamped, whether a confirmation can ever arrive, and whether node
     * ids have to be recycled.
     */
    get scoring(): boolean {
        return this.conn?.timeMode === "score";
    }
    /**
     * How long a reply is waited for, in seconds, when a call does not say.
     * The handle's, not a literal repeated at each call site: every method
     * taking a `timeout` leaves it optional and an absent one resolves here,
     * so `server.timeout = 30` moves them all at once.
     */
    timeout = DEFAULT_TIMEOUT;

    /**
     * Whether {@link Server.boot} brought this server up — ownership, the way
     * the reference client remembers the process it started. `close` leaves a
     * booted server standing exactly as it leaves an attached one; what reads
     * this is {@link Server.quit}'s message and anything asking whether this
     * page owns what it is talking to.
     */
    booted = false;

    private pending = new Set<Pending>();
    private handlers = new Set<(msg: OscMessage) => void>();
    private syncCounter = 0;
    /** The transport's frame ceiling, read once and cached (`bulkChunk`). */
    private maxFrame: number | null = null;
    /**
     * The receiving door this server's connection is read through — the one
     * place a packet is decoded, and the receiver a responder registers with
     * (`new OscFunc(fn, "/node_start", { recv: server.receiver })`, which is
     * also what the ambient default resolves to).
     *
     * The server's own reply handling is one handler on it, so what this
     * handle waits for and what a page's responders match arrive the same way
     * and in the same order.
     */
    get receiver(): OscReceiver {
        if (!this.recv) {
            throw new ServerError(
                "this handle has no receiving door yet — it opens with the " +
                    "carrier, in boot() or attach().",
            );
        }
        return this.recv;
    }
    private readonly listener: OscHandler;

    /**
     * A handle on a server. **Reaches nothing**: it builds the allocators and
     * sends no packet, exactly as the reference client's `Server(...)` does,
     * which is what makes it cheap to name a server before there is one to
     * talk to.
     *
     * `transport` names where that server is, the way the reference names it
     * with a transport and an address, and the handle opens the carrier itself
     * when a verb needs it — asynchronously, which a constructor cannot be but
     * {@link Server.boot} and {@link Server.attach} already are:
     *
     * - `"page"` (the default) — the audio server compiled to wasm in this tab.
     * - `"ws"` — a `clausters --ws` server at `url`.
     *
     * Then one of the two verbs, and **they are what say who owns what**, as in
     * the reference: {@link Server.boot} brings up a server this handle owns —
     * so two booted handles are two servers, not two views of one — and
     * {@link Server.attach} connects to one already running and owns nothing.
     * With neither, the allocators keep `sizing` (the compiled defaults when it
     * is absent), which is right only for a server whose configuration you
     * already know — an offline score, or one you sized yourself.
     */
    constructor(
        {
            transport = "page",
            url = DEFAULT_WS_URL,
            engine,
            connection,
            latency,
            sizing,
            timeout = DEFAULT_TIMEOUT,
            share = WHOLE_SHARE,
        }: ServerOptions = {},
    ) {
        this.carrierKind = transport;
        this.carrierUrl = url;
        this.audio = engine ?? null;
        this.conn = connection ?? null;
        this.sizing = {
            audioBuses: DEFAULT_AUDIO_BUSES,
            controlBuses: DEFAULT_CONTROL_BUSES,
            maxNodes: DEFAULT_MAX_NODES,
            maxBuffers: DEFAULT_MAX_BUFFERS,
            channels: 2,
            taps: DEFAULT_TAPS,
            ...sizing,
        };
        this.timeout = timeout;
        this.share = share;
        // An offline score has no `/node_end` stream to recycle from and no
        // real-time bound on how many ids its length needs, so the registry is
        // unbounded there — the reference client's rule.
        const resolved = this.sizing;
        this.nodes = this.scoring
            ? NodeIdAllocator.unbounded(resolved.maxNodes)
            : NodeIdAllocator.forMaxNodes(resolved.maxNodes, share);
        if (this.scoring) this.latency = 0.0;
        this.audioBuses = new AudioBusAllocator(
            resolved.audioBuses, resolved.channels, share);
        this.controlBuses = new ControlBusAllocator(resolved.controlBuses, share);
        this.buffers = new BufferAllocator(resolved.maxBuffers, share);
        if (latency !== undefined) this.latency = latency;
        this.listener = (addr, args) => this.dispatch({ addr, args });
        if (this.conn) this.openReceiver(this.conn);
    }

    /**
     * Wires the receiving door onto a carrier, once it exists — from the
     * constructor when one was handed in, else from `boot`/`attach` when the
     * handle opens its own.
     */
    private openReceiver(connection: Connection): void {
        this.recv = new OscReceiver(connection);
        this.recv.add(this.listener);
    }

    /**
     * Opens this handle's carrier, or returns the one it already has.
     *
     * `own` is the difference between the two verbs and is the whole of it: a
     * boot gets an engine of its own (its own `AudioContext`, its own node, bus
     * and buffer space), an attach the one already running in this page. A
     * socket is neither — it points at a server this page did not start and
     * cannot, so `boot` refuses it before we get here.
     */
    private async openCarrier(own: boolean): Promise<Connection> {
        if (this.conn) return this.conn;
        await loadCore();
        if (this.carrierKind === "ws") {
            this.conn = await WsConnection.open(this.carrierUrl);
        } else {
            const found = this.audio ?? (own ? ownEngine() : pageEngineIfUp());
            if (!found) {
                throw new ServerError(
                    "no engine is running in this page — attach() is for a " +
                        "server already up, and nothing has booted one here. " +
                        "boot() one instead, which brings up its own.",
                );
            }
            this.ownsEngine = own && !this.audio;
            this.audio = await found;
            this.conn = await pageConnection(this.audio);
        }
        this.openReceiver(this.conn);
        return this.conn;
    }

    // ---- coming up, and going down ----

    /**
     * Bring up the server this handle points at, and return `this`.
     *
     * What that means is the **carrier's** to say, exactly as in the reference
     * client, where a socket has a process behind it and an offline interface
     * has nothing to start:
     *
     * - `transport: "page"` brings up an **engine of this handle's own** — its
     *   own `AudioContext`, its own node, bus and buffer space. So two booted
     *   handles are two servers that share nothing, which is what
     *   `Server().boot()` and `Server(port=57130).boot()` are in the reference
     *   client, and neither needs an id share against the other. The context is
     *   created suspended, so this starts its audio and must be reached from a
     *   gesture (a click handler) like any other;
     * - a score has nothing to start and boots as a no-op rather than an error;
     * - `transport: "ws"` points at a machine this page cannot spawn anything
     *   on, so it refuses and names `attach`, the way the reference client
     *   refuses to boot a handle pointing at another host.
     *
     * **The page's shared engine is not what this returns**, and that is the
     * point of the pair: components on one page belong to one mix, and reaching
     * that one is {@link Server.attach} — the verb for a server this handle did
     * not start. Booting where one is already up is a second server here rather
     * than the reference's refusal, because a tab can hold several engines
     * where a machine holds one server per port; what does not change is that a
     * boot never silently hands you somebody else's.
     *
     * Then the allocators are reconciled with the server that came up and the
     * handle registers for its pushes, which is what recycles node ids as each
     * `/node_end` arrives.
     *
     * Pair it with {@link Server.quit}, which takes down what this booted;
     * {@link Server.close} only lets go of the carrier.
     */
    async boot({
        reconcile = true,
        notify = true,
        timeout = this.timeout,
        adoptDefault = true,
    }: ServerBootOptions = {}): Promise<this> {
        if (this.carrierKind === "ws" && !this.conn) {
            throw new ServerError(
                `this handle points at ${this.carrierUrl} and a page can start nothing ` +
                    "there — attach() to the server running at that address, or " +
                    "boot() one with the default transport, which brings up an " +
                    "engine in this tab.",
            );
        }
        const connection = await this.openCarrier(true);
        if (!connection.boot) {
            throw new ServerError(
                `this carrier goes to ${connection.url ?? "somewhere"} and a ` +
                    "page can start nothing there — attach() to the server " +
                    "running at that address.",
            );
        }
        await connection.boot();
        this.booted = true;
        if (reconcile) await this.reconcile(timeout);
        if (notify) await this.notify(true, timeout);
        if (adoptDefault) main.server ??= this;
        return this;
    }

    /**
     * Connect this handle to a server **already running**, and return `this`.
     *
     * The other half of `boot`, for the server nobody here started: one on
     * another machine, one launched from a terminal, one another tab owns.
     * Ownership is the difference and it runs through the pair — this handle
     * did not start that server, so `close` releases the carrier and leaves it
     * standing, and stopping it is `quit`, which it obeys over the wire.
     *
     * Unlike a bare `new Server(...)`, this **verifies**: a carrier open with
     * nothing behind it (a socket that accepts but speaks no OSC, a port wired
     * to an engine that never came up) throws here instead of swallowing every
     * later message. That refusal is the reference client's `attach`, and it is
     * not optional — it is what the verb is for.
     */
    async attach({
        reconcile = true,
        notify = true,
        timeout = this.timeout,
        adoptDefault = true,
    }: ServerBootOptions = {}): Promise<this> {
        await this.openCarrier(false);
        try {
            await this.queryInfo(timeout);
        } catch (error) {
            if (!(error instanceof ReplyTimeout)) throw error;
            throw new ServerError(
                `no server answers on ${this.connection.url ?? "this carrier"} — ` +
                    `nothing replied to /server_query within ${timeout}s. ` +
                    "boot() one, or point this handle where one is running.",
            );
        }
        if (reconcile) await this.reconcile(timeout);
        if (notify) await this.notify(true, timeout);
        if (adoptDefault) main.server ??= this;
        return this;
    }

    /**
     * Resize this handle's allocators from what the running server reports
     * (`/server_query`), and return `this`.
     *
     * The constructor sizes them from what it was told, which is the client's
     * *own* picture of the server — right by construction for one this page
     * booted, a guess for any other. This replaces the guess with the answer,
     * and `boot`/`attach` call it for you.
     *
     * Reconciling **rebuilds** the allocators, so it belongs before anything
     * has been allocated: a handle that has already handed out node ids would
     * hand out the same ones again.
     */
    async reconcile(timeout = this.timeout): Promise<this> {
        const info = await this.queryInfo(timeout);
        this.sizing = {
            audioBuses: info.audioBuses,
            controlBuses: info.controlBuses,
            maxNodes: info.maxNodes,
            maxBuffers: info.maxBuffers,
            channels: info.channels,
            taps: info.taps,
        };
        this.nodes = this.scoring
            ? NodeIdAllocator.unbounded(this.sizing.maxNodes)
            : NodeIdAllocator.forMaxNodes(this.sizing.maxNodes, this.share);
        this.audioBuses = new AudioBusAllocator(
            this.sizing.audioBuses, this.sizing.channels, this.share);
        this.controlBuses = new ControlBusAllocator(this.sizing.controlBuses, this.share);
        this.buffers = new BufferAllocator(this.sizing.maxBuffers, this.share);
        return this;
    }

    // ---- the reply stream ----

    /**
     * One decoded message off the receiver: what a pending request is waiting
     * for, then what `onReply` subscribed to. Decoding happened once, at the
     * door.
     */
    private dispatch(msg: OscMessage): void {
        for (const p of [...this.pending]) {
            if (p.match(msg)) {
                this.pending.delete(p);
                clearTimeout(p.timer);
                p.resolve(msg);
            }
        }
        for (const handler of [...this.handlers]) handler(msg);
    }

    /**
     * Subscribes to every decoded reply message; returns the unsubscribe.
     *
     * The raw seam under the responders: it sees everything, in arrival order,
     * with no matching of its own. To respond to *one* address, `OscFunc` is
     * the door (`new OscFunc(fn, "/node_end", { recv: server.receiver })`) —
     * it filters by address, sender and arguments, and it is what the reference
     * client offers under the same name.
     */
    onReply(handler: (msg: OscMessage) => void): () => void {
        this.handlers.add(handler);
        return () => this.handlers.delete(handler);
    }

    /**
     * Resolves with the first reply message `match` accepts, or rejects with
     * `ReplyTimeout` after `timeout` seconds (absent: the handle's). Registered
     * *before* whatever send provokes the reply, so a fast server cannot outrun
     * it.
     *
     * This and `requestBatch` are the two places a `timeout` is finally
     * resolved: everything above just passes the argument down.
     */
    awaitReply(
        match: (msg: OscMessage) => boolean,
        timeout?: number,
        what = "a reply",
    ): Promise<OscMessage> {
        const secs = timeout ?? this.timeout;
        return new Promise((resolve, reject) => {
            const entry: Pending = {
                match,
                resolve,
                reject,
                timer: setTimeout(() => {
                    this.pending.delete(entry);
                    reject(new ReplyTimeout(`no ${what} within ${secs}s`));
                }, secs * 1000),
            };
            this.pending.add(entry);
        });
    }

    // ---- raw OSC ----

    /**
     * Sends one message. **A message has no time**: in a bundle it would
     * carry the immediate timetag, and alone it means exactly that. Logical
     * time belongs to the bundle path, which a later milestone brings; use
     * this for what has no place in a timeline — sending defs, allocating
     * buffers, opening the groups a piece is built on.
     */
    sendMsg(addr: string, ...args: MsgArg[]): void {
        if (this.scoring) {
            // A message has no time, so in a score it lands at the top —
            // which is exactly what "no time" means for a render.
            this.connection.addBundle!(0, [{ addr, args: args.map(oscArg) }]);
            return;
        }
        this.connection.send(encodeMessage(addr, args.map(oscArg)));
    }

    // ---- timed sends: the bundle path ----
    //
    // Where `sendMsg` means "now", these carry a *time*. The time is the
    // running routine's exact logical beat — yield-accumulated, never
    // wall-clock — so a sequence stays tight however late the wake-up was, and
    // `latency` is the headroom that absorbs that lateness.

    /**
     * Emits a bundle of messages at `at` (default: the ambient `Moment`) plus
     * `delayBeats`, plus this server's `latency`.
     *
     * Inside a routine the moment is the routine's **exact logical beat** —
     * the yield-accumulated one, never wall-clock — so a sequence stays tight
     * however late the wake-up was. Outside any routine it is wall-clock now,
     * and the delay reads as seconds.
     *
     * What this adds to a plain OSC bundle is what belongs to *this* server:
     * its `latency`, and scheduling by absolute sample (`/sched_at`) when the
     * clock is anchored to the server's own — drift-free and exact to the
     * sample. For any other application, `OscDestination` sends standard
     * bundles with the same logical timing.
     */
    sendBundle(
        messages: readonly TimedMessage[],
        { delayBeats = 0, clock, at }: {
            delayBeats?: number;
            clock?: TempoClock;
            at?: Moment;
        } = {},
    ): void {
        const when = (at ?? Moment.current(clock)).at(delayBeats);
        if (this.scoring) {
            // NRT: seconds from the render's start — logical, and independent
            // of any timebase, since no wall clock is involved.
            this.connection.addBundle!(when.secs(), toBundle(messages));
            return;
        }
        const timebase = when.clock?.timebase;
        if (timebase instanceof SampleTimebase) {
            // Anchored to the server's sample clock: schedule by absolute
            // sample. The seconds->sample rounding is the core's, shared with
            // the server.
            const origin = when.clock!.pacingOrigin ?? 0;
            this.sendSched(
                timebase.sampleAt(origin + when.secs() + this.latency),
                messages,
            );
            return;
        }
        this.sendTimetagged(when.instant() + this.latency, messages);
    }

    /**
     * Emits a bundle at wall-clock now + `delaySecs` (+ `latency`), ignoring
     * whatever clock is in flight — the **clockless** entry point to
     * `sendBundle`, for a delay that is a duration in seconds rather than a
     * position in the music.
     */
    sendBundleAfter(delaySecs: number, messages: readonly TimedMessage[]): void {
        this.sendBundle(messages, { at: new Moment(null, delaySecs) });
    }

    private sendTimetagged(unixSecs: number, messages: readonly TimedMessage[]): void {
        this.connection.send(encodeBundle(unixSecs, toBundle(messages)));
    }

    /**
     * `/sched_at <absolute sample> <packet>`: the sample-exact path. The inner
     * bundle is immediate — the outer command's own target carries the time.
     */
    private sendSched(sample: number, messages: readonly TimedMessage[]): void {
        this.sendMsg("/sched_at", ["h", sample], [
            "b",
            encodeImmediateBundle(toBundle(messages)),
        ]);
    }

    /**
     * Plays a note `Event` as OSC: `/synth_new`, then its release (`gate 0` when
     * the event releases by gate, else `/node_free`) after the sustain. The OSC
     * side of the event's double dispatch. Returns the synth's node id, or
     * `null` for a rest.
     *
     * One timing path, whatever the context. Both messages go out as timed
     * bundles at the ambient `Moment`: inside a routine that is its exact
     * logical beat, so a sequence stays sample-tight; outside any clock it is
     * wall-clock now, and the sustain reads as seconds — so a single
     * `new Event().play(server)` sounds now and frees itself with no clock at
     * all.
     */
    playEvent(event: Event): number | null {
        if (event.get("type") === "rest") return null;
        const node = this.nodes.alloc();
        const sNew: TimedMessage = [
            "/synth_new",
            String(event.get("instrument")),
            ["i", node],
            ["i", Math.trunc(Number(event.get("addAction")))],
            ["i", Math.trunc(Number(event.get("target")))],
            ...event.controlArgs(),
        ];
        const release: TimedMessage = event.releasesByGate()
            ? ["/node_set", ["i", node], "gate", ["f", 0]]
            : ["/node_free", ["i", node]];
        this.sendBundle([sNew]);
        this.sendBundle([release], { delayBeats: event.sustain() });
        return node;
    }

    /**
     * Sends `addr` and resolves with the first reply whose address is in
     * `expect`. `cmd` additionally requires the reply's first argument to
     * name that command, which is what makes concurrent requests safe (the
     * server echoes the command in `/done`/`/fail`).
     */
    async request(
        addr: string,
        args: MsgArg[] = [],
        {
            expect,
            cmd,
            timeout,
        }: { expect: readonly string[]; cmd?: string; timeout?: number },
    ): Promise<OscMessage> {
        const reply = this.awaitReply(
            (msg) =>
                expect.includes(msg.addr) &&
                (cmd === undefined || msg.args[0] === cmd),
            timeout,
            `reply to ${addr}`,
        );
        this.sendMsg(addr, ...args);
        return reply;
    }

    /**
     * Sends `addr` and collects every `reply` message until the batch's
     * `/done` terminator — the shape the introspection queries take, whose
     * result is a variable number of messages.
     */
    requestBatch(
        addr: string,
        args: MsgArg[] = [],
        { reply, timeout }: { reply: string; timeout?: number },
    ): Promise<OscMessage[]> {
        const secs = timeout ?? this.timeout;
        return new Promise((resolve, reject) => {
            const collected: OscMessage[] = [];
            const timer = setTimeout(() => {
                unsubscribe();
                reject(new ReplyTimeout(`no reply to ${addr} within ${secs}s`));
            }, secs * 1000);
            const unsubscribe = this.onReply((msg) => {
                if (msg.addr === reply) {
                    collected.push(msg);
                } else if (msg.addr === "/done" && msg.args[0] === addr) {
                    clearTimeout(timer);
                    unsubscribe();
                    resolve(collected);
                } else if (msg.addr === "/fail" && msg.args[0] === addr) {
                    clearTimeout(timer);
                    unsubscribe();
                    reject(new CommandError(`${addr} failed: ${msg.args.join(" ")}`));
                }
            });
            this.sendMsg(addr, ...args);
        });
    }

    /**
     * Sends `addr` and awaits its `/done`, throwing `CommandError` on
     * `/fail`. The shape every asynchronous command answers with.
     */
    async command(
        addr: string,
        args: MsgArg[],
        timeout?: number,
    ): Promise<OscMessage> {
        if (this.scoring) {
            // Nothing answers a score. The command still goes in — it is part
            // of the piece — and the confirmation this call is named for
            // simply does not exist offline.
            this.sendMsg(addr, ...args);
            return { addr: "/done", args: [addr] };
        }
        const msg = await this.request(addr, args, {
            expect: ["/done", "/fail"],
            cmd: addr,
            timeout,
        });
        if (msg.addr === "/fail") {
            throw new CommandError(`${addr} failed: ${msg.args.slice(1).join(" ")}`);
        }
        return msg;
    }

    /**
     * `sync`, but a `/fail` from the work being waited on ends the wait instead
     * of being dropped.
     *
     * This is what a **batched** async send needs. Awaiting each command's own
     * `/done` costs one round trip per command, which is what makes a chunked
     * bulk write (`Buffer.setSamples`) slow in proportion to its length rather
     * than its size; firing the batch and closing it with one barrier costs one
     * round trip for the whole of it. What that would otherwise give up is the
     * error, since a chunk's `/fail` arrives while nobody is listening — so the
     * barrier listens for both, and the first one wins.
     */
    async barrier(timeout?: number): Promise<void> {
        const id = ++this.syncCounter;
        if (this.scoring) return;
        const reply = this.awaitReply(
            (msg) =>
                msg.addr === "/fail" ||
                (msg.addr === "/server_sync.reply" && msg.args[0] === id),
            timeout,
            `/server_sync.reply ${id}`,
        );
        this.sendMsg("/server_sync", ["i", id]);
        const msg = await reply;
        if (msg.addr === "/fail") {
            throw new CommandError(
                `${msg.args[0] ?? "a command"} failed: ${msg.args.slice(1).join(" ")}`,
            );
        }
    }

    /**
     * The async barrier (scsynth `/server_sync`): resolves only once every async
     * command sent earlier — def compiles, buffer jobs — has completed.
     * Returns the id used.
     */
    async sync(timeout?: number): Promise<number> {
        const id = ++this.syncCounter;
        // A score is not concurrent: everything in it is already ordered by
        // the time it carries, so the barrier has nothing to wait for.
        if (this.scoring) return id;
        const reply = this.awaitReply(
            (msg) => msg.addr === "/server_sync.reply" && msg.args[0] === id,
            timeout,
            `/server_sync.reply ${id}`,
        );
        this.sendMsg("/server_sync", ["i", id]);
        await reply;
        return id;
    }

    /**
     * Renders the score this handle accumulated (a **score carrier** only).
     *
     * The one surface where an offline `Server` differs from a live one, and
     * it is not a command: nothing is sent, the score is handed to the
     * engine's own renderer and the samples come back. `Session.render` drains
     * the clock first and then calls this.
     *
     * Schedule a closing bundle — freeing the root group, or whatever ends the
     * piece — so the render has a defined length: it stops when the score
     * does, and commands do not sound.
     */
    async render(options: RenderOptions = {}): Promise<RenderStats> {
        const connection = this.connection;
        if (!(connection instanceof ScoreConnection)) {
            throw new TypeError(
                "render() needs a Server opened over a ScoreConnection "
                    + "(Session.nrt() builds one)",
            );
        }
        const { renderScore } = await import("../../render.ts");
        return renderScore(connection.score.bytes(), options);
    }

    // ---- definitions ----

    /**
     * Removes defs from the server's def table by name (`/def_free`).
     *
     * A def is not freed by itself: in use it is *overwritten* by sending
     * another under the same name. This is the table's own command, for
     * reclaiming what a session no longer names.
     */
    freeDef(...names: string[]): void {
        this.sendMsg("/def_free", ...names);
    }

    // ---- bulk sizing ----

    /**
     * Samples per bulk round trip **for this carrier**: a carrier bounded by
     * one fixed-size delivery — a datagram, the page's shared ring — keeps the
     * classic 1024; a stream carrier uses the frame ceiling the server
     * advertises (`/server_query`, queried once and cached), minus headroom for
     * the reply's OSC envelope. A server that does not answer leaves the
     * conservative number too.
     *
     * Which of the two a carrier is is the carrier's own answer
     * (`Connection.stream`), never a list of types here — and it is what
     * decides whether a reply comes back at all: the ring drops one it cannot
     * hold, silently, so a chunk sized from a stream's ceiling reads nothing on
     * a page.
     */
    async bulkChunk(timeout?: number): Promise<number> {
        if (!this.connection.stream) return 1024;
        if (this.maxFrame === null) {
            try {
                this.maxFrame = (await this.queryInfo(timeout)).maxFrame;
            } catch (error) {
                if (!(error instanceof ReplyTimeout)) throw error;
                return 1024; // no reply: stay conservative, retry next call
            }
        }
        return Math.max(1024, Math.floor((this.maxFrame - 256) / 4));
    }

    // ---- server control ----

    /**
     * The live counters (`/server_status`) as a {@link ServerStatus}: what is
     * playing, what is loaded, and what the audio thread is spending.
     *
     * Structured, like every other reply this client hands back: the field
     * order is the protocol's business, not the caller's.
     * `formatServerStatus` reads it out.
     *
     * The peak CPU is the worst block **since the previous call**, so two
     * readers polling the same server each reset the other's window.
     */
    async status(timeout?: number): Promise<ServerStatus> {
        const msg = await this.request("/server_status", [], {
            expect: ["/server_status.reply"],
            timeout,
        });
        const at = (i: number): number => Number(msg.args[i]);
        return {
            ugens: at(0),
            synths: at(1),
            groups: at(2),
            defs: at(3),
            avgCpu: at(4),
            peakCpu: at(5),
            nominalSampleRate: at(6),
            actualSampleRate: at(7),
            lateBlocks: msg.args.length > 8 ? at(8) : 0,
        };
    }

    /**
     * Registers (or drops) this client for the server's pushes — `/node_end`
     * node deaths, `/node_trigger` triggers, the transport broadcasts. Registering is
     * what lets the node-id registry recycle.
     */
    async notify(flag = true, timeout?: number): Promise<void> {
        if (this.scoring) return; // no pushes to register for, and no ids to recycle
        const reply = this.awaitReply(
            (msg) => msg.addr === "/done" && msg.args[0] === "/server_notify",
            timeout,
            "/done /server_notify",
        );
        this.sendMsg("/server_notify", ["i", flag ? 1 : 0]);
        try {
            await reply;
        } catch (error) {
            if (!(error instanceof ReplyTimeout)) throw error;
            console.warn(
                "clausters: no /done for /server_notify; node ids will not recycle",
            );
            return;
        }
        if (flag) this.recycleNodeIds();
    }

    private recycling: OscFunc | null = null;

    /**
     * Returns a node id to the registry as its `/node_end` arrives — the
     * side-channel that keeps the client range from exhausting.
     *
     * It is an ordinary `OscFunc` on this server's receiver: the client's own
     * reply handling uses the door it gives a page, rather than a private one
     * beside it.
     */
    private recycleNodeIds(): void {
        if (this.recycling) return;
        this.recycling = new OscFunc(
            (msg) => this.nodes.free(Number(msg[1])),
            "/node_end",
            { recv: this.receiver },
        );
    }

    // ---- the sample clock ----

    /**
     * A timebase on **this server's** sample counter, for a clock that should
     * schedule on the server's own axis instead of a wall-clock timetag.
     *
     * The Server resolves it, because the Server is what knows the carrier:
     *
     * - **in-page** — the engine runs in this page's `AudioContext`, so one
     *   anchor fixes the integer offset between the two counters and the
     *   sample is then readable synchronously, exactly, with no drift.
     * - **over a socket** — `/clock_query` round trips feed the core's sample-clock
     *   model, which regresses local time against the server's counter. The
     *   warmup spreads `anchors` round trips `gap` seconds apart (a
     *   regression needs a span, not a burst), and `trackEvery` keeps
     *   anchoring afterwards so the slope stays fresh; `trackEvery: 0` stops
     *   after the warmup.
     *
     * A server that does not answer leaves you on wall-clock time: the
     * returned timebase is a `MonotonicTimebase` and a warning is logged, so
     * a page whose master is unreachable keeps working.
     *
     * Hand the result to a clock (`new TempoClock(2, { timebase })`); the
     * clock never talks to a server itself.
     */
    async sampleTimebase(options: {
        timeout?: number;
        anchors?: number;
        gap?: number;
        trackEvery?: number;
    } = {}): Promise<Timebase> {
        const clock = await sampleClockFor(this, options);
        if (clock === null) {
            console.warn(
                "clausters: no /clock_query reply; the clock stays on wall-clock time",
            );
            return new MonotonicTimebase();
        }
        this.clock?.close();
        this.clock = clock;
        return clock.timebase();
    }

    /** The sample clock this server is tracked by, once one is built. */
    private clock: ServerSampleClock | null = null;

    /**
     * The drift the sample-clock model has measured, in parts per million, or
     * `null` when this server is not being tracked (the in-page carrier needs
     * no model — it shares the page's audio clock).
     */
    get clockDriftPpm(): number | null {
        return this.clock?.driftPpm ?? null;
    }

    /**
     * Frees every node on the server, leaving it running and empty
     * (`/group_deepFree` on the root group) — sclang's `CmdPeriod`.
     *
     * The panic button, and the one that keeps the most: whatever is sounding
     * stops, while the server holds on to its defs and buffers. {@link quit} is
     * the heavier one (the server stops), {@link close} the client-side one
     * (this end lets go).
     */
    freeAll(): void {
        this.sendMsg("/group_deepFree", ROOT_NODE_ID);
    }

    /**
     * Stop the server — the pair of {@link Server.boot}, and the verb for a
     * server this page did not start either.
     *
     * `/server_quit` goes out first, which is the whole story for a server
     * listening on a socket: it obeys whoever sends it. Then the carrier is
     * asked to take its own server down, because the page's engine is not a
     * process listening for anything — it *is* the server, so stopping it is
     * closing its `AudioContext`, and nothing restarts it.
     *
     * {@link Server.close} is the other end: it lets this handle go and leaves
     * the server standing.
     */
    async quit(): Promise<void> {
        this.sendMsg("/server_quit");
        await this.connection.quit?.();
        this.booted = false;
    }

    /**
     * Detaches this server from its connection (the connection itself, and
     * any shared in-page engine, keep running). Pending requests reject.
     */
    close(): void {
        this.clock?.close();
        this.clock = null;
        this.recycling?.free();
        this.recycling = null;
        this.recv?.remove(this.listener);
        this.recv?.stop();
        for (const p of this.pending) {
            clearTimeout(p.timer);
            p.reject(new ReplyTimeout("the server was closed"));
        }
        this.pending.clear();
        this.handlers.clear();
    }
}

// Mixin composition: the queries and the stream subscriptions are grouped in
// their own modules but are still `Server`'s own methods, exactly as the
// Python package's mixins are — copying the prototypes is what makes
// `server.queryTree(...)` the same call it was before the split.
for (const mixin of [ServerQueries, ServerStreams, ServerTransport]) {
    for (const name of Object.getOwnPropertyNames(mixin.prototype)) {
        if (name === "constructor") continue;
        const descriptor = Object.getOwnPropertyDescriptor(mixin.prototype, name);
        if (descriptor) Object.defineProperty(Server.prototype, name, descriptor);
    }
}
