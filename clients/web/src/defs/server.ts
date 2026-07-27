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

import { decodePacket, encodeMessage } from "../base/osc.ts";
import type { OscArg, OscMessage } from "../base/osc.ts";
import type { Connection } from "../base/connection.ts";
import { CommandError, ReplyTimeout } from "../errors.ts";
import {
    AddAction,
    Group,
    NodeIdAllocator,
    ROOT_NODE_ID,
    Synth,
    nodeId,
} from "./node.ts";
import type { NodeLike } from "./node.ts";
import { AudioBusAllocator, Bus, ControlBusAllocator, busIndex } from "./bus.ts";
import type { BusLike } from "./bus.ts";
import { Buffer, BufferAllocator, bufferNumber } from "./buffer.ts";
import type { BufferLike } from "./buffer.ts";
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

/// The sizes a client's allocators need. They are a property of the *server*,
/// so `Server.open` reads them from `/server_info` rather than guessing;
/// pass them explicitly to skip that round trip.
export interface ServerSizing {
    audioBuses: number;
    controlBuses: number;
    maxNodes: number;
    maxBuffers: number;
    /// Hardware output channels — the audio buses reserved at the bottom of
    /// the space, which the allocator never hands out.
    channels: number;
}

/// The static configuration a running server reports over `/server_info`.
export interface ServerInfo extends ServerSizing {
    blockSize: number;
    nominalSampleRate: number;
    actualSampleRate: number;
    inputChannels: number;
    maxGraphChildren: number;
    maxUgenInputs: number;
    /// Audio-tap region shape; 0/0 when the server has no segment.
    taps: number;
    tapFrames: number;
    /// The stream-transport frame ceiling in bytes.
    maxFrame: number;
}

/// One entry of a def's control surface, as `queryDefs` reports it.
export interface ControlInfo {
    name: string;
    default: number;
    /// The control type the def declared: `"kr"`, `"tr"` or `"ir"`.
    rate: string;
    /// A Faust parameter's declared range (its UI widget's).
    min?: number;
    max?: number;
    step?: number;
    /// A graph def's port: the member controls it drives.
    targets?: PortTargetInfo[];
}

/// What the server holds under a def name.
export interface DefInfo {
    name: string;
    /// `"synth"`, `"faust"` or `"graph"` — empty when the name is unknown.
    family: string;
    controls: ControlInfo[];
}

/// A node as `/n_query` reports it: a group carries `head`/`tail`, a synth
/// its `def` and control values.
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

/// One inner target of a graph def's surface port, as `/d_info` reports it.
export interface PortTargetInfo {
    member: number;
    control: string;
    mul: number;
    add: number;
}

/// A node-tree entry: a group carries `children`, a synth a `def` and its
/// control values.
export interface TreeNode {
    id: number;
    children?: TreeNode[];
    def?: string;
    controls?: Record<string, number | string>;
}

/// A plain value a message argument may take, or an explicit `[tag, value]`
/// pair when the inferred type is wrong.
export type MsgArg = number | string | bigint | Uint8Array | OscArg;

/// Control values, by name. The reserved `in`/`out` bus controls are
/// expressible here like any other name.
export type Controls = Record<string, number> | readonly (readonly [string, number])[];

interface Pending {
    match: (msg: OscMessage) => boolean;
    resolve: (msg: OscMessage) => void;
    reject: (error: Error) => void;
    timer: ReturnType<typeof setTimeout>;
}

function isOscArg(x: MsgArg): x is OscArg {
    return Array.isArray(x) && x.length === 2 && typeof x[0] === "string";
}

function oscArg(value: MsgArg): OscArg {
    if (isOscArg(value)) return value;
    if (typeof value === "bigint") return ["h", value];
    if (typeof value === "string") return ["s", value];
    if (value instanceof Uint8Array) return ["b", value];
    return Number.isInteger(value) ? ["i", value] : ["f", value];
}

/// Control values flattened into the `name value name value …` tail every
/// node command takes. Accepts an object or a list of pairs.
function flattenControls(controls?: Controls): OscArg[] {
    if (!controls) return [];
    const entries = Array.isArray(controls)
        ? (controls as readonly (readonly [string, number])[])
        : Object.entries(controls as Record<string, number>);
    const out: OscArg[] = [];
    for (const [name, value] of entries) out.push(["s", name], ["f", value]);
    return out;
}

