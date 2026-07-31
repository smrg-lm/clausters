// The audio server, driven over the W0 carrier seam (mirrors
// `clausters/defs/server.py`).
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

import {
    decodePacket,
    encodeBundle,
    encodeImmediateBundle,
    encodeMessage,
    oscArg,
    toBundle,
} from "../base/osc.ts";
import type { MsgArg, OscArg, OscMessage, TimedMessage } from "../base/osc.ts";
export type { TimedMessage } from "../base/osc.ts";
import type { Connection } from "../base/connection.ts";
import { Moment } from "../base/moment.ts";
import { MonotonicTimebase, SampleTimebase } from "../base/timebase.ts";
import type { Timebase } from "../base/timebase.ts";
import { SampleClockModel } from "../core/clausters_core_web.js";
import type { TempoClock } from "../base/clock.ts";
import type { Event } from "../seq/event.ts";
import { CommandError, ReplyTimeout } from "../errors.ts";
import { NodeIdAllocator, ROOT_NODE_ID, nodeId } from "./node.ts";
import type { NodeLike } from "./node.ts";
import { AudioBusAllocator, Bus, ControlBusAllocator, busIndex } from "./bus.ts";
import type { BusLike } from "./bus.ts";
import { Buffer, BufferAllocator, bufferNumber } from "./buffer.ts";
import { fetchAudio, interleave } from "../data/samples.ts";
import type { BufferLike } from "./buffer.ts";
import { DEFAULT_TAPS } from "./tap.ts";
import { SynthDef } from "./synthdef.ts";
import { FaustDef } from "./faustdef.ts";
import { GraphDef } from "./graphdef.ts";

// The server's compiled defaults — what a `/server_info` query falls back to
// when the server does not answer (or is too old to report a field).
export const DEFAULT_AUDIO_BUSES = 128;
export const DEFAULT_CONTROL_BUSES = 16384;
export const DEFAULT_SAMPLE_RATE = 48000;
export const DEFAULT_MAX_NODES = 8192;
export const DEFAULT_MAX_BUFFERS = 4096;
export const DEFAULT_MAX_GRAPH_CHILDREN = 512;
export const DEFAULT_MAX_UGEN_INPUTS = 32;

/**
 * The sizes a client's allocators need. They are a property of the *server*,
 * so `Server.open` reads them from `/server_info` rather than guessing;
 * pass them explicitly to skip that round trip.
 */
export interface ServerSizing {
    audioBuses: number;
    controlBuses: number;
    maxNodes: number;
    maxBuffers: number;
    /**
     * Hardware output channels — the audio buses reserved at the bottom of
     * the space, which the allocator never hands out.
     */
    channels: number;
    /** Audio-tap rings (`--taps`); 0 on a server with no tap region. */
    taps: number;
}

/** The static configuration a running server reports over `/server_info`. */
export interface ServerInfo extends ServerSizing {
    blockSize: number;
    nominalSampleRate: number;
    actualSampleRate: number;
    inputChannels: number;
    maxGraphChildren: number;
    maxUgenInputs: number;
    /** Audio-tap region shape; 0/0 when the server has no segment. */
    taps: number;
    tapFrames: number;
    /** The stream-transport frame ceiling in bytes. */
    maxFrame: number;
}

/** One entry of a def's control surface, as `queryDefs` reports it. */
export interface ControlInfo {
    name: string;
    default: number;
    /** The control type the def declared: `"kr"`, `"tr"` or `"ir"`. */
    rate: string;
    /** A Faust parameter's declared range (its UI widget's). */
    min?: number;
    max?: number;
    step?: number;
    /** A graph def's port: the member controls it drives. */
    targets?: PortTargetInfo[];
}

/** What the server holds under a def name. */
export interface DefInfo {
    name: string;
    /** `"synth"`, `"faust"` or `"graph"` — empty when the name is unknown. */
    family: string;
    controls: ControlInfo[];
}

/**
 * A node as `/n_query` reports it: a group carries `head`/`tail`, a synth
 * its `def` and control values.
 */
export interface NodeInfo {
    id: number;
    parent: number;
    previous: number;
    next: number;
    isGroup: boolean;
    head?: number;
    tail?: number;
    def?: string;
    controls?: Record<string, number>;
}

/** One inner target of a graph def's surface port, as `/d_info` reports it. */
export interface PortTargetInfo {
    member: number;
    control: string;
    mul: number;
    add: number;
}

/**
 * A node-tree entry: a group carries `children`, a synth a `def` and its
 * control values.
 */
