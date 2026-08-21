// Score generation: the client's sequencing data into MEI (mirrors
// `clausters/gui/notation/mei.py`).
//
// The third way into the engraver, beside typed score text and the SVG adapter:
// turn the client's own `seq` data (`Event`, `Timeline`) into MEI — the format
// `engrave` already reads — so a melody or a bounced timeline is *seen* and
// edited as notation, the inverse of the score→sound flow.
//
// **The seam this module is** is worth naming, because it is where the
// agnostic/shell line falls and it is what a richer encoding extends: the
// reduction here is the client's half (it reads this language's types and
// flattens them into a *voice*, a monophonic-per-slot stream of ticks and MIDI
// pitches), and laying that voice out into barred, tied measures is the shared
// half in `clausters_core::notation`. Every client writes the same document
// from the same voice.

import { Event as SeqEvent } from "../../seq/event.ts";
import { Timeline } from "../../seq/timeline.ts";
import { voiceToMei as coreVoiceToMei } from "../../core/clausters_core_web.js";

/**
 * 32nd-note resolution: every duration snaps to an integer number of these, so
 * the encoder's barline splitting and tie decomposition are exact integer
 * arithmetic. Mirrors `clausters_core::notation`, which does that work.
 */
const TPW = 32; // ticks per whole note

/** One slot of the reduced voice: a note or chord, or a rest with no pitches. */
export interface Slot {
    midis?: number[];
    ticks: number;
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
 * Returns the MEI to hand to `engrave` (a one-shot display list) or to `Score`
 * (to edit and redraw).
 *
 * A duration that is not a single note value is written as **tied** notes (a
 * dotted value when exact, e.g. `1.5` beats → a dotted quarter), and a note that
 * overruns a barline is split and tied across it. Off-grid durations (finer than
 * a 32nd, e.g. a triplet) snap to the grid — tuplets are the
 * engraving-refinements milestone.
 */
export function fromNotes(
    notes: Iterable<SeqEvent>,
    { meter = "4/4", clef = "G2", key = "C", beatUnit = 4 }: MeiOptions = {},
): string {
    return voiceToMei(voiceFromNotes(notes, beatUnit), { meter, clef, key });
}

/**
 * Engrave a `seq.Timeline` into an MEI string.
 *
 * The timeline's placements become the score's rhythm: events **sharing a beat**
 * are written as one chord, a gap between a group's written end and the next
 * onset becomes a rest, and a gap before the first onset is a leading rest.
 * Items that carry no pitch (an `OscEvent`) are skipped, as are rest events
 * (they read as silence, i.e. a gap).
 *
 * Each group is written for its **shortest** `dur` (one layer, so it is clamped
 * never to overrun the next onset — mixed-duration polyphony is the
 * engraving-refinements milestone). Options and the tie/barline behaviour are as
 * {@link fromNotes}.
 */
export function fromTimeline(
    timeline: Timeline | Iterable<readonly [number, unknown]>,
    { meter = "4/4", clef = "G2", key = "C", beatUnit = 4 }: MeiOptions = {},
): string {
    return voiceToMei(voiceFromTimeline(timeline, beatUnit), { meter, clef, key });
}

// -- the intermediate voice: back-to-back slots -----------------------------
// One flat, monophonic-per-slot stream both entry points reduce to; a note slot
// carries one midi, a chord slot several, a rest none. It crosses to the shared
// encoder as JSON, one object per slot, which lays it out into barred, tied
// measures and emits the XML.

/** Hand a reduced voice to the shared MEI encoder. */
function voiceToMei(
    voice: Slot[],
    { meter, clef, key }: { meter: string; clef: string; key: string },
): string {
    return coreVoiceToMei(JSON.stringify(voice), meter, clef, key);
}

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
        if (event.get("type") === "rest") voice.push({ ticks });
        else voice.push({ midis: [Math.round(event.midinote())], ticks });
    }
    return voice;
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
        voice.push({ midis: events.map((e) => Math.round(e.midinote())), ticks });
        end = onset + ticks;
    }
    return voice;
}