export class Server {
    readonly connection: Connection;
    /// The sizes this client's allocators were built against.
    readonly sizing: ServerSizing;
    readonly nodes: NodeIdAllocator;
    readonly audioBuses: AudioBusAllocator;
    readonly controlBuses: ControlBusAllocator;
    readonly buffers: BufferAllocator;
    /// Seconds added to every timed send — the scheduling headroom. Kept here
    /// so the sequencing layer (a later milestone) has one place to read it.
    latency = 0.05;

    private pending = new Set<Pending>();
    private handlers = new Set<(msg: OscMessage) => void>();
    private syncCounter = 0;
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

    /// Opens a server over `connection`.
    ///
    /// With no `sizing` it asks the server for its own (`/server_info`), so
    /// the allocators match the server that is actually running; a server
    /// that does not answer within `timeout` leaves the compiled defaults in
    /// place. `notify` (default `true`) registers for the server's pushes,
    /// which is what recycles node ids as their `/n_end` arrives.
    ///
    /// The core wasm must be loaded first (`await loadOsc()`).
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

    /// Subscribes to every decoded reply message; returns the unsubscribe.
    /// The seam a responder layer builds on.
    onReply(handler: (msg: OscMessage) => void): () => void {
        this.handlers.add(handler);
        return () => this.handlers.delete(handler);
    }

    /// Resolves with the first reply message `match` accepts, or rejects with
    /// `ReplyTimeout` after `timeout` seconds. Registered *before* whatever
    /// send provokes the reply, so a fast server cannot outrun it.
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

    /// Sends one message. **A message has no time**: in a bundle it would
    /// carry the immediate timetag, and alone it means exactly that. Logical
    /// time belongs to the bundle path, which a later milestone brings; use
    /// this for what has no place in a timeline — sending defs, allocating
    /// buffers, opening the groups a piece is built on.
    sendMsg(addr: string, ...args: MsgArg[]): void {
        this.connection.send(encodeMessage(addr, args.map(oscArg)));
    }

    /// Sends `addr` and resolves with the first reply whose address is in
    /// `expect`. `cmd` additionally requires the reply's first argument to
    /// name that command, which is what makes concurrent requests safe (the
    /// server echoes the command in `/done`/`/fail`).
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

    /// Sends `addr` and collects every `reply` message until the batch's
    /// `/done` terminator — the shape the introspection queries take, whose
    /// result is a variable number of messages.
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

