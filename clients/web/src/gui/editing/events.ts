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
import { MidiItem, OscItem, Timeline, itemData, itemFromData } from "../../seq/timeline.ts";
import { flatNotes, flatOsc, window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { View } from "./view.ts";

/** One note, as the roll draws it. */
export type Note = [start: number, dur: number, pitch: number, velocity: number, channel: number];

/** One event as the crate holds it. */
export interface CrateEvent {
    at: number;
    data?: Record<string, unknown>;
}

/**
 * One item's data as a string two of them can be compared by — key order is the
 * serializer's business and not a difference between two items.
 */
function stable(data: unknown): string {
    return JSON.stringify(sorted(data));
}

/** `value` with every object's keys in one order, however deep. */
function sorted(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(sorted);
    if (value === null || typeof value !== "object") return value;
    const held = value as Record<string, unknown>;
    return Object.keys(held)
        .sort()
        .map((key) => [key, sorted(held[key])]);
}

/**
 * The label the roll's OSC lane draws for an item, or `null` when the item is
 * not one of that lane's — an `OscItem` labels with its address, a `MidiItem`
 * with a short tag.
 */
function labelOf(item: unknown): string | null {
    if (item instanceof OscItem) return String(item.addr);
    if (item instanceof MidiItem) return "midi";
    return null;
}

/**
 * A timeline's vocabulary: the crate's `events`, with each item's own parameters
 * carried in its `data`.
 *
 * **Every item is an event here, not only the notes.** A timeline holds OSC
 * markers and raw MIDI beside its notes, the roll draws them in a lane of their
 * own, and the crate is explicit that an event's `data` is the client's and that
 * a lane of markers is one of the things this domain is for. So the state is the
 * whole timeline and the two lanes are two *gestures* over it — which is what
 * makes a marker dragged in the roll an edit with an inverse, instead of a
 * picture that quietly stops agreeing with the data.
 */
export class NotesDomain extends Domain<Timeline> {
    override readonly name = EVENTS;
    override readonly ingested = true;

    /**
     * What a beat is worth on the view's axis. The roll draws in timeline
     * samples and a timeline is in beats, so the crossing happens in the
     * reading — the editor's bridge is what supplies this.
     */
    unitsPerBeat = 1.0;

    /**
     * Whether a note may be written back onto this timeline. A roll over what a
     * **generator** produced is a rendering of an algorithm, so there is
     * nothing to write it onto — the view says so with the widget's own
     * `notesEditable`, and this is the second half of it, for a host that does
     * not read the prop.
     */
    editable = true;

    /**
     * The report, the timeline it is over, and the axis it was drawn on.
     *
     * **The whole timeline travels, not the lane the gesture drew.** Both lanes
     * state a whole-list intent, so a payload that named only the notes would be
     * an edit that deletes every marker — and the reading needs the untouched
     * lane in hand to carry it through.
     */
    override request(
        structure: Timeline,
        _tag: string,
        values: readonly unknown[],
    ): Record<string, unknown> {
        return {
            values: [...values],
            state: this.state(structure),
            unitsPerBeat: this.unitsPerBeat || 1.0,
            editable: this.editable,
        };
    }

    /**
     * The timeline as the crate holds it — every item, notes and markers alike,
     * since both are edited through this vocabulary.
     */
    state(structure: Timeline): CrateEvent[] {
        const out: CrateEvent[] = [];
        for (const [beat, item] of structure) {
            const data = itemData(item);
            if (data !== null) out.push({ at: Number(beat), data: plain(data) });
        }
        return out;
    }

    current(structure: Timeline, payload: unknown): unknown {
        return domainEdit(this.name, this.state(structure), payload)?.current ?? null;
    }

    project(structure: Timeline, payload: unknown): boolean {
        const edited = domainEdit(this.name, this.state(structure), payload);
        if (edited === undefined || !edited.applied) return false;
        // **What this build cannot describe is kept.** An item that is neither
        // an event nor a marker never entered the state, so it is held aside and
        // put back rather than rebuilt from a description nobody wrote.
        const others = [...structure].filter(([, item]) => itemData(item) === null);
        // **An item the edit did not change is the same object**, matched by
        // what it says rather than by where it sits — so a marker the notes
        // gesture never touched, and a note that only moved, come out the other
        // side as themselves, keeping whatever the JSON seam cannot carry (a
        // message's arguments, an event's resolved server). Only what the
        // gesture actually rewrote is built from its description.
        const held: [string, unknown][] = [];
        for (const [, item] of structure) {
            const data = itemData(item);
            if (data !== null) held.push([stable(plain(data)), item]);
        }
        const rebuilt: [number, unknown][] = [];
        for (const event of edited.state as CrateEvent[]) {
            const data = event.data ?? {};
            const key = stable(data);
            const was = held.findIndex(([heldKey, item]) => item !== null && heldKey === key);
            let item: unknown;
            if (was >= 0) {
                item = held[was][1];
                held[was] = [key, null];
            } else {
                item = itemFromData(data);
            }
            rebuilt.push([Number(event.at ?? 0), item]);
        }
        // **One step, not a clear and a rebuild.** Same call as the Python
        // client's, in the same place — see `Timeline.replace` for what a
        // half-rebuilt timeline costs the client whose loop has a thread.
        structure.replace([...rebuilt, ...others]);
        return true;
    }
}

/** One `pianoroll`: the timeline's notes on the beat grid. */
export class NotesView extends View<Timeline> {
    build(editor: Editor<Timeline>): GuiNode {
        // The pitch window the roll fits to its notes is the crate's, and so is
        // saying **before the hand tries** that a roll over what a generator
        // produced has nothing to write onto — the widget refuses the press
        // instead of offering a drag it will unwind.
        const editable = !(editor.domain instanceof NotesDomain) || editor.domain.editable;
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            this.catalogue(editor, "pianoroll", "roll", editor.structure, {
                notes: flatNotes(drawn(editor)),
                osc: flatOsc(markers(editor)),
                ruler: "beats",
                // The ruler draws the timeline's beats through the timeline's
                // map: configuration of the ruler, read from the data it shows.
                tempo_map: editor.structure.map.dump(),
                sample_rate: editor.sampleRate,
                editable,
            }),
            ...editor.extra,
        );
    }

    override props(editor: Editor<Timeline>): Record<string, PropValue> {
        // **Both lanes**: a correction is what the widget should be drawing, and
        // a refused marker is answered by the markers as they still are. The
        // ruler's map goes with them, so a tempo edited on the timeline redraws.
        return {
            notes: flatNotes(drawn(editor)) as PropValue,
            osc: flatOsc(markers(editor)) as PropValue,
            tempo_map: editor.structure.map.dump(),
        };
    }
}

