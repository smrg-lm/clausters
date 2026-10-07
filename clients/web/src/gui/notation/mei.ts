// The two directions between the client's sequencing data and a score (mirrors
// `clausters/gui/notation/mei.py`).
//
// The third way into the engraver, beside typed score text and the SVG adapter:
// turn the client's own `seq` data (an `Event` run, a `Timeline`, an
// `EventSequence`) into a score, so a melody, a bounced timeline or a take
// played from a keyboard is *seen* and edited as notation -- and back again,
// {@link toTimeline} and {@link toSequence}, which read a sheet into what it
// sounds.
//
// **Both directions are the core's.** A score is rendered into events by
// `renderEvents`, and events are read into a score by `readEvents`: what the
// events say of their page is written as they say it, and what they do not --
// when a note falls on the page, in which voice, spelled how -- is decided
// there, once, for every client. What this module adds is the client's own
// types on either side.

import { Event } from "../../seq/event.ts";
import { Timeline } from "../../seq/timeline.ts";
import { EventSequence } from "../../seq/sequence.ts";
import type { Interpretation, Sheet, Transcription } from "./sheet.ts";
import { readEvents, renderEvents, toMei } from "./sheet.ts";
import type { RenderedSequence } from "./sheet.ts";

/**
 * One slot of the reduced voice: a note or chord, or a rest with no pitches.
 *
 * Everything past `midis` and `ticks` is **what is written on the note** -- each
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

/** What a sequence, a timeline or `[beat, event]` pairs are read from. */
export type Placed = EventSequence | Timeline | Iterable<readonly [number, unknown]>;

/** A transcription, and the reading whose dynamics name a level. */
export type ReadOptions = Transcription & { interp?: Interpretation };

/**
 * Engrave a **monophonic** run of events into an MEI string.
 *
 * `notes` is any iterable of `seq.Event` (a `rest` is a silence); each occupies
 * its written `dur` beats back to back, so this is the notation of a melody
 * the way a `Pbind`/`Routine` sequence reads it. `how` is the transcription,
 * as {@link sheetFromEvents} takes it. Returns the MEI to hand to `engrave` or
 * to `Score`.
 */
export function fromNotes(notes: Iterable<Event>, how: ReadOptions = {}): string {
    return toMei(sheetFromNotes(notes, how));
}

/**
 * Engrave a `seq.Timeline` -- its placed events, as {@link sheetFromEvents}
 * reads them -- into an MEI string.
 */
export function fromTimeline(timeline: Placed, how: ReadOptions = {}): string {
    return toMei(sheetFromEvents(timeline, how));
}

// -- stopping at the model ----------------------------------------------------
// The same reductions, handing back the **sheet** rather than the MEI. What
// they are for is everything the model can do that a string cannot: operate on
// the score, and read it back into sound.

/**
 * Read an `EventSequence` -- or a `seq.Timeline`, or any `[beat, event]` pairs
 * -- into a sheet: the way back from {@link toSequence}.
 *
 * An event's notation keys (`seq.NOTATION_KEYS`) are written as they say, and
 * a sequence a score was rendered into is read back as it was written. What
 * the events do not say is decided by the transcription (`readEvents`
 * describes each key): `meter`, `key`, `clef`, `beatUnit`, `division` (the
 * smallest written value an onset is snapped to), `tuplets`, `voices` and
 * `dynamics`. `interp` is the reading whose dynamics name a level.
 *
 * Events that carry no pitch (an `"osc"` or `"midi"` one) are skipped, and a
 * rest is a silence. **The sequence is not changed** -- a take keeps the times
 * it was played with, and is read again with another `division` by calling
 * this again.
 */
export function sheetFromEvents(sequence: Placed, { interp, ...how }: ReadOptions = {}): Sheet {
    return readEvents(dataOf(sequence), how, interp).sheet;
}

/**
 * {@link fromNotes}, stopping at the score model instead of the MEI: the run
 * placed back to back, each event at the end of the one before it, and read as
 * {@link sheetFromEvents} reads a sequence -- in one voice, as a line is.
 */
export function sheetFromNotes(
    notes: Iterable<Event>,
    { interp, ...how }: ReadOptions = {},
): Sheet {
    let at = 0;
    const placed: [number, Event][] = [];
    for (const event of notes) {
        placed.push([at, event]);
        at += Number(event.get("dur"));
    }
    return readEvents(dataOf(placed), { voices: 1, ...how }, interp).sheet;
}

/**
 * {@link sheetFromEvents}, under the name it had: a timeline's placed events
 * read into a sheet.
 */
export function sheetFromTimeline(timeline: Placed, how: ReadOptions = {}): Sheet {
    return sheetFromEvents(timeline, how);
}

/**
 * What `readEvents` takes: a sequence's data, or the events of `[beat, event]`
 * pairs as one.
 */
function dataOf(sequence: Placed): unknown {
    if (sequence instanceof EventSequence) return sequence.data();
    const events: { at: number; data: Record<string, unknown> }[] = [];
    for (const [beat, item] of sequence) {
        if (item instanceof Event) events.push({ at: Number(beat), data: item.keysData() });
    }
    return { events };
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
 * and the **heard** one as `sustain` -- which is the pair the page keeps apart
 * and the reason a staccato quarter is still a quarter.
 *
 * `instruments` binds a staff to what plays it, since the notation does not
 * say. Left out, events take the client's default instrument.
 *
 * **What is on the page comes with it.** Each event also carries the marks the
 * note was written with (`seq.NOTATION_KEYS`) -- its articulations verbatim, not the
 * `sustain` they produced -- so a timeline read from a score and written back
 * with {@link sheetFromEvents} engraves the same page. What does not survive
 * that trip is everything that is not one note's: a slur, a hairpin, a tuplet,
 * the meter and the barlines, the title -- none of them can ride an event, and
 * they are the reason a score is a score rather than a list of notes.
 */
export function toTimeline(
    score: Sheet,
    { instruments, interp, event = {} }: PlaybackOptions = {},
): Timeline {
    const out = new Timeline();
    for (const rendered of rendering(score, { instruments, interp, event }).events) {
        out.add(rendered.at, new Event(rendered.data));
    }
    return out;
}

/**
 * Renders a sheet into an `EventSequence`, one way ({@link renderEvents}): its
 * events as concrete data a notes editor edits -- each with an id, in beats,
 * on its voice's channel -- the staves' dynamics as curves of those channels,
 * and what is no note's in the sequence's `notation` section. The options are
 * {@link toTimeline}'s.
 */
export function toSequence(score: Sheet, options: PlaybackOptions = {}): EventSequence {
    return EventSequence.fromData(rendering(score, options));
}

/**
 * The sheet rendered ({@link renderEvents}), each event given what plays its
 * staff and the keys a score has no symbol for.
 */
function rendering(
    score: Sheet,
    { instruments, interp, event = {} }: PlaybackOptions,
): RenderedSequence {
    const data = renderEvents(score, interp);
    for (const rendered of data.events) {
        const keys: Record<string, unknown> = { ...event, ...rendered.data };
        const instrument = instrumentFor(instruments, Number(keys.staff));
        if (instrument !== undefined) keys.instrument = instrument;
        rendered.data = keys;
    }
    return data;
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
