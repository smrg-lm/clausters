/**
 * The editing context of one structure — **whose history it is**.
 *
 * An undo stack belongs to the data, not to the view. Two windows over one
 * composition share a history, and an undo in either updates both; a stack
 * minted per editor sees only the gestures *that* editor made, so stepping one
 * of them reverts across the other's edits and writes a state nobody was ever
 * in. The crate placed its pile beside the data for exactly that reason, and
 * this is the same argument one level up: an editor asks the *data* for its
 * editing context instead of building one of its own.
 *
 * What a context owns is everything that is true of the work rather than of a
 * window: the {@link History} over it, the version, and the list of views to
 * tell when one of them edits. What stays a view's is what a view can see — its
 * selection, its zoom, which layer the hand is on. Those never enter a history
 * either, which is the same line drawn twice.
 *
 * The context is reached through {@link Editing.of}, which keeps it in a
 * `WeakMap` keyed by the structure: what is edited is loose objects, so the
 * object itself is the only thing two editors are guaranteed to have in common.
 * It lives as long as the data does and dies with it, which is what the crate's
 * own rule asks for — a history is session state, never serialized, and it goes
 * when the data goes.
 *
 * **The arrangement's context is a subclass**, not this one: a held `Document`,
 * the node index and the id to mint next are the tree's and live with
 * `FormEditor` ({@link FormEditing}). What is here is what is true of editing
 * anything.
 *
 * @module
 */

import { History } from "../../document.ts";
import type { Intent } from "../../document.ts";

/**
 * The version an unedited context is at. One rather than zero, because zero is
 * what an edit means by *unstated* when it names the state it was made against
 * — the same reservation the GUI host's sequence numbers make.
 *
 * It is the same number as `form/document.ts`'s `FIRST_VERSION` and deliberately
 * not the same symbol: that one is what a **file** says its version is, this one
 * is what an editing context counts from.
 */
export const FIRST_VERSION = 1;

/**
 * What a context needs of a view: something with a window it can bring back in
 * step. {@link Editor} is the one that implements it.
 */
export interface Adopting {
    /**
     * Another view of this composition edited it: bring this window in step —
     * as props, or as a whole redraw when `whole`.
     */
    adopt(intents: readonly Intent[], whole: boolean): void;
}

/** Where a structure's context lives, keyed by the object it belongs to. */
export const contexts = new WeakMap<object, Editing>();

/**
 * One structure's history, and the views drawing it.
 *
 * Not built through `new`: {@link Editing.of} is the door, so two editors over
 * one thing cannot end up with two.
 */
export class Editing {
    /**
     * The pile: one editing context, one order over whatever is registered in
     * it. A dedicated roll or a standalone curve opened over this composition
     * registers itself **here**, which is what makes one undo walk one order
     * across all of them.
     */
    readonly history: History;
    /**
     * The version — the counter a view reports to its host and the host names
     * back on its next gesture. It moves on every edit and on every redefine.
     */
    version = FIRST_VERSION;
    /**
     * The views drawing this composition, weakly: an editor that goes away
     * takes its window with it, and a context does not keep one alive.
     */
    /**
     * What each structure was registered in the pile as. One identity per
     * structure and not per view: two windows over one thing are one structure
     * in the order, and minting a second identity for the second window would
     * leave its undo walking legs that name somebody else.
     */
    protected readonly structures = new Map<object, number>();
    protected readonly views = new Set<WeakRef<Adopting>>();
    /**
     * How deep the current turn is, and whether anything moved in it. One
     * gesture can reach here twice — {@link Editing.turn} around an `apply`
     * that routes an `"undo"` into `undo`, which changes the composition on its
     * own — and the other windows want *one* redraw, not two.
     */
    protected depth = 0;
    /**
     * What the turn being run did: the intents it projected, and whether it
     * changed **which widgets exist**. The two are answered differently by the
     * other windows, which is the whole reason they are collected rather than
     * reduced to a bit.
     */
    protected intents: Intent[] = [];
    protected structural = false;
    protected changedInTurn = false;

