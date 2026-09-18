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
 * composition per edit — 205 ms for one placement on a 10240-event multitrack,
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

import {
    Document as CoreDocument,
    History as CoreHistory,
    domainCoalesceKey as coreDomainCoalesceKey,
    editingIntake as coreEditingIntake,
    editingStitch as coreEditingStitch,
    editingLoad as coreEditingLoad,
    domainEdit as coreDomainEdit,
} from "./core/clausters_core_web.js";
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
    | {
        intent: "writesamples";
        node: NodeId;
        /** Which channel of the node's samples the span belongs to (0 when omitted). */
        channel?: number;
        start: number;
        values: number[];
    };

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

/** One multitrack of samples a selection landed on. */
export interface Resolved {
    /** The element the span belongs to. */
    node: NodeId;
    /** Its samples. */
    source: number;
    /** Which generation of those samples this was resolved against. */
    generation: number;
    /** The span within the source, in frames: trim and placement both applied. */
    range: { start: number; end: number };
    /** Where this multitrack starts inside the selection, in frames. */
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

    /**
     * Open a document from its JSON (or an empty composition), **with the core
     * already loaded** — `await loadCore()` once, then build as many as you
     * like. {@link Document.open} is the same thing for a caller that has not.
     *
     * The synchronous door exists because an editor's gestures are
     * synchronous: an event handler applies an edit and answers the host in
     * the same turn, and an `await` in the middle of that is a window in which
     * a second gesture arrives against a document that is not open yet.
     */
    constructor(document?: string | ClaustersDocument) {
        this.#inner = new CoreDocument(
            document === undefined
                ? undefined
                : typeof document === "string"
                  ? document
                  : JSON.stringify(document),
        );
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
        return new Document(document);
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
     * What makes two edits over the arrangement *the same thing done the same
     * way* — the key a {@link History} coalesces on, or `""` when the intent
     * will not parse.
     *
     * It belongs to the document and not to the history because it is a
     * sentence in **this** vocabulary: the kind of edit and the node it names.
     * The pile reads no vocabulary, so a caller recording its own entries asks
     * the domain, which is what keeps a second spelling of the rule out of
     * every client.
     */
    static coalesceKey(intent: Intent): string {
        return CoreDocument.coalesceKey(JSON.stringify(intent));
    }

