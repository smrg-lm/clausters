/**
 * Editing a **timeline of events**: the roll, with no composition under it.
 *
 * A `Timeline` a page filled is edited by the same gesture that edits a track's
 * notes in the multitrack, and until now the only way to write one back was an
 * aggregate's `SetMembers` — which needs a tree to be a member *of*. This is that
 * gesture over the timeline itself: the crate's `events` vocabulary, one
 * `pianoroll`, and the object the caller already holds written in place.
 *
 * **What an event is stays the client's.** The crate carries an event's `data`
 * and never reads it, so an `Event` travels whole and comes back whole — the
 * pitch, the length, the instrument and whatever else the author put on it. What
 * the roll can say about a note is five numbers; what the note *is* is more than
 * that, and an edit that rebuilt one from the five would drop the rest.
 *
 * @module
 */

import { EVENTS, domainEdit } from "../../document.ts";
import { Event as SeqEvent } from "../../seq/event.ts";
import { Timeline } from "../../seq/timeline.ts";
import { flatNotes, pianoroll, window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { View } from "./view.ts";

/** What the `pianoroll` widget sends and takes per note. */
export const QUINTUPLE = 5;

/** One note, as the roll draws it. */
export type Note = [start: number, dur: number, pitch: number, velocity: number, channel: number];

/** One event as the crate holds it. */
export interface CrateEvent {
    at: number;
    data?: Record<string, unknown>;
}

/**
 * A flat `notes` payload as `[start, dur, pitch, velocity, channel]` tuples,
 * dropping a trailing partial group rather than guessing at it.
 */
export function quintuples(flat: readonly unknown[]): Note[] {
    const values = flat.map(Number);
    const out: Note[] = [];
    for (let i = 0; i + QUINTUPLE <= values.length; i += QUINTUPLE) {
        out.push(values.slice(i, i + QUINTUPLE) as Note);
    }
    return out;
}

/**
 * A timeline's vocabulary: the crate's `events`, with each event's own
 * parameters carried in its `data`.
 */
export class NotesDomain extends Domain<Timeline> {
    override readonly name = EVENTS;

    /**
     * What a beat is worth on the view's axis. The roll draws in timeline
     * samples and a timeline is in beats, so the crossing happens here — the
     * editor's bridge is what supplies it.
     */
    unitsPerBeat = 1.0;

    payload(structure: Timeline, tag: string, values: readonly unknown[]): unknown {
        if (tag !== "notes") return null;
        const held = [...structure]
            .map(([, item]) => item)
            .filter((item): item is SeqEvent => item instanceof SeqEvent);
        const events: CrateEvent[] = [];
        quintuples(values).forEach(([start, dur, pitch, velocity, channel], i) => {
            const was = held[i];
            const length = dur / this.unitsPerBeat;
            let params: Record<string, unknown>;
            if (was !== undefined) {
                // **An edit updates the note it names; it does not rebuild it.**
                // Order is the only identity the payload carries, so the i-th
                // note's own event is copied and the drawn fields written over
                // it — which keeps the instrument and everything else the author
                // put there.
                params = { ...was.props, midinote: Math.trunc(pitch), sustain: length };
                if (Math.trunc(velocity) !== velocityOf(was)) {
                    params.velocity = Math.trunc(velocity);
                    params.amp = Math.max(0, Math.min(1, Math.trunc(velocity) / 127));
                }
            } else {
                params = {
                    midinote: Math.trunc(pitch),
                    dur: length,
                    legato: 1.0,
                    amp: Math.max(0, Math.min(1, Math.trunc(velocity) / 127)),
                    velocity: Math.trunc(velocity),
                };
            }
            if (Math.trunc(channel)) params.channel = Math.trunc(channel);
            events.push({ at: start / this.unitsPerBeat, data: plain(params) });
        });
        return { intent: "setevents", events };
    }

    /** The timeline as the crate holds it. */
    state(structure: Timeline): CrateEvent[] {
        const out: CrateEvent[] = [];
        for (const [beat, item] of structure) {
            if (item instanceof SeqEvent) out.push({ at: Number(beat), data: plain(item.props) });
        }
        return out;
    }

    current(structure: Timeline, payload: unknown): unknown {
        return domainEdit(this.name, this.state(structure), payload)?.current ?? null;
    }

    project(structure: Timeline, payload: unknown): boolean {
        const edited = domainEdit(this.name, this.state(structure), payload);
        if (edited === undefined || !edited.applied) return false;
        // **What is not a note is kept.** A timeline holds OSC and MIDI items
        // too, and a roll draws none of them; rebuilding from the events alone
        // would silently drop what the view could not see.
        const others = [...structure].filter(([, item]) => !(item instanceof SeqEvent));
        structure.clear();
        for (const event of edited.state as CrateEvent[]) {
            structure.add(Number(event.at ?? 0), new SeqEvent({ ...(event.data ?? {}) }));
        }
        for (const [beat, item] of others) structure.add(beat, item);
        return true;
    }

    override label(): string {
        return "edit the notes";
    }
}

/** One `pianoroll`: the timeline's notes on the beat grid. */
export class NotesView extends View<Timeline> {
    /** The pitch window a roll falls back to when the timeline is empty. */
    static readonly DEFAULT_PITCH: [number, number] = [48, 84];
    static readonly PAD = 4;

    build(editor: Editor<Timeline>): GuiNode {
        const wid = this.register(editor.newId(), editor.structure);
        const notes = drawn(editor);
        const body: { min?: number; max?: number } = {};
        if (notes.length > 0) {
            const pitches = notes.map((n) => n[2]);
            body.min = Math.min(Math.min(...pitches) - NotesView.PAD, NotesView.DEFAULT_PITCH[1]);
            body.max = Math.max(Math.max(...pitches) + NotesView.PAD, NotesView.DEFAULT_PITCH[0]);
        }
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            pianoroll({
                id: wid,
                notes: notes.length > 0 ? notes : undefined,
                ruler: "beats",
                tempo: editor.tempo,
                sampleRate: editor.sampleRate,
                ...body,
            }),
        );
    }

    override props(editor: Editor<Timeline>): Record<string, PropValue> {
        return { notes: flatNotes(drawn(editor)) as PropValue };
    }
}

