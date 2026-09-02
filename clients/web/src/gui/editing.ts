/**
 * The editing context of one arrangement — **whose history it is**.
 *
 * An undo stack belongs to the data, not to the view. Two windows over one
 * composition share a history, and an undo in either updates both; a stack
 * minted per editor sees only the gestures *that* editor made, so stepping one
 * of them reverts across the other's edits and writes a state nobody was ever
 * in. The crate placed its pile beside the data for exactly that reason, and
 * this is the same argument one level up: an editor asks the *arrangement* for
 * its editing context instead of building one of its own.
 *
 * What a context owns is everything that is true of the composition rather than
 * of a window: the held {@link Document}, the {@link History} over it, the index
 * from node id to what an intent writes to, the next id to mint, and the
 * version. What stays a view's is what a view can see — its selection, its
 * zoom, which layer the hand is on. Those never enter a history either, which is
 * the same line drawn twice.
 *
 * The context is reached through {@link Editing.of}, which keeps it in a
 * `WeakMap` keyed by the element: the arrangement is loose objects, so the
 * element is the only thing two editors are guaranteed to have in common. It
 * lives as long as the composition does and dies with it, which is what the
 * crate's own rule asks for — a history is session state, never serialized, and
 * it goes when the data goes.
 *
 * @module
 */

import { Document, History, Log } from "../document.ts";
import { Aggregate, Element, docIdOf, nextNodeId, toDocument } from "../form/index.ts";
import type { Member } from "../form/index.ts";
import { FIRST_VERSION } from "../form/document.ts";

/**
 * What a context needs of a view: something with a window it can bring back in
 * step. {@link gui.Editor} is the one that implements it.
 */
export interface View {
    /** Another view of this composition edited it: redraw this window. */
    adopt(): void;
}

/** What an index entry says an intent naming that node writes to. */
export type Indexed = [Aggregate | null, Member | null, Element];

/** Where a composition's context lives, keyed by the element it belongs to. */
const contexts = new WeakMap<Element, Editing>();

/**
 * One arrangement's held document, its history, and the index between them.
 *
 * Not built directly: {@link Editing.of} is the door, so two editors over one
 * composition cannot end up with two.
 */
export class Editing {
    /**
     * The pile: one editing context, one order over whatever is registered in
     * it. A dedicated roll or a standalone curve opened over this composition
     * registers itself **here**, which is what makes one undo walk one order
     * across all of them.
     */
    readonly history: History;
    /** The arrangement's face of that pile. */
    log: Log | null;
    /**
     * The crate's held document — opened once and kept, so a gesture costs the
     * edit rather than the composition.
     */
    doc: Document | null = null;
    /**
     * Whether the held document has to be derived from the arrangement again
     * before the next edit. Set wherever the tree moves by a route that is not
     * an intent.
     */
    rederive = false;
    /** node id → the arrangement object an intent naming that node writes to. */
    byNode = new Map<number, Indexed>();
    /** The next node id to mint for a node a gesture creates. */
    nextNode: number | null = null;
    /** The composition's version — the document half of the two counters. */
    version = FIRST_VERSION;
    /**
     * The views drawing this composition, weakly: an editor that goes away
     * takes its window with it, and a context does not keep one alive.
     */
    private readonly views = new Set<WeakRef<View>>();
    /**
     * How deep the current turn is, and whether anything moved in it. One
     * gesture can reach here twice — {@link Editing.turn} around an `apply`
     * that routes an `"undo"` into `undo`, which changes the composition on its
     * own — and the other windows want *one* redraw, not two.
     */
    private depth = 0;
    private movedInTurn = false;

    private constructor() {
        this.history = new History();
        this.log = new Log(0, 0, this.history);
    }

    /**
     * The context of this composition, made on first ask.
     *
     * Every editor over one element gets the same one — the whole point, and
     * the reason this is a static rather than a constructor.
     */
    static of(element: Element): Editing {
        let context = contexts.get(element);
        if (context === undefined) {
            context = new Editing();
            contexts.set(element, context);
        }
        return context;
    }

