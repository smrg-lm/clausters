// Client-side allocation of GUI widget ids (mirrors `clausters/gui/ids.py`).
//
// Widget ids name nodes of the host's one widget namespace, exactly as node
// ids name slots of the audio server's node table — so this allocator is the
// GUI sibling of `NodeIdAllocator`, built on the same core `Registry`
// occupancy map. Two things are worth spelling out:
//
// - **Bounded, so the ids recycle.** `alloc` hands ids out of a fixed window
//   `[base, base + capacity)` and, once the high-water mark reaches the top,
//   reuses the ones `free` has returned. The numeric id space never climbs
//   without bound over a live session — which matters because a view that
//   redraws re-allocates its whole widget range each time.
// - **The client drives the recycle.** A node id returns to the pool when the
//   server reports the node's death (`/node_end`); a widget id has no such
//   side-channel, so it returns when the client frees the widget (`GuiHost`'s
//   `free`/`close`, and a redraw re-defining a window, which frees the old
//   subtree first).

import { AllocationError } from "../errors.ts";
import { Registry } from "../base/core.ts";
import { shareOf } from "../base/ids.ts";
import type { IdShare } from "../base/ids.ts";

/**
 * The first id the allocator hands out. Hand-picked ids below this never
 * collide with assigned ones (the documented `/gui_def` id convention).
 */
export const BASE_ID = 1000;

/**
 * The size of the id window. Far beyond any real count of simultaneously
 * live widgets, so the space recycles inside it without ever exhausting in
 * practice.
 */
export const CAPACITY = 1 << 20;

/**
 * The registry of a host client's widget-id space.
 *
 * An occupancy map, not a counter: every id handed out by `alloc` stays
 * tracked until `free` returns it, which makes it allocatable again — so a
 * long session that opens and closes many windows recycles ids within a fixed
 * window instead of climbing without bound.
 */
export class GuiIdAllocator {
    private registry: Registry;

    /**
     * Over `[base, base + capacity)`, or one slice of it when a host has more
     * than one client naming widgets on it (`IdShare`) — a driving client
     * drawing into a page that holds a client of the same host.
     */
    constructor(
        base: number = BASE_ID,
        capacity: number = CAPACITY,
        share?: IdShare,
    ) {
        this.registry = new Registry(...shareOf(base, capacity, share));
    }

    /**
     * A fresh id, unique across everything this allocator names. Throws when
     * the whole window is live at once — a client bug (that many widgets
     * never coexist; freed ones recycle).
     */
    alloc(): number {
        const id = this.registry.alloc(1);
        if (id === undefined) {
            throw new AllocationError(
                "out of gui widget ids: the id window is fully in use " +
                    "(freed widgets recycle their ids — this many live at once " +
                    "is a leak)",
            );
        }
        return id;
    }

    /**
     * Returns `id` to the pool. Ids outside this allocator's window (a
     * hand-picked id below the base) and ids not currently allocated are
     * ignored, so freeing is always safe.
     */
    free(id: number): void {
        if (this.registry.contains(id)) this.registry.release(id, 1);
    }

    /** How many ids are allocated right now. */
    get inUse(): number {
        return this.registry.inUse;
    }
}
