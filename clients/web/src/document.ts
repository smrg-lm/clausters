/**
 * The document: the composition's authoritative model, and the one place an
 * edit is applied.
 *
 * The model lives in a Rust crate (`crates/clausters-document`) and every
 * client binds that one — this client, the Python client, and a `standalone`
 * GUI host with no language attached. **The tree stays there**: a client opens
 * a {@link Document}, applies intents to it, and asks for the JSON when it
 * actually wants the JSON.
 *
 * That is not how this started. The first binding passed the whole document in
 * and took the whole new one back, which cost a serialization of the entire
 * composition per edit — 205 ms for one placement on a 10240-event piece,
 * whatever the edit touched. It is not an accessor handle either: the same
 * three verbs, plus `snapshot`.
 *
 * The discipline is unchanged and is the point: the crate is the **only** thing
 * that applies an intent, so no client can apply an edit and then report what
 * it did — which is what would let three clients mean three different things by
 * the same gesture.
 *
 * @module
 */

import { Document as CoreDocument, Log as CoreLog } from "./core/clausters_core_web.js";
import { loadCore } from "./base/core.ts";

/**
 * How {@link Log} reaches the wasm object inside a {@link Document} without
 * that object becoming part of the public surface. Module-private on purpose:
 * the two classes are peers here and nowhere else.
 */
const coreOf = Symbol("core");

/** A node's identity within a document. Stable across edits. */
export type NodeId = number;

/**
 * An edit, in the owner's terms and stating the value it **results in** — never
 * an increment. That is what makes it idempotent (a resend over a lossy leg is
 * harmless) and what makes replay unnecessary: a view that drew an edit
 * optimistically leaves its picture standing over whatever arrives.
 */
export type Intent =
    | { intent: "place"; node: NodeId; offset: number; dur?: number }
    | { intent: "configure"; node: NodeId; config: unknown }
    | { intent: "setmembers"; node: NodeId; members: unknown[] }
    | { intent: "writesamples"; node: NodeId; start: number; values: number[] };

/**
 * The state an edit was made against.
 *
 * Omit it — or pass a `version` of zero — to apply unchecked, which is what a
 * script that just read the document wants. An edit naming a version the
 * document has left behind comes back refused and marked stale.
 */
export interface Against {
    version: number;
    generation?: number;
}

/**
 * What applying did, and what the document now says.
 *
 * There is no success flag to branch on: `effective` is the edit describing the
 * document as it now stands, so *applied*, *applied transformed* (a snap, a
 * clamp) and *refused* are one shape — a refusal is simply the previous value
 * handed back.
 */
export interface Outcome {
    /** The edit describing the document as it now stands. */
    effective: Intent;
    /** Whether the document changed. */
    applied: boolean;
    /** Why it was refused or transformed, when the owner had something to say. */
    reason: string | null;
    /**
     * Whether the refusal was staleness rather than a rule. The mechanism does
     * not read it; it exists because the two say different things to a person —
     * a rule means *not here*, staleness means *someone else changed this*.
     */
    stale: boolean;
}

/** A document: a monotonic version and a root node. Never zero — zero is what
 * an edit means by *unstated* when it names the state it was made against. */
export interface ClaustersDocument {
    version: number;
    root: unknown;
}

/** What applying an edit gives back. */
export interface Applied {
    document: ClaustersDocument;
    outcome: Outcome;
}

/** One piece of material a selection landed on. */
export interface Resolved {
    /** The element the span belongs to. */
    node: NodeId;
    /** Its material. */
    source: number;
    /** Which generation of that material this was resolved against. */
    generation: number;
    /** The span within the source, in frames: trim and placement both applied. */
    range: { start: number; end: number };
    /** Where this piece starts inside the selection, in frames. */
    at: number;
}

/** What is selected: a span of time, and whatever narrows it. A selection that
 * is only a span is exactly the two numbers the wire has always carried. */
export interface Selection {
    start: number;
    len: number;
    nodes?: NodeId[];
    value?: { min: number; max: number };
    bins?: { low: number; high: number };
    mask?: { cols: number; rows: number; bits: number[] };
}

/**
 * One composition, held by the crate.
 *
 * ```ts
 * const doc = await Document.open(json);
 * doc.apply({ intent: "place", node: 3, offset: 4 });
 * const saved = doc.snapshot();
 * doc.free();
 * ```
 */
