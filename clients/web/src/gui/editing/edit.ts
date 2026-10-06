/**
 * `edit(x)`: the verb, and what it opens.
 *
 * One call over the fundamental structures -- a buffer's samples, a break-point
 * curve, a timeline of events, and a **multitrack** -- each of which is an
 * {@link Editor} with its own domain and its own view and nothing else. It
 * dispatches on **what the structure is** rather than on a keyword, because that
 * is the question a caller has already answered by holding one.
 *
 * **A multitrack is one of them.** It used to be excluded on the grounds that a
 * multitrack is an application rather than an editor over a structure -- but what
 * made that true was that the picture and the reading of a gesture were written
 * per client, so a multitrack opened here would have been a second implementation of
 * both. They are the crate's now (`multitrackProps`/`editingIntake`), so a
 * multitrack is a structure with a vocabulary, a picture and an inverse like any
 * other, and opening it here is what gives it the history every other editor
 * has.
 *
 * **A second call over a structure hands back the editor already open on it**
 * (of the same kind, and -- for a roll -- over the same axis), with its window
 * as it is: a view names its widgets by what they draw, so a second window of
 * one role over one structure would ask for the same widgets as the first and
 * draw on them. Views of different roles over one structure are separate
 * windows and **one stack** -- a roll in hertz beside one in MIDI notes --
 * since the editing context is the data's ({@link Editing}), so an undo in
 * either updates both. That is not a feature of this verb -- it is what asking
 * the data for its history means, and `edit` inherits it for free.
 *
 * @module
 */

import type { Application } from "./application.ts";
import { Editing } from "../../history.ts";
import type { Editor } from "./editor.ts";
import type { GuiHost, Stage } from "../host.ts";
import { NotesEditor, isEvents } from "./events.ts";
import { Timeline } from "../../seq/timeline.ts";
import { MultitrackEditor, isMultitrack } from "./multitrack.ts";
import type { MultitrackEditorOptions } from "./multitrack.ts";
import type { AudioEditorOptions } from "./audio.ts";
import type { Server } from "../../defs/server/index.ts";
import { PointsEditor, isCurve } from "./points.ts";
import type { PointsEditorOptions } from "./points.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { AudioEditor, isTake } from "./audio.ts";
import { ScoreEditor, isScore } from "./score.ts";
import { main } from "../../base/main.ts";

/** What `edit` passes on to whichever editor the structure asks for. */
export interface EditOptions {
    /**
     * The engine's rate, which fixes the data<->view bridge. A take knows its own
     * and needs none; anything else takes the ambient server's nominal rate --
     * the current session's, else the default session's, as a play resolves
     * its server -- and 48 kHz with no server anywhere.
     */
    sampleRate?: number;
    title?: string;
    width?: number;
    height?: number;
    baseId?: number;
    /**
     * Widgets of the page's own, appended to the editor's window and resolved
     * by name on its `window`.
     */
    extra?: GenericEditorOptions<never>["extra"];
    /**
     * `false` opens what is edited with none of the application's chrome
     * around it, the keys kept ({@link GenericEditorOptions.chrome}).
     */
    chrome?: boolean;
    /**
     * An editing context the caller already has, for a view that joins one --
     * which is what makes a composed window undo across several structures in
     * one order.
     */
    context?: Editing | null;
    /**
     * An {@link Application} the caller already has, for an editor that joins a
     * window set -- one host, one widget-id space, one undo walk. Its own
     * {@link Echo} stays its own, since a conversation's floor is one view's.
     */
    app?: Application | null;
    /** The host to open on. Absent: the ambient one, booted if it has to be. */
    host?: GuiHost;
    /**
     * **A timeline's own**, ignored by every other structure: the beat its
     * render stops at, for one that does not end on its own. A timeline is
     * opened by rendering it into an `EventSequence`, which the editor's
     * `sequence` then holds; the timeline is not changed.
     */
    until?: number;
    /**
     * **A sequence's own** (or a timeline's, rendered into one), ignored by
     * every other structure: the roll's vertical axis, `"midi"` or `"hz"`
     * ({@link NotesEditor}).
     */
    yAxis?: "midi" | "hz";
    /** The element the window's canvas takes the box of. Absent: one of its own. */
    stage?: Stage | null;
    /**
     * `false` to build the editor without opening it -- for a caller that wants
     * to inspect the picture, join a context, or hand the editor to a window it
     * is composing.
     */
    open?: boolean;
    /**
     * **A multitrack's own two**, ignored by every other structure: which server
     * buffer each source was read into, and the server the multitrack sounds on.
     * Given a server the editor keeps a reader per box and draws the transport
     * row; a multitrack opened with none still edits.
     */
    sources?: MultitrackEditorOptions["sources"];
    server?: Server;
    /**
     * **A curve's own four**, ignored by every other structure: the ranges its
     * values (`min`/`max`) and its times (`start`/`end`) are kept in, each
     * pair both or neither ({@link PointsEditorOptions}).
     */
    min?: PointsEditorOptions["min"];
    max?: PointsEditorOptions["max"];
    start?: PointsEditorOptions["start"];
    end?: PointsEditorOptions["end"];
    /**
     * **A buffer's own three**, ignored by every other structure: the audio
     * editor's history limits and where a take leaves memory for
     * ({@link AudioEditorOptions}).
     */
    historyBytes?: number | null;
    residentBytes?: number | null;
    scratch?: string;
}

