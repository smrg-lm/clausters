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
     * Another view of this composition edited it: bring this window in step.
     *
     * Neither argument is read by anything that implements this. They are the
     * seam for a view that could answer an intent as a prop instead of redrawing
     * — see {@link Editing.moved} for why nothing needs to.
     */
    adopt(intents: readonly Intent[], whole: boolean): void;
    /**
     * The data changed in a turn — this view's own gesture, another's, or a
     * step of the history.
     *
     * Separate from {@link Adopting.adopt}, which is about *drawing*: a page
     * that sounds a piece or writes a file wants to be told whoever made the
     * edit, and the window that made it is not exempt from having changed.
     */
    dataChanged?(): void;
    /**
     * Put back the legs of a history step that name **this** participant's
     * structure, and say whether anything moved.
     *
     * What {@link Editing.distribute} hands round. A participant need not draw
     * anything: a `Score` answers this and has no window at all.
     */
    projectLegs(legs: readonly unknown[]): boolean;
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
    protected readonly attached = new Set<WeakRef<Adopting>>();
    /**
     * How deep the current turn is, and whether anything moved in it. One
     * gesture can reach here twice — {@link Editing.turn} around an `apply`
     * that routes an `"undo"` into `undo`, which changes the composition on its
     * own — and the other windows want *one* redraw, not two.
     */
    protected depth = 0;
    /**
     * The intents the turn being run projected onto the composition. Carried to
     * {@link Adopting.adopt} for a view that can answer one as a **prop**; no
     * view does today, and what they are still read for is the one bit `adopt`
     * acts on — a turn that projected none is one nothing here can describe.
     */
    protected intents: Intent[] = [];
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
     * Hand a step's legs round **everything registered here** and say whether
     * anything moved.
     *
     * One entry can name several structures — a stroke over a take and a bend of
     * the curve above it are one order, and so is an edit to a page beside a
     * lane — so the step is offered to every participant and each takes the
     * legs naming the structure it holds. Whoever is walking is included whether or not it is in the
     * list, since a structure with no window open still holds legs the step may
     * name.
     *
     * It lives here rather than on the editor because a participant need not be
     * one: what this asks of a thing is {@link Adopting.projectLegs}, and a
     * `Score` answers it without drawing anything.
     */
    distribute(legs: readonly unknown[], walker: Adopting): boolean {
        const views = this.views();
        const walkers = views.includes(walker) ? views : [walker, ...views];
        let stepped = false;
        for (const view of walkers) stepped = view.projectLegs(legs) || stepped;
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
     * One intent this turn wrote onto the data — a {@link Editing.changed} that
     * says *what* changed.
     *
     * **No view answers one as a prop today**, and the doc comment used to
     * claim otherwise: adopting the placement or the length instead of redrawing
     * is what this was collected for, and it stopped being needed when
     * {@link Application.publish} stopped sending differences — the host
     * reconciles a whole tree now and keeps the screen state that a redefine
     * used to drop, so there is nothing left for a prop to save.
     *
     * What the list is still read for is its length, in {@link Editing.turn}: a
     * turn that changed something and projected no intent is one nothing here
     * can describe.
     */
    moved(intent: Intent): void {
        this.intents.push(intent);
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
                const { intents, changedInTurn: changed } = this;
                this.intents = [];
                this.changedInTurn = false;
                if (changed) {
                    // A turn that changed something and projected no intent is
                    // one nothing here can describe — a trim, a patch cord, a
                    // gesture applied to the objects directly — so the honest
                    // answer for the other windows is the whole picture.
                    const whole = intents.length === 0;
                    for (const held of [...this.attached]) {
                        const view = held.deref();
                        if (view === undefined) this.attached.delete(held);
                        else if (view !== source) view.adopt(intents, whole);
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
