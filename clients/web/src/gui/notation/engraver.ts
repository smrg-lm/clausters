// The engraver and its output (mirrors `clausters/gui/notation/engraver.py`).
//
// A score is laid out into SVG by verovio and that SVG is walked into the flat,
// resolution-independent display list the host's `score` widget consumes. Both
// steps are the shared core's: the walk is `clausters_core::notation`, and the
// stateful, editable document is `clausters_core::notation::Score` — the order
// an edit is made in, the reload that keeps the timemap honest, the undo stack.
// What is here is the shell: idiomatic names, plain objects, and the handle's
// lifetime.
//
// The engraver is the pinned verovio compiled to wasm (`_verovio.ts`), the same
// sources and the same options the native library is built and configured with,
// so a page and a window engrave one score into one drawing.

import {
    Score as CoreScore,
    engraveOptions,
    svgToDisplayList as coreSvgToDisplayList,
} from "../../core/clausters_core_web.js";
import { Toolkit } from "./_verovio.ts";
import { Editing } from "../editing/context.ts";
import type { Intent } from "../../document.ts";
import { fromNotes, fromTimeline } from "./mei.ts";
import type { MeiOptions } from "./mei.ts";
import type { Op, Sheet } from "./sheet.ts";
import type { Event as SeqEvent } from "../../seq/event.ts";
import type { Timeline } from "../../seq/timeline.ts";

/**
 * The display-list keys the host draws from — everything but `notes`, which is
 * the client's own layer. {@link pageJson} and the `score` builder send exactly
 * these.
 */
const PAGE_LAYERS = ["vb", "glyphs", "prims", "cursors", "step", "elements"] as const;

/** One engraved page: what is drawn, where the cursor goes, and what sounds. */
export interface Page {
    /** The `[w, h]` page-unit viewBox. */
    vb: number[];
    /** A SMuFL codepoint-to-outline table. */
    glyphs: Record<string, unknown>;
    /** The placed glyphs, lines, fills and texts. */
    prims: Record<string, unknown>[];
    /** Page units per diatonic step — the quantum a pitch drag counts in. */
    step: number;
    /** The timemap folded into geometry: `{t, x, y0, y1}` per onset, `t` in ms. */
    cursors: Record<string, number>[];
    /** One `{t, dur, pitch, id}` per note (ms and MIDI pitch). Client-side. */
    notes: { t: number; dur: number; pitch: number; id: string }[];
    [key: string]: unknown;
}

/** How a score is engraved: the staff size, the wrap width, extra options. */
export interface EngraveOptions {
    /** Staff size (verovio `scale`). */
    scale?: number;
    /** The page units the score wraps into systems at. */
    pageWidth?: number;
    /** Extra engraver options, merged over the defaults. */
    options?: Record<string, unknown> | null;
}

/**
 * A loaded score, kept alive so it can be **edited** and re-engraved.
 *
 * {@link engrave} is the one-shot form — load, draw, discard. This is the
 * stateful one: it holds the engraver's document open, so an edit can be applied
 * to the same one the display list was drawn from and the page re-engraved
 * against it. The MEI `xml:id`s survive editing, which is what lets the host keep
 * its selection across the round trip: the id the user clicked still names the
 * same note afterwards.
 *
 * The edit cycle is the shared layer's (`clausters_core::notation::Score`), not
 * this shell's — as it is in the Python client, which holds the same model over
 * the C ABI.
 *
 * **The undo order is the editing context's**, like every other editable
 * structure's. A score registers in `Editing.of(score)` under the `"score"`
 * vocabulary and records each edit as the MEI it produced, with the previous one
 * as its inverse — an absolute payload, so a step is idempotent and carries no
 * direction. That is what makes a window holding a lane and a page walk **one**
 * order: before this, an engraved page had a real history of its own and Ctrl+Z
 * meant one of two different things depending on what the pointer was over.
 *
 * The shared layer's own snapshot stack is still there and is what a caller with
 * no such context uses — a standalone host holding a page and nothing else. From
 * here it is not read: {@link Score.undo} and {@link Score.redo} walk the
 * context's pile and put a state back through the crate's `load`.
 *
 * **Opening is asynchronous where the Python client's is not**, and only because
 * of where verovio is: a page loads the engraver module the first time one is
 * needed, and a page may not block. Once open, every method is synchronous, as
 * there it is.
 */
export class Score {
    private readonly inner: CoreScore;
    private readonly toolkit: Toolkit;

    private constructor(inner: CoreScore, toolkit: Toolkit) {
        this.inner = inner;
        this.toolkit = toolkit;
    }

    /**
     * Load `data` — a score in any format the engraver auto-detects — and keep
     * the document open.
     */
    static async open(
        data: string,
        { scale = 40, pageWidth = 2100, options = null }: EngraveOptions = {},
    ): Promise<Score> {
        const configured = JSON.parse(
            engraveOptions(scale, pageWidth, options ? JSON.stringify(options) : undefined),
        ) as Record<string, unknown>;
        const toolkit = await Toolkit.open(configured);
        try {
            return new Score(new CoreScore(toolkit as unknown as object, data), toolkit);
        } catch (error) {
            toolkit.free();
            throw error;
        }
    }