/** Builds the editor `structure` asks for, without opening it. */
function editorFor(structure: unknown, options: EditOptions): Editor<never> {
    const {
        sampleRate = 0, host: _h, stage: _s, open: _o, min, max, start, end, ...rest
    } = options;
    if (isTake(structure)) {
        return new AudioEditor(structure, {
            sampleRate,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isCurve(structure)) {
        return new PointsEditor(structure, {
            sampleRate,
            min,
            max,
            start,
            end,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isEvents(structure) && !(structure instanceof Timeline)) {
        return new NotesEditor(structure, {
            sampleRate,
            ...rest,
        }) as unknown as Editor<never>;
    }
    if (isScore(structure)) {
        return new ScoreEditor(structure, rest) as unknown as Editor<never>;
    }
    if (isMultitrack(structure)) {
        // A multitrack states its own tempo, like a timeline.
        return new MultitrackEditor(structure, {
            sampleRate,
            ...rest,
        }) as unknown as Editor<never>;
    }
    throw new TypeError(
        `nothing edits a ${(structure as object)?.constructor?.name ?? typeof structure}: ` +
            "`edit` opens a Buffer (its samples), an Automation (its curve), an " +
            "EventSequence or a Timeline (its notes), a Multitrack (the multitrack) or a " +
            "Score (its page).",
    );
}

/**
 * Opens `structure` in an editor of its own kind -- a `Buffer` (its samples), an
 * `Automation` (its curve), a `Timeline` (its notes), a `Multitrack` (the
 * multitrack) or a `Score` (a symbolic score, on its page, in place) -- and
 * answers the open editor: the one already open over `structure`, when there
 * is one of its kind (and, for a roll, over the same axis), in which case
 * nothing is opened and the options of this call are not applied.
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
 * Throws for something none of the three domains reads, naming what they are --
 * an unopenable structure is a question about the data, and answering it with a
 * bare failure teaches nothing.
 */
export async function edit(
    structure: unknown,
    options: EditOptions = {},
): Promise<Editor<never>> {
    // **A timeline is rendered, and the roll edits what it produced**: the
    // events, as concrete data in the timeline's beats with its map. The
    // timeline itself is code and is left as it was; the sequence is the
    // editor's `sequence`.
    if (options.open !== false) {
        const already = alreadyOpen(structure, options);
        if (already !== null) return already;
    }
    const { until, ...rest } = options;
    const opened = structure instanceof Timeline ? await structure.renderEvents(until) : structure;
    // A take knows its own rate; anything else takes the ambient server's.
    if (!rest.sampleRate && !isTake(opened)) rest.sampleRate = await ambientRate();
    const editor = editorFor(opened, rest);
    if (options.open !== false) {
        await editor.open(options.host, { stage: options.stage });
    }
    return editor;
}

/**
 * **The editor already open over `structure`** that this call would open
 * again -- the same kind, the same structure, the same axis for a roll, on the
 * same host when one is named -- or `null`.
 *
 * Found among the views of the structure's editing context, since every
 * editor attaches there when it opens. A timeline is rendered into a new
 * sequence by each call, so it never finds one.
 */
function alreadyOpen(structure: unknown, options: EditOptions): Editor<never> | null {
    const kinds: [(s: unknown) => boolean, new (...args: never[]) => object][] = [
        [isTake, AudioEditor],
        [isCurve, PointsEditor],
        [isEvents, NotesEditor],
        [isScore, ScoreEditor],
        [isMultitrack, MultitrackEditor],
    ];
    const kind = kinds.find(([test]) => test(structure))?.[1];
    if (kind === undefined || structure instanceof Timeline) return null;
    const context = options.context ?? Editing.of(structure as object);
    const axis = options.yAxis ?? "midi";
    for (const view of context.views()) {
        const editor = view as unknown as Editor<never>;
        if (
            editor.constructor === kind &&
            editor.structure === structure &&
            editor.window !== null &&
            (options.host === undefined || editor.app.host === options.host) &&
            (kind !== NotesEditor || (editor as unknown as NotesEditor).yAxis === axis)
        ) {
            return editor;
        }
    }
    return null;
}

/**
 * The ambient server's nominal rate -- the current session's, else the default
 * session's -- or 48 kHz with none, the rate a window drawn with no engine is
 * laid out at.
 */
async function ambientRate(): Promise<number> {
    let server: Server;
    try {
        server = main.resolveServer();
    } catch {
        return 48_000;
    }
    return (await server.queryInfo()).nominalSampleRate;
}
