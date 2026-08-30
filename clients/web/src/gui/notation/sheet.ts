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
    interpretation as coreInterpretation,
    meiToSheet,
    sheetApply,
    sheetOps,
    sheetPerform,
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

/**
 * How the symbols are read: the whole of what the interpreter believes. Loose
 * for the same reason a {@link Sheet} is — the fields are the core's, they grow
 * with the reading, and a client that pinned them would drift from it.
 */
export interface Interpretation {
    beat_unit: number;
    amp: number;
    dynamics: Record<string, number>;
    accents: unknown[];
    [key: string]: unknown;
}

/** One sounding note, as the interpreter heard it. */
export interface PerformedNote {
    /** Onset, in beats. */
    t: number;
    /** The **written** value, in beats. */
    dur: number;
    /** How long it is **held**, in beats. */
    sustain: number;
    /** MIDI note number. */
    pitch: number;
    /** Linear amplitude. */
    amp: number;
    /** The staff it is written on, 0-based from the top. */
    staff: number;
    /** The voice of that staff, 0-based. */
    voice: number;
    /** The model id of the item it came from. */
    id: number;
}

/** One entry of the operation catalog. */
export interface OpSpec {
    op: string;
    required: string[];
    optional: string[];
}

/** What is written above the music: the title, and who wrote it. */
export interface HeaderFields {
    title?: string;
    subtitle?: string;
    composer?: string;
    lyricist?: string;
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
 * Read an MEI **document** into a sheet.
 *
 * The other return path, and not the one {@link toNotes} is: that turns a score
 * into sound, this turns a *document* into a score. A page opened from typed
 * text — ABC, MusicXML, a hand-written MEI — is a document and nothing else
 * until this reads one, which is why none of the verbs above can touch it
 * before that.
 *
 * There is one input format rather than four: the engraver normalizes whatever
 * it loaded to MEI, so hand it `Score.mei()` and every importer verovio has is
 * covered.
 *
 * What the model does not hold is **what the engraver recomputes when nobody
 * chose it** — automatic beaming, the line breaks that merely fit, the staff
 * geometry — so it is not read and is not loss. What a writer chose is held:
 * the header, the barlines, the breaks, the beams. Ids written by this layer
 * come back; a document from anywhere else gets fresh ones.
 *
 * Throws when the text is not readable XML or carries no score.
 */
export function fromMei(mei: string): Sheet {
    return JSON.parse(meiToSheet(mei)) as Sheet;
}

/**
 * The default **reading** of a score: every number {@link toNotes} depends on.
 *
 * What a staccato does to a length, what `mf` is in amplitude, how far a
 * crescendo travels, which positions in the bar are stressed. Read it, change
 * what you disagree with, and pass it back to {@link toNotes} — that is the
 * whole of overriding an interpretation, and nothing in the core is edited to
 * play a score in another style.
 *
 * It comes from Rust rather than being written here for the same reason the
 * operations do: two clients each holding their own copy of the dynamics table
 * play the same score at two amplitudes, and nothing compares them. It is also
 * the **parity surface** for the reading, since the interpretation rides inside
 * a payload and the binding table cannot see its fields.
 */
export function interpretation(): Interpretation {
    return JSON.parse(coreInterpretation()) as Interpretation;
}

/**
 * Read `sheet` into the notes it **sounds**, in time order.
 *
 * The path back out of the score, and the reason it is not a conversion: the
 * symbols mean something. A staccato shortens the sound and moves no attack, a
 * dynamic governs every note after it until the next one, a hairpin is a shape
 * over a stretch of notes rather than a mark on any of them, and a tie is one
 * sound of the summed length.
 *
 * Each note carries **two lengths** — `dur`, what is written, and `sustain`,
 * what is heard — in beats (a quarter is one beat by default,
 * `interp.beat_unit`); plus `t`, `pitch`, `amp`, the `staff` and `voice` it was
 * written on, and the model `id` it came from. The pair of lengths maps
 * straight onto an `Event`'s `dur` and `sustain`, which is what
 * {@link toTimeline} does.
 *
 * `interp` is the reading ({@link interpretation}); left out, the default. Any
 * field left out of it keeps its default, so overriding one is a one-key
 * object.
 *
 * **The instrument is not in the notation** — a staff does not say what plays
 * it — so the notes name their staff and the binding is made where the score is
 * rendered.
 */
export function toNotes(sheet: Sheet, interp?: Interpretation): PerformedNote[] {
    return JSON.parse(
        sheetPerform(JSON.stringify(sheet), interp ? JSON.stringify(interp) : ""),
    ) as PerformedNote[];
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

/**
 * A written pitch: the letter its notehead sits on, its scientific octave (`4`
 * is the octave of middle C) and how many semitones it is altered by.
 *
 * Naming a pitch, not deriving one. Spelling a MIDI number is a *rule* — `F#`
 * and `Gb` are one number and two notes — so it happens in the core, on the way
 * in through {@link fromVoice}; a caller writing a note into a score names the
 * note it means.
 */
export function pitch(step: string, octave: number, alter = 0): unknown {
    return { step, octave, alter };
}

/** An exact ratio, as a pair or a whole number. */
function ratio(value: Ratio | number): Ratio {
    return Array.isArray(value) ? value : [value, 1];
}

function withSpan(op: Op, span?: unknown): Op {
    if (span !== undefined) op.span = span;
    return op;
}

// -- the algebra: operations that rearrange a score ---------------------------

/**
 * `other` after `sheet`.
 *
 * Each voice continues the voice in the same position, with a rest filling any
 * that ran short. The **grid is the first score's, continued**: when it ends on
 * a barline, the second score's meters follow it, so a 4/4 section before a 3/4
 * one is exactly that. When it ends mid-measure there is no barline for the
 * second grid to start at, and a second score with a metric layout of its own is
 * refused rather than silently re-barred.
 */
export function concat(sheet: Sheet, other: Sheet): Sheet {
    return apply(sheet, { op: "concat", sheet: other });
}

/**
 * `other` at the same time as `sheet`.
 *
 * `asStaff: false` writes its voices on the same staves — counterpoint on one
 * staff; `true` appends staves below — a second hand or instrument. Both are
 * superposition; the difference is where the notes are written.
 *
 * Throws when the two grids differ: two scores cannot share a moment while
 * disagreeing about where the barlines are.
 */
export function stack(sheet: Sheet, other: Sheet, { asStaff = false } = {}): Sheet {
    return apply(sheet, { op: "stack", sheet: other, as_staff: asStaff });
}

/**
 * A stretch played `count` times in a row — `2` is one repeat, `1` changes
 * nothing.
 *
 * The copies go where the original is, pushing what follows later, and the grid
 * grows by as many measures as the stretch spans. `count: 0` is refused: that is
 * a deletion, and it has its own verb.
 */
export function repeat(sheet: Sheet, count: number, { span }: { span?: unknown } = {}): Sheet {
    return apply(sheet, withSpan({ op: "repeat", count }, span));
}

/**
 * The span's items in reverse order, voice by voice.
 *
 * The durations come back mirrored, so the stretch lasts exactly as long as it
 * did and the grid is untouched. A tie travels with the pair it joined.
 */
export function retrograde(sheet: Sheet, { span }: { span?: unknown } = {}): Sheet {
    return apply(sheet, withSpan({ op: "retrograde" }, span));
}

/**
 * Mirror the span's pitches about `axis` ({@link pitch}).
 *
 * Exact in both dimensions the model keeps apart: the notehead reflects across
 * the axis on the staff and the sound reflects across it in semitones, with the
 * accidental taking up what is left — which is what an inversion written by hand
 * looks like. Without an axis, the line turns about its own first note.
 */
export function invert(
    sheet: Sheet,
    { axis, span }: { axis?: unknown; span?: unknown } = {},
): Sheet {
    const op: Op = { op: "invert" };
    if (axis !== undefined) op.axis = axis;
    return apply(sheet, withSpan(op, span));
}

/**
 * Multiply the span's written values. Augmentation is `[2, 1]`, diminution
 * `[1, 2]`, and anything else is the same operation at another ratio.
 *
 * **The grid does not move**, which is the point: the phrase is re-barred
 * against the barlines it already had, tying across them where a value now
 * overruns one.
 */
export function stretch(
    sheet: Sheet,
    factor: Ratio | number,
    { span }: { span?: unknown } = {},
): Sheet {
    return apply(sheet, withSpan({ op: "stretch", factor: ratio(factor) }, span));
}

// -- the algebra: operations on the metric layout ------------------------------

/**
 * Put `count`/`unit` in force from `measure` (counting from 1).
 *
 * The grid alone changes: the same notes fall in different measures afterwards,
 * which is what changing the meter of a piece means.
 */
export function setMeter(sheet: Sheet, measure: number, count: number, unit: number): Sheet {
    return apply(sheet, { op: "set_meter", measure, count, unit });
}

/**
 * Open `count` empty measures before measure `at`.
 *
 * Time is added, so both structures move: a rest of the new measures' length is
 * written in, and every meter after the cut slides along with the music.
 */
export function insertMeasures(sheet: Sheet, at: number, count: number): Sheet {
    return apply(sheet, { op: "insert_measures", at, count });
}

/**
 * Take measures `first` to `last` out, with whatever was written in them. The
 * other half of {@link insertMeasures}, and the same rule.
 */
export function removeMeasures(sheet: Sheet, first: number, last: number): Sheet {
    return apply(sheet, { op: "remove_measures", first, last });
}

// -- the edit verbs: what a hand does to one item ------------------------------

/**
 * Write a new note, chord or rest into a voice.
 *
 * `after` names the item it follows (by id) and puts it in that item's own
 * voice; without one it goes first, on `staff`/`voice`. No `pitches` is a rest.
 * Everything after it moves later by `dur`: writing a note into finished music
 * adds time.
 */
export function insert(
    sheet: Sheet,
    dur: Ratio | number,
    { after, pitches = [], staff = 0, voice = 0 }: {
        after?: number;
        pitches?: unknown[];
        staff?: number;
        voice?: number;
    } = {},
): Sheet {
    const op: Op = { op: "insert", dur: ratio(dur), pitches, staff, voice };
    if (after !== undefined) op.after = after;
    return apply(sheet, op);
}

/**
 * Take an item out; everything after it moves earlier by its value.
 *
 * Not {@link silence} — that leaves a rest and nothing moves. Confusing the two
 * is how a piece comes out shorter than it was with no obvious sign of where.
 */
export function del(sheet: Sheet, id: number): Sheet {
    return apply(sheet, { op: "delete", id });
}

/**
 * Turn an item into a rest of the same length. Nothing moves, and it is still
 * the same item, so an id kept for it still names it.
 */
export function silence(sheet: Sheet, id: number): Sheet {
    return apply(sheet, { op: "silence", id });
}

/**
 * Give an item a different written value. What follows moves by the difference,
 * and the measures it now falls across are worked out when the page is written.
 */
export function setDur(sheet: Sheet, id: number, dur: Ratio | number): Sheet {
    return apply(sheet, { op: "set_dur", id, dur: ratio(dur) });
}

/**
 * Give an item different pitches — one for a note, several for a chord, none to
 * make it a rest. The value and the id are kept, so this is the same item newly
 * spelled rather than a replacement.
 */
export function setPitches(sheet: Sheet, id: number, pitches: unknown[]): Sheet {
    return apply(sheet, { op: "set_pitches", id, pitches });
}

/**
 * Tie an item into the one after it, or untie it.
 *
 * This is the tie you *write* — the note goes on sounding through the next item.
 * The ties added where a value crosses a barline are made when the page is
 * written and are never stored, so the two compose.
 */
export function tie(sheet: Sheet, id: number, tied = true): Sheet {
    return apply(sheet, { op: "tie", id, tied });
}

/**
 * Move items to another voice on their staff, leaving rests where they were.
 *
 * How two lines written as one come apart: the items keep their ids and their
 * place in time, and a rest holds each gap open, so nothing around either line
 * moves. Throws when the items are not all in one voice.
 */
export function toVoice(sheet: Sheet, ids: number[], voice: number): Sheet {
    return apply(sheet, { op: "to_voice", ids, voice });
}

/** What {@link marks} takes. */
export interface MarkOptions {
    /** Articulations, by their MEI names (`stacc`, `acc`, `ten`, `marc`). */
    articulations?: string[];
    /** A dynamic written under the staff at this note (`pp`…`ff`). */
    dynamic?: string;
    /** An ornament: `trill`, `mordent`, `turn`, `fermata`. */
    ornament?: string;
    /** That this is a grace note: `acc` (acciaccatura) or `unacc`. */
    grace?: string;
    /** A forced stem direction, `up` or `down`. */
    stem?: string;
    /** How long it **sounds**, when that is not how long it is written. */
    sounding?: Ratio | number;
}

/**
 * What a note carries beyond its pitch and value.
 *
 * Every one of them is a fact about the note, not an instruction to the
 * engraver, which is what lets a player read them back. `sounding` is kept apart
 * from the written value because a page that shortened the value would be a
 * different piece of music.
 */
export function marks(options: MarkOptions = {}): unknown {
    const out: Record<string, unknown> = {};
    if (options.articulations?.length) out.articulations = options.articulations;
    for (const key of ["dynamic", "ornament", "grace", "stem"] as const) {
        if (options[key] !== undefined) out[key] = options[key];
    }
    if (options.sounding !== undefined) out.sounding = ratio(options.sounding);
    return out;
}

/**
 * Give an item the marks it carries ({@link marks}).
 *
 * It **replaces** rather than merges: reading the marks, changing one and
 * sending them back is two calls and no ambiguity, where a merge would leave no
 * way to remove a mark at all. Throws on a rest, which has nothing to
 * articulate.
 */
export function setMarks(sheet: Sheet, id: number, m: unknown): Sheet {
    return apply(sheet, { op: "set_marks", id, marks: m });
}

/**
 * Write something between two notes: `"slur"`, `"crescendo"` or
 * `"diminuendo"`.
 *
 * It cannot go on an item because it has two ends, so it goes on the sheet
 * beside the staves. Adding the same one twice changes nothing; naming an item
 * that is not there throws, because a hairpin that never appears with no reason
 * given is worse than an error.
 */
export function addSpanner(sheet: Sheet, kind: string, from: number, to: number): Sheet {
    return apply(sheet, { op: "add_spanner", kind, from, to });
}

/**
 * Take back what {@link addSpanner} wrote. Removing one that is not there
 * changes nothing rather than throwing, since the state asked for holds.
 */
export function removeSpanner(sheet: Sheet, kind: string, from: number, to: number): Sheet {
    return apply(sheet, { op: "remove_spanner", kind, from, to });
}

/**
 * What is written above the music, for {@link setHeader}.
 *
 * Every field is optional because most of them are most of the time: a score
 * built by operating on a motif is untitled until somebody names it, and that
 * is a state rather than something missing.
 */
export function header(fields: HeaderFields = {}): HeaderFields {
    const out: HeaderFields = {};
    for (const key of ["title", "subtitle", "composer", "lyricist"] as const) {
        if (fields[key]) out[key] = fields[key];
    }
    return out;
}

/**
 * Move an item along the staff by `steps` **diatonic** places, up when positive
 * — what dragging a note on the page is.
 *
 * The arrival takes the **key signature's** alteration for the letter it lands
 * on, which is what reading in a key means: dragging a note onto a B in E flat
 * gives a B flat, and nobody has to say so. That is the difference between this
 * and {@link transpose}, which moves by a named *interval* and keeps the
 * alteration the arithmetic implies.
 *
 * A chord moves whole. Refused on a rest, which has no pitch to move.
 */
export function moveSteps(sheet: Sheet, id: number, steps: number): Sheet {
    return apply(sheet, { op: "move_steps", id, steps });
}

/**
 * Write what is above the music ({@link header}).
 *
 * It **replaces** rather than merges, as {@link setMarks} does: with a merge
 * there would be no way to clear a field at all, since an omitted one and an
 * emptied one look identical on the wire.
 */
export function setHeader(sheet: Sheet, fields: HeaderFields): Sheet {
    return apply(sheet, { op: "set_header", header: fields });
}

/**
 * Give `measure` (1-based) a right barline: `"end"`, `"rptstart"`, `"rptend"`,
 * `"rptboth"`, `"dbl"`, `"invis"` — or `"single"`, which takes the override
 * back rather than storing one saying "ordinary".
 *
 * A repeat barline is **notation**: it is drawn, and it is not what makes a
 * passage play twice. Repetition is written out ({@link repeat}), which is why
 * the interpreter has nothing to expand.
 */
export function setBarline(sheet: Sheet, measure: number, kind: string): Sheet {
    return apply(sheet, { op: "set_barline", measure, kind });
}

/**
 * Break the `"system"` or the `"page"` before `measure` (1-based); `"none"`
 * takes it back.
 *
 * This is layout, and it is an edit for the same reason a forced stem is one:
 * the engraver breaks lines wherever they fit, and a break somebody *chose* is
 * a statement about the page that no recomputation recovers.
 */
export function setBreak(sheet: Sheet, measure: number, kind: string): Sheet {
    return apply(sheet, { op: "set_break", measure, kind });
}