    /**
     * An editable score built from a **monophonic** run of events — the
     * {@link fromNotes} encoder handed straight to {@link Score.open}.
     */
    static fromNotes(
        notes: Iterable<SeqEvent>,
        options: MeiOptions & EngraveOptions = {},
    ): Promise<Score> {
        return Score.open(fromNotes(notes, options), options);
    }

    /**
     * An editable score built from a `Timeline` (chords from simultaneous
     * events, rests from gaps).
     */
    static fromTimeline(
        timeline: Timeline,
        options: MeiOptions & EngraveOptions = {},
    ): Promise<Score> {
        return Score.open(fromTimeline(timeline, options), options);
    }

    /**
     * This score engraved into a page — from the live document, so it reflects
     * every edit applied so far.
     */
    displayList(page = 1): Page {
        return JSON.parse(this.inner.displayList(page)) as Page;
    }

    /** The score as MEI, ids and all — the format to persist. */
    mei(): string {
        return this.inner.mei();
    }

    /**
     * The open score as the **model**.
     *
     * Every one of the model's verbs applies to it, including on a score that was
     * typed rather than built: the engraver normalizes whatever it loaded and
     * this reads that, so a phrase in ABC is as editable as one made by
     * operating on a motif.
     *
     * Throws when the document could not be read into a model — a state and not
     * a failure, since the page still draws and still plays and only the
     * model's verbs are unavailable on it.
     */
    sheet(): Sheet {
        return JSON.parse(this.inner.sheet()) as Sheet;
    }

    /**
     * Apply one **model** operation as a single undo step, and re-engrave.
     *
     * This is the edit path. `op` is the payload the sheet verbs build, so an
     * edit to an open score and an edit to a sheet in hand are the same
     * operation through the same code — which is what lets a standalone host
     * with no client language perform it too.
     *
     * `false` when the document has no model behind it or the operation was
     * refused; either way the page and the model are as they were.
     */
    apply(op: Op): boolean {
        const before = this.mei();
        if (!this.inner.apply(JSON.stringify(op))) return false;
        this.record(before, String((op as { op?: unknown }).op ?? "edit the score"));
        return true;
    }

    // ---- the editing context: one order over everything being edited ----

    /**
     * This score's editing context, attaching it on first ask.
     *
     * The context hangs on the **data**, so a page opened twice is one structure
     * in one order; attaching makes the score a participant, which is what lets
     * an undo made in a *neighbouring* window step it too.
     */
    private get editing(): Editing {
        const context = Editing.of(this);
        context.attach(this);
        return context;
    }

    /** This score's identity in the pile — minted once, per score. */
    private get structure(): number {
        return this.editing.identity(this, "score");
    }

    /**
     * Record one edit: the MEI it produced, and the one it replaced.
     *
     * **A state, not a step.** The page a score is at describes it whole, the
     * way a curve's points and a timeline's events do, so an entry is idempotent
     * and reads the same in both directions — which is the only shape a pile
     * shared with other structures can carry.
     */
    private record(before: string, label: string): void {
        const after = this.mei();
        if (after === before) return; // a resend is not an edit
        const context = this.editing;
        context.turn(this, () => {
            // `forward` is a **step** and `backward` is a bare payload: the pile
            // hands an inverse back as what it holds, and wrapping it a second
            // time hands the reader a leg it cannot read.
            context.history.record(
                [
                    {
                        structure: this.structure,
                        // The payload is this domain's own vocabulary and
                        // the pile never reads it; the arrangement's `Intent` is
                        // only what the type happens to name.
                        forward: { edit: { mei: after } as unknown as Intent },
                        backward: { mei: before },
                    },
                ],
                { label },
            );
            context.changed();
        });
    }

    /**
     * Put back the legs of a history step that name **this** score.
     *
     * What the context hands round. A page is not a view — nothing here redraws
     * — so a caller re-engraves after a step exactly as it does after an edit.
     */
    projectLegs(legs: readonly unknown[]): boolean {
        let moved = false;
        for (const leg of legs as { structure?: number; payload?: { mei?: unknown } }[]) {
            if (Math.trunc(Number(leg.structure ?? -1)) !== this.structure) continue;
            const mei = leg.payload?.mei;
            if (typeof mei !== "string") continue;
            moved = this.load(mei) || moved;
        }
        return moved;
    }

    /**
     * Another window in this context edited. Nothing here: a score is data and
     * draws nothing of its own — whoever engraved the page redraws it.
     */
    adopt(): void {}

    /**
     * Replace the document with `mei` — **a state, not a step**.
     *
     * The door the pile puts a previous page back through. It clears the shared
     * layer's own stack, so one score has one history.
     */
    load(mei: string): boolean {
        return this.inner.load(mei);
    }

    /**
     * Whether the **context's** pile has an edit to step back over — which may
     * be an edit to something else entirely, since the order is one.
     */
    get canUndo(): boolean {
        return this.editing.history.canUndo;
    }