/**
 * A timeline on screen, editable back into the `Timeline` the caller already
 * holds.
 */
export class NotesEditor extends Editor<Timeline> {
    constructor(timeline: Timeline, options: GenericEditorOptions<Timeline>) {
        const domain = new NotesDomain();
        super(timeline, { title: "Notes", ...options, domain, view: new NotesView() });
        // The bridge is the editor's, so the domain reads it from here rather
        // than keeping a second one.
        domain.unitsPerBeat = this.unitsPerBeat;
    }
}

/**
 * The timeline's notes as the roll draws them: `[start, dur, pitch, velocity,
 * channel]` in timeline samples.
 */
function drawn(editor: Editor<Timeline>): Note[] {
    const out: Note[] = [];
    for (const [beat, item] of editor.structure) {
        const pitch = pitchOf(item);
        if (pitch === null) continue;
        const event = item as SeqEvent;
        const at = editor.beatsToUnits(Number(beat));
        out.push([
            at,
            editor.beatsToUnits(Number(beat) + lengthOf(event)) - at,
            pitch,
            velocityOf(event),
            Math.trunc(Number(event.get("channel") ?? 0)),
        ]);
    }
    return out;
}

/**
 * How long a note sounds, in beats: what it was drawn at (`sustain`) when
 * something set one, else its own length.
 */
function lengthOf(event: SeqEvent): number {
    for (const key of ["sustain", "dur"]) {
        const value = event.get(key);
        if (value !== null && value !== undefined) return Number(value);
    }
    return 1.0;
}

/**
 * The MIDI pitch of a timeline item, or `null` when it carries none — an OSC
 * marker, a rest, anything that is not an event.
 */
function pitchOf(item: unknown): number | null {
    if (!(item instanceof SeqEvent) || item.get("type") === "rest") return null;
    try {
        return Number(item.midinote());
    } catch {
        return null;
    }
}

/**
 * The MIDI velocity of a note: an explicit `velocity`, else the linear `amp`
 * mapped onto the velocity range, else the default.
 */
function velocityOf(event: SeqEvent): number {
    const vel = event.get("velocity");
    if (vel !== null && vel !== undefined) {
        return Math.max(0, Math.min(127, Math.trunc(Number(vel))));
    }
    const amp = event.get("amp");
    if (amp !== null && amp !== undefined) {
        return Math.max(1, Math.min(127, Math.round(Number(amp) * 127)));
    }
    return 100;
}

/**
 * An event's parameters as plain JSON-able data — what is not, travels as the
 * name that answers for it, which is the rule `toDocument` already follows for a
 * clang's configuration.
 */
function plain(value: unknown): never;
function plain(value: Record<string, unknown>): Record<string, unknown>;
function plain(value: unknown): unknown {
    if (Array.isArray(value)) return value.map((v) => plain(v as Record<string, unknown>));
    if (value !== null && typeof value === "object") {
        const out: Record<string, unknown> = {};
        for (const [key, held] of Object.entries(value)) {
            out[key] = plain(held as Record<string, unknown>);
        }
        return out;
    }
    if (value === null || ["string", "number", "boolean"].includes(typeof value)) return value;
    const name = (value as { name?: unknown }).name;
    return typeof name === "string" && name ? name : null;
}

/** Whether `edit` should open this as a roll. */
export function isEvents(structure: unknown): structure is Timeline {
    return structure instanceof Timeline;
}
