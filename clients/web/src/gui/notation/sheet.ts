// The score model: notation as data, and operations as data over it (mirrors
// `clausters/gui/notation/sheet.py`).
//
// A **sheet** is a plain object — two durational structures that do not contain
// each other, the metric layout (`grid`) and the content (`staves` of voices of
// items), with every duration an exact rational written `[numerator,
// denominator]`. It is data all the way: this module holds no handle, nothing
// has to be freed, and composing operations creates no intermediate anybody
// owns.
//
// **The logic is not here.** Every operation — its arithmetic, its validation
// and its refusals — is `clausters_core::notation`, reached through the core's
// wasm door, and this module is a shell of names that assembles the operation
// and hands it over. That is not only the non-divergence rule: a **standalone
// host has no client language in the process at all**, and a score it opens has
// to be editable there, which is only true while the whole vocabulary lives on
// the Rust side. An operation that worked because a client computed something
// first would be one a standalone could not perform.
//
// The same rule draws the line inside this file. Naming an operation is this
// shell's; *resolving* one is not. `transpose(sheet, 2, { span: measures(3,
// 10) })` builds a payload and sends it; turning "measures 3 to 10" into a
// stretch of time is arithmetic against the grid, it changes the moment a meter
// changes or a bar is irregular, and it is done once, in Rust, for every
// client.

import {
    sheetApply,
    sheetOps,
    sheetToMei,
    voiceToSheet as coreVoiceToSheet,
} from "../../core/clausters_core_web.js";
import type { Slot } from "./mei.ts";

/**
 * An exact rational, as `[numerator, denominator]` — a duration or a position
 * in whole notes, so a quarter is `[1, 4]` and a triplet eighth `[1, 12]`.
 */
export type Ratio = [number, number];

/**
 * A score as data. Deliberately loose here: the shape is the core's, it grows
 * with each milestone, and a client that pinned every field would have to be
 * edited every time the model does — which is how two clients come to disagree
 * about what a sheet is.
 */
export interface Sheet {
    grid: unknown;
    key: string;
    staves: unknown[];
    [key: string]: unknown;
}

/** One operation: the verb under `op`, its parameters beside it. */
export interface Op {
    op: string;
    [key: string]: unknown;
}

/** One entry of the operation catalog. */
export interface OpSpec {
    op: string;
    required: string[];
    optional: string[];
}

/** What {@link transpose} takes past the interval itself. */
export interface TransposeOptions {
    /** The diatonic size of the interval, in places on the staff. */
    steps?: number;
    /** What to move; everything, by default. */
    span?: unknown;
}

/**
 * Lift a **voice** — the flat slot stream, `{ midis: [60], ticks: 8 }` per note
 * or chord and `{ ticks: 8 }` per rest — into a sheet.
 *
 * The bridge a client crosses once. Reducing `seq` data to slots reads this
 * language's types and stays in this client; everything above the slot is the
 * shared model. Ticks become exact durations and MIDI numbers become
 * **spelled** pitches in the accidental world `key` implies — the only choice a
 * bare number leaves, and the reason the key is asked for here rather than at
 * the end.
 */
export function fromVoice(
    voice: Slot[],
    { meter = "4/4", clef = "G2", key = "C" }: { meter?: string; clef?: string; key?: string } = {},
): Sheet {
    return JSON.parse(coreVoiceToSheet(JSON.stringify(voice), meter, clef, key)) as Sheet;
}

/**
 * Apply one operation to `sheet`, returning the new sheet.
 *
 * `op` names the verb under `op` and carries its parameters beside it
 * (`{ op: "transpose", semitones: 2 }`). The verb-shaped helpers below build
 * these, and this is what they all call; reach for it directly to send an
 * operation this shell has no helper for yet.
 *
 * Throws with the core's own sentence when the operation is refused — a measure
 * range that runs backwards, a parameter that is not readable. Nothing changes
 * on a refusal: the sheet crossed by value, so the caller still holds what it
 * sent.
 */
export function apply(sheet: Sheet, op: Op): Sheet {
    return JSON.parse(sheetApply(JSON.stringify(sheet), JSON.stringify(op))) as Sheet;
}

/**
 * Write `sheet` out as MEI — what {@link engrave} and {@link Score} read.
 *
 * Throws with the emitter's reason when the model holds something MEI cannot be
 * written for yet: a duration that is not an exact note value (a tuplet), an
 * accidental past a double, or more than one voice. Each says which it is, so a
 * caller knows whether it is wrong or early.
 */
export function toMei(sheet: Sheet): string {
    return sheetToMei(JSON.stringify(sheet));
}

/**
 * Every operation the core knows, each naming its required and optional
 * parameters.
 *
 * **The parity surface the binding table cannot provide.** Operations ride
 * inside a payload through one symbol, so nothing fails when one client grows a
 * verb the other lacks — the same structural blindness that let five builder
 * divergences stand. Both clients are read against this list instead.
 */
export function ops(): OpSpec[] {
    return JSON.parse(sheetOps()) as OpSpec[];
}

/**
 * The span of measures `first` to `last`, **1-based and inclusive** — the
 * numbers a reader says out loud.
 *
 * This only *names* the span. What stretch of time it covers is resolved by the
 * core against the grid, because that answer changes with a meter change or an
 * irregular bar and two clients computing it separately would disagree about
 * which notes an edit touches.
 */
export function measures(first: number, last: number): unknown {
    return { measures: [first, last] };
}

/**
 * Move every note by an interval, keeping the spelling the interval implies: a
 * major third up from C is E, not F-flat.
 *
 * `semitones` is the chromatic size, positive upward. `steps` is the diatonic
 * size — how many places the notehead moves on the staff — and left out it is
 * the ordinary reading of that many semitones (4 semitones is a major third, so
 * 2 steps). Pass it to ask for the interval nobody's shorthand means, a
 * diminished third over a major second.
 *
 * `span` limits what moves ({@link measures}); left out, everything moves.
 */
export function transpose(
    sheet: Sheet,
    semitones: number,
    { steps, span }: TransposeOptions = {},
): Sheet {
    const op: Op = { op: "transpose", semitones };
    if (steps !== undefined) op.steps = steps;
    if (span !== undefined) op.span = span;
    return apply(sheet, op);
}