export class Document {
    #inner: CoreDocument;

    private constructor(inner: CoreDocument) {
        this.#inner = inner;
    }

    /** @internal — how {@link Log} reaches the same wasm object. */
    get [coreOf](): CoreDocument {
        return this.#inner;
    }

    /**
     * Opens a document from its JSON, or an empty composition from nothing.
     *
     * @throws if the JSON is not a document.
     */
    static async open(document?: ClaustersDocument): Promise<Document> {
        await loadCore();
        return new Document(
            new CoreDocument(document === undefined ? undefined : JSON.stringify(document)),
        );
    }

    /**
     * The whole tree as JSON — to save it, or to rebuild the client's own
     * objects from it. The one call still the size of the composition, and it
     * is asked for rather than paid on every edit.
     */
    snapshot(): ClaustersDocument {
        return JSON.parse(this.#inner.snapshot()) as ClaustersDocument;
    }

    /** The monotonic version, bumped by every applied edit. Never zero. */
    get version(): number {
        return Number(this.#inner.version);
    }

    /**
     * Apply one edit.
     *
     * @param intent - the edit, stating the resulting value.
     * @param options - `against`, the state the edit was made against (omit to
     *   apply unchecked), and `quant`, the musical grid a placement snaps to in
     *   beats (`0` snaps nothing).
     * @returns what applying did. The document changed in place.
     * @throws if the intent will not parse.
     */
    apply(intent: Intent, options: { against?: Against; quant?: number } = {}): Outcome {
        return JSON.parse(
            this.#inner.apply(
                JSON.stringify({
                    intent,
                    against: options.against ?? null,
                    quant: options.quant ?? 0,
                }),
            ),
        ) as Outcome;
    }

    /**
     * Resolve a selection to the spans of material underneath it — placement,
     * trim and the clamp at both ends already applied.
     *
     * @param selection - what is selected.
     * @param framesPerBeat - the bridge between the arrangement's beats and the
     *   material's frames. Supplied rather than derived: tempo is the caller's,
     *   the arithmetic is the crate's.
     * @param inBeats - whether the selection's numbers are beats rather than
     *   frames on the shared axis.
     * @returns the spans, in tree order. Empty when nothing material was
     *   underneath — a group and a generator are in the way of a selection, not
     *   under it.
     */
    resolve(selection: Selection, framesPerBeat: number, inBeats = false): Resolved[] {
        return JSON.parse(
            this.#inner.resolve(JSON.stringify({ selection, framesPerBeat, inBeats })),
        ) as Resolved[];
    }

    /** Release the document. Idempotent. */
    free(): void {
        this.#inner.free();
    }
}

/**
 * Apply one edit to a document given and returned **by value**.
 *
 * The convenience form, built out of {@link Document} — open, apply, snapshot,
 * free — for a caller that has a document in hand and wants the edited one
 * back. It costs a serialization of the whole composition either way, which is
 * why it is a wrapper rather than the binding: an editor applying a gesture per
 * drag holds a `Document` instead and pays nothing per edit.
 */
export async function applyIntent(
    document: ClaustersDocument,
    intent: Intent,
    options: { against?: Against; quant?: number } = {},
): Promise<Applied> {
    const doc = await Document.open(document);
    try {
        const outcome = doc.apply(intent, options);
        return { document: doc.snapshot(), outcome };
    } finally {
        doc.free();
    }
}

/**
 * Resolve a selection against a document given by value — the convenience form
 * of {@link Document.resolve}.
 */
export async function resolveSelection(
    document: ClaustersDocument,
    selection: Selection,
    framesPerBeat: number,
    inBeats = false,
): Promise<Resolved[]> {
    const doc = await Document.open(document);
    try {
        return doc.resolve(selection, framesPerBeat, inBeats);
    } finally {
        doc.free();
    }
}

/** One move forward: an ordinary edit, or a deterministic operation the owner
 * re-runs rather than replays — which is what makes a redo of an edit over a
 * million samples cost a few bytes. */
export type Step = { edit: Intent } | { recompute: unknown };

/** What an undo did. The document changed in place. */
export interface Undone {
    /** The inverses, in the order they were applied. */
    undone: Intent[];
}

/** What a redo did, and what it could not. The document changed in place. */
export interface Redone {
    /**
     * The steps from the first one the crate **cannot perform** onward — a
     * deterministic operation kept as its parameters, which you re-run because
     * the crate holds no algorithms. It stops at the first rather than skipping
     * it, so a later edit is never applied over a state the operation before it
     * was meant to produce. Usually empty.
     */
    remaining: Step[];
}

/**
 * The undo history of one document.
 *
 * Undo belongs with the document and not with a view: a view's log sees only
 * the gestures *it* made, so a script editing the arrangement, a second editor
 * or a re-render leaves it describing a document that has moved on — and undo
 * then writes a state nobody was ever in.
 *
 * It is an object for its own reason, beyond the one {@link Document} has: the
 * spill store. A bulk inverse leaves the log on purpose, so passing one by
 * value would carry every spilled span on every call, which is the cost
 * spilling exists to avoid.
 *
 * ```ts
 * const doc = await Document.open(json);
 * const log = await Log.open();
 * log.apply(doc, { intent: "place", node: 3, offset: 4 }, { label: "move the clip" });
 * log.undo(doc);   // exactly where it was
 * ```
 */
export class Log {
    #inner: CoreLog;