    /** @see {@link Score.canUndo} */
    get canRedo(): boolean {
        return this.editing.history.canRedo;
    }

    /**
     * Step the editing context back one edit, and say whether anything moved.
     * `false` when there is nothing to undo.
     *
     * The step is the **context's**, not this score's: a page edited beside a
     * lane steps in the order the two were edited in, and a leg naming another
     * structure is projected by whoever holds it.
     */
    undo(): boolean {
        return this.step("undo");
    }

    /** Step forward again after {@link Score.undo}; `false` when there is nothing to redo. */
    redo(): boolean {
        return this.step("redo");
    }

    private step(direction: "undo" | "redo"): boolean {
        const context = this.editing;
        const legs = context.step(direction);
        return legs === undefined ? false : context.distribute(legs, this);
    }

    /**
     * Move the note `elementId` by `steps` **diatonic** steps along the staff —
     * up when positive — as one undo step.
     *
     * It is the **model's** move where the page named a model item (the note
     * takes the key signature's alteration for the letter it lands on, which is
     * what reading in a key means) and falls back to the engraver's editor only
     * for an element this layer did not write. The relative form; reach for it
     * only when the delta is what you actually have.
     */
    transpose(elementId: string, steps: number): boolean {
        const before = this.mei();
        if (!this.inner.transpose(elementId, steps)) return false;
        this.record(before, "transpose");
        return true;
    }

    /**
     * Move the note `elementId` **to** the diatonic staff position `position` on
     * `page` — the shape an edit travels in, so a resend cannot move the note
     * twice. True when the note is now there, including when it already was.
     */
    transposeTo(elementId: string, position: number, page = 1): boolean {
        const before = this.mei();
        if (!this.inner.transposeTo(elementId, position, page)) return false;
        // **A note already there is not an edit.** The call answers true for it,
        // because the requested state holds; `record` sees the same MEI on both
        // sides and leaves the pile alone, which is the rule a resend follows
        // everywhere else.
        this.record(before, "transpose");
        return true;
    }

    /**
     * Apply one raw editor action (`set`, `insert`, `delete`, …) as a single undo
     * step — the escape hatch for what {@link Score.transpose} does not cover.
     * A rejected action leaves the score untouched.
     */
    edit(action: string, param: Record<string, unknown> = {}): boolean {
        const before = this.mei();
        if (!this.inner.edit(action, JSON.stringify(param))) return false;
        this.record(before, action);
        return true;
    }

    /** The engraver's version — what both clients must agree on. */
    engraverVersion(): string {
        return this.toolkit.version();
    }

    /**
     * Free the document and its toolkit. A page that keeps engraving keeps the
     * module: what is released here is one score, not the engraver.
     */
    free(): void {
        this.inner.free();
        this.toolkit.free();
    }
}

/**
 * Engrave `data` (a score in any format the engraver auto-detects) into a
 * display list.
 *
 * One-shot: the score is loaded, drawn and discarded. Use {@link Score} instead
 * when the page has to be **edited** and redrawn.
 *
 * The result holds one engraving in three layers — what the host **draws**
 * (`vb`, `glyphs`, `prims`, `step`), where the **cursor** goes (`cursors`, the
 * timemap folded into geometry) and what **sounds** (`notes`, which stays on the
 * client: it is what a driver plays). The engraver mints fresh ids per load, so
 * all three must come from one engraving — which is why one call produces them
 * all.
 */
export async function engrave(
    data: string,
    { page = 1, ...options }: EngraveOptions & { page?: number } = {},
): Promise<Page> {
    const score = await Score.open(data, options);
    try {
        return score.displayList(page);
    } finally {
        score.free();
    }
}

/**
 * The **drawing** layers of a display list, as the object a live
 * `host.set(scoreId, { displayList })` takes — how a re-engraved page replaces
 * the one on screen after an edit, without redefining the window.
 *
 * The same layers the `score` builder sends when it builds the widget, so the
 * widget looks the same either way: the client-side `notes` stay here, and so
 * does the host's own chrome (the playhead and the selection survive the
 * replacement, which is what keeps the edited note selected across the round
 * trip).
 */
export function pageJson(displayList: Page): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (const key of PAGE_LAYERS) {
        if (key in displayList) out[key] = displayList[key];
    }
    return out;
}

/**
 * Walk an engraver SVG into a display list. Split out of {@link engrave} so it
 * is testable on a captured SVG, and shared with every other client — the walk
 * itself is `clausters_core::notation`, so a page feeding it the same SVG a
 * window feeds it gets the identical list.
 *
 * Each primitive carries the id of the element it belongs to, and a **sounding
 * element owns everything drawn inside it**: the engraver identifies a note's
 * stem and flag separately, and collapsing them onto the note's id is what makes
 * one note one thing to select and drag. A chord keeps its notes distinct, so one
 * of them can still be transposed alone.
 */
export function svgToDisplayList(svg: string): Record<string, unknown> {
    const out = coreSvgToDisplayList(svg);
    return out ? (JSON.parse(out) as Record<string, unknown>) : {};
}
