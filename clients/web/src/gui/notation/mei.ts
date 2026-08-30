// The two directions between the client's sequencing data and a score (mirrors
// `clausters/gui/notation/mei.py`).
//
// The third way into the engraver, beside typed score text and the SVG adapter:
// turn the client's own `seq` data (`Event`, `Timeline`) into MEI — the format
// `engrave` already reads — so a melody or a bounced timeline is *seen* and
// edited as notation, the inverse of the score→sound flow.
//
// **And back again.** {@link toTimeline} is the return trip: a sheet read into
// what it sounds (`toNotes`, which is where the symbols are honoured) and placed
// on a `seq.Timeline` of `Event`s. It is here rather than beside the model
// because it is the same seam in the other direction — building `Event`s reads
// this language's types and stays in this client, while what a staccato *means*
// is one implementation in Rust.
//
// **The seam this module is** is worth naming, because it is where the
// agnostic/shell line falls and it is what a richer encoding extends: the
// reduction here is the client's half (it reads this language's types and
// flattens them into a *voice*, a monophonic-per-slot stream of ticks and MIDI
// pitches), and laying that voice out into barred, tied measures is the shared
// half in `clausters_core::notation`. Every client writes the same document
// from the same voice.

import { NOTATION_KEYS, Event as SeqEvent } from "../../seq/event.ts";
import { Timeline } from "../../seq/timeline.ts";
import type { Interpretation, Sheet } from "./sheet.ts";
import { fromVoice, toMei, toNotes } from "./sheet.ts";

/**
 * 32nd-note resolution: every duration snaps to an integer number of these, so
 * the encoder's barline splitting and tie decomposition are exact integer
 * arithmetic. Mirrors `clausters_core::notation`, which does that work.
 */
const TPW = 32; // ticks per whole note

/**
 * One slot of the reduced voice: a note or chord, or a rest with no pitches.
 *
 * Everything past `midis` and `ticks` is **what is written on the note** — each
 * field optional, and each a musical fact rather than an instruction to the
 * engraver, which is what lets the same field be read in both directions. A
 * slot carrying none of them produces exactly the item it always did; an
 * unknown key is refused by the core rather than dropped.
 *
 * What a slot cannot say is anything that is not one note's: a slur, a hairpin,
 * a meter change or a title span notes or the document, and they are written
 * *beside* the voice with the model's own verbs. The **nth slot becomes the
 * item with id `n + 1`**, which is how a caller names its own notes to them.
 */
export interface Slot {
    /** The MIDI pitches sounding: one for a note, several for a chord. */
    midis?: number[];
    /** How long it lasts, in 32nd-notes. */
    ticks: number;
    /** Articulations, by their MEI names (`stacc`, `acc`, `ten`, `marc`). */
    articulations?: string[];
    /** A dynamic written at this note, governing the ones after it. */
    dynamic?: string;
    /** An ornament: `trill`, `mordent`, `turn`, `fermata`. */
    ornament?: string;
    /** That this is a grace note: `acc` (acciaccatura) or `unacc`. */
    grace?: string;
    /** A stem direction the writer forced, `up` or `down`. */
    stem?: string;
    /** How long it is **held**, in ticks, when no symbol already says it. */
    sounding?: number;
    /** Which enharmonic to spell an altered pitch as: `"sharp"`/`"flat"`. */
    spelling?: string;
    /** `"written"` for an accidental to be printed, else `"sounding"`. */
    accidental?: string;
    /** That this note ties into the next slot. */
    tie?: boolean;
}

/** What both entry points take past the data itself. */
export interface MeiOptions {
    /** The barring, as `"num/den"`. */
    meter?: string;
    /** The staff: a shape and a line, `"G2"`/`"F4"`/`"C3"`. */
    clef?: string;
    /** The key signature, and with it the sharp-vs-flat spelling. */
    key?: string;
    /** What one beat is worth (`4` = a quarter). */
    beatUnit?: number;
}

/**
 * Engrave a **monophonic** run of events into an MEI string.
 *
 * `notes` is any iterable of `seq.Event` (a `rest` becomes a rest); each
 * occupies its written `dur` beats back to back, so this is the notation of a
 * melody the way a `Pbind`/`Routine` sequence reads it. The pitch is the event's
 * `midinote()` (rounded to the nearest semitone), the value is `dur`.
 *
 * An event may also say what the note is **on a page** (`seq.NOTATION_KEYS`):
 * `articulations`, `dynamic`, `ornament`, `grace`, `stem`, `spelling`,
 * `accidental` and `tie` reach the score under their own names, and an explicit
 * `sustain` becomes how long the note is *held* — but only where no
 * articulation already says so, since a staccato that was also written as a
 * short length would be shortened twice on the way back.
 *
 * Returns the MEI to hand to `engrave` (a one-shot display list) or to `Score`
 * (to edit and redraw).
 *
 * A duration that is not a single note value is written as **tied** notes (a
 * dotted value when exact, e.g. `1.5` beats → a dotted quarter), and a note that
 * overruns a barline is split and tied across it. Off-grid durations (finer than
 * a 32nd, e.g. a triplet) snap to the grid here, on the way in: the model itself
 * holds an exact rational, so a tuplet is representable the moment a caller can
 * express one — writing it is the emission milestone.
 */
