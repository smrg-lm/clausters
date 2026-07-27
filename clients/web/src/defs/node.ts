// Nodes (synths and groups) and client-side id allocation.
//
// The server's node tree: the root group is id 0; clients allocate positive
// ids. Add actions match the server: head/tail of a group, before/after a
// node, or replace. `Synth` and `Group` are flat handles holding an id; the
// `Server` does the OSC.
//
// (`Node` shadows the DOM's global of the same name inside a module that
// imports it. The name is the one the protocol and every other client use, so
// it stays; reach for `globalThis.Node` in the rare page that needs both.)

import { AllocationError } from "../errors.ts";
import { Registry, nodeIdPartition } from "../base/core.ts";

export const ROOT_NODE_ID = 0;

/** Where `/s_new`/`/g_new` places the new node relative to its target. */
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

export class Node {
    readonly id: number;
    /**
     * The `Server` that created this handle (set by `server.synth` and
     * friends), so `free` knows where to send without being told.
     */
    readonly server?: { free(...nodes: NodeLike[]): void };

    constructor(id: number, server?: { free(...nodes: NodeLike[]): void }) {
        this.id = id;
        this.server = server;
    }

    /**
     * Free this node now (`/n_free`) — the way to cut something whose life
     * is long. Sends through the server that created the handle.
     */
    free(): void {
        if (!this.server) {
            throw new Error(
                `node ${this.id} has no server: free it with server.free(node)`,
            );
        }
        this.server.free(this);
    }
}

export class Synth extends Node {
    readonly defname: string;

    constructor(
        id: number,
        defname: string,
        server?: { free(...nodes: NodeLike[]): void },
    ) {
        super(id, server);
        this.defname = defname;
    }
}

export class Group extends Node {}

/**
 * The registry of the client's node-id range.
 *
 * Node ids name slots of a finite boot-time resource (the server's node
 * table), so the allocator is an occupancy map, not a counter: every id
 * handed out stays tracked until the server reports the node's death
 * (`/n_end`, fed in through `free`), which makes it allocatable again — the
 * space never exhausts while nodes keep dying.
 *
 * It carries no range of its own: the client range is a property of the
 * server (the partition scales from `--max-nodes`), so the `Server` sizes it
 * through `nodeIdPartition`, the same formula the server applies.
 */
export class NodeIdAllocator {
    private registry: Registry;

    constructor(base: number, capacity: number) {
        this.registry = new Registry(base, capacity);
    }

    /** The allocator for a server whose node table holds `maxNodes` slots. */
    static forMaxNodes(maxNodes: number): NodeIdAllocator {
        const p = nodeIdPartition(maxNodes);
        return new NodeIdAllocator(p.clientBase, p.clientCapacity);
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
                    "(nodes are recycled when their /n_end arrives)",
            );
        }
        return id;
    }

    /**
     * Returns `id` to the pool — called when its `/n_end` arrives. Ids
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
