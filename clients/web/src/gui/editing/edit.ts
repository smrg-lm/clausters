/**
 * `edit(x)`: the verb, and what it opens.
 *
 * One call over the fundamental structures — a buffer's samples, a break-point
 * curve, a timeline of events, and a **piece** — each of which is an
 * {@link Editor} with its own domain and its own view and nothing else. It
 * dispatches on **what the structure is** rather than on a keyword, because that
 * is the question a caller has already answered by holding one.
 *
 * **A piece is one of them.** It used to be excluded on the grounds that a
 * multitrack is an application rather than an editor over a structure — but what
 * made that true was that the picture and the reading of a gesture were written
 * per client, so a piece opened here would have been a second implementation of
 * both. They are the crate's now (`multitrackProps`/`editingIntake`), so a
 * piece is a structure with a vocabulary, a picture and an inverse like any
 * other, and opening it here is what gives it the history every other editor
 * has.
 *
 * Two calls over one structure give **two windows and one stack**: the editing
 * context is the data's ({@link Editing}), so an undo in either updates both.
 * That is not a feature of this verb — it is what asking the data for its history
 * means, and `edit` inherits it for free.
 *
 * @module
 */

import type { Application } from "./application.ts";
import type { Editing } from "./context.ts";
import type { Editor } from "./editor.ts";
import type { GuiHost, Stage } from "../host.ts";
import { NotesEditor, isEvents } from "./events.ts";
import { MultitrackEditor, isPiece } from "./multitrack.ts";
import type { MultitrackEditorOptions } from "./multitrack.ts";
import type { Server } from "../../defs/server/index.ts";
import { PointsEditor, isCurve } from "./points.ts";
import { SamplesEditor, isSamples } from "./samples.ts";

/** What `edit` passes on to whichever editor the structure asks for. */
export interface EditOptions {
    /**
     * The engine's rate, which fixes the data↔view bridge. A take knows its own
     * and needs none.
     */
    sampleRate?: number;
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
    /**
     * An {@link Application} the caller already has, for an editor that joins a
     * window set — one host, one widget-id space, one undo walk. Its own
     * {@link Echo} stays its own, since a conversation's floor is one view's.
     */
    app?: Application | null;
    /** The host to open on. Absent: the ambient one, booted if it has to be. */
    host?: GuiHost;
    /** The element the window's canvas takes the box of. Absent: one of its own. */
    stage?: Stage | null;
    /**
     * `false` to build the editor without opening it — for a caller that wants
     * to inspect the picture, join a context, or hand the editor to a window it
     * is composing.
     */
    open?: boolean;
    /**
     * **A piece's own two**, ignored by every other structure: which server
     * buffer each source was read into, and the server the piece sounds on.
     * Given a server the editor keeps a reader per box and draws the transport
     * row; a piece opened with none still edits.
     */
    sources?: MultitrackEditorOptions["sources"];
    server?: Server;
}

/** Builds the editor `structure` asks for, without opening it. */
function editorFor(structure: unknown, options: EditOptions): Editor<never> {
    const { sampleRate = 0, host: _h, stage: _s, open: _o, ...rest } = options;
    if (isSamples(structure)) {
        return new SamplesEditor(structure, {
            sampleRate,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isCurve(structure)) {
        return new PointsEditor(structure, {
            sampleRate: sampleRate || 48_000,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isEvents(structure)) {
        // No `tempo`: a timeline holds its own map, and the editor reads it.
        return new NotesEditor(structure, {
            sampleRate: sampleRate || 48_000,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isPiece(structure)) {
        // A piece states its own tempo, like a timeline.
        return new MultitrackEditor(structure, {
            sampleRate: sampleRate || 48_000,
            ...rest,
        }) as unknown as Editor<never>;
    }
    throw new TypeError(
        `nothing edits a ${(structure as object)?.constructor?.name ?? typeof structure}: ` +
            "`edit` opens a Buffer (its samples), an Automation (its curve), a " +
            "Timeline (its notes) or a Multitrack (the piece).",
    );
}

/**
 * Opens `structure` in an editor of its own kind — a `Buffer` (its samples), an
 * `Automation` (its curve), a `Timeline` (its notes) or a `Multitrack` (the
 * piece) — and answers the open editor.
 *
 * **It opens.** The window is up and listening when this resolves, so the
 * structure the caller already holds is the edited one from that moment: read
 * it whenever, and it says what the hand has left there. `{ open: false }`
 * builds the editor without a window, for a caller composing one.
 *
 * Async where the reference client's `edit` is not, for the reason `plot` and
 * `View.open` are: resolving the ambient host may have to boot it. The verb, the
 * dispatch and what comes back are the same.
 *
 * Throws for something none of the three domains reads, naming what they are —
 * an unopenable structure is a question about the data, and answering it with a
 * bare failure teaches nothing.
 */
export async function edit(
    structure: unknown,
    options: EditOptions = {},
): Promise<Editor<never>> {
    const editor = editorFor(structure, options);
    if (options.open !== false) {
        await editor.open(options.host, { stage: options.stage });
    }
    return editor;
}
