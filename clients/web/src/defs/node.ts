// Nodes (synths and groups) and client-side id allocation.
//
// The server's node tree: the root group is id 0; clients allocate positive
// ids. Add actions match the server: head/tail of a group, before/after a
// node, or replace. `Synth` and `Group` hold an id and the server it lives on,
// and own the commands addressed to it: **the constructor creates the node** —
// `new Synth(…)`, `new Group(…)`, `Group.graph(…)` — and `set`, `map`, `run`
// and `free` drive it. The id pool itself belongs to the `Server`.
//
// `fromId` is the other door: a handle on a node that **already** exists,
// named by an id something else reported (a responder, a tree query, the GUI).
// It sends nothing, and the handle may carry no server — it then falls back to
// the ambient one, the rule the free `play` follows.
//
// The server is the last thing named here, in the options bag, because a
// session resolves it: `new Synth("blip", { freq: 440 })` reaches the ambient
// session's server, and `{ server }` names another.
//
// (`Node` shadows the DOM's global of the same name inside a module that
// imports it. The name is the one the protocol and every other client use, so
// it stays; reach for `globalThis.Node` in the rare page that needs both.)

import { AllocationError } from "../errors.ts";
import { Registry, nodeIdPartition, requireCore } from "../base/core.ts";
import { shareOf } from "../base/ids.ts";
import type { IdShare } from "../base/ids.ts";
import { busIndex } from "./bus.ts";
import type { BusLike } from "./bus.ts";
import { parseNodeInfo } from "./info.ts";
import type { NodeInfo } from "./info.ts";
import { resolveServer } from "./wire.ts";
import type { MsgArg, OscArg } from "../base/osc.ts";
import type { Server } from "./server/index.ts";

export const ROOT_NODE_ID = 0;

/** Where `/synth_new`/`/group_new` places the new node relative to its target. */
export const AddAction = {
    HEAD: 0,
    TAIL: 1,
    BEFORE: 2,
    AFTER: 3,
    REPLACE: 4,
} as const;

export type AddAction = (typeof AddAction)[keyof typeof AddAction];

/** Anything a command can address by node id: a handle or the bare number. */
export type NodeLike = Node | number;

/** The node id behind a handle or a bare number. */
export function nodeId(node: NodeLike): number {
    return typeof node === "number" ? node : node.id;
}

/** Control values as an object or a list of `[name, value]` pairs. */
export type Controls = Record<string, number> | readonly (readonly [string, number])[];

/**
 * Control values flattened into the `name value name value …` tail every
 * node command takes. Accepts an object or a list of pairs.
 */
export function flattenControls(controls?: Controls): OscArg[] {
    if (!controls) return [];
    const entries = Array.isArray(controls)
        ? (controls as readonly (readonly [string, number])[])
        : Object.entries(controls as Record<string, number>);
    const out: OscArg[] = [];
    for (const [name, value] of entries) out.push(["s", name], ["f", value]);
    return out;
}

/** Where a new node goes, and on which server, for every constructor here. */
export type Placement = {
    /** The node the new one is placed relative to; the root group by default. */
    target?: NodeLike;
    /** Where relative to `target`; the tail of it by default. */
    action?: AddAction;
    /** The server to create it on; the ambient session's by default. */
    server?: Server;
    /**
     * The id of a node that **already exists**, adopted instead of creating
     * one. The door is `fromId`; this is how it reaches the constructor.
     *
     * @internal
     */
    adopt?: number;
};

/** A `Placement` plus the optional label a new group is created with. */
export type GroupOptions = Placement & { name?: string };

export class Node {
    readonly id: number;
    /**
     * The server this node lives on (set by the constructor), so its
     * commands know where to go without being told.
     */
    readonly server?: Server;

    constructor(id: number, server?: Server) {
        this.id = id;
        this.server = server;
    }

    /**
     * This node's server, or the ambient one — a handle built from a reported
     * id (a responder, the GUI, a tree query) carries none.
     */
    protected srv(): Server {
        return resolveServer(this.server);
    }

    /**
     * Sets controls by name (`/node_set`). On a GraphDef instance the names
     * resolve against the graph's surface, not its private members.
     */
    set(controls: Controls): void {
        this.srv().sendMsg("/node_set", ["i", this.id], ...flattenControls(controls));
    }

    /** Maps a control to a bus (`/node_map`, or `/node_mapAudio` for an audio bus). */
    map(name: string, bus: BusLike, { audio = false } = {}): void {
        this.srv().sendMsg(
            audio ? "/node_mapAudio" : "/node_map",
            ["i", this.id],
            name,
            ["i", busIndex(bus)],
        );
    }