/**
 * A timeline on screen, editable back into the `Timeline` the caller already
 * holds.
 */
export class NotesEditor extends Editor<Timeline> {
    constructor(timeline: Timeline, options: NotesEditorOptions) {
        const domain = new NotesDomain();
        domain.editable = options.editable ?? true;
        super(timeline, { title: "Notes", ...options, domain, view: new NotesView() });
        // The bridge is the editor's, so the domain reads it from here rather
        // than keeping a second one.
        domain.unitsPerBeat = this.unitsPerBeat;
    }
}

/** {@link NotesEditor}'s options: the generic ones plus whether it writes. */
export interface NotesEditorOptions extends GenericEditorOptions<Timeline> {
    /**
     * Whether a note may be written back. `false` for a roll over what a
     * forward-only generator produced.
     */
    editable?: boolean;
}

/**
 * The timeline's OSC (and raw MIDI) items as `[timeUnits, label]` pairs — the
 * roll's OSC lane. An `OscItem` labels with its address, a `MidiItem` with a
 * short tag.
 *
 * The label is the whole of what the lane can say — the message's arguments are
 * not drawn — which is why a marker moved or removed there is matched back to
 * its item **by label** and one added there is refused: the address is what a
 * marker sends, and the lane has no way to type one.
 */
function markers(editor: Editor<Timeline>): [number, string][] {
    const out: [number, string][] = [];
    for (const [beat, item] of editor.structure) {
        const label = labelOf(item);
        if (label !== null) out.push([editor.beatsToUnits(Number(beat)), label]);
    }
    return out;
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
 * How long a note **sounds**, in beats — `Event.sustain`, which is
 * `dur * legato` when nothing set one outright.
 *
 * That is what a roll draws and what a drag on a note's edge sets, so reading
 * the explicit key alone would draw an articulated note at its grid length and
 * hand the edit-back a number the hand never saw.
 */
function lengthOf(event: SeqEvent): number {
    try {
        return Number(event.sustain());
    } catch {
        const value = event.get("dur");
        return value === null || value === undefined ? 1.0 : Number(value);
    }
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
 * name that answers for it, which is the rule the document already follows for a
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
