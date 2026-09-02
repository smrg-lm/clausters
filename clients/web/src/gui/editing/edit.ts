/**
 * `edit(x)`: the verb, and what it opens.
 *
 * One call over the three fundamental structures — a buffer's samples, a
 * break-point curve, a timeline of events — each of which is an {@link Editor}
 * with its own domain and its own view and nothing else. It dispatches on **what
 * the structure is** rather than on a keyword, because that is the question a
 * caller has already answered by holding one.
 *
 * What it deliberately does not open is a composition: an arrangement is edited
 * by {@link FormEditor}, which knows a tree from a leaf and holds a document.
 * `edit` over a piece would be a second door to the same place with a worse
 * answer.
 *
 * Two calls over one structure give **two windows and one stack**: the editing
 * context is the data's ({@link Editing}), so an undo in either updates both.
 * That is not a feature of this verb — it is what asking the data for its history
 * means, and `edit` inherits it for free.
 *
 * @module
 */

import type { Editing } from "./context.ts";
import type { Editor } from "./editor.ts";
import { NotesEditor, isEvents } from "./events.ts";
import { PointsEditor, isCurve } from "./points.ts";
import { SamplesEditor, isSamples } from "./samples.ts";

/** What `edit` passes on to whichever editor the structure asks for. */
export interface EditOptions {
    /**
     * The engine's rate, which fixes the data↔view bridge. A take knows its own
     * and needs none.
     */
    sampleRate?: number;
    /** The clock's tempo in beats per second, for the structures placed in beats. */
    tempo?: number;
    title?: string;
    width?: number;
    height?: number;
    baseId?: number;
    /**
     * An editing context the caller already has, for a view that joins one —
     * which is what makes a composed window undo across several structures in
     * one order.
     */
    context?: Editing | null;
}

/**
 * Opens `structure` in an editor of its own kind — a `Buffer` (its samples), an
 * `Automation` (its curve) or a `Timeline` (its notes).
 *
 * The editor is not opened: {@link Editor.open} is a separate step, so a caller
 * can inspect the picture, join a context or hand the editor to a window it is
 * composing.
 *
 * Throws for something none of the three domains reads, naming what they are —
 * an unopenable structure is a question about the data, and answering it with a
 * bare failure teaches nothing.
 */
export function edit(structure: unknown, options: EditOptions = {}): Editor<never> {
    const { sampleRate = 0, tempo = 1.0, ...rest } = options;
    if (isSamples(structure)) {
        return new SamplesEditor(structure, {
            sampleRate,
            tempo,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isCurve(structure)) {
        return new PointsEditor(structure, {
            sampleRate: sampleRate || 48_000,
            tempo,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isEvents(structure)) {
        return new NotesEditor(structure, {
            sampleRate: sampleRate || 48_000,
            tempo,
            ...rest,
        }) as unknown as Editor<never>;
    }
    throw new TypeError(
        `nothing edits a ${(structure as object)?.constructor?.name ?? typeof structure}: ` +
            "`edit` opens a Buffer (its samples), an Automation (its curve) or a " +
            "Timeline (its notes). A composition is FormEditor's.",
    );
}
