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
import { fromNotes, fromTimeline } from "./mei.ts";
import type { MeiOptions } from "./mei.ts";
import type { Event as SeqEvent } from "../../seq/event.ts";
import type { Timeline } from "../../seq/timeline.ts";

/**
 * The display-list keys the host draws from — everything but `notes`, which is
 * the client's own layer. {@link pageJson} and the `score` builder send exactly
 * these.
 */
const PAGE_LAYERS = ["vb", "glyphs", "prims", "cursors", "step"] as const;

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
 * The edit cycle and the undo stack are the shared layer's
 * (`clausters_core::notation::Score`), not this shell's — as they are in the
 * Python client, which holds the same model over the C ABI.
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

    /** Whether there is an edit to step back over. */
    get canUndo(): boolean {
        return this.inner.canUndo;
    }

    /** Whether there is an undone edit to step forward into. */
    get canRedo(): boolean {
        return this.inner.canRedo;
    }

    /** Step back one edit; `false` when there is nothing to undo. */
    undo(): boolean {
        return this.inner.undo();
    }

    /** Step forward again after an undo; `false` when there is nothing to redo. */
    redo(): boolean {
        return this.inner.redo();
    }

    /**
     * Move the note `elementId` by `steps` **diatonic** steps along the staff —
     * up when positive — as one undo step. The relative form; reach for it only
     * when the delta is what you actually have.
     */
    transpose(elementId: string, steps: number): boolean {
        return this.inner.transpose(elementId, steps);
    }

    /**
     * Move the note `elementId` **to** the diatonic staff position `position` on
     * `page` — the shape an edit travels in, so a resend cannot move the note
     * twice. True when the note is now there, including when it already was.
     */
    transposeTo(elementId: string, position: number, page = 1): boolean {
        return this.inner.transposeTo(elementId, position, page);
    }

    /**
     * Apply one raw editor action (`set`, `insert`, `delete`, …) as a single undo
     * step — the escape hatch for what {@link Score.transpose} does not cover.
     * A rejected action leaves the score untouched.
     */
    edit(action: string, param: Record<string, unknown> = {}): boolean {
        return this.inner.edit(action, JSON.stringify(param));
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