export function fromNotes(
    notes: Iterable<SeqEvent>,
    { meter = "4/4", clef = "G2", key = "C", beatUnit = 4 }: MeiOptions = {},
): string {
    return toMei(sheetFromNotes(notes, { meter, clef, key, beatUnit }));
}

/**
 * Engrave a `seq.Timeline` into an MEI string.
 *
 * The timeline's placements become the score's rhythm: events **sharing a beat**
 * are written as one chord, a gap between a group's written end and the next
 * onset becomes a rest, and a gap before the first onset is a leading rest.
 * Items that carry no pitch (an `OscItem`) are skipped, as are rest events
 * (they read as silence, i.e. a gap).
 *
 * Each group is written for its **shortest** `dur` (one layer, so it is clamped
 * never to overrun the next onset — the model holds several voices already, and
 * writing them is the emission milestone). Options and the tie/barline
 * behaviour are as {@link fromNotes}.
 */
export function fromTimeline(
    timeline: Timeline | Iterable<readonly [number, unknown]>,
    { meter = "4/4", clef = "G2", key = "C", beatUnit = 4 }: MeiOptions = {},
): string {
    return toMei(sheetFromTimeline(timeline, { meter, clef, key, beatUnit }));
}

// -- stopping at the model ----------------------------------------------------
// The same two reductions, handing back the **sheet** rather than the MEI. What
// they are for is everything the model can do that a string cannot: operate on
// the score, and read it back into sound.

/**
 * {@link fromNotes}, stopping at the score model instead of the MEI.
 *
 * The sheet is what `toMei` writes and what `toNotes` reads back, so a caller
 * that wants to operate on the score — or hear it as the page says rather than
 * as the events said — starts here.
 */
export function sheetFromNotes(
    notes: Iterable<SeqEvent>,
    { meter = "4/4", clef = "G2", key = "C", beatUnit = 4 }: MeiOptions = {},
): Sheet {
    return fromVoice(voiceFromNotes(notes, beatUnit), { meter, clef, key });
}

/** {@link fromTimeline}, stopping at the score model instead of the MEI. */
export function sheetFromTimeline(
    timeline: Timeline | Iterable<readonly [number, unknown]>,
    { meter = "4/4", clef = "G2", key = "C", beatUnit = 4 }: MeiOptions = {},
): Sheet {
    return fromVoice(voiceFromTimeline(timeline, beatUnit), { meter, clef, key });
}

/** What {@link toTimeline} takes past the sheet itself. */
export interface PlaybackOptions {
    /**
     * What plays each staff: one def name for every staff, or a mapping from
     * staff index (0 is the top one) to def name.
     */
    instruments?: string | Record<number, string>;
    /** The reading (`interpretation`); left out, the default. */
    interp?: Interpretation;
    /** Merged into every event, for what a score has no symbol for at all. */
    event?: Record<string, unknown>;
}

/**
 * Read a sheet into a `seq.Timeline` that plays it.
 *
 * The return trip, and the one `toNotes` does the thinking for: each sounding
 * note becomes an `Event` at its onset, carrying the **written** value as `dur`
 * and the **heard** one as `sustain` — which is the pair the page keeps apart
 * and the reason a staccato quarter is still a quarter.
 *
 * `instruments` binds a staff to what plays it, since the notation does not
 * say. Left out, events take the client's default instrument.
 *
 * **What is on the page comes with it.** Each event also carries the marks the
 * note was written with (`seq.NOTATION_KEYS`) — its articulations verbatim, not the
 * `sustain` they produced — so a timeline read from a score and written back
 * with {@link sheetFromTimeline} engraves the same page. What does not survive
 * that trip is everything that is not one note's: a slur, a hairpin, a tuplet,
 * the meter and the barlines, the title — none of them can ride an event, and
 * they are the reason a score is a score rather than a list of notes.
 */
export function toTimeline(
    score: Sheet,
    { instruments, interp, event = {} }: PlaybackOptions = {},
): Timeline {
    const out = new Timeline();
    for (const note of toNotes(score, interp)) {
        const fields: Record<string, unknown> = {
            ...event,
            midinote: note.pitch,
            dur: note.dur,
            sustain: note.sustain,
            amp: note.amp,
        };
        Object.assign(fields, note.marks ?? {});
        delete fields.sounding; // `sustain` already holds it, in beats
        for (const key of ["spelling", "accidental"] as const) {
            if (note[key] !== undefined) fields[key] = note[key];
        }
        const instrument = instrumentFor(instruments, note.staff);
        if (instrument !== undefined) fields.instrument = instrument;
        out.add(note.t, new SeqEvent(fields));
    }
    return out;
}