export interface TreeNode {
    id: number;
    children?: TreeNode[];
    def?: string;
    controls?: Record<string, number | string>;
}

/**
 * A plain value a message argument may take, or an explicit `[tag, value]`
 * pair when the inferred type is wrong (the codec's own type — re-exported
 * here, where the commands take it).
 */
export type { MsgArg };

/**
 * Control values, by name. The reserved `in`/`out` bus controls are
 * expressible here like any other name.
 */

/**
 * One `/clock` observation: the server's counter and rate, and the local time
 * the exchange is centred on.
 */
interface ClockReply {
    local: number;
    sample: number;
    rate: number;
    oscTime: number;
}

interface Pending {
    match: (msg: OscMessage) => boolean;
    resolve: (msg: OscMessage) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
}


export class Server {
    readonly connection: Connection;
    /** The sizes this client's allocators were built against. */
    readonly sizing: ServerSizing;
    readonly nodes: NodeIdAllocator;
    readonly audioBuses: AudioBusAllocator;
    readonly controlBuses: ControlBusAllocator;
    readonly buffers: BufferAllocator;
    /**
     * Seconds added to every timed send — the scheduling headroom. Kept here
     * so the sequencing layer (a later milestone) has one place to read it.
     */
    latency = 0.05;

    private pending = new Set<Pending>();
    private handlers = new Set<(msg: OscMessage) => void>();
    private syncCounter = 0;
    /** The transport's frame ceiling, read once and cached (`bulkChunk`). */
    private maxFrame: number | null = null;
    private readonly listener: (packet: Uint8Array) => void;

    private constructor(connection: Connection, sizing: ServerSizing) {
        this.connection = connection;
        this.sizing = sizing;
        this.nodes = NodeIdAllocator.forMaxNodes(sizing.maxNodes);
        this.audioBuses = new AudioBusAllocator(sizing.audioBuses, sizing.channels);
        this.controlBuses = new ControlBusAllocator(sizing.controlBuses);
        this.buffers = new BufferAllocator(sizing.maxBuffers);
        this.listener = (packet) => this.dispatch(packet);
        connection.addReply(this.listener);
    }