    /// Sends `addr` and awaits its `/done`, throwing `CommandError` on
    /// `/fail`. The shape every asynchronous command answers with.
    private async command(
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

    /// The async barrier (scsynth `/sync`): resolves only once every async
    /// command sent earlier — def compiles, buffer jobs — has completed.
    /// Returns the id used.
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

    /// Sends a def of any family — dispatches by type. `wait` (default
    /// `true`) resolves on the server's `/done`; `wait: false` is
    /// fire-and-forget, to be paired with `sync()`.
    addDef(
        def: SynthDef | FaustDef | GraphDef,
        options?: { wait?: boolean; timeout?: number },
    ): Promise<string> {
        if (def instanceof GraphDef) return this.addGraphDef(def, options);
        if (def instanceof FaustDef) return this.addFaustDef(def, options);
        return this.addSynthDef(def, options);
    }

    /// Sends a UGen `SynthDef` via `/d_recv`. Compilation is asynchronous on
    /// the server, so `wait: true` (the default) resolves on `/done` and
    /// rejects with `CommandError` on `/fail`.
    async addSynthDef(
        def: SynthDef,
        { wait = true, timeout = 10.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<string> {
        const payload = def.dumpDef();
        if (!wait) {
            this.sendMsg("/d_recv", payload);
            return def.name;
        }
        await this.command("/d_recv", [payload], timeout);
        return def.name;
    }

    /// Sends a `FaustDef` via `/d_faust`, which JIT-compiles it on the
    /// server's network thread. Reaches a **native** server only: the in-page
    /// engine is the `synth,embed` build with no LLVM, and answers `/fail`.
    async addFaustDef(
        def: FaustDef,
        { wait = true, timeout = 10.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<string> {
        if (!wait) {
            this.sendMsg("/d_faust", def.name, def.dumpDef());
            return def.name;
        }
        await this.command("/d_faust", [def.name, def.dumpDef()], timeout);
        return def.name;
    }

    /// Sends a `GraphDef` via `/d_graph`. Loading one is cheap on the server
    /// (no JIT — it only validates and references the member defs), but it is
    /// still asynchronous, so the same barrier discipline applies.
    async addGraphDef(
        def: GraphDef,
        { wait = true, timeout = 10.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<string> {
        const payload = def.dumpDef();
        if (!wait) {
            this.sendMsg("/d_graph", payload);
            return def.name;
        }
        await this.command("/d_graph", [payload], timeout);
        return def.name;
    }

    /// Frees defs by name (`/d_free`).
    freeDef(...names: string[]): void {
        this.sendMsg("/d_free", ...names);
    }

    // ---- nodes ----

    /// Creates a synth (`/s_new`) and returns its handle.
    synth(
        defname: string,
        controls?: Controls,
        {
            target = ROOT_NODE_ID,
            action = AddAction.TAIL,
        }: { target?: NodeLike; action?: AddAction } = {},
    ): Synth {
        const id = this.nodes.alloc();
        this.sendMsg(
            "/s_new",
            defname,
            ["i", id],
            ["i", action],
            ["i", nodeId(target)],
            ...flattenControls(controls),
        );
        return new Synth(id, defname, this);
    }

    /// Creates a group (`/g_new`) and returns its handle.
    group({
        target = ROOT_NODE_ID,
        action = AddAction.TAIL,
    }: { target?: NodeLike; action?: AddAction } = {}): Group {
        const id = this.nodes.alloc();
        this.sendMsg("/g_new", ["i", id], ["i", action], ["i", nodeId(target)]);
        return new Group(id, this);
    }

    /// Instantiates a GraphDef (`/graph_new`) as a wired group, with `ports`
    /// overriding the def defaults. Drive the returned group through the
    /// surface with `set` (`/n_set` resolves names against the surface, not
    /// the private members) and tear it down with `free` (which also reclaims
    /// its private buses).
    graph(
        defname: string,
        ports?: Controls,
        {
            target = ROOT_NODE_ID,
            action = AddAction.TAIL,
        }: { target?: NodeLike; action?: AddAction } = {},
    ): Group {
        const id = this.nodes.alloc();
        this.sendMsg(
            "/graph_new",
            defname,
            ["i", id],
            ["i", action],
            ["i", nodeId(target)],
            ...flattenControls(ports),
        );
        return new Group(id, this);
    }

    /// Spawns a per-voice sub-graph (`/graph_voice`) inside a running
    /// GraphDef `instance`, wired to its shared private buses.
    graphVoice(instance: NodeLike, ports?: Controls): Group {
        const id = this.nodes.alloc();
        this.sendMsg(
            "/graph_voice",
            ["i", nodeId(instance)],
            ["i", id],
            ...flattenControls(ports),
        );
        return new Group(id, this);
    }

    /// Sets a node's controls (`/n_set`).
    set(node: NodeLike, controls: Controls): void {
        this.sendMsg("/n_set", ["i", nodeId(node)], ...flattenControls(controls));
    }

    /// Maps a node's control to a bus (`/n_map`, or `/n_mapa` for audio).
    map(node: NodeLike, name: string, bus: BusLike, { audio = false } = {}): void {
        this.sendMsg(
            audio ? "/n_mapa" : "/n_map",
            ["i", nodeId(node)],
            name,
            ["i", busIndex(bus)],
        );
    }

    /// Sends a typed command to **one UGen instance** inside a synth
    /// (`/u_cmd nodeID ugenIndex name args…`); an unrecognized `name` is a
    /// no-op on the server.
    uCmd(node: NodeLike, ugenIndex: number, name: string, ...args: number[]): void {
        this.sendMsg(
            "/u_cmd",
            ["i", nodeId(node)],
            ["i", Math.trunc(ugenIndex)],
            name,
            ...args.map((a): OscArg => ["f", a]),
        );
    }

    /// Frees nodes (`/n_free`). The id is **not** returned to the registry
    /// here: it stays tracked until the server confirms the death with
    /// `/n_end` — releasing at send time could re-hand an id whose node is
    /// still alive on the server.
    free(...nodes: NodeLike[]): void {
        for (const node of nodes) this.sendMsg("/n_free", ["i", nodeId(node)]);
    }

    /// Pauses (`flag: false`) or resumes a node — a synth or a whole group —
    /// with `/n_run`. A paused node stays in the tree and keeps its state but
    /// is skipped; this is what resumes a synth parked by `PAUSE_SELF`.
    run(node: NodeLike, flag = true): void {
        this.sendMsg("/n_run", ["i", nodeId(node)], ["i", flag ? 1 : 0]);
    }

    /// Pauses a node (`/n_run … 0`).
    pause(node: NodeLike): void {
        this.run(node, false);
    }

    /// Resumes a paused node (`/n_run … 1`).
    resume(node: NodeLike): void {
        this.run(node, true);
    }

    // ---- buses ----

    /// A run of `channels` contiguous audio buses.
    audioBus(channels = 1): Bus {
        return this.audioBuses.alloc(channels);
    }

    /// One control bus.
    controlBus(): Bus {
        return this.controlBuses.alloc(1);
    }

    /// Returns a bus's run to its allocator.
    freeBus(bus: Bus): void {
        if (bus.rate === "audio") this.audioBuses.free(bus);
        else this.controlBuses.free(bus);
    }

    /// Sets a control bus's value (`/c_set`).
    setBus(bus: BusLike, value: number): void {
        this.sendMsg("/c_set", ["i", busIndex(bus)], ["f", value]);
    }

    /// Reads a control bus's value (`/c_get`).
    async getBus(bus: BusLike, timeout = 5.0): Promise<number> {
        const index = busIndex(bus);
        const msg = await this.request("/c_get", [["i", index]], {
            expect: ["/c_set"],
            timeout,
        });
        return Number(msg.args.at(-1));
    }

    // ---- buffers ----

    /// Allocates a zeroed buffer (`/b_alloc`).
    async allocBuffer(
        frames: number,
        channels = 1,
        { wait = true, timeout = 5.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<Buffer> {
        const bufnum = this.buffers.alloc();
        const args: MsgArg[] = [
            ["i", bufnum],
            ["i", Math.trunc(frames)],
            ["i", Math.trunc(channels)],
        ];
        if (!wait) {
            this.sendMsg("/b_alloc", ...args);
            return new Buffer(bufnum, frames, channels);
        }
        try {
            await this.command("/b_alloc", args, timeout);
        } catch (error) {
            this.buffers.free(bufnum);
            throw error;
        }
        return new Buffer(bufnum, frames, channels);
    }

    /// Fills a buffer through `/b_gen` (the wavetable/generator commands:
    /// `"env"`, `"sine1"`/`"sine2"`/`"sine3"`, `"cheby"`, `"copy"`).
    ///
    /// `args` follow each command's own shape — the wavetable generators take
    /// an integer flag word first, then their values. They are tagged by the
    /// same rule as `sendMsg` (an integral number is an int32), so a flag
    /// word arrives as the int the server requires.
    async genBuffer(
        buf: BufferLike,
        cmd: string,
        args: MsgArg[] = [],
        { wait = true, timeout = 5.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<void> {
        const payload: MsgArg[] = [["i", bufferNumber(buf)], cmd, ...args];
        if (!wait) {
            this.sendMsg("/b_gen", ...payload);
            return;
        }
        await this.command("/b_gen", payload, timeout);
    }

    /// Zeroes a buffer (`/b_zero`).
    async zeroBuffer(
        buf: BufferLike,
        { wait = true, timeout = 5.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<void> {
        const args: MsgArg[] = [["i", bufferNumber(buf)]];
        if (!wait) {
            this.sendMsg("/b_zero", ...args);
            return;
        }
        await this.command("/b_zero", args, timeout);
    }

    /// Loads a sound file into a freshly allocated buffer (`/b_allocRead`).
    /// The path is the **server's**, so this reaches a native server over the
    /// WebSocket carrier; the in-page engine has no filesystem (feed it
    /// decoded samples instead).
    async readBuffer(
        path: string,
        {
            fileStart = 0,
            numFrames = 0,
            timeout = 10.0,
        }: { fileStart?: number; numFrames?: number; timeout?: number } = {},
    ): Promise<Buffer> {
        const bufnum = this.buffers.alloc();
        try {
            await this.command(
                "/b_allocRead",
                [["i", bufnum], path, ["i", fileStart], ["i", numFrames]],
                timeout,
            );
        } catch (error) {
            this.buffers.free(bufnum);
            throw error;
        }
        return this.queryBuffer(bufnum, timeout);
    }

    /// Frees a buffer on the server and returns its index to the pool.
    freeBuffer(buf: BufferLike): void {
        const bufnum = bufferNumber(buf);
        this.sendMsg("/b_free", ["i", bufnum]);
        this.buffers.free(bufnum);
    }

    /// A buffer's shape as the server reports it (`/b_query`).
    async queryBuffer(buf: BufferLike, timeout = 5.0): Promise<Buffer> {
        const bufnum = bufferNumber(buf);
        const msg = await this.request("/b_query", [["i", bufnum]], {
            expect: ["/b_info"],
            timeout,
        });
        const [, frames, channels, sampleRate] = msg.args;
        return new Buffer(bufnum, Number(frames), Number(channels), Number(sampleRate));
    }

    // ---- server introspection ----

    /// The server's static configuration: bus counts, output/input channels,
    /// block size, sample rate and the boot-time pool sizes. The appended
    /// capacity fields degrade to the compiled defaults against a server too
    /// old to report them.
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

    /// The live counters (`/status`): `[unused, ugens, synths, groups, defs,
    /// avgCpu, peakCpu, nominalSr, actualSr]`.
    async status(timeout = 5.0): Promise<(number | string | boolean | null | Uint8Array)[]> {
        const msg = await this.request("/status", [], {
            expect: ["/status.reply"],
            timeout,
        });
        return msg.args;
    }

    /// The defs the server holds, each with its control surface (`/d_query`,
    /// answered by one `/d_info` per def). With `names`, details exactly
    /// those — an unknown one comes back with an empty `family` rather than
    /// failing; with none, every loaded def of every family.
    ///
    /// The def store persists across restarts, so a server may well hold defs
    /// this client never sent: this is how you find out.
    async queryDefs(names: string[] = [], timeout = 5.0): Promise<DefInfo[]> {
        const replies = await this.requestBatch("/d_query", names, {
            reply: "/d_info",
            timeout,
        });
        return replies.map((msg) => parseDefInfo(msg.args));
    }

    /// One node's place in the tree (`/n_query`). A group reports its
    /// `head`/`tail`; a synth its `def` and control values.
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

    /// The node tree from `group` down (`/g_queryTree`): a group is
    /// `{id, children}`, a synth `{id, def, controls}`.
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

    /// The server's rendered node graph as text (`/g_dumpGraph`) — a
    /// debugging aid; for machine use prefer `queryTree`.
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

    /// Registers (or drops) this client for the server's pushes — `/n_end`
    /// node deaths, `/tr` triggers, the transport broadcasts. Registering is
    /// what lets the node-id registry recycle.
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

    /// Returns a node id to the registry as its `/n_end` arrives — the
    /// side-channel that keeps the client range from exhausting.
    private recycleNodeIds(): void {
        if (this.recycling) return;
        this.recycling = true;
        this.onReply((msg) => {
            if (msg.addr === "/n_end") this.nodes.free(Number(msg.args[0]));
        });
    }

    /// Stops the server (`/quit`). Over the in-page carrier this stops the
    /// page's engine, which nothing restarts.
    quit(): void {
        this.sendMsg("/quit");
    }

    /// Detaches this server from its connection (the connection itself, and
    /// any shared in-page engine, keep running). Pending requests reject.
    close(): void {
        this.connection.removeReply(this.listener);
        for (const p of this.pending) {
            clearTimeout(p.timer);
            p.reject(new ReplyTimeout("the server was closed"));
        }
        this.pending.clear();
        this.handlers.clear();
    }
}

/// Any decoded OSC argument — what a reply parser walks.
type ReplyArgs = readonly (number | string | boolean | null | Uint8Array)[];

/// A control identifier in a reply is a name string, or an int index when the
/// server could not resolve a name.
const controlKey = (key: ReplyArgs[number]): string =>
    typeof key === "string" ? key : String(Number(key));

/// Recursively parses `count` nodes of a `/g_queryTree.reply` starting at
/// `i`; returns the nodes and the next index. A synth has child-count −1.
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

/// An `/n_info` reply: the four fixed neighbours and the group flag, then
/// either the group's head/tail or the synth's def and controls.
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

/// One `/d_info` reply: `name, family, numControls` then per control `name,
/// default, rate` — plus `min, max, step` for a Faust parameter, or
/// `numTargets` and the target tuples for a graph port.
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