    /**
     * This node as the server holds it **right now** (`/node_query` → `/node_query.reply`):
     * where it sits in the tree, and for a synth its def, its controls, its
     * `/node_map` bindings and the buses it reads and writes.
     *
     * A photograph, not a state: a running envelope or a mapped control moves
     * under the record's feet, so nothing caches it. A node that is gone —
     * freed, or ended by a `doneAction` — comes back with `exists` false
     * rather than throwing.
     */
    async info(timeout?: number): Promise<NodeInfo> {
        const server = this.srv();
        const reply = server.awaitReply(
            (msg) => msg.addr === "/node_query.reply" && Number(msg.args[0]) === this.id,
            timeout,
            `/node_query.reply for node ${this.id}`,
        );
        server.sendMsg("/node_query", ["i", this.id]);
        return parseNodeInfo((await reply).args);
    }

    /**
     * Sends a typed command to **one UGen instance** inside this synth
     * (`/node_ugenCmd nodeID ugenIndex name args…`); an unrecognized `name` is a
     * no-op on the server.
     */
    uCmd(ugenIndex: number, name: string, ...args: number[]): void {
        this.srv().sendMsg(
            "/node_ugenCmd",
            ["i", this.id],
            ["i", Math.trunc(ugenIndex)],
            name,
            ...args.map((a): OscArg => ["f", a]),
        );
    }

    /**
     * Frees this node now (`/node_free`) — the way to cut something whose life
     * is long; a GraphDef instance too, private buses included.
     *
     * The id is **not** returned to the registry here: it stays tracked until
     * the server confirms the death with `/node_end` — releasing at send time
     * could re-hand an id whose node is still alive on the server.
     */
    free(): void {
        this.srv().sendMsg("/node_free", ["i", this.id]);
    }

    /**
     * Pauses (`flag: false`) or resumes this node — a synth or a whole group —
     * with `/node_run`. A paused node stays in the tree and keeps its state but
     * is skipped; this is what resumes a synth parked by `PAUSE_SELF`.
     */
    run(flag = true): void {
        this.srv().sendMsg("/node_run", ["i", this.id], ["i", flag ? 1 : 0]);
    }

    /** Pauses this node (`/node_run … 0`). */
    pause(): void {
        this.run(false);
    }

    /** Resumes this node (`/node_run … 1`). */
    resume(): void {
        this.run(true);
    }
}

export class Synth extends Node {
    readonly defname: string;

    /**
     * Starts a synth from a def already loaded on the server, by name
     * (`/synth_new`). **Building one is starting it**: the id comes from the
     * server's `NodeIdAllocator` and the command goes out here, so the synth
     * is sounding by the time the constructor returns.
     *
     * ```ts
     * const g = new Group();
     * const n = new Synth("beep", { freq: 440 }, { target: g });
     * ```
     *
     * An unknown def name throws nothing here: the command is
     * fire-and-forget, the server answers `/fail` on its own channel, and the
     * handle carries an id no node was created for — `info()` reports it as
     * not existing.
     *
     * @param defname a def already installed on the server, of either family.
     * @param controls the controls overriding the def's defaults, as an
     *   object or a list of `[name, value]` pairs.
     */
    constructor(defname: string, controls?: Controls, options: Placement = {}) {
        const server = resolveServer(options.server);
        super(options.adopt ?? createNode(server, "/synth_new", defname, controls, options),
            server);
        this.defname = defname;
    }

    /**
     * A handle on a synth that is **already** on the server, named by the id
     * something else reported (a responder, a tree query, the GUI). Sends
     * nothing.
     */
    static fromId(id: number, defname: string, server?: Server): Synth {
        return new Synth(defname, undefined, { adopt: id, server });
    }
}

/**
 * Allocates an id, sends the creation command and returns the id — the one
 * shape `/synth_new` and `/graph_new` share.
 */
function createNode(
    server: Server,
    addr: string,
    defname: string,
    controls: Controls | undefined,
    { target = ROOT_NODE_ID, action = AddAction.TAIL }: Placement,
): number {
    const id = server.nodes.alloc();
    server.sendMsg(
        addr,
        defname,
        ["i", id],
        ["i", action],
        ["i", nodeId(target)],
        ...flattenControls(controls),
    );
    return id;
}

export class Group extends Node {
    /**
     * An empty group in the node tree (`/group_new`), optionally labelled —
     * see {@link Group.rename} for what a name is. Building one creates it,
     * as with `Synth`.
     *
     * The label travels with the creation, in one message: a group is born
     * knowing what it is. `rename` is for changing it afterwards. A name the
     * server refuses (see {@link Group.rename} for the rules) refuses the
     * **creation**: no group appears, rather than an anonymous one you did not
     * ask for.
     */
    constructor(options: GroupOptions = {}) {
        const server = resolveServer(options.server);
        super(options.adopt ?? createGroup(server, options), server);
    }

    /**
     * A handle on a group that is **already** on the server, named by the id
     * something else reported. Sends nothing.
     */
    static fromId(id: number, server?: Server): Group {
        return new Group({ adopt: id, server });
    }