    /**
     * Opens a server over `connection`.
     *
     * With no `sizing` it asks the server for its own (`/server_info`), so
     * the allocators match the server that is actually running; a server
     * that does not answer within `timeout` leaves the compiled defaults in
     * place. `notify` (default `true`) registers for the server's pushes,
     * which is what recycles node ids as their `/n_end` arrives.
     *
     * The core wasm must be loaded first (`await loadOsc()`).
     */
    static async open(
        connection: Connection,
        {
            sizing,
            notify = true,
            timeout = 2.0,
        }: {
            sizing?: Partial<ServerSizing>;
            notify?: boolean;
            timeout?: number;
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
        const probe = new Server(connection, defaults);
        let resolved: ServerSizing = { ...defaults, ...sizing };
        if (!sizing) {
            try {
                const info = await probe.queryInfo(timeout);
                resolved = {
                    audioBuses: info.audioBuses,
                    controlBuses: info.controlBuses,
                    maxNodes: info.maxNodes,
                    maxBuffers: info.maxBuffers,
                    channels: info.channels,
                    taps: info.taps,
                };
            } catch (error) {
                if (!(error instanceof ReplyTimeout)) throw error;
                console.warn(
                    "clausters: no /server_info reply; sizing the allocators " +
                        "from the compiled defaults",
                );
            }
        }
        probe.close();
        const server = new Server(connection, resolved);
        if (notify) await server.notify(true, timeout);
        return server;
    }

    // ---- the reply stream ----

    private dispatch(packet: Uint8Array): void {
        let messages: OscMessage[];
        try {
            messages = decodePacket(packet);
        } catch (error) {
            console.warn(`clausters: undecodable reply packet: ${String(error)}`);
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
            for (const handler of [...this.handlers]) handler(msg);
        }
    }

    /**
     * Subscribes to every decoded reply message; returns the unsubscribe.
     * The seam a responder layer builds on.
     */
    onReply(handler: (msg: OscMessage) => void): () => void {
        this.handlers.add(handler);
        return () => this.handlers.delete(handler);
    }

    /**
     * Resolves with the first reply message `match` accepts, or rejects with
     * `ReplyTimeout` after `timeout` seconds. Registered *before* whatever
     * send provokes the reply, so a fast server cannot outrun it.
     */
    awaitReply(
        match: (msg: OscMessage) => boolean,
        timeout = 5.0,
        what = "a reply",
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

    // ---- raw OSC ----

    /**
     * Sends one message. **A message has no time**: in a bundle it would
     * carry the immediate timetag, and alone it means exactly that. Logical
     * time belongs to the bundle path, which a later milestone brings; use
     * this for what has no place in a timeline — sending defs, allocating
     * buffers, opening the groups a piece is built on.
     */
    sendMsg(addr: string, ...args: MsgArg[]): void {
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
     * its `latency`, and scheduling by absolute sample (`/sched`) when the
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
     * `/sched <absolute sample> <packet>`: the sample-exact path. The inner
     * bundle is immediate — the outer command's own target carries the time.
     */
    private sendSched(sample: number, messages: readonly TimedMessage[]): void {
        this.sendMsg("/sched", ["h", sample], [
            "b",
            encodeImmediateBundle(toBundle(messages)),
        ]);
    }

    /**
     * Plays a note `Event` as OSC: `/s_new`, then its release (`gate 0` when
     * the event releases by gate, else `/n_free`) after the sustain. The OSC
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
            "/s_new",
            String(event.get("instrument")),
            ["i", node],
            ["i", Math.trunc(Number(event.get("addAction")))],
            ["i", Math.trunc(Number(event.get("target")))],
            ...event.controlArgs(),
        ];
        const release: TimedMessage = event.releasesByGate()
            ? ["/n_set", ["i", node], "gate", ["f", 0]]
            : ["/n_free", ["i", node]];
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
            timeout = 5.0,
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
        { reply, timeout = 5.0 }: { reply: string; timeout?: number },
    ): Promise<OscMessage[]> {
        return new Promise((resolve, reject) => {
            const collected: OscMessage[] = [];
            const timer = setTimeout(() => {
                unsubscribe();
                reject(new ReplyTimeout(`no reply to ${addr} within ${timeout}s`));
            }, timeout * 1000);
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
        timeout: number,
    ): Promise<OscMessage> {
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
     * The async barrier (scsynth `/sync`): resolves only once every async
     * command sent earlier — def compiles, buffer jobs — has completed.
     * Returns the id used.
     */
    async sync(timeout = 5.0): Promise<number> {
        const id = ++this.syncCounter;
        const reply = this.awaitReply(
            (msg) => msg.addr === "/synced" && msg.args[0] === id,
            timeout,
            `/synced ${id}`,
        );
        this.sendMsg("/sync", ["i", id]);
        await reply;
        return id;
    }

    // ---- definitions ----

    /**
     * Removes defs from the server's def table by name (`/d_free`).
     *
     * A def is not freed by itself: in use it is *overwritten* by sending
     * another under the same name. This is the table's own command, for
     * reclaiming what a session no longer names.
     */
    freeDef(...names: string[]): void {
        this.sendMsg("/d_free", ...names);
    }

    // ---- bus and tap subscriptions (one per client, over a set) ----

    /**
     * Subscribes this client to a periodic `/c_set` snapshot of `buses`
     * (`/c_stream`): the server sends one immediately and then one every
     * `periodMs` (10 ms floor, at most 128 buses) with no further requests —
     * the message-based counterpart of reading the shared-memory segment, and
     * what a meter or a control-rate scope in the page feeds on.
     *
     * One subscription per client, **replaced** by each call; `periodMs <= 0`
     * (or no buses) cancels. Resolves on the `/done` ack. Read the snapshots
     * with `onReply`, or let `busStream` do all of it.
     */
    async streamBuses(
        periodMs: number,
        buses: readonly BusLike[],
        timeout = 5.0,
    ): Promise<void> {
        const args: MsgArg[] = [["i", Math.trunc(periodMs)]];
        for (const bus of buses) args.push(["i", busIndex(bus)]);
        await this.command("/c_stream", args, timeout);
    }

    /**
     * Subscribes this client to a periodic `/tap_data` snapshot of `buses`
     * (`/tap_stream`): every `periodMs` (10 ms floor) the server sends, per
     * bus, its newest `frames` samples — the path an oscilloscope, a
     * phasescope or a spectrum in the page reads.
     *
     * The subscription **is** the watch: it starts recording each bus it
     * lists and stops when it is replaced, cancelled or the connection dies,
     * so a streaming client never calls `watch` itself. `frames` is clamped to
     * the transport's bound and to half the ring; at most 8 buses; one
     * subscription per client, replaced by each call, `periodMs <= 0` (or no
     * buses) cancels. Resolves on the `/done` ack; `tapStream` wraps the whole
     * thing.
     */
    async streamTaps(
        periodMs: number,
        frames: number,
        buses: readonly BusLike[],
        timeout = 5.0,
    ): Promise<void> {
        const args: MsgArg[] = [
            ["i", Math.trunc(periodMs)],
            ["i", Math.trunc(frames)],
        ];
        for (const bus of buses) args.push(["i", busIndex(bus)]);
        await this.command("/tap_stream", args, timeout);
    }

    // ---- bulk sizing ----

    /**
     * Samples per bulk round trip for this carrier: the frame ceiling the
     * server advertises (`/server_info`, queried once and cached), minus
     * headroom for the reply's OSC envelope. A server that does not answer
     * leaves the conservative 1024 a datagram fits.
     */
    async bulkChunk(timeout: number): Promise<number> {
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

    // ---- server introspection ----

    /**
     * The server's static configuration: bus counts, output/input channels,
     * block size, sample rate and the boot-time pool sizes. The appended
     * capacity fields degrade to the compiled defaults against a server too
     * old to report them.
     */
    async queryInfo(timeout = 5.0): Promise<ServerInfo> {
        const msg = await this.request("/server_info", [], {
            expect: ["/server_info.reply"],
            timeout,
        });
        const a = msg.args;
        const at = (i: number, fallback: number) =>
            i < a.length ? Number(a[i]) : fallback;
        return {
            audioBuses: Number(a[0]),
            controlBuses: Number(a[1]),
            channels: Number(a[2]),
            blockSize: Number(a[3]),
            nominalSampleRate: Number(a[4]),
            actualSampleRate: Number(a[5]),
            inputChannels: at(6, 0),
            maxNodes: at(7, DEFAULT_MAX_NODES),
            maxBuffers: at(8, DEFAULT_MAX_BUFFERS),
            maxGraphChildren: at(9, DEFAULT_MAX_GRAPH_CHILDREN),
            maxUgenInputs: at(10, DEFAULT_MAX_UGEN_INPUTS),
            taps: at(11, 0),
            tapFrames: at(12, 0),
            maxFrame: at(13, 65536),
        };
    }

    /**
     * The live counters (`/status`): `[unused, ugens, synths, groups, defs,
     * avgCpu, peakCpu, nominalSr, actualSr]`.
     */
    async status(timeout = 5.0): Promise<(number | string | boolean | null | Uint8Array)[]> {
        const msg = await this.request("/status", [], {
            expect: ["/status.reply"],
            timeout,
        });
        return msg.args;
    }

    /**
     * The defs the server holds, each with its control surface (`/d_query`,
     * answered by one `/d_info` per def). With `names`, details exactly
     * those — an unknown one comes back with an empty `family` rather than
     * failing; with none, every loaded def of every family.
     *
     * The def store persists across restarts, so a server may well hold defs
     * this client never sent: this is how you find out.
     */
    async queryDefs(names: string[] = [], timeout = 5.0): Promise<DefInfo[]> {
        const replies = await this.requestBatch("/d_query", names, {
            reply: "/d_info",
            timeout,
        });
        return replies.map((msg) => parseDefInfo(msg.args));
    }

    /**
     * One node's place in the tree (`/n_query`). A group reports its
     * `head`/`tail`; a synth its `def` and control values.
     */
    async nodeQuery(node: NodeLike, timeout = 5.0): Promise<NodeInfo> {
        const id = nodeId(node);
        const reply = this.awaitReply(
            (msg) => msg.addr === "/n_info" && Number(msg.args[0]) === id,
            timeout,
            `/n_info for node ${id}`,
        );
        this.sendMsg("/n_query", ["i", id]);
        return parseNodeInfo((await reply).args);
    }

    /**
     * The node tree from `group` down (`/g_queryTree`): a group is
     * `{id, children}`, a synth `{id, def, controls}`.
     */
    async queryTree(
        group: NodeLike = ROOT_NODE_ID,
        { controls = true, timeout = 5.0 }: { controls?: boolean; timeout?: number } = {},
    ): Promise<TreeNode> {
        const msg = await this.request(
            "/g_queryTree",
            [["i", nodeId(group)], ["i", controls ? 1 : 0]],
            { expect: ["/g_queryTree.reply"], timeout },
        );
        const a = msg.args;
        const withControls = Number(a[0]) === 1;
        const [children] = parseTreeNodes(a, 3, Number(a[2]), withControls);
        return { id: Number(a[1]), children };
    }

    /**
     * The server's rendered node graph as text (`/g_dumpGraph`) — a
     * debugging aid; for machine use prefer `queryTree`.
     */
    async dumpGraph(group: NodeLike = ROOT_NODE_ID, timeout = 5.0): Promise<string> {
        const msg = await this.request("/g_dumpGraph", [["i", nodeId(group)]], {
            expect: ["/g_dumpGraph.reply", "/fail"],
            timeout,
        });
        if (msg.addr === "/fail") {
            throw new CommandError(`/g_dumpGraph failed: ${msg.args.join(" ")}`);
        }
        return String(msg.args[1]);
    }

    // ---- server control ----

    /**
     * Registers (or drops) this client for the server's pushes — `/n_end`
     * node deaths, `/tr` triggers, the transport broadcasts. Registering is
     * what lets the node-id registry recycle.
     */
    async notify(flag = true, timeout = 5.0): Promise<void> {
        const reply = this.awaitReply(
            (msg) => msg.addr === "/done" && msg.args[0] === "/notify",
            timeout,
            "/done /notify",
        );
        this.sendMsg("/notify", ["i", flag ? 1 : 0]);
        try {
            await reply;
        } catch (error) {
            if (!(error instanceof ReplyTimeout)) throw error;
            console.warn(
                "clausters: no /done for /notify; node ids will not recycle",
            );
            return;
        }
        if (flag) this.recycleNodeIds();
    }

    private recycling = false;

    /**
     * Returns a node id to the registry as its `/n_end` arrives — the
     * side-channel that keeps the client range from exhausting.
     */
    private recycleNodeIds(): void {
        if (this.recycling) return;
        this.recycling = true;
        this.onReply((msg) => {
            if (msg.addr === "/n_end") this.nodes.free(Number(msg.args[0]));
        });
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
     * - **over a socket** — `/clock` round trips feed the core's sample-clock
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
    async sampleTimebase({
        timeout = 2.0,
        anchors = 5,
        gap = 0.05,
        trackEvery = 0.5,
    }: {
        timeout?: number;
        anchors?: number;
        gap?: number;
        trackEvery?: number;
    } = {}): Promise<Timebase> {
        if (this.connection.sampleClock) {
            const clock = await this.connection.sampleClock();
            return new SampleTimebase(clock.sample, clock.sampleRate);
        }
        let info: ClockReply;
        try {
            info = await this.clockAnchor(timeout);
        } catch (error) {
            if (!(error instanceof ReplyTimeout)) throw error;
            console.warn(
                "clausters: no /clock reply; the clock stays on wall-clock time",
            );
            return new MonotonicTimebase();
        }
        const model = new SampleClockModel(info.rate, 64);
        model.addAnchor(info.local, info.sample, info.rate);
        // Firm the model up before anything schedules against it: one anchor
        // gives an offset, several give a rate — but only if they are spread
        // over enough time. Back-to-back round trips all land inside a couple
        // of milliseconds, and a regression over that span is noise.
        for (let i = 1; i < anchors; i++) {
            if (gap > 0) await new Promise((done) => setTimeout(done, gap * 1000));
            const next = await this.clockAnchor(timeout);
            model.addAnchor(next.local, next.sample, next.rate);
        }
        if (trackEvery > 0) {
            this.clockTracker = setInterval(() => {
                this.clockAnchor(timeout)
                    .then((a) => model.addAnchor(a.local, a.sample, a.rate))
                    .catch(() => {
                        /* a missed anchor is not fatal: the model holds. */
                    });
            }, trackEvery * 1000);
        }
        this.clockModel = model;
        return new SampleTimebase(
            () => model.sampleAt(performance.now() / 1000),
            info.rate,
        );
    }

    private clockTracker: ReturnType<typeof setInterval> | null = null;
    private clockModel: SampleClockModel | null = null;

    /**
     * One `/clock` round trip, timestamped at the midpoint of the exchange —
     * the best estimate of when the server read its own counter.
     */
    private async clockAnchor(timeout: number): Promise<ClockReply> {
        const sent = performance.now() / 1000;
        const msg = await this.request("/clock", [], {
            expect: ["/clock.reply"],
            timeout,
        });
        const received = performance.now() / 1000;
        return {
            local: (sent + received) / 2,
            sample: Number(msg.args[0]),
            rate: Number(msg.args[1]),
            oscTime: Number(msg.args[2]),
        };
    }

    /**
     * The drift the sample-clock model has measured, in parts per million, or
     * `null` when this server is not being tracked (the in-page carrier needs
     * no model — it shares the page's audio clock).
     */
    get clockDriftPpm(): number | null {
        return this.clockModel?.driftPpm ?? null;
    }

    /**
     * Stops the server (`/quit`). Over the in-page carrier this stops the
     * page's engine, which nothing restarts.
     */
    quit(): void {
        this.sendMsg("/quit");
    }

    /**
     * Detaches this server from its connection (the connection itself, and
     * any shared in-page engine, keep running). Pending requests reject.
     */
    close(): void {
        if (this.clockTracker !== null) clearInterval(this.clockTracker);
        this.clockTracker = null;
        this.connection.removeReply(this.listener);
        for (const p of this.pending) {
            clearTimeout(p.timer);
            p.reject(new ReplyTimeout("the server was closed"));
        }
        this.pending.clear();
        this.handlers.clear();
    }
}

/** Any decoded OSC argument — what a reply parser walks. */
type ReplyArgs = readonly (number | string | boolean | null | Uint8Array)[];

/**
 * A control identifier in a reply is a name string, or an int index when the
 * server could not resolve a name.
 */
const controlKey = (key: ReplyArgs[number]): string =>
    typeof key === "string" ? key : String(Number(key));

/**
 * Recursively parses `count` nodes of a `/g_queryTree.reply` starting at
 * `i`; returns the nodes and the next index. A synth has child-count −1.
 */
function parseTreeNodes(
    args: ReplyArgs,
    i: number,
    count: number,
    withControls: boolean,
): [TreeNode[], number] {
    const out: TreeNode[] = [];
    for (let n = 0; n < count; n++) {
        const id = Number(args[i++]);
        const children = Number(args[i++]);
        if (children === -1) {
            const node: TreeNode = { id, def: String(args[i++]) };
            if (withControls) {
                const numControls = Number(args[i++]);
                const controls: Record<string, number | string> = {};
                for (let c = 0; c < numControls; c++) {
                    controls[controlKey(args[i++])] = Number(args[i++]);
                }
                node.controls = controls;
            }
            out.push(node);
        } else {
            const [kids, next] = parseTreeNodes(args, i, children, withControls);
            i = next;
            out.push({ id, children: kids });
        }
    }
    return [out, i];
}

/**
 * An `/n_info` reply: the four fixed neighbours and the group flag, then
 * either the group's head/tail or the synth's def and controls.
 */
function parseNodeInfo(args: ReplyArgs): NodeInfo {
    const isGroup = Number(args[4]) === 1;
    const info: NodeInfo = {
        id: Number(args[0]),
        parent: Number(args[1]),
        previous: Number(args[2]),
        next: Number(args[3]),
        isGroup,
    };
    if (isGroup) {
        info.head = Number(args[5]);
        info.tail = Number(args[6]);
        return info;
    }
    let i = 5;
    info.def = String(args[i++]);
    const count = Number(args[i++]);
    const controls: Record<string, number> = {};
    for (let c = 0; c < count; c++) {
        controls[controlKey(args[i++])] = Number(args[i++]);
    }
    info.controls = controls;
    return info;
}

/**
 * One `/d_info` reply: `name, family, numControls` then per control `name,
 * default, rate` — plus `min, max, step` for a Faust parameter, or
 * `numTargets` and the target tuples for a graph port.
 */
function parseDefInfo(args: ReplyArgs): DefInfo {
    const name = String(args[0]);
    const family = String(args[1]);
    const count = Number(args[2]);
    const controls: ControlInfo[] = [];
    let i = 3;
    for (let c = 0; c < count; c++) {
        const info: ControlInfo = {
            name: String(args[i]),
            default: Number(args[i + 1]),
            rate: String(args[i + 2]),
        };
        i += 3;
        if (family === "faust") {
            info.min = Number(args[i]);
            info.max = Number(args[i + 1]);
            info.step = Number(args[i + 2]);
            i += 3;
        } else if (family === "graph") {
            const numTargets = Number(args[i++]);
            const targets: PortTargetInfo[] = [];
            for (let t = 0; t < numTargets; t++) {
                targets.push({
                    member: Number(args[i]),
                    control: String(args[i + 1]),
                    mul: Number(args[i + 2]),
                    add: Number(args[i + 3]),
                });
                i += 4;
            }
            info.targets = targets;
        }
        controls.push(info);
    }
    return { name, family, controls };
}
