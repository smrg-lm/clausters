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
// `Server`'s to size from `/server_info`, so a page doing both should hand its
// client's allocators to the mount rather than let the two pools overlap; the
// mount takes them as an argument for exactly that.

import { Registry } from "./core.ts";

/// The widget-id window, matching the client's own (`gui/ids.ts`): ids below
/// the base are the documented hand-picked range and never collide with
/// allocated ones.
export const WIDGET_BASE = 1000;
export const WIDGET_CAPACITY = 1 << 20;

/// The node-id base the server's client range starts at (scsynth convention,
/// `clausters_core::registry::NodeIdPartition`).
export const NODE_BASE = 1000;
export const NODE_CAPACITY = 1 << 15;

/// Buses and buffers: the bottom of each space, above the few a hand-written
/// def or a `boot.json` writes to by convention.
export const CONTROL_BUS_BASE = 64;
export const CONTROL_BUS_CAPACITY = 4096;
export const AUDIO_BUS_BASE = 64;
export const AUDIO_BUS_CAPACITY = 1024;
export const BUFFER_BASE = 32;
export const BUFFER_CAPACITY = 1024;

/// One finite id space a mount draws from. `Registry` is the core's occupancy
/// map, so an id returned by `release` is allocatable again and a long-lived
/// page recycles inside a fixed window instead of climbing without bound.
export interface Pool {
    alloc(width?: number): number;
    release(first: number, width?: number): void;
}

/// The id spaces one mount needs.
export interface Pools {
    widgets: Pool;
    nodes: Pool;
    controlBuses: Pool;
    audioBuses: Pool;
    buffers: Pool;
}

/// A `Pool` over a bounded core `Registry`, throwing rather than returning a
/// silent `undefined` when the space is full — an exhausted id space is a
/// programming error, not a value to carry.
export function pool(base: number, capacity: number, what: string): Pool {
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
    };
}

let instance: Pools | null = null;

/// The page's pools, made on first use. Every component on the page allocates
/// from these, which is what keeps two instances of one bundle apart.
export function pagePools(): Pools {
    instance ??= {
        widgets: pool(WIDGET_BASE, WIDGET_CAPACITY, "widget"),
        nodes: pool(NODE_BASE, NODE_CAPACITY, "node"),
        controlBuses: pool(CONTROL_BUS_BASE, CONTROL_BUS_CAPACITY, "control bus"),
        audioBuses: pool(AUDIO_BUS_BASE, AUDIO_BUS_CAPACITY, "audio bus"),
        buffers: pool(BUFFER_BASE, BUFFER_CAPACITY, "buffer"),
    };
    return instance;
}