    /**
     * Relabels this group (`/group_name`), or clears the label with `""`.
     *
     * A name does not replace the id: every command still addresses the group
     * by id, and this one is no exception. What it adds is a way to *say* which
     * group you mean — the label comes back in every node report
     * ({@link Node.info}, `Server.queryTree`) and names one segment of the group's
     * path, which `Server.groupAt` resolves. That is what makes a mixer's
     * channels, its busses and its master addressable by what they are instead
     * of by the ids they happened to get.
     *
     * The server rejects a name already taken by a sibling, one that is all
     * digits (an unnamed group answers to its id in a path, so a numeric name
     * would be ambiguous) and one containing `/` (the server composes the path,
     * the client does not).
     */
    rename(name: string): void {
        this.srv().sendMsg("/group_name", ["i", this.id], name);
    }

    /**
     * Instantiates a GraphDef already loaded on the server, by name
     * (`/graph_new`), as a wired group, with `ports` overriding the def
     * defaults. Drive the returned group through the surface with `set`
     * (`/node_set` resolves names against the surface, not the private members)
     * and tear it down with `free` (which also reclaims its private buses).
     */
    static graph(defname: string, ports?: Controls, options: Placement = {}): Group {
        const server = resolveServer(options.server);
        return Group.fromId(
            createNode(server, "/graph_new", defname, ports, options),
            server,
        );
    }

    /**
     * Spawns a per-voice sub-graph (`/graph_newVoice`) inside this running
     * GraphDef instance, wired to its shared private buses.
     */
    voice(ports?: Controls): Group {
        const server = this.srv();
        const id = server.nodes.alloc();
        server.sendMsg(
            "/graph_newVoice",
            ["i", this.id],
            ["i", id],
            ...flattenControls(ports),
        );
        return Group.fromId(id, server);
    }
}

/** Allocates an id and sends the `/group_new` that creates the group. */
function createGroup(
    server: Server,
    { name, target = ROOT_NODE_ID, action = AddAction.TAIL }: GroupOptions,
): number {
    const id = server.nodes.alloc();
    const args: MsgArg[] = [["i", id], ["i", action], ["i", nodeId(target)]];
    if (name) args.push(name);
    server.sendMsg("/group_new", ...args);
    return id;
}

/**
 * The registry of the client's node-id range.
 *
 * Node ids name slots of a finite boot-time resource (the server's node
 * table), so the allocator is an occupancy map, not a counter: every id
 * handed out stays tracked until the server reports the node's death
 * (`/node_end`, fed in through `free`), which makes it allocatable again — the
 * space never exhausts while nodes keep dying.
 *
 * It carries no range of its own: the client range is a property of the
 * server (the partition scales from `--max-nodes`), so the `Server` sizes it
 * through `nodeIdPartition`, the same formula the server applies.
 */
export class NodeIdAllocator {
    private registry: Registry;

    /** Bounded over `[base, base + capacity)`; open-ended with no `capacity`. */
    constructor(base: number, capacity?: number) {
        requireCore("a node id allocator");
        this.registry = capacity === undefined
            ? Registry.unbounded(base)
            : new Registry(base, capacity);
    }

    /**
     * The allocator for a server whose node table holds `maxNodes` slots.
     *
     * `share` takes one slice of the client range instead of all of it, for a
     * server with more than one client on it (see `IdShare`).
     */
    static forMaxNodes(maxNodes: number, share?: IdShare): NodeIdAllocator {
        const p = nodeIdPartition(maxNodes);
        return new NodeIdAllocator(...shareOf(p.clientBase, p.clientCapacity, share));
    }

    /**
     * The registry an **offline score** allocates from: ids ascend from the
     * client base and allocation never fails.
     *
     * Recycling is what bounds a live client — an id is reusable once its
     * `/node_end` arrives — and a score has no `/node_end` stream and no
     * real-time bound on how many nodes its length asks for. So the range is
     * open there, which is the reference client's rule too.
     */
    static unbounded(maxNodes: number): NodeIdAllocator {
        return new NodeIdAllocator(nodeIdPartition(maxNodes).clientBase);
    }

    /**
     * A free node id. Throws when the whole range is in flight — allocation
     * never wraps into ids that may still be alive.
     */
    alloc(): number {
        const id = this.registry.alloc(1);
        if (id === undefined) {
            throw new AllocationError(
                "out of node ids: the client range is fully in flight " +
                    "(nodes are recycled when their /node_end arrives)",
            );
        }
        return id;
    }

    /**
     * Returns `id` to the pool — called when its `/node_end` arrives. Ids
     * outside the client range (another owner's) and ids not currently
     * allocated are ignored: every node death on the server is reported, not
     * only those of nodes this client created.
     */
    free(id: number): void {
        if (this.registry.contains(id)) this.registry.release(id, 1);
    }

    /** How many ids are allocated (alive or in flight) right now. */
    get inUse(): number {
        return this.registry.inUse;
    }
}
