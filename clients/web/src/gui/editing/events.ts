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
import { flatNotes, flatOsc, pianoroll, window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { View } from "./view.ts";

/** What the `pianoroll` widget sends and takes per note. */
export const QUINTUPLE = 5;

/** And per OSC marker: the time and the label. */
export const PAIR = 2;

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
 * A flat `osc` payload as `[time, label]` tuples, dropping a trailing odd value
 * the same way {@link quintuples} drops a partial group.
 */
export function pairs(flat: readonly unknown[]): [number, string][] {
    const out: [number, string][] = [];
    for (let i = 0; i + PAIR <= flat.length; i += PAIR) {
        out.push([Number(flat[i]), String(flat[i + 1])]);
    }
    return out;
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

    /**
     * What a beat is worth on the view's axis. The roll draws in timeline
     * samples and a timeline is in beats, so the crossing happens here — the
     * editor's bridge is what supplies it.
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
     * What the last payload was a gesture *of*. Both lanes state the same
     * whole-list intent, so the payload alone cannot say which hand made it, and
     * an undo menu that called a dragged marker "edit the notes" would be naming
     * the wrong lane.
     */
    private verb = "edit the notes";

    payload(structure: Timeline, tag: string, values: readonly unknown[]): unknown {
        if (!this.editable) return null;
        if (tag === "notes") {
            this.verb = "edit the notes";
            return { intent: "setevents", events: this.notesNow(structure, values) };
        }
        if (tag === "osc") {
            const markers = this.markersNow(structure, values);
            if (markers === null) return null; // an unnamed marker — see `refusal`
            this.verb = "edit the markers";
            return { intent: "setevents", events: markers };
        }
        return null;
    }

    /**
     * Why a marker gesture this domain understands cannot be written.
     *
     * A marker *is* a message, and the address is the whole of what it sends;
     * the roll has no way to type one, so a marker added there has nothing to
     * become. Saying so is the point — a picture that springs back with nothing
     * attached teaches "sometimes it does not work" rather than "not here".
     */
    override refusal(structure: Timeline, tag: string, values: readonly unknown[]): string | null {
        if (tag === "osc" && this.editable && this.markersNow(structure, values) === null) {
            return (
                "a marker is the message it sends, and a roll cannot say which: " +
                "add it with timeline.add(beat, new OscItem(addr, ...)) and drag it here"
            );
        }
        return null;
    }

    /**
     * The whole timeline after a `notes` gesture: the drawn notes, with every
     * marker left exactly where it is.
     */
    private notesNow(structure: Timeline, values: readonly unknown[]): CrateEvent[] {
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
        return events.concat(this.kept(structure, (item) => item instanceof SeqEvent));
    }

    /**
     * The whole timeline after an `osc` gesture — the notes untouched and the
     * markers as the lane now holds them — or `null` when the gesture added one
     * that has no message to send.
     *
     * **A marker is matched by its label**, which is its address, and only then
     * by order among the ones that share it. The payload carries the label the
     * lane drew, so the message a marker sends survives being dragged and —
     * unlike the notes one lane up, where order is the only identity there is —
     * survives a *neighbour* being removed as well.
     */
    private markersNow(structure: Timeline, values: readonly unknown[]): CrateEvent[] | null {
        const held = [...structure].filter(([, item]) => labelOf(item) !== null);
        const taken = new Set<number>();
        const markers: CrateEvent[] = [];
        for (const [time, label] of pairs(values)) {
            const was = held.findIndex(
                ([, item], i) => !taken.has(i) && labelOf(item) === label,
            );
            if (was < 0) return null;
            taken.add(was);
            markers.push({
                at: time / this.unitsPerBeat,
                data: plain(itemData(held[was][1]) ?? {}),
            });
        }
        return this.kept(structure, (item) => labelOf(item) !== null).concat(markers);
    }

    /**
     * The items the gesture did **not** draw, as the crate holds them — what
     * keeps the lane nobody touched out of the edit that rebuilt the other one,
     * and out of the inverse that puts it back.
     */
    private kept(structure: Timeline, drawn: (item: unknown) => boolean): CrateEvent[] {
        const out: CrateEvent[] = [];
        for (const [beat, item] of structure) {
            const data = itemData(item);
            if (!drawn(item) && data !== null) out.push({ at: Number(beat), data: plain(data) });
        }
        return out;
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
        structure.clear();
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
            structure.add(Number(event.at ?? 0), item);
        }
        for (const [beat, item] of others) structure.add(beat, item);
        return true;
    }

    override label(): string {
        return this.verb;
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
        const body: { min?: number; max?: number; notesEditable?: boolean } = {};
        if (notes.length > 0) {
            const pitches = notes.map((n) => n[2]);
            body.min = Math.min(Math.min(...pitches) - NotesView.PAD, NotesView.DEFAULT_PITCH[1]);
            body.max = Math.max(Math.max(...pitches) + NotesView.PAD, NotesView.DEFAULT_PITCH[0]);
        }
        // **Say it before the hand tries.** A roll over what a generator
        // produced has nothing to write onto, so the widget refuses the press
        // instead of offering a drag it will unwind.
        if (editor.domain instanceof NotesDomain && !editor.domain.editable) {
            body.notesEditable = false;
        }
        const osc = markers(editor);
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            pianoroll({
                id: wid,
                notes: notes.length > 0 ? notes : undefined,
                osc: osc.length > 0 ? osc : undefined,
                ruler: "beats",
                tempo: editor.tempo,
                sampleRate: editor.sampleRate,
                ...body,
            }),
            ...editor.extra,
        );
    }

    override props(editor: Editor<Timeline>): Record<string, PropValue> {
        // **Both lanes**: a correction is what the widget should be drawing, and
        // a refused marker is answered by the markers as they still are.
        return {
            notes: flatNotes(drawn(editor)) as PropValue,
            osc: flatOsc(markers(editor)) as PropValue,
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
        if (item instanceof OscItem) {
            out.push([editor.beatsToUnits(Number(beat)), String(item.addr)]);
        } else if (item instanceof MidiItem) {
            out.push([editor.beatsToUnits(Number(beat)), "midi"]);
        }
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
