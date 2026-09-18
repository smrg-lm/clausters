// The page's id pools — what a mounted component allocates from.
//
// Mounting a bundle means allocating its symbols: a block of widget ids, a
// node id per declared node, a bus per declared bus, a buffer per declared
// sample. Two instances of one bundle must get different ones, or they collide
// in the shared namespaces of the one host and the one engine a page has. That
// is the whole reason the resolver takes an allocation instead of making one —
// the caller owns the id spaces, and this is the browser's caller.
//
// The pools live here, in `base/`, rather than on the `Server` in `defs/`, for
// two reasons: they are page state (one host, one engine, however many
// components), and the component run time must not reach the def builders at
// all — see `../runtime.ts`.
//
// **Widget ids are shared with the client** (`GuiHost` allocates from this
// same pool), because a widget id names a node of the one host's one widget
// namespace: a page that mounts components *and* opens windows from script
// must not hand the same id to both. Node ids, buses and buffers are a
// `Server`'s to size from `/server_query`.
//
// **On the page's engine they are one `IdSpaces`** (`pageIds`): the pools draw
// from it, a `Server` attached to the page's engine with no share of its own
// draws from it, and when the page's GUI host boots the page splits it once
// (`splitPageIds`) — the page keeps the first half and the host, which
// allocates on the same engine for its voices, its take monitor and the multitrack
// it plays, takes the second. It is the page's form of what a script does when
// it launches a host with `--id-share`.

import { IdSpaces, Registry, requireCore } from "./core.ts";
import type { IdShare } from "./ids.ts";

/**
 * The widget-id window, matching the client's own (`gui/ids.ts`): ids below
 * the base are the documented hand-picked range and never collide with
 * allocated ones.
 */
export const WIDGET_BASE = 1000;
export const WIDGET_CAPACITY = 1 << 20;

/**
 * One finite id space a mount draws from. `Registry` is the core's occupancy
 * map, so an id returned by `release` is allocatable again and a long-lived
 * page recycles inside a fixed window instead of climbing without bound.
 */
export interface Pool {
    alloc(width?: number): number;
    release(first: number, width?: number): void;
    /**
     * How many ids are allocated right now — what makes a leak visible. A
     * document that mounts and unmounts components over an afternoon must
     * come back to the occupancy it started at, and this is where that is
     * read (the acceptance for the unmount does exactly that).
     */
    readonly inUse: number;
}

/** The id spaces one mount needs. */
export interface Pools {
    widgets: Pool;
    nodes: Pool;
    controlBuses: Pool;
    audioBuses: Pool;
    buffers: Pool;
}

/**
 * A `Pool` over a bounded core `Registry`, throwing rather than returning a
 * silent `undefined` when the space is full — an exhausted id space is a
 * programming error, not a value to carry.
 */
export function pool(base: number, capacity: number, what: string): Pool {
    requireCore(`the ${what} id pool`);
    const registry = new Registry(base, capacity);
    return {
        alloc(width = 1) {
            const first = registry.alloc(width);
            if (first === undefined) {
                throw new Error(`clausters: out of ${what} ids (${capacity} in use)`);
            }
            return first;
        },
        release(first, width = 1) {
            registry.release(first, width);
        },
        get inUse() {
            return registry.inUse;
        },
    };
}

let instance: Pools | null = null;

/**
 * A fresh set of pools, owning its id ranges and sharing them with nobody.
 *
 * What `pagePools` hands out is one set for the whole document, which is right
 * for components that share the page's engine and host: they are one client
 * between them, and one pool is what keeps two instances of a bundle from
 * colliding. It stopped being right for everything the day the page could hold
 * more than one client — a host and an engine of one's own (`newGuiHost`,
 * `engine`) want an id space of their own too, since the whole point of an
 * independent client is that its ids are its own and may repeat another's.
 */
export function newPools(shape: PoolShape = ENGINE_SHAPE): Pools {
    requireCore("the id pools");
    // **One client's spaces, shaped by the server**: the node table's client
    // range, the audio buses above the engine's outputs, both bus spaces clear
    // of their GraphDef windows. The policy is the core's
    // (`clausters_core::ids`), the one every endpoint allocates by; widget ids
    // are the host's namespace, not the server's, and keep their own window.
    return poolsOver(
        new IdSpaces(
            shape.maxNodes,
            shape.audioBuses,
            shape.outputs,
            shape.controlBuses,
            shape.buffers,
            0,
            1,
        ),
    );
}

/** The pools as views of `ids`, with widget ids in their own window. */
function poolsOver(ids: IdSpaces): Pools {
    return {
        widgets: pool(WIDGET_BASE, WIDGET_CAPACITY, "widget"),
        nodes: space(ids, "nodes", "node"),
        controlBuses: space(ids, "control", "control bus"),
        audioBuses: space(ids, "audio", "audio bus"),
        buffers: space(ids, "buffers", "buffer"),
    };
}

/** What a server says about itself that decides the shape of the pools. */
export interface PoolShape {
    maxNodes: number;
    audioBuses: number;
    outputs: number;
    controlBuses: number;
    buffers: number;
}

/**
 * The in-page engine's own shape — the sizes it boots with — which is the
 * server the page's components share.
 */
export const ENGINE_SHAPE: PoolShape = {
    maxNodes: 8192,
    audioBuses: 1024,
    outputs: 2,
    controlBuses: 16384,
    buffers: 4096,
};

/** A `Pool` over one space of an `IdSpaces`, throwing rather than returning `undefined`. */
function space(
    ids: IdSpaces,
    name: "nodes" | "audio" | "control" | "buffers",
    what: string,
): Pool {
    return {
        alloc(width = 1) {
            try {
                return ids.alloc(name, width);
            } catch {
                throw new Error(`clausters: out of ${what} ids (${ids.inUse(name)} in use)`);
            }
        },
        release(first, width = 1) {
            try {
                ids.release(name, first, width);
            } catch {
                // A release of what this pool never handed out is ignored here, as
                // a registry's refused release always was on this door.
            }
        },
        get inUse() {
            return ids.inUse(name);
        },
    };
}

/**
 * The page's pools, made on first use. Every component sharing the page's
 * engine and host allocates from these, which is what keeps two instances of
 * one bundle apart. A client of its own takes `newPools` instead.
 */
export function pagePools(): Pools {
    instance ??= poolsOver(pageIds());
    return instance;
}

let pageSpaces: IdSpaces | null = null;
let pageSplit: IdShare | null = null;

/**
 * **The page engine's one id space**: what the page's pools and a `Server`
 * attached to the page's engine with no share of its own allocate from, so the
 * page is one client of its engine however many handles it holds.
 */
export function pageIds(): IdSpaces {
    requireCore("the page's id spaces");
    pageSpaces ??= new IdSpaces(
        ENGINE_SHAPE.maxNodes,
        ENGINE_SHAPE.audioBuses,
        ENGINE_SHAPE.outputs,
        ENGINE_SHAPE.controlBuses,
        ENGINE_SHAPE.buffers,
        0,
        1,
    );
    return pageSpaces;
}

/**
 * **Splits the page's id space with the page's GUI host**, once, and returns
 * the host's share.
 *
 * The page keeps the first half and every id it already holds; the host takes
 * the second. A second call answers the same share without splitting again,
 * since there is one host per page engine. Throws, changing nothing, when
 * something the page holds lies in the half it would give away.
 */
export function splitPageIds(): IdShare {
    if (pageSplit !== null) return pageSplit;
    pageIds().narrow(0, 2);
    pageSplit = { index: 1, of: 2 };
    return pageSplit;
}
