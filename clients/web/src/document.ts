/**
 * The document: the composition's authoritative model, and the one place an
 * edit is applied.
 *
 * The model lives in a Rust crate (`crates/clausters-document`) and every
 * client binds that one — this client, the Python client, and a `standalone`
 * GUI host with no language attached. What crosses is the format rather than a
 * handle: the document and the edit go across by value and the new document
 * comes back, so a client's document *is* the crate's document rather than a
 * parallel structure that synchronizes with it.
 *
 * That shape is the point rather than a limitation. The crate is the **only**
 * thing that applies an intent, so no client can apply an edit and then report
 * what it did — which is what would let three clients mean three different
 * things by the same gesture.
 *
 * @module
 */

import { document_apply, document_resolve } from "./core/clausters_core_web.js";
import { loadCore } from "./base/core.ts";

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
 * Apply one edit to a document.
 *
 * @param document - the document to edit. It is not mutated; the edited one
 *   comes back.
 * @param intent - the edit, stating the resulting value.
 * @param options - `against`, the state the edit was made against (omit to
 *   apply unchecked), and `quant`, the musical grid a placement snaps to in
 *   beats (`0` snaps nothing).
 * @returns the new document and the outcome.
 * @throws if the document or the intent will not parse.
 */
export async function applyIntent(
    document: ClaustersDocument,
    intent: Intent,
    options: { against?: Against; quant?: number } = {},
): Promise<Applied> {
    await loadCore();
    return JSON.parse(
        document_apply(
            JSON.stringify({
                document,
                intent,
                against: options.against ?? null,
                quant: options.quant ?? 0,
            }),
        ),
    ) as Applied;
}

/**
 * Resolve a selection to the spans of material underneath it — placement, trim
 * and the clamp at both ends already applied.
 *
 * @param document - the document.
 * @param selection - what is selected.
 * @param framesPerBeat - the bridge between the arrangement's beats and the
 *   material's frames. Supplied rather than derived: tempo is the caller's, the
 *   arithmetic is the crate's.
 * @param inBeats - whether the selection's numbers are beats rather than frames
 *   on the shared axis.
 * @returns the spans, in tree order. Empty when nothing material was underneath
 *   — a group and a generator are in the way of a selection, not under it.
 */
export async function resolveSelection(
    document: ClaustersDocument,
    selection: Selection,
    framesPerBeat: number,
    inBeats = false,
): Promise<Resolved[]> {
    await loadCore();
    return JSON.parse(
        document_resolve(
            JSON.stringify({ document, selection, framesPerBeat, inBeats }),
        ),
    ) as Resolved[];
}