    constructor() {
        this.history = new History();
    }

    /**
     * The context of this composition, made on first ask.
     *
     * Every editor over one element gets the same one — the whole point, and
     * the reason this is a static rather than a constructor.
     */
    static of<T extends Editing>(this: new () => T, structure: object): T {
        let context = contexts.get(structure);
        if (context === undefined) {
            context = new this();
            contexts.set(structure, context);
        }
        return context as T;
    }

    /**
     * Take an unnamed structure into this history and get its identity — the
     * crate's `History.register`. {@link Editing.identity} is the door an editor
     * uses; this one is for a caller registering something it will route itself.
     */
    register(domain: string): number {
        return this.history.register(domain);
    }

    /**
     * This structure's identity in the pile, minted on first ask.
     *
     * **Once per structure, not once per view.** Two windows over one thing are
     * one structure in the undo order, so a second identity for the second
     * window would leave its undo walking legs that name somebody else — which
     * looks exactly like a dead button.
     */
    identity(structure: object, domain: string): number {
        let found = this.structures.get(structure);
        if (found === undefined) {
            found = this.history.register(domain);
            this.structures.set(structure, found);
        }
        return found;
    }

    /**
     * Take a view into this data's list, so an edit made in one window can reach
     * the others.
     */
    attach(view: Adopting): void {
        for (const held of this.views) if (held.deref() === view) return;
        this.views.add(new WeakRef(view));
    }

    /** Drop a view whose window is gone. */
    detach(view: Adopting): void {
        for (const held of this.views) {
            const alive = held.deref();
            if (alive === undefined || alive === view) this.views.delete(held);
        }
    }

    /**
     * Say that the data changed in the turn being run.
     *
     * Not the notification: a turn can reach here more than once, and what the
     * other windows want is one answer at the end of the gesture rather than
     * one per leg of it.
     */
    changed(): void {
        this.changedInTurn = true;
    }

    /**
     * One intent this turn wrote onto the data.
     *
     * The other windows adopt these as **props** — the placement, the length,
     * the notes — which is what keeps a foreign edit from costing a redefine. A
     * redefine rebuilds every widget and drops what the host had in flight, so
     * doing it per edit makes a window flicker under a hand that is not even in
     * it.
     */
    moved(intent: Intent): void {
        this.intents.push(intent);
        this.changedInTurn = true;
    }

    /**
     * Say that the turn changed **which widgets exist** — a cut, a split, a
     * join, an undo of one. This is the case no prop can carry: a widget that
     * was not there a moment ago is not a value, so the other windows have to
     * be redrawn whole.
     */
    restructured(): void {
        this.structural = true;
        this.changedInTurn = true;
    }

    /**
     * One gesture, from whichever view made it.
     *
     * On the way out, every **other** view of this data is told what it is
     * drawing has moved — which nothing else would do: an acknowledgement
     * goes to the window whose gesture it answered, so a second window would go
     * on drawing a piece that had changed under it. Nested turns collapse into
     * one, because a gesture that reaches here twice is still one gesture.
     */
    turn<T>(source: Adopting, run: () => T): T {
        this.depth += 1;
        try {
            return run();
        } finally {
            this.depth -= 1;
            if (this.depth === 0) {
                const { intents, structural, changedInTurn: changed } = this;
                this.intents = [];
                this.structural = false;
                this.changedInTurn = false;
                if (changed) {
                    // A turn that changed something and projected no intent is
                    // one nothing here can describe — a trim, a patch cord, a
                    // gesture applied to the objects directly — so the honest
                    // answer for the other windows is the whole picture.
                    const whole = structural || intents.length === 0;
                    for (const held of [...this.views]) {
                        const view = held.deref();
                        if (view === undefined) this.views.delete(held);
                        else if (view !== source) view.adopt(intents, whole);
                    }
                }
            }
        }
    }

    /**
     * Release the crate's handles. What the data going away leaves behind; a
     * view closing is not an event of a history.
     */
    free(): void {
        this.history.free();
    }
}