/** What plays `staff`: one name for every staff, or a mapping. */
function instrumentFor(
    instruments: string | Record<number, string> | undefined,
    staff: number,
): string | undefined {
    if (instruments === undefined) return undefined;
    if (typeof instruments === "string") return instruments;
    return instruments[staff];
}

// -- the intermediate voice: back-to-back slots -----------------------------
// One flat, monophonic-per-slot stream both entry points reduce to; a note slot
// carries one midi, a chord slot several, a rest none. It crosses to the shared
// encoder as JSON, one object per slot, which lays it out into barred, tied
// measures and emits the XML.

/**
 * A *duration* in beats → 32nd-note ticks (a whole note is `beatUnit` beats).
 * At least one tick — a sounding note never has zero length.
 */
function durTicks(beats: number, beatUnit: number): number {
    return Math.max(1, Math.round((Number(beats) * TPW) / beatUnit));
}

/**
 * A *position* on the beat axis → 32nd-note ticks. Unlike a duration this may be
 * zero: beat 0 is tick 0, not tick 1, or a downbeat onset would push a spurious
 * rest before the first note and knock the whole bar off the grid.
 */
function posTicks(beat: number, beatUnit: number): number {
    return Math.round((Number(beat) * TPW) / beatUnit);
}

function voiceFromNotes(notes: Iterable<SeqEvent>, beatUnit: number): Slot[] {
    const voice: Slot[] = [];
    for (const event of notes) {
        const ticks = durTicks(Number(event.get("dur")), beatUnit);
        if (event.get("type") === "rest") {
            voice.push({ ticks });
            continue;
        }
        const slot: Slot = { midis: [Math.round(event.midinote())], ticks };
        writeMarks(slot, [event], ticks, beatUnit);
        voice.push(slot);
    }
    return voice;
}

/**
 * Put what `events` say about the *page* onto `slot`.
 *
 * Every key is carried under its own name (`seq.Event` and the slot agree on
 * the vocabulary, which is what keeps the two directions one thing), except the
 * length in the air, which is the one place the two do not line up:
 *
 * **A `sustain` reaches the page only when nothing on the page already says
 * it.** An event that is both staccato and short is not two facts: the staccato
 * is the fact, and the short length is what an interpretation makes of it.
 * Written as both, the next reading would shorten an already shortened note. So
 * `sounding` is what the sustain says that no symbol said — and it is left out
 * entirely when the note is held for its written value, where it says nothing.
 *
 * A chord is **one** slot and the model puts one set of marks on it, so the
 * events sharing a beat are read together and the first to say something wins
 * that key. Which is right rather than a compromise: what is written is written
 * on the chord, so a staccato any of its notes carries is the chord's. A slot
 * cannot hold two notes marked differently, and that is the documented loss.
 */
function writeMarks(slot: Slot, events: SeqEvent[], ticks: number, beatUnit: number): void {
    for (const key of NOTATION_KEYS) {
        for (const event of events) {
            const value = event.get(key);
            if (value !== undefined && value !== null) {
                (slot as unknown as Record<string, unknown>)[key] = value;
                break;
            }
        }
    }
    const stated = events.find((e) => {
        const sustain = e.get("sustain");
        return sustain !== undefined && sustain !== null;
    });
    if (stated === undefined) return;
    if ((slot.articulations ?? []).length) return;
    const held = durTicks(stated.sustain(), beatUnit);
    if (held !== ticks) slot.sounding = held;
}

/**
 * Group the timeline by onset beat into chord/note slots, filling the gaps
 * between them with rests.
 */
function voiceFromTimeline(
    timeline: Timeline | Iterable<readonly [number, unknown]>,
    beatUnit: number,
): Slot[] {
    const groups = new Map<number, SeqEvent[]>();
    for (const [beat, item] of timeline) {
        // Skip what has no pitch (a raw OSC item) or is silence (a rest).
        if (!(item instanceof SeqEvent) || item.get("type") === "rest") continue;
        const at = Number(beat);
        const group = groups.get(at);
        if (group === undefined) groups.set(at, [item]);
        else group.push(item);
    }

    const beats = [...groups.keys()].sort((a, b) => a - b);
    const voice: Slot[] = [];
    let end = 0; // ticks consumed so far
    for (let i = 0; i < beats.length; i++) {
        const beat = beats[i] as number;
        const events = groups.get(beat) as SeqEvent[];
        const onset = posTicks(beat, beatUnit);
        if (onset > end) voice.push({ ticks: onset - end }); // a gap → a rest
        let ticks = durTicks(
            Math.min(...events.map((e) => Number(e.get("dur")))),
            beatUnit,
        );
        if (i + 1 < beats.length) {
            // One layer: never overrun the next onset.
            const next = posTicks(beats[i + 1] as number, beatUnit);
            if (next > onset) ticks = Math.min(ticks, next - onset);
        }
        const slot: Slot = { midis: events.map((e) => Math.round(e.midinote())), ticks };
        writeMarks(slot, events, ticks, beatUnit);
        voice.push(slot);
        end = onset + ticks;
    }
    return voice;
}