    private constructor(inner: CoreLog) {
        this.#inner = inner;
    }

    /**
     * Opens a log. `budget` is how many entries it keeps before the oldest
     * falls off and `spillAbove` how many `f32` values a sample payload must
     * reach before it leaves the log; either as 0 takes the crate's default.
     */
    static async open(budget = 0, spillAbove = 0): Promise<Log> {
        await loadCore();
        return new Log(new CoreLog(budget, spillAbove));
    }

    /**
     * Apply an edit **and record it**, in one call — the inverse has to be read
     * out of the document before the edit lands, so applying first and
     * recording second would record the wrong thing. Nothing is recorded unless
     * the document changed, so a refusal leaves no entry and neither does a
     * resend.
     */
    apply(
        document: Document,
        intent: Intent,
        options: { against?: Against; quant?: number; label?: string } = {},
    ): Outcome {
        return JSON.parse(
            this.#inner.apply(
                document[coreOf],
                JSON.stringify({
                    intent,
                    against: options.against ?? null,
                    quant: options.quant ?? 0,
                    label: options.label ?? "edit",
                }),
            ),
        ) as Outcome;
    }

    /**
     * Record an entry the document cannot supply the inverse for — the
     * destructive case, whose overwritten samples are not in the tree. This
     * applies nothing: the write has happened, and what is recorded is how to
     * put it back.
     *
     * `coalesce` merges into the entry before it when both touch the same node
     * the same way, so a run of small adjustments is one undo. You decide,
     * because only you know where the hand stopped.
     */
    record(
        forward: Step,
        backward: Intent,
        options: { label?: string; coalesce?: boolean } = {},
    ): void {
        this.#inner.record(
            JSON.stringify({
                forward,
                backward,
                label: options.label ?? "edit",
                coalesce: options.coalesce ?? false,
            }),
        );
    }

    /** Undo the last transaction, or `undefined` when there is nothing to undo. */
    undo(document: Document): Undone | undefined {
        const result = this.#inner.undo(document[coreOf]);
        return result === undefined ? undefined : (JSON.parse(result) as Undone);
    }

    /** Redo what was last undone, or `undefined` when there is nothing. */
    redo(document: Document): Redone | undefined {
        const result = this.#inner.redo(document[coreOf]);
        return result === undefined ? undefined : (JSON.parse(result) as Redone);
    }

    /** Whether there is anything to undo. */
    get canUndo(): boolean {
        return this.#inner.canUndo;
    }

    /** Whether there is anything to redo. */
    get canRedo(): boolean {
        return this.#inner.canRedo;
    }

    /** What an undo would be called, for a menu item. */
    get undoLabel(): string | undefined {
        return this.#inner.undoLabel;
    }

    /** What a redo would be called. */
    get redoLabel(): string | undefined {
        return this.#inner.redoLabel;
    }

    /** How many entries the log holds. */
    get length(): number {
        return this.#inner.len;
    }

    /** Forget everything, releasing what was spilled. */
    clear(): void {
        this.#inner.clear();
    }

    /** Release the log. Idempotent. */
    free(): void {
        this.#inner.free();
    }
}
