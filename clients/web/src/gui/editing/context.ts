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
 * whoever holds a tree. What is here is what is true of editing
 * anything.
 *
 * @module
 */

import { History } from "../../document.ts";

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
/** What can put one payload of a history step back onto a structure. */
export interface Applier {
    project(structure: never | object, payload: unknown): boolean;
}

export interface Adopting {
    /**
     * Another view of this composition edited it: bring this window in step.
     *
     * It carries nothing. It used to carry the turn's intents so a view could
     * adopt a placement or a length as a prop instead of redrawing — but what
     * {@link Editor.adopt} does is already props, one correction per widget and
     * never a redefine, so the intents would only have narrowed *which* widgets.
     * They were passed for months and read by nobody.
     */
    adopt(): void;
    /**
     * The data changed in a turn — this view's own gesture, another's, or a
     * step of the history.
     *
     * Separate from {@link Adopting.adopt}, which is about *drawing*: a page
     * that sounds a piece or writes a file wants to be told whoever made the
     * edit, and the window that made it is not exempt from having changed.
     */
    dataChanged?(): void;
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
    /**
     * What each structure was registered in the pile as, and **what can put an
     * edit back onto it**. The applier is held here because the pile's scope is
     * this context while a window's is a window: see {@link Editing.distribute}.
     */
    protected readonly structures = new Map<object, { id: number; applier: Applier | null }>();
    protected readonly attached = new Set<WeakRef<Adopting>>();
    /**
     * How deep the current turn is, and whether anything moved in it. One
     * gesture can reach here twice — {@link Editing.turn} around an `apply`
     * that routes an `"undo"` into `undo`, which changes the composition on its
     * own — and the other windows want *one* redraw, not two.
     */
    protected depth = 0;
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
     * This structure's identity in the pile, minted on first ask, with **what
     * can put an edit back onto it**.
     *
     * **Once per structure, not once per view.** Two windows over one thing are
     * one structure in the undo order, so a second identity for the second
     * window would leave its undo walking legs that name somebody else — which
     * looks exactly like a dead button.
     *
     * `applier` is anything answering `project(structure, payload)` — an editor
     * hands its {@link Domain}, and a `Score` hands itself. It is kept **here**,
     * beside the identity, because that is the scope the pile has: an entry
     * names a structure and the order over entries is global, so an entry that
     * only *some of the time* has somebody to apply it is an entry that blocks
     * every entry behind it. Registered once and kept, applying an edit stops
     * depending on whether a window happens to be open.
     */
    identity(structure: object, domain: string, applier: Applier | null = null): number {
        const found = this.structures.get(structure);
        if (found === undefined) {
            const id = this.history.register(domain);
            this.structures.set(structure, { id, applier });
            return id;
        }
        // Registered by something that could not apply (a caller that only
        // wanted the number); the first that can, wins the slot.
        if (found.applier === null && applier !== null) found.applier = applier;
        return found.id;
    }

    /**
     * What puts an edit back onto the structure this identity names, with the
     * structure itself — or `undefined` for one nothing registered.
     */
    applierOf(identity: number): { structure: object; applier: Applier } | undefined {
        for (const [structure, held] of this.structures) {
            if (held.id === identity && held.applier !== null) {
                return { structure, applier: held.applier };
            }
        }
        return undefined;
    }

    /**
     * Take a view into this data's list, so an edit made in one window can reach
     * the others.
     */
    attach(view: Adopting): void {
        for (const held of this.attached) if (held.deref() === view) return;
        this.attached.add(new WeakRef(view));
    }

    /** The views still alive, dropping the ones that are not. */
    views(): Adopting[] {
        const alive: Adopting[] = [];
        for (const held of [...this.attached]) {
            const view = held.deref();
            if (view === undefined) this.attached.delete(held);
            else alive.push(view);
        }
        return alive;
    }

    /**
     * Take one step off the pile and give back what each structure must apply —
     * `undefined` when there was nothing to take.
     *
     * The legs come **routed**: one entry per structure, its payloads in the
     * order it must apply them. Which side of an entry a direction reads and
     * which legs a structure owns are the crate's ({@link History.walk}),
     * because every client was writing both for itself.
     */
    step(direction: "undo" | "redo"): unknown[] | undefined {
        return this.history.walk(direction)?.legs;
    }

    /**
     * Put a step's legs back onto the structures they name, and say whether
     * anything moved.
     *
     * One entry can name several structures — a stroke over a take and a bend of
     * the curve above it are one order, and so is an edit to a page beside a
     * lane — so the legs come routed and each goes to whatever was registered
     * for that identity.
     *
     * **It asks the structures, not the windows**, and that is the whole of why
     * it is written this way. A pile is ordered and global to this context,
     * while a window comes and goes: when the applier was a *view*, a box
     * entered from a piece and then closed left an entry nobody could apply, the
     * step was refused, and — since a refused step puts the cursor back — every
     * edit behind it became unreachable too. The pile was not missing one step,
     * it was **blocked**. What can put an edit back is a structure and its
     * vocabulary, neither of which is on screen, so that is what
     * {@link Editing.identity} registers and this is what asks.
     *
     * `walker` is kept for callers that pass it and is not read: who drew the
     * gesture matters to the redraw, which is the turn's, not to this.
     */
    distribute(legs: readonly unknown[], _walker?: Adopting): boolean {
        let stepped = false;
        for (const leg of legs) {
            const named = leg as { structure?: number; payloads?: readonly unknown[] };
            const held = this.applierOf(Number(named.structure ?? -1));
            if (held === undefined) continue;
            for (const payload of named.payloads ?? []) {
                if (payload !== null && typeof payload === "object") {
                    stepped = held.applier.project(held.structure, payload) || stepped;
                }
            }
        }
        return stepped;
    }

    /** Drop a view whose window is gone. */
    detach(view: Adopting): void {
        for (const held of this.attached) {
            const alive = held.deref();
            if (alive === undefined || alive === view) this.attached.delete(held);
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
                const changed = this.changedInTurn;
                this.changedInTurn = false;
                if (changed) {
                    for (const held of [...this.attached]) {
                        const view = held.deref();
                        if (view === undefined) this.attached.delete(held);
                        else if (view !== source) view.adopt();
                    }
                    // ...and **every** view is told the data changed, the one
                    // that made the gesture included. See
                    // {@link Adopting.dataChanged} for why that is a different
                    // question from bringing a window in step.
                    // The source is told whether or not it is **attached**: a
                    // view attaches when its window opens, and an editor
                    // driving something off the data is entitled to be told
                    // before it is on screen.
                    const tell: Adopting[] = [];
                    for (const held of [...this.attached]) {
                        const view = held.deref();
                        if (view !== undefined) tell.push(view);
                    }
                    if (source !== null && !tell.includes(source)) tell.push(source);
                    for (const view of tell) view.dataChanged?.();
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
