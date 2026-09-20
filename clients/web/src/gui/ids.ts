// Client-side allocation of GUI widget ids (mirrors `clausters/gui/ids.py`).
//
// Widget ids name nodes of the host's one widget namespace, exactly as node
// ids name slots of the audio server's node table -- so this allocator is the
// GUI sibling of `NodeIdAllocator`. It is built on the core's `WidgetIds`,
// which is that same occupancy map **plus a second door**, and the two doors
// are the thing to understand here:
//
// - **A lease** (`alloc`) is what a hand-built GuiDef takes: an id for a widget
//   nothing names, handed out in order and returned by `free`.
// - **A name** (`idFor`) is what a view takes: an id asked for by saying what
//   it draws -- the structure's identity in the history, the role the widget
//   plays, and which one it is -- which gives back the **same number** for as
//   long as that name keeps being drawn. A leased id changes on every redraw,
//   so anything in flight across one lands on the wrong widget: an edit-back
//   the owner has not answered yet, a correction travelling the other way, the
//   screen state of a widget that no longer exists under that number.
//
// Both doors take from one occupancy map, which is what makes them impossible
// to collide -- and an anonymous `free` deliberately cannot take back a named
// id, so a redefine freeing a subtree widget by widget does not hand a live
// name's number to somebody else.
//
// Three more things are worth spelling out:
//
// - **Bounded, so the ids recycle.** Ids come out of a fixed window
//   `[base, base + capacity)` and, once the high-water mark reaches the top,
//   reuse the ones that were returned. The numeric id space never climbs
//   without bound over a live session.
// - **The client drives the recycle.** A node id returns to the pool when the
//   server reports the node's death (`/node_end`); a widget id has no such
//   side-channel, so a leased one returns when the client frees the widget
//   (`GuiHost`'s `free`/`close`, and a redraw re-defining a window), and a
//   named one returns when a draw stops asking for it (`retire`).
// - **A draw names its drawer.** One table serves a whole host and a host
//   carries more than one drawer, so `begin`/`retire` take an `owner` handed
//   out by `owner`. Without it either drawer would retire the other's widgets
//   simply by redrawing.

import { AllocationError } from "../errors.ts";
import { WidgetIds, requireCore } from "../base/core.ts";
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
 * tracked until `free` returns it, which makes it allocatable again -- so a
 * long session that opens and closes many windows recycles ids within a fixed
 * window instead of climbing without bound.
 */
export class GuiIdAllocator {
    private registry: WidgetIds;

    /**
     * Over `[base, base + capacity)`, or one slice of it when a host has more
     * than one client naming widgets on it (`IdShare`) -- a driving client
     * drawing into a page that holds a client of the same host.
     */
    constructor(
        base: number = BASE_ID,
        capacity: number = CAPACITY,
        share?: IdShare,
    ) {
        requireCore("a widget id allocator");
        this.registry = new WidgetIds(...shareOf(base, capacity, share));
    }

    /**
     * A fresh drawer, for the `begin`/`retire` cycle below.
     *
     * One table serves the whole host, so a drawer is a number the table hands
     * out rather than one a caller invents: two of them inventing their own
     * would eventually pick the same one, and each would then take back the
     * other's widgets by redrawing.
     */
    owner(): number {
        return this.registry.owner();
    }

    /**
     * A fresh id, unique across everything this allocator names. Throws when
     * the whole window is live at once -- a client bug (that many widgets
     * never coexist; freed ones recycle).
     */
    alloc(): number {
        return this.checked(this.registry.alloc());
    }

    /**
     * The id `owner` draws `(structure, role, key)` with -- the **same** one for
     * as long as that name keeps being drawn.
     *
     * Throws on exhaustion, as `alloc` does and for the same reason: a name
     * that cannot be given a number is a client bug, not a value to handle.
     */
    idFor(owner: number, structure: number, role: string, key: string): number {
        return this.checked(this.registry.idFor(owner, structure, role, key));
    }

    /**
     * The id that draws `(structure, role, key)` **if it already has one** -- no
     * minting, and no effect on any draw. The inverse a view asks when it needs
     * to know what is drawing something.
     */
    idOf(structure: number, role: string, key: string): number | undefined {
        return this.registry.idOf(structure, role, key);
    }

    /** Give one name's id back, outside any draw. Answers the id released. */
    forget(structure: number, role: string, key: string): number | undefined {
        return this.registry.forget(structure, role, key);
    }

    /**
     * Start `owner`'s draw: every name it asks for until `retire` counts as
     * still drawn. Another drawer's cycle is untouched.
     */
    begin(owner: number): void {
        this.registry.begin(owner);
    }

    /**
     * End `owner`'s draw and take back every name of that owner's it did not
     * ask for, answering the ids released, ascending.
     */
    retire(owner: number): number[] {
        return Array.from(this.registry.retire(owner));
    }

    /**
     * Drop every name and every id: the table as it was made.
     *
     * A client reset. Only an id space nothing outside it holds may be cleared
     * -- an editor's private one before it has a host, never a live host's.
     */
    clear(): void {
        this.registry.clear();
    }

    private checked(id: number | undefined): number {
        if (id === undefined) {
            throw new AllocationError(
                "out of gui widget ids: the id window is fully in use " +
                    "(freed widgets recycle their ids -- this many live at once " +
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
        if (this.registry.contains(id)) this.registry.release(id);
    }

    /** How many ids are allocated right now, leased and named together. */
    get inUse(): number {
        return this.registry.inUse;
    }

    /** How many of them answer to a name. */
    get named(): number {
        return this.registry.named;
    }
}
