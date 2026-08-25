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
import { ScoreConnection } from "../../base/connection.ts";
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
} from "./options.ts";
export type { ServerInfo, ServerSizing } from "./options.ts";
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

interface Pending {
    match: (msg: OscMessage) => boolean;
    resolve: (msg: OscMessage) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
}

/** The mixin surface, merged so `server.queryTree(...)` types as its own. */
export interface Server extends ServerQueries, ServerStreams, ServerTransport {}

export class Server {
    readonly connection: Connection;
    /**
     * The slice of the server's client id space this handle allocates from —
     * the whole of it unless a second client shares the server (`IdShare`).
     */
    readonly share: IdShare;
    /** The sizes this client's allocators were built against. */
    readonly sizing: ServerSizing;
    readonly nodes: NodeIdAllocator;
    readonly audioBuses: AudioBusAllocator;
    readonly controlBuses: ControlBusAllocator;
    readonly buffers: BufferAllocator;
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
        return this.connection.timeMode === "score";
    }
    /**
     * How long a reply is waited for, in seconds, when a call does not say.
     * The handle's, not a literal repeated at each call site: every method
     * taking a `timeout` leaves it optional and an absent one resolves here,
     * so `server.timeout = 30` moves them all at once.
     */
    timeout = DEFAULT_TIMEOUT;

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
    readonly receiver: OscReceiver;
    private readonly listener: OscHandler;

    private constructor(
        connection: Connection,
        sizing: ServerSizing,
        timeout: number,
        share: IdShare = WHOLE_SHARE,
    ) {
        this.connection = connection;
        this.sizing = sizing;
        this.timeout = timeout;
        this.share = share;
        // An offline score has no `/node_end` stream to recycle from and no
        // real-time bound on how many ids its length needs, so the registry is
        // unbounded there — the reference client's rule.
        this.nodes = this.scoring
            ? NodeIdAllocator.unbounded(sizing.maxNodes)
            : NodeIdAllocator.forMaxNodes(sizing.maxNodes, share);
        if (this.scoring) this.latency = 0.0;
        this.audioBuses = new AudioBusAllocator(sizing.audioBuses, sizing.channels, share);
        this.controlBuses = new ControlBusAllocator(sizing.controlBuses, share);
        this.buffers = new BufferAllocator(sizing.maxBuffers, share);
        this.receiver = new OscReceiver(connection);
        this.listener = (addr, args) => this.dispatch({ addr, args });
        this.receiver.add(this.listener);
    }

    /**
     * Opens a server over `connection`.
     *
     * With no `sizing` it asks the server for its own (`/server_query`), so
     * the allocators match the server that is actually running; a server
     * that does not answer within `timeout` leaves the compiled defaults in
     * place. `notify` (default `true`) registers for the server's pushes,
     * which is what recycles node ids as their `/node_end` arrives.
     *
     * `timeout` becomes the handle's own default (`Server.timeout`), and is
     * what the opening round trips are given.
     *
     * The core wasm must be loaded first (`await loadOsc()`).
     */
    static async open(
        connection: Connection,
        {
            sizing,
            notify = true,
            verify = false,
            timeout = DEFAULT_TIMEOUT,
            share = WHOLE_SHARE,
            adoptDefault = true,
        }: {
            sizing?: Partial<ServerSizing>;
            notify?: boolean;
            /**
             * Require a server to actually answer, instead of falling back to
             * the compiled sizing when it does not. This is the browser's half
             * of the reference client's `Server.attach`: a carrier can be open
             * with nothing behind it — a WebSocket endpoint that accepts but
             * speaks no OSC, a port wired to an engine that never came up — and
             * without this the handle is built anyway and every later command
             * leaves without a trace. With it, that is a `ServerError` here.
             *
             * It forces the `/server_query` round trip even when `sizing` is
             * given, since what is being verified is the server, not the
             * numbers.
             */
            verify?: boolean;
            timeout?: number;
            /**
             * The slice of the server's client id space this handle allocates
             * from, when the server has **more than one client** — a script
             * authoring beside a page of its own, an embedder holding two.
             * Each client is given its own index over the same `of`, and the
             * slices are disjoint by arithmetic (see `IdShare`). The default
             * takes the whole space, which is right for a server's only
             * client.
             */
            share?: IdShare;
            /**
             * Make this the **default session's** server when there is none, so
             * the free-standing verbs (`play`, `render`, a bare `new Synth`)
             * resolve it with nothing else wired. A server already adopted is
             * not displaced: whichever claimed the slot first keeps it.
             *
             * This is the reference client's `adopt_default` on `Server.boot`
             * and `Server.attach`, under the verb a page has: there is no
             * process to spawn here, so opening the carrier *is* the boot.
             */
            adoptDefault?: boolean;
        } = {},
    ): Promise<Server> {
        const defaults: ServerSizing = {
            audioBuses: DEFAULT_AUDIO_BUSES,
            controlBuses: DEFAULT_CONTROL_BUSES,
            maxNodes: DEFAULT_MAX_NODES,
            maxBuffers: DEFAULT_MAX_BUFFERS,
            channels: 2,
            taps: DEFAULT_TAPS,
        };
        // A provisional server, so the query below goes through the same
        // reply dispatch every other command uses; the real sizing replaces
        // it once the answer is in.
        const probe = new Server(connection, defaults, timeout);
        let resolved: ServerSizing = { ...defaults, ...sizing };
        // Whether the carrier is open with nothing answering behind it.
        let silent = false;
        if (!sizing || verify) {
            try {
                const info = await probe.queryInfo(timeout);
                // Explicit sizing still wins; the query was for the answer's
                // existence, not its numbers.
                if (!sizing) {
                    resolved = {
                        audioBuses: info.audioBuses,
                        controlBuses: info.controlBuses,
                        maxNodes: info.maxNodes,
                        maxBuffers: info.maxBuffers,
                        channels: info.channels,
                        taps: info.taps,
                    };
                }
            } catch (error) {
                if (!(error instanceof ReplyTimeout)) throw error;
                silent = true;
            }
        }
        probe.close();
        if (silent) {
            if (verify) {
                throw new ServerError(
                    `no server answers on ${connection.url ?? "this carrier"} — ` +
                        `nothing replied to /server_query within ${timeout}s`,
                );
            }
            console.warn(
                "clausters: no /server_query reply; sizing the allocators " +
                    "from the compiled defaults",
            );
        }
        const server = new Server(connection, resolved, timeout, share);
        if (notify) await server.notify(true, timeout);
        if (adoptDefault) main.server ??= server;
        return server;
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
     * The live counters (`/server_status`): `[unused, ugens, synths, groups, defs,
     * avgCpu, peakCpu, nominalSr, actualSr]`.
     */
    async status(timeout?: number): Promise<(number | string | boolean | null | Uint8Array)[]> {
        const msg = await this.request("/server_status", [], {
            expect: ["/server_status.reply"],
            timeout,
        });
        return msg.args;
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
     * Stops the server (`/server_quit`). Over the in-page carrier this stops the
     * page's engine, which nothing restarts.
     */
    quit(): void {
        this.sendMsg("/server_quit");
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
        this.receiver.remove(this.listener);
        this.receiver.stop();
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