    /**
     * The log and the document, deriving the document if that is what it takes.
     *
     * The document is opened once and kept: rebuilding it per gesture handed
     * back the whole of what holding the tree in the crate had won. What a
     * rebuild was quietly doing is explicit here — `toDocument` stamps each
     * element with the id it keeps, so a re-derivation names the same nodes and
     * the history keeps its footing.
     */
    held(element: Element): [Log, Document] {
        this.log ??= new Log(0, 0, this.history);
        if (this.doc === null || this.rederive) {
            const document = toDocument(element, { version: this.version });
            this.doc?.free();
            this.doc = new Document(document);
            // **The index is added to, not replaced.** An element that has left
            // the tree — a clip a cut removed, the half a join swallowed — is
            // still named by the inverses in the pile, and putting it back is
            // placing *that object* again rather than rebuilding one from a
            // node (a rebuilt element is a different identity to every widget
            // and every pending edit). Clearing here made an undo of a cut, and
            // a redo of a split, quietly do nothing.
            this.index(element, null, null);
            this.nextNode = nextNodeId(element);
            this.rederive = false;
        }
        return [this.log, this.doc];
    }

    /**
     * Walk the arrangement collecting node id → what an intent writes to.
     *
     * A `place` needs the owning aggregate and the member handle (a placement is
     * the aggregate's, not the element's); everything else needs the element.
     * The walk mirrors `form/document.ts`'s own, which is what keeps the two
     * agreeing about what has an id.
     */
    index(element: Element, owner: Aggregate | null, member: Member | null): void {
        // The id belongs to the **placement** when there is one: a clip is a
        // window onto samples, so what an intent names is the window.
        const node = docIdOf(member ?? element);
        if (node !== null) this.byNode.set(node, [owner, member, element]);
        // A view opened over a *part* of this composition — a dedicated roll of
        // one track — must reach this context and not mint a second one, so the
        // walk claims what it passes. Only where there is none: a part that
        // already had a context of its own was being edited on its own terms,
        // and taking its history away without being asked is not this walk's to
        // do.
        if (!contexts.has(element)) contexts.set(element, this);
        if (element instanceof Aggregate) {
            for (const handle of element.handles) {
                this.index(handle.element, element, handle);
            }
        }
    }

    /**
     * Take a view into this composition's list, so an edit made in one window
     * can reach the others.
     */
    attach(view: View): void {
        for (const held of this.views) if (held.deref() === view) return;
        this.views.add(new WeakRef(view));
    }

    /** Drop a view whose window is gone. */
    detach(view: View): void {
        for (const held of this.views) {
            const alive = held.deref();
            if (alive === undefined || alive === view) this.views.delete(held);
        }
    }

    /**
     * Say that the composition changed in the turn being run.
     *
     * Not the notification: a turn can reach here more than once, and what the
     * other windows want is one redraw at the end of the gesture rather than
     * one per leg of it.
     */
    moved(): void {
        this.movedInTurn = true;
    }

    /**
     * One gesture, from whichever view made it.
     *
     * On the way out, every **other** view of this composition is told the data
     * it is drawing has moved — which nothing else would do: an acknowledgement
     * goes to the window whose gesture it answered, so a second window would go
     * on drawing a piece that had changed under it. Nested turns collapse into
     * one, because a gesture that reaches here twice is still one gesture.
     */
    turn<T>(source: View, run: () => T): T {
        this.depth += 1;
        try {
            return run();
        } finally {
            this.depth -= 1;
            if (this.depth === 0 && this.movedInTurn) {
                this.movedInTurn = false;
                for (const held of [...this.views]) {
                    const view = held.deref();
                    if (view === undefined) this.views.delete(held);
                    else if (view !== source) view.adopt();
                }
            }
        }
    }

    /** The next id for a node a gesture creates — a note added in a roll. */
    mint(element: Element): number {
        this.nextNode ??= nextNodeId(element);
        const node = this.nextNode;
        this.nextNode += 1;
        return node;
    }

    /**
     * Release the crate's handles. What the composition going away leaves
     * behind; a view closing is not an event of a history.
     */
    free(): void {
        this.log?.free();
        this.log = null;
        this.doc?.free();
        this.doc = null;
    }
}