    /**
     * The edit that would put this node back the way it is — the inverse of
     * `intent`, read **before** anything is applied, or `undefined` when the
     * document cannot describe it (the node is gone, or its body holds nothing
     * of that shape).
     *
     * {@link Log.apply} does this for you and is what an ordinary edit wants.
     * This is for the caller that records its **own** entry — a leg of a
     * transaction spanning several structures, which nothing but the caller can
     * apply. For a `writesamples` it is the empty write rather than the span,
     * which is why a destructive caller reads the samples it is about to
     * overwrite instead of asking here.
     */
    inverse(intent: Intent): Intent | undefined {
        const result = this.#inner.inverse(JSON.stringify(intent));
        return result === undefined ? undefined : (JSON.parse(result) as Intent);
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
     * Resolve a selection to the spans of samples underneath it — placement,
     * trim and the clamp at both ends already applied.
     *
     * @param selection - what is selected.
     * @param framesPerBeat - the bridge between the arrangement's beats and the
     *   samples' frames. Supplied rather than derived: tempo is the caller's,
     *   the arithmetic is the crate's.
     * @param framesPerSecond - the same bridge for a length already measured in
     *   seconds — a take's, which no tempo moves. Both are needed because the
     *   document measures a placement in beats and what it places in the unit
     *   of that element's own data.
     * @param inBeats - whether the selection's numbers are beats rather than
     *   frames on the shared axis.
     * @returns the spans, in tree order. Empty when nothing with samples was
     *   underneath — a group and a generator are in the way of a selection, not
     *   under it.
     */
    resolve(
        selection: Selection,
        framesPerBeat: number,
        framesPerSecond: number,
        inBeats = false,
    ): Resolved[] {
        return JSON.parse(
            this.#inner.resolve(
                JSON.stringify({ selection, framesPerBeat, framesPerSecond, inBeats }),
            ),
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
    framesPerSecond: number,
    inBeats = false,
): Promise<Resolved[]> {
    const doc = await Document.open(document);
    try {
        return doc.resolve(selection, framesPerBeat, framesPerSecond, inBeats);
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
    /** What the entry that inverted was called. */
    label: string;
    /** The inverses, in the order they were applied. */
    undone: Intent[];
    /**
     * The legs of the step that belong to another structure, left alone — which
     * a `Log` never sees unless somebody else registered a structure in the
     * same history.
     */
    others: (Leg | RemainingLeg)[];
    /**
     * See {@link Redone.remaining}. Always empty here: an inverse is always an
     * edit, and the two directions answer the same shape so a caller reads one.
     */
    remaining: Step[];
    /**
     * The entries the walk passed over because nothing can invert them, by
     * label. See {@link Walked.skipped}.
     */
    skipped: string[];
}

/** What a redo did, and what it could not. The document changed in place. */
export interface Redone {
    /**
     * The intents the redo applied, in order — the same shape an undo answers
     * with, so a view projects a redo exactly as it projects an undo instead of
     * adopting the whole document and reconciling it against its own objects.
     */
    redone: Intent[];
    /**
     * The steps from the first one the crate **cannot perform** onward — a
     * deterministic operation kept as its parameters, which you re-run because
     * the crate holds no algorithms. It stops at the first rather than skipping
     * it, so a later edit is never applied over a state the operation before it
     * was meant to produce. Usually empty.
     */
    remaining: Step[];
    /** What the entry being redone was called. */
    label: string;
    /** See {@link Undone.others}. */
    others: (Leg | RemainingLeg)[];
    /**
     * The entries the walk passed over because nothing can invert them, by
     * label. See {@link Walked.skipped}.
     */
    skipped: string[];
}

/** One leg of an entry being recorded: what was done there, and how to undo it. */
export interface RecordedLeg {
    /** The identity {@link History.register} handed back. */
    structure: number;
    /**
     * A step — `{ edit }`, or `{ recompute }` for a deterministic operation the
     * owner re-runs rather than replays, which is what makes a redo of a
     * million-sample operation cost a few bytes.
     */
    forward: Step;
    /**
     * The inverse, a payload in that structure's own vocabulary — for the
     * arrangement, {@link Document.inverse} read before the edit landed.
     *
     * Omit it when nothing can put this leg back: an act with no inverse is
     * still recorded, marked, and walked past in both directions, with the walk
     * naming it in `skipped`. Recording it beats dropping it — a hole in the
     * history that announces itself is what lets a person understand why an
     * undo did not go where they expected.
     */
    backward?: unknown;
    /**
     * What makes two edits *the same thing done the same way*:
     * {@link Document.coalesceKey} for the arrangement, one verb and one key
     * for a curve. Absent never coalesces.
     */
    key?: string;
}

/**
 * Every vocabulary the document crate speaks — what a structure is registered
 * under, and what {@link domainCoalesceKey} dispatches on.
 *
 * Named here rather than spelled at each call site so a typo cannot quietly
 * mint a structure in a domain nobody reads.
 */
export const TREE = "tree";
/**
 * The **multitrack's** vocabulary — what a multitrack editor does, in
 * `MultitrackIntent`'s words. See {@link TREE}.
 *
 * A domain of its own rather than more of the tree's: the multitrack and the tree
 * are two descriptions, and one history holds both without either knowing the
 * other's words.
 */
export const MULTITRACK = "multitrack";
/** The break-point curve's vocabulary. See {@link TREE}. */
export const POINTS = "points";
/** A span of samples' vocabulary. See {@link TREE}. */
export const SAMPLES = "samples";
/** A timeline of events' vocabulary. See {@link TREE}. */
export const EVENTS = "events";

/**
 * The version an unedited document and an unedited multitrack are at.
 *
 * The counter moves when the description changes, so an edit made against a
 * version the document has left behind is refused as stale rather than applied
 * blind.
 */
export const FIRST_VERSION = 1;

/**
 * The session format this build writes — the crate's `session::FORMAT`.
 *
 * It moves when a reader that does not know the new shape would read a file
 * **wrongly** — never for an added field, which an older reader ignores and a
 * newer one defaults. **2** added a source whose samples are spans of other
 * sources, which is a tagged variant an older reader must fail on rather than
 * read as something else. **3** measures the multitrack in seconds where 2
 * measured it in beats, and states a tempo in beats per second; an older file is
 * migrated on reading (`Session.read`).
 *
 * A literal rather than a call into the core, deliberately: a `Session` is plain
 * data and `multitrack.ts` opens the core for nothing else, so reading the
 * number from wasm would make writing a session need an `await loadCore()` it
 * has never needed. What keeps the two in step instead is a **test** that asks
 * the crate (`sessionFormat`) and compares — which is the check that did not
 * exist when this said 1 and the crate had moved to 2.
 */
export const SESSION_FORMAT = 3;

/**
 * What makes two of a **domain's** edits *the same thing done the same way* —
 * the key a caller recording its own entry passes to {@link History.record},
 * or `""` when the payload is not written in that vocabulary (or the domain is
 * one the crate does not speak).
 *
 * A free function rather than a method because a caller here holds no structure
 * to ask: a curve, a span of samples and a timeline live in this page's own
 * memory, and only their *vocabulary* is the crate's.
 * {@link Document.coalesceKey} stays as it is — the arrangement's own door —
 * and this is the same rule for the domains that have no handle.
 */
export function domainCoalesceKey(domain: string, payload: unknown): string {
    return coreDomainCoalesceKey(domain, JSON.stringify(payload));
}

/**
 * What a reading of one gesture came to: the payloads, what an undo menu calls
 * them, and — where there is one — the inverse the gesture carried or the reason
 * it cannot be written.
 */
export interface Intake {
    /**
     * The edits, in the order they are applied. Empty for a tag this domain
     * does not answer for, which is the ordinary case rather than a failure.
     */
    payloads: unknown[];
    /** What an undo menu calls this gesture. */
    label: string;
    /**
     * The inverse, where the **gesture** carried it rather than the structure —
     * a stroke over samples, which arrives with the run it replaced.
     */
    inverse?: unknown;
    /**
     * Why a gesture this domain understands cannot be written, as the sentence
     * the user is shown.
     */
    refusal?: string;
}

/**
 * **What a gesture means**, in a structure's own vocabulary.
 *
 * A host reports a gesture as a tag and a flat list of values, and this is what
 * turns one into the payloads {@link domainEdit} will apply. One door over all
 * four domains — `"points"`, `"samples"`, `"events"`, `"multitrack"` — because
 * a host reports every gesture the same way, and because a fifth vocabulary then
 * has nowhere to grow.
 *
 * `request` carries what that domain reads: `values` always, `state` for the two
 * that need the structure (the multitrack, the timeline), `unitsPerBeat` and
 * `editable` for a roll, and `rate` and `sources` for a multitrack.
 *
 * No payloads and no refusal is "nothing to say" — a tag this domain does not
 * answer for — which is the ordinary answer rather than a failure.
 */
export function editingIntake(
    domain: string,
    tag: string,
    request: Record<string, unknown>,
): Intake {
    const answer = coreEditingIntake(domain, tag, JSON.stringify(request));
    if (!answer) return { payloads: [], label: "edit" };
    const taken = JSON.parse(answer) as Partial<Intake>;
    return { ...taken, payloads: taken.payloads ?? [], label: taken.label ?? "edit" };
}

/** One part of a resolved join: a run of one buffer, and its channel map. */
export interface StitchPart {
    /** The buffer this span is read from. */
    buffer: number;
    /** Its first frame. */
    start: number;
    /** How many frames it contributes. */
    frames: number;
    /** Frames of linear fade at its head. */
    fadeIn: number;
    /** Frames of linear fade at its tail. */
    fadeOut: number;
    /** One entry per channel of the join: the part's channel, or `-1`. */
    channels: number[];
}

/** What {@link editingStitch} answers: the buffer a join is. */
export interface Stitch {
    /** How wide the join is. */
    channels: number;
    /** Its rate, or `0` for the server's. */
    rate: number;
    /** How many frames it comes to. */
    frames: number;
    /** The parts, in the order they play. */
    parts: StitchPart[];
}

/**
 * **What a source made of spans comes to** — the buffer a join is, resolved
 * against the caller's table.
 *
 * One reading for every endpoint that realizes a minted join, so the width, the
 * spans and the channel map a narrow part fills the join with are the crate's
 * and not this client's. `held` is source id → `{ buffer, channels, frames }`.
 * `undefined` where there is nothing to make: not a join, or a part over a
 * source nobody loaded.
 */
export function editingStitch(
    source: Record<string, unknown>,
    held: Record<string, { buffer: number; channels: number; frames: number }>,
): Stitch | undefined {
    const answer = coreEditingStitch(JSON.stringify(source), JSON.stringify(held));
    if (!answer || answer === "null") return undefined;
    return JSON.parse(answer) as Stitch;
}

/** What {@link editingLoad} answers: a session's sources, as a load. */
export interface Loading {
    /** What each source became, by source id; 0 where the table states no width or length. */
    takes: Record<string, { buffer: number; channels: number; frames: number; path?: string }>;
    /** The steps to walk through a runner: each read and stitch, then its `/done`. */
    steps: unknown[];
    /** `[source, why]` for every source that will not load. */
    unresolved: [number, string][];
    /** The buffer numbers the load did not take. */
    unused: number[];
    /** Set, and nothing else is, when the request named no session. */
    error?: string;
}

/**
 * **A session's sources, loaded** — the steps that read every take and stitch
 * every join into the buffers set aside.
 *
 * `session` is the session as the crate writes it, `beside` the folder its
 * relative paths are read against, and `buffers` the numbers set aside: one per
 * source in the table is always enough.
 */
export function editingLoad(session: unknown, beside: string, buffers: number[]): Loading {
    return JSON.parse(coreEditingLoad(JSON.stringify({ session, beside, buffers }))) as Loading;
}

/** What {@link domainEdit} answers: both directions of one edit. */
export interface Edited {
    /** The structure as it now stands, in its own vocabulary. */
    state: unknown;
    /**
     * Whether anything moved. A resend states what is already there and is
     * applied by nobody and recorded by nobody.
     */
    applied: boolean;
    /** Why not, when the payload was refused for a rule rather than a resend. */
    reason?: string;
    /**
     * The payload that puts the structure back — read before the edit landed.
     * Absent when the structure cannot describe it, which is what makes an edit
     * unloggable rather than uninvertible.
     */
    current?: unknown;
}

/**
 * Applies `payload` to a structure the crate holds **as its own state**, and
 * answers what it now is together with the payload that puts it back.
 *
 * The other half of what a domain brings: an edit and its inverse are one
 * vocabulary's rule, so a page computing the inverse itself would be spelling
 * that rule again in TypeScript. Both directions come back in one answer because
 * the inverse has to be read *before* the edit lands — a surface that let you
 * apply first and read second would let you record the wrong thing.
 *
 * `undefined` for a vocabulary whose state is not a value a caller can hand
 * over: {@link TREE} (it needs a version to check against and a grid to snap to,
 * and {@link Document.apply} is its door) and {@link SAMPLES} (a borrowed view
 * whose frames are in a server buffer, never in a string — reading a span back
 * is what its inverse costs).
 */
export function domainEdit(
    domain: string,
    state: unknown,
    payload: unknown,
): Edited | undefined {
    const answer = coreDomainEdit(domain, JSON.stringify(state), JSON.stringify(payload));
    return answer ? (JSON.parse(answer) as Edited) : undefined;
}

/** What one structure has to apply for a step, in the order it must apply it. */
export interface Leg {
    /** The identity {@link History.register} handed back. */
    structure: number;
    /** The edits, in that structure's own vocabulary. */
    payloads: unknown[];
}

/** What one structure's **owner** has to re-run for a step. */
export interface RemainingLeg {
    /** The identity {@link History.register} handed back. */
    structure: number;
    /** The steps, for the owner to re-run in order. */
    steps: Step[];
}

/** What a step of the pile hands back, applied by nobody. */
export interface Walked {
    /** What the entry the walk lands on was called. */
    label: string;
    /**
     * What each structure has to apply — **one entry per structure**, not one
     * per leg, each structure's payloads in the order it must apply them. The
     * order kept is the order *within* a structure: a caller applies through
     * one vocabulary at a time.
     */
    legs: Leg[];
    /**
     * What each structure's owner has to re-run, from the first step the crate
     * cannot describe as an edit onward — a deterministic operation kept as its
     * parameters, which you re-run because the crate holds no algorithms. It
     * stops at the first rather than skipping it, so a later edit is never
     * applied over a state the operation before it was meant to produce. Going
     * back it is always empty: an inverse is always an edit.
     */
    remaining: RemainingLeg[];
    /**
     * The entries the walk passed over because nothing can invert them, by
     * label. A hole in the history that announces itself is what lets a person
     * understand why an undo did not go where they expected.
     */
    skipped: string[];
}

/**
 * One editing context's history.
 *
 * A history holds the structures registered in it and **one ordered pile** over
 * them, so what you decide by choosing a history is *what shares an undo
 * order*: a structure you built with no composition behind it is a history with
 * one structure in it; an application composing several editable views
 * registers them all in one, and the interleaved order its undo walks **is**
 * the pile; two views of one structure hold one history between them, which is
 * what keeps an undo in either from writing a state nobody was in.
 *
 * What decides what shares a history is which history a structure was
 * registered in, never which view is looking at it. A structure belongs to
 * exactly one, and {@link History.record} refuses an entry naming an identity
 * this history did not mint.
 *
 * It is an object for its own reason, beyond the one {@link Document} has: the
 * spill store. A bulk payload leaves the pile on purpose, so passing one by
 * value would carry every spilled span on every call, which is the cost
 * spilling exists to avoid.
 */
export class History {
    #inner: CoreHistory;

    /**
     * A history, **with the core already loaded** — the synchronous door, for
     * the same reason {@link Document}'s constructor is one.
     * {@link History.open} is the awaiting form.
     */
    constructor(budget = 0, spillAbove = 0) {
        this.#inner = new CoreHistory(budget, spillAbove);
    }

    /**
     * Opens a history. `budget` is how many entries it keeps before the oldest
     * falls off and `spillAbove` how many bytes a payload must reach,
     * serialized, before it leaves the pile; either as 0 takes the crate's
     * default.
     */
    static async open(budget = 0, spillAbove = 0): Promise<History> {
        await loadCore();
        return new History(budget, spillAbove);
    }

    /**
     * Takes a structure into this history and returns its identity.
     *
     * `domain` names the vocabulary its payloads are written in — `"tree"` for
     * the arrangement, `"points"` for a break-point curve — and the history
     * carries it so a caller routing what comes back knows which reader a leg
     * belongs to. Nothing in the crate reads it.
     *
     * The identity is minted here rather than carried by the data, because a
     * structure you built has no id and is not going to be given a stable one
     * for this. It is also the read-back path: the identity that opened an
     * editable view is the one its edited state is read out through.
     */
    register(domain: string): number {
        return Number(this.#inner.register(domain));
    }

    /**
     * Apply an edit to `document` **and record it** against `structure`, in one
     * call — the inverse has to be read out of the document before the edit
     * lands, so applying first and recording second would record the wrong
     * thing. Nothing is recorded unless the document changed, so a refusal
     * leaves no entry and neither does a resend.
     *
     * The arrangement's door alone, because the document is the one state the
     * crate can reach; for anything else you apply the edit yourself and hand
     * the pair to {@link History.record}.
     */
    apply(
        structure: number,
        document: Document,
        intent: Intent,
        options: { against?: Against; quant?: number; label?: string } = {},
    ): Outcome {
        return JSON.parse(
            this.#inner.apply(
                BigInt(structure),
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
     * Record one entry — one gesture, and what it takes to reverse it.
     *
     * The door for everything {@link History.apply} cannot do: a destructive
     * write, whose overwritten samples are not in the tree, and every domain
     * that is not the arrangement, whose state the crate cannot reach. This
     * applies nothing: the edits have happened, and what is recorded is how to
     * put them back.
     *
     * **Several legs are one transaction**: applied in the order given,
     * inverted in reverse, and undone in one step. That is what a gesture
     * touching more than one structure needs — a drag that moves a clip and
     * rewrites the curve it carries — and it is why the whole entry goes in one
     * call: half a transaction is worse than none. It is not coalescing, which
     * merges *successive* entries over one structure.
     *
     * `coalesce` merges into the entry before it when every leg's structure and
     * `key` match, so a run of small adjustments is one undo. You decide,
     * because only you know where the hand stopped.
     *
     * @returns whether it was recorded: an entry with no leg, or one naming a
     *   structure this history did not mint, is refused rather than opening a
     *   second order over data that already has one.
     */
    record(legs: RecordedLeg[], options: { label?: string; coalesce?: boolean } = {}): boolean {
        return this.#inner.record(
            JSON.stringify({
                label: options.label ?? "edit",
                coalesce: options.coalesce ?? false,
                legs,
            }),
        );
    }

    /**
     * **One step of the pile, routed** — what each structure has to apply, in
     * order, and what only its owner can re-run. `undefined` when there was
     * nothing to walk.
     *
     * One call and not two because picking the side a direction reads, and
     * keeping the legs one structure owns, are rules rather than plumbing, and
     * every caller was writing both for itself.
     *
     * It applies nothing: a history holds structures the crate cannot reach, so
     * applying the legs it *could* would leave the rest to you out of order,
     * which is how a transaction half-happens.
     */
    walk(direction: "undo" | "redo"): Walked | undefined {
        const result = this.#inner.walk(direction);
        return result === undefined ? undefined : (JSON.parse(result) as Walked);
    }

    /** Whether there is anything to undo. */
    get canUndo(): boolean {
        return this.#inner.canUndo;
    }

    /** Whether there is anything to redo. */
    get canRedo(): boolean {
        return this.#inner.canRedo;
    }

    /**
     * What an undo would be called, for a menu item — and what a person needs
     * when one pile holds several structures, since the label is the only thing
     * saying which one a keystroke is about to move.
     */
    get undoLabel(): string | undefined {
        return this.#inner.undoLabel;
    }

    /** What a redo would be called. */
    get redoLabel(): string | undefined {
        return this.#inner.redoLabel;
    }

    /** How many entries the history holds. */
    get length(): number {
        return this.#inner.len;
    }

    /**
     * The data behind a structure is gone: drop it from the registry, and say
     * whether its memory may go now.
     *
     * `true` when nothing in the pile names it any more, `false` when you must
     * wait for {@link History.released} — because undoing a deletion has to be
     * able to give the data back, so a structure that is out of the tree stays
     * alive while an entry can still restore what referred to it.
     *
     * It also **invalidates the entries that name it**: they cannot be applied
     * to data that is gone, so they become non-invertible — kept, marked, and
     * walked past with the walk saying so. Undoing a deletion returns the data,
     * not its history.
     */
    forget(structure: number): boolean {
        return this.#inner.forget(BigInt(structure));
    }

    /**
     * The forgotten structures no entry names any more — you may free their
     * data now. Drains: each is reported once.
     */
    released(): number[] {
        return Array.from(this.#inner.released(), Number);
    }

    /**
     * Stamp the pile where it stands: this is what is on disk.
     *
     * A save is an event of the whole editing context and the mark is the
     * pile's, so one save stamps one mark, and a structure registered later
     * starts behind it.
     */
    markSaved(): void {
        this.#inner.markSaved();
    }

    /**
     * Whether the work differs from what was last saved.
     *
     * Crossing the mark backwards is allowed, and this is the announcement —
     * which has to be accurate: nothing on disk changed, and the file still
     * holds those edits until the next save. Crossing forward again returns to
     * clean.
     */
    get dirty(): boolean {
        return this.#inner.dirty;
    }

    /**
     * Whether the saved state can still be reached by walking this history.
     *
     * `false` after the case the warning earns its place for: undo past the
     * mark and then edit, and the redo is truncated — so the saved state stops
     * being reachable, and {@link History.dirty} will never go quiet again on
     * its own.
     */
    get savedReachable(): boolean {
        return this.#inner.savedReachable;
    }

    /**
     * Forget every entry, releasing what was spilled. The structures stay
     * registered: it is the order that is gone, not the identities you hold.
     */
    clear(): void {
        this.#inner.clear();
    }

    /** Release the history. Idempotent. */
    free(): void {
        this.#inner.free();
    }
}

/**
 * The undo history of one document: a {@link History} with one structure in it.
 *
 * Undo belongs with the document and not with a view: a view's log sees only
 * the gestures *it* made, so a script editing the arrangement, a second editor
 * or a re-render leaves it describing a document that has moved on — and undo
 * then writes a state nobody was ever in. This is that history read in the
 * arrangement's own terms, so there is one order however many surfaces edit.
 *
 * It is a **face**, not a second pile: {@link Log.history} is the one
 * underneath, and a caller composing several editable structures registers them
 * there rather than opening a second `Log`.
 *
 * ```ts
 * const doc = await Document.open(json);
 * const log = await Log.open();
 * log.apply(doc, { intent: "place", node: 3, offset: 4 }, { label: "move the clip" });
 * log.undo(doc);   // exactly where it was
 * ```
 */
export class Log {
    /** The domain name a document's structure is registered under. */
    static readonly TREE = "tree";

    /**
     * The pile this log is a face of — what a caller composing several editable
     * structures in one context reaches for.
     */
    readonly history: History;
    /** The document's identity within that history. */
    readonly structure: number;
    #owned: boolean;

    /**
     * A log, **with the core already loaded** — the synchronous door, for the
     * same reason {@link Document}'s constructor is one. {@link Log.open} is
     * the awaiting form. Pass a `history` to register the document in one that
     * already exists; freeing this log then leaves that history alone.
     */
    constructor(budget = 0, spillAbove = 0, history?: History) {
        this.#owned = history === undefined;
        this.history = history ?? new History(budget, spillAbove);
        this.structure = this.history.register(Log.TREE);
    }

    /** Opens a log. Arguments are {@link History.open}'s. */
    static async open(budget = 0, spillAbove = 0, history?: History): Promise<Log> {
        await loadCore();
        return new Log(budget, spillAbove, history);
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
        return this.history.apply(this.structure, document, intent, options);
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
        // The key is the arrangement's own sentence, so it is asked of the
        // arrangement rather than spelled again here: a second spelling is how
        // a run coalesces through one door and not through another.
        const edit = "edit" in forward ? forward.edit : undefined;
        const key = edit === undefined ? "" : Document.coalesceKey(edit);
        this.history.record([{ structure: this.structure, forward, backward, key }], options);
    }

    /**
     * Undo the last transaction, applying its inverses to `document`, or
     * `undefined` when there is nothing to undo.
     */
    undo(document: Document): Undone | undefined {
        const walked = this.#step("undo", document);
        return walked === undefined
            ? undefined
            : { label: walked.label, undone: walked.mine, others: walked.others,
                remaining: walked.remaining, skipped: walked.skipped };
    }

    /**
     * Redo what was last undone, applying what it can to `document`, or
     * `undefined` when there is nothing.
     */
    redo(document: Document): Redone | undefined {
        const walked = this.#step("redo", document);
        return walked === undefined
            ? undefined
            : { label: walked.label, redone: walked.mine, others: walked.others,
                remaining: walked.remaining, skipped: walked.skipped };
    }

    /**
     * One step, applying the legs addressed to this document and reporting the
     * rest.
     *
     * The routing is the crate's ({@link History.walk}): which side of an entry
     * a direction reads, and which legs a structure owns, are the two rules this
     * used to write for itself.
     */
    #step(
        direction: "undo" | "redo",
        document: Document,
    ): { label: string; mine: Intent[]; others: (Leg | RemainingLeg)[];
         remaining: Step[]; skipped: string[] } | undefined {
        const walked = this.history.walk(direction);
        if (walked === undefined) return undefined;
        const mine: Intent[] = [];
        const others: (Leg | RemainingLeg)[] = [];
        for (const leg of walked.legs) {
            if (leg.structure !== this.structure) {
                others.push(leg);
                continue;
            }
            for (const payload of leg.payloads) {
                // An undo is authoritative: it states what the document was, so
                // it is not checked against a version it predates.
                document.apply(payload as Intent);
                mine.push(payload as Intent);
            }
        }
        const remaining: Step[] = [];
        for (const leg of walked.remaining) {
            if (leg.structure === this.structure) remaining.push(...leg.steps);
            else others.push(leg);
        }
        return { label: walked.label, mine, others, remaining, skipped: walked.skipped };
    }

    /** Whether there is anything to undo. */
    get canUndo(): boolean {
        return this.history.canUndo;
    }

    /** Whether there is anything to redo. */
    get canRedo(): boolean {
        return this.history.canRedo;
    }

    /** What an undo would be called, for a menu item. */
    get undoLabel(): string | undefined {
        return this.history.undoLabel;
    }

    /** What a redo would be called. */
    get redoLabel(): string | undefined {
        return this.history.redoLabel;
    }

    /** How many entries the history holds. */
    get length(): number {
        return this.history.length;
    }

    /**
     * Stamp the pile where it stands: this is what is on disk. See
     * {@link History.markSaved}.
     */
    markSaved(): void {
        this.history.markSaved();
    }

    /** Whether the document differs from what was last saved. */
    get dirty(): boolean {
        return this.history.dirty;
    }

    /** Whether the saved state can still be reached by walking this history. */
    get savedReachable(): boolean {
        return this.history.savedReachable;
    }

    /** Forget everything, releasing what was spilled. */
    clear(): void {
        this.history.clear();
    }

    /** Release the history, unless it came from elsewhere. Idempotent. */
    free(): void {
        if (this.#owned) this.history.free();
    }
}
