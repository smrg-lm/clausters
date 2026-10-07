// Event sequences: events as concrete data, held by the document (mirrors
// `clausters/seq/sequence.py`).
//
// An `EventSequence` is what a notes editor edits and what a timeline renders
// into. Where a `Timeline` holds **playables** -- events, patterns, routines,
// other timelines, each code that runs when it plays -- a sequence holds
// **events**, each with an identity of its own, in beats, with the tempo map
// that times them. The step from one to the other is a render, and it goes one
// way: a timeline's generators, its nesting and its tempo curve become the
// events they produced, and nothing rebuilds the timeline from them.
//
// The sequence itself lives on the Rust side (the document's `EventSequence`);
// this object is a handle to it, so every client edits the same structure and
// a notes editor opened on it edits it in place, with no copy to write back.

import { JsEventSequence, event_of_midi as coreEventOfMidi } from "../core/clausters_core_web.js";
import { midiReadClip, midiReadSmf, midiWriteClipUmp, midiWriteSmfTempo, requireCore } from "../base/core.ts";
import { Moment } from "../base/moment.ts";
import type { TempoClock } from "../base/clock.ts";
import { TempoMap } from "../base/time.ts";
import type { TimedMessage } from "../base/osc.ts";
import { Event, eventOfKeys } from "./event.ts";
import type { EventProps } from "./event.ts";
import { Automation } from "../multitrack.ts";
import { UndoHistory, contexts, keyOf } from "../history.ts";
import { NotesPlayback, playSequence } from "./playback.ts";
import { main } from "../base/main.ts";
import type { Server } from "../defs/server/index.ts";
import type { Transport } from "../defs/server/transport.ts";
import type { PointLike } from "../multitrack.ts";

/** One event of a sequence as the document writes it. */
interface Written {
    id: number;
    at: number;
    data?: Record<string, unknown>;
    automation?: Record<string, unknown>[];
}

/**
 * A sequence of events in beats, each with an identity of its own.
 *
 * Built from `[beat, event]` pairs, like a `Timeline`; an event is an `Event` or
 * an object of its keys. Iterating yields `[beat, Event]` pairs in beat order,
 * as copies. **What a page reads and writes is objects**:
 * {@link EventSequence.events} is the events as {@link SeqEvent}s, and
 * {@link EventSequence.automation} the curves over the whole sequence, each a
 * view of what the sequence holds -- so a change is made through the object it
 * changes (`event.at = 2.0`, `event.set("midinote", 62)`, `event.remove()`),
 * and no call takes or answers an id.
 */
export class EventSequence {
    /**
     * The sequence itself, in the shared crate -- what a notes editor opened
     * over this handle edits in place.
     *
     * @internal
     */
    readonly seq: JsEventSequence;

    /**
     * @param events `[beat, event]` pairs.
     * @param options `tempoMap`: the `TempoMap` that times the beats.
     */
    constructor(
        events: Iterable<[number, Event | EventProps]> = [],
        { tempoMap }: { tempoMap?: TempoMap | null } = {},
    ) {
        requireCore("EventSequence");
        const data: Record<string, unknown> = {
            events: [...events].map(([beat, event]) => ({ at: Number(beat), data: keysOf(event) })),
        };
        if (tempoMap) data.tempo_map = JSON.parse(tempoMap.dump());
        this.seq = new JsEventSequence(JSON.stringify(data));
    }

    /** The sequence `data` wrote (or a bare list of `{at, data}`). */
    static fromData(data: unknown): EventSequence {
        requireCore("EventSequence.fromData");
        const read = new JsEventSequence(JSON.stringify(data));
        // Built empty and then given the sequence read, so the fields a
        // constructor sets (the identity map) are set here too.
        const sequence = new EventSequence();
        (sequence as unknown as { seq: JsEventSequence }).seq = read;
        return sequence;
    }

    private call(verb: string, args: Record<string, unknown> = {}): any {
        const answer = JSON.parse(this.seq.call(JSON.stringify({ verb, ...args })));
        if (answer !== null && typeof answer === "object" && "error" in answer) {
            throw new Error(answer.error);
        }
        return answer;
    }

    /**
     * The sequence as plain data: its events with their ids, its tempo map and
     * its automation -- what a session stores and {@link EventSequence.fromData}
     * reads.
     */
    data(): Record<string, unknown> {
        return this.call("state");
    }

    // ---- its structures, as objects ----

    /** The identity map: `kind:id` to the one object that represents it. */
    #objects = new Map<string, WeakRef<SeqEvent | Automation>>();

    #object<T extends SeqEvent | Automation>(key: string, make: () => T): T {
        const found = this.#objects.get(key)?.deref();
        if (found !== undefined) return found as T;
        const made = make();
        this.#objects.set(key, new WeakRef(made));
        return made;
    }

    /** The object of the event with this id -- the same one every time. @internal */
    eventOf(id: number): SeqEvent {
        return this.#object(`event:${id}`, () => SeqEvent.of(this, id));
    }

    /**
     * The object of curve `id` -- of the sequence for `event` `null`, else of
     * that event -- the same one every time.
     *
     * @internal
     */
    curveOf(event: number | null, id: number): Automation {
        return this.#object(`curve:${id}`, () => Automation.heldBy(this, event, id));
    }

    /**
     * The curves as written -- the sequence's for `null`, else that event's,
     * and `null` when it holds no such event.
     *
     * @internal
     */
    curvesOf(event: number | null): Record<string, unknown>[] | null {
        const written = this.call("automation", event === null ? {} : { id: event }) as
            { automation?: Record<string, unknown>[] } | null;
        return written === null ? null : [...(written.automation ?? [])];
    }

    /** The events' ids in beat order, of a window when one is given. @internal */
    idsOf(window: { at?: number; from?: number; to?: number } = {}): number[] {
        return (this.call("ids", window).ids as number[]).map(Number);
    }

    /** One event as written, or `null`. @internal */
    writtenOf(id: number): Written | null {
        return this.call("event", { id }) as Written | null;
    }

    /**
     * **The events, as objects**, in beat order: a live collection -- iterate
     * it, index it, ask it what is {@link SeqEvents.at} a beat or in a
     * {@link SeqEvents.range} -- whose members are {@link SeqEvent}s, each a
     * view of one event the sequence holds. The same event read twice is the
     * same object.
     */
    get events(): SeqEvents {
        return new SeqEvents(this);
    }

    /**
     * **The curves over the whole sequence**: a live collection of
     * `Automation` views, their points on the sequence's beats. The notes
     * editor draws each as a row under the roll.
     */
    get automation(): SeqAutomation {
        return new SeqAutomation(this, null);
    }

    // ---- reading ----

    /** How many events it holds. */
    get length(): number {
        return Number(this.call("len").len);
    }

    /**
     * `[beat, Event]` pairs in beat order: copies of the keys, as a `Timeline`
     * iterates -- {@link EventSequence.events} is the events themselves.
     */
    *[Symbol.iterator](): IterableIterator<[number, Event]> {
        for (const written of (this.data().events ?? []) as Written[]) {
            yield [Number(written.at), eventOfKeys(written.data ?? {})];
        }
    }

    /** Where the last event stops sounding, in beats. */
    duration(): number {
        return Number(this.call("duration").duration);
    }

    /**
     * The `TempoMap` that times the beats, or `null`. Setting one is an edit.
     */
    get tempoMap(): TempoMap | null {
        const written = this.data().tempo_map;
        return written === undefined || written === null
            ? null
            : TempoMap.load(JSON.stringify(written)) ?? null;
    }

    set tempoMap(value: TempoMap | null) {
        const written = value === null ? null : JSON.parse(value.dump());
        this.edit({ intent: "tempo", tempo_map: written }, "change the tempo map");
    }

    /**
     * **Which MIDI specification the sequence is written for**: `"1.0"`,
     * `"mpe"` or `"2.0"` -- or `null`, a sequence for the server, where any
     * curve is legal. It decides which curves the sequence can hold: per
     * note, MIDI 1.0 says only pressure, MPE bend, pressure and timbre, 2.0
     * those and per-note controllers; over a channel a MIDI spec says a CC,
     * the bend, pressure and timbre, never a bare `control`. A file read is
     * MIDI 1.0. Change it with {@link EventSequence.setMidi}.
     */
    get midi(): string | null {
        const written = this.data().midi;
        if (written === undefined || written === null) return null;
        if (typeof written === "object") return Object.keys(written)[0] ?? null;
        return String(written);
    }

    /**
     * Writes the sequence for `spec` -- `"1.0"`, `"mpe"`, `"2.0"` or `null` --
     * an edit. An MPE zone is the lower one (master channel 1) unless
     * `upper`, with `members` member channels. Throws when a curve the
     * sequence holds has no spelling in that spec.
     */
    setMidi(spec: string | null, { upper = false, members = 15 }: { upper?: boolean; members?: number } = {}): void {
        const written = spec === "mpe" ? { mpe: { upper, members: Math.trunc(members) } } : spec;
        this.edit({ intent: "midi", midi: written }, "write it for another MIDI spec");
    }

    // ---- editing ----

    /**
     * Applies one edit in the sequence's vocabulary and answers `{applied,
     * current?, id?}` -- `current` the edit that puts it back, read before this
     * one landed, unless `inverse` is `false`. Throws when refused. The door the
     * objects write through.
     *
     * @internal
     */
    applyIntent(intent: Record<string, unknown>, inverse = true): { applied: boolean; current?: unknown; id?: number } {
        return this.call("apply", { intent, inverse });
    }

    /**
     * **One change a page makes through an object**: applied, and answered as
     * the door answers. `label` is what an undo would call it.
     *
     * A sequence with a history -- one an editor is open on, or one a page
     * asked for {@link EventSequence.history} -- takes the change as a turn of
     * it: recorded, and every view over the sequence told. One with none just
     * changes.
     *
     * @internal
     */
    edit(intent: Record<string, unknown>, label: string): { applied: boolean; id?: number } {
        const context = contexts.get(this);
        const answer = context === undefined
            ? this.applyIntent(intent, false)
            : context.scriptEdit(this, intent, label);
        // Heard where it plays: the lane holding it takes it again, once
        // however many views over it asked.
        if (answer.applied) NotesPlayback.changed(this, context === undefined ? null : context.version);
        return answer;
    }

    /**
     * **The sequence's history**: the undo order its editors share, made on
     * first ask. From then on every change made through the sequence's objects
     * is an entry of it, and a turn the windows over the sequence see;
     * `seq.history.entry("humanize", () => ...)` makes everything inside it one
     * entry.
     */
    get history(): UndoHistory {
        return new UndoHistory(this);
    }

    /**
     * Writes a curve whole -- the sequence's for `event` `null`, else that
     * event's -- and answers its id: the one it had, or a new one for a curve
     * with `id` 0.
     *
     * @internal
     */
    writeCurve(event: number | null, written: Record<string, unknown>, label: string): number {
        const intent = event === null
            ? { intent: "automation", automation: written }
            : { intent: "eventautomation", id: event, automation: written };
        return Number(this.edit(intent, label).id);
    }

    /**
     * Curve `id` as written, while the sequence -- or, with `event`, that
     * event -- holds it: the curve holder's read, as a multitrack's.
     *
     * @internal
     */
    writtenCurve(event: number | null, id: number): Record<string, unknown> | null {
        return (this.curvesOf(event) ?? []).find((c) => Number(c.id) === id) ?? null;
    }

    /** Removes curve `id` from the sequence, or from `event`. @internal */
    removeCurve(event: number | null, id: number): void {
        this.edit(
            event === null
                ? { intent: "removeautomation", curve: id }
                : { intent: "removeeventautomation", id: event, curve: id },
            "remove a curve",
        );
    }

    // ---- what a history asks of it ----

    /** The key and the domain it joins a history under -- a notes editor's. @internal */
    scriptKey(): [string, string] {
        return [keyOf("sequence", this), "events"];
    }

    /** The edit that puts the sequence back as it is now. @internal */
    restore(): Record<string, unknown> {
        return { intent: "restore", sequence: this.data() };
    }

    /**
     * The edit a redo applies: `intent` with the identity it was given, so a
     * redone add brings back the same event -- and the same object. An edit
     * that mints identities of its own is redone as the state it left.
     *
     * @internal
     */
    forwardOf(intent: Record<string, unknown>, minted: number | undefined): Record<string, unknown> {
        if (minted === undefined) return intent;
        if (intent.intent === "add") {
            return { ...intent, event: { ...(intent.event as Record<string, unknown>), id: Number(minted) } };
        }
        if (intent.intent === "automation" || intent.intent === "eventautomation") {
            return {
                ...intent,
                automation: { ...(intent.automation as Record<string, unknown>), id: Number(minted) },
            };
        }
        return this.restore();
    }

    /** Makes a free curve the view of curve `id`, in the identity map. @internal */
    adoptCurve(curve: Automation, event: number | null, id: number): void {
        curve.bind(this, event, id);
        this.#objects.set(`curve:${id}`, new WeakRef(curve));
    }

    // ---- playing it ----

    /**
     * **Plays the sequence on the server**, from beat `at`: its events become an
     * event lane's data on a transport of its own, which plays them by its
     * position, and the pass ends where the last note does. Answers that
     * `Transport` -- its `pause`, `locate`, `loop` and `stop` speak this
     * sequence's beats, and `wait()` resolves when the pass ends. A change made
     * through the sequence's objects while it plays is heard from where the
     * position is.
     *
     * **One transport per sequence**: two sequences played are two transports
     * and sound together, and playing this one again answers the transport it
     * already has. It is the sequence's until it is freed (`free()` on what
     * this answers), and a server has a fixed number of them -- with none left
     * this throws, saying to free one or boot the server with more
     * (`--transports`). With no `server` and none anywhere, the default
     * session boots one.
     */
    async play({ at = 0, server }: { at?: number; server?: Server | null } = {}): Promise<Transport> {
        return playSequence(this, at, await main.serverOrBoot(server));
    }

    // ---- plain data, and parts ----

    /**
     * **The sequence as plain data**: a list of rows, each the values of
     * `keys` in that order --
     *
     * ```ts
     * sequence.toRows(["midinote", "dur"]);   // [[60, 1], [62, 0.5], ...]
     * ```
     *
     * -- one row per event, in beat order. A key is read **as the event means
     * it**, not as it happens to spell it: a note written by its `degree`,
     * its `freq` or only its `pitches` has a `midinote`, one that states a
     * `velocity` has an `amp`, and `sustain` is read through `dur` and
     * `legato`. `"at"` is the event's beat and `"id"` its identity; any other
     * key is the value the event holds, and a key it does not hold is `null`.
     *
     * With `line: true` the rows are **one line played back to back**, the
     * way a pattern writes one: notes that start together are one row, a key
     * they differ in holding the list of their values (a chord); `dur` is the
     * time to the next row; and a silence -- where a row's own length ends
     * before the next one starts -- is a row of its own, every key `null` but
     * its `dur`. A sequence of several voices is one line a voice at a time
     * ({@link EventSequence.separate}). {@link EventSequence.fromRows} reads
     * either shape back.
     */
    toRows(keys: string[], { line = false }: { line?: boolean } = {}): unknown[][] {
        return this.call("rows", { keys, line }).rows;
    }

    /**
     * **A sequence from plain data**: rows of `keys`, the way back from
     * {@link EventSequence.toRows} --
     *
     * ```ts
     * EventSequence.fromRows([[60, 1], [null, 0.5], [[64, 67], 1.5]]);
     * ```
     *
     * Where `"at"` is among the keys each row is placed there; otherwise the
     * rows play back to back, each lasting its `dur` (a beat where there is
     * none). A row that states none of the pitch keys it was asked for is a
     * silence: it takes its time and is no event. A key holding a list is a
     * chord, a note to each value -- and `pitches`, a note to each written
     * pitch. `null` is a key the event does not state. Throws for a row that
     * is not as long as `keys`.
     */
    static fromRows(rows: unknown[][], keys: string[] = ["midinote", "dur"]): EventSequence {
        const sequence = new EventSequence();
        sequence.call("loadrows", { keys, rows });
        return sequence;
    }

    /**
     * **The sequence in parts**: a new sequence for each voice (`"voice"`,
     * the events' `staff` and `voice`), each staff (`"staff"`) or each
     * channel (`"channel"`) that has events, in that order. What a score
     * rendered into one sequence (`gui.notation.Score.renderEvents`) is taken
     * apart with, a line to a sequence -- to read each as rows, or to put
     * each on a track of its own.
     *
     * Each part holds the events of its line with the ids they had, the
     * curves of the channels those events are on, the tempo map, and what the
     * whole says of its page, cut to the part: by voice or by staff the part
     * is a score of its own, whose staff is the top one. Where the whole
     * names a MIDI specification each part names the narrowest that says what
     * it holds: MIDI 1.0 where no note carries a curve of its own, MPE where
     * one does and the part is one line (a channel to each note), MIDI 2.0
     * where it is several. This sequence is not changed. Throws for another
     * `by`.
     */
    separate(by: "voice" | "staff" | "channel" = "voice"): EventSequence[] {
        const parts: unknown[] = this.call("separate", { by }).parts;
        return parts.map((part) => EventSequence.fromData(part));
    }

    // ---- MIDI files ----

    /**
     * The sequence as the MIDI messages a file of it holds -- the render
     * {@link EventSequence.toSmf} writes -- as `[beat, bytes]` pairs, in
     * order, at `ppq` ticks per beat. What a MIDI destination plays.
     */
    midiMessages(ppq = 960): [number, Uint8Array][] {
        const written = this.call("midi", { ppq }) as { events: [number, number[]][] };
        return written.events.map(([tick, bytes]) => [tick / ppq, Uint8Array.from(bytes)]);
    }

    /**
     * The sequence as a Standard MIDI File, at `ppq` ticks per beat: every
     * event's MIDI messages -- a note as its on and off, a `"midi"` event as its
     * message -- its automation as its channels' messages and its notes' as
     * theirs, as its {@link EventSequence.midi} spec says them (MIDI 1.0
     * when it names none; a 2.0 sequence as MPE), and the tempo map as the
     * file's tempo. An `"osc"` event has no MIDI spelling and is left out, as
     * is a curve the spec cannot say; a ramp is sampled where the MIDI value
     * changes, and a tempo ramp is written as the step at its breakpoint,
     * since a file's tempo only steps.
     */
    toSmf(ppq = 480): Uint8Array {
        const written = this.call("midi", { ppq }) as {
            events: [number, number[]][];
            tempo: [number, number][];
        };
        const ticks = Uint32Array.from(written.events, ([tick]) => tick);
        const msgs = new Uint8Array(3 * written.events.length);
        written.events.forEach(([, bytes], i) => msgs.set(bytes.slice(0, 3), 3 * i));
        return midiWriteSmfTempo(
            ticks,
            msgs,
            ppq,
            Uint32Array.from(written.tempo, ([tick]) => tick),
            Uint32Array.from(written.tempo, ([, micros]) => micros),
        );
    }

    /**
     * The sequence a Standard MIDI File holds: its notes -- each note-on with the
     * note-off that closes it -- its streams as curves (a channel's CC, bend and
     * pressure as the sequence's automation; poly pressure, and an MPE zone's
     * member channels, as the notes'), its other messages as `"midi"` events, in beats,
     * with the file's tempo as the tempo map (its default 120 quarter notes a
     * minute when it states none). Its {@link EventSequence.midi} spec is MPE
     * when the file declares a zone, else MIDI 1.0.
     */
    static fromSmf(data: Uint8Array): EventSequence {
        const read = JSON.parse(midiReadSmf(data));
        if (read.error) throw new Error(read.error);
        const sequence = new EventSequence();
        sequence.call("loadmidi", { ppq: read.ppq, events: read.events, tempo: read.tempo });
        return sequence;
    }

    /**
     * The sequence as a MIDI 2.0 Clip File (SMF2CLIP), at `ppq` ticks per
     * beat: its notes at 16-bit velocity, its automation as 32-bit channel
     * messages, its notes' as per-note ones -- per-note pitch bend,
     * poly pressure, the registered per-note controller 74 for timbre and an
     * assignable one for a CC -- and its tempo map as Set Tempo messages. What
     * its {@link EventSequence.midi} spec cannot say of one note is left out.
     */
    toClip(ppq = 480): Uint8Array {
        const written = this.call("ump", { ppq }) as { events: [number, number[]][] };
        return midiWriteClipUmp(
            Uint32Array.from(written.events, ([tick]) => tick),
            Uint8Array.from(written.events, ([, words]) => words.length),
            Uint32Array.from(written.events.flatMap(([, words]) => words)),
            ppq,
        );
    }

    /**
     * The sequence a MIDI 2.0 Clip File holds: its notes, its channels'
     * messages as its automation and its per-note messages as the notes',
     * its Set Tempo messages as the tempo map (120 quarter notes a minute when
     * it has none), and `"2.0"` as its {@link EventSequence.midi} spec.
     */
    static fromClip(data: Uint8Array): EventSequence {
        const read = JSON.parse(midiReadClip(data));
        if (read.error) throw new Error(read.error);
        const sequence = new EventSequence();
        sequence.call("loadump", { ppq: read.ppq, events: read.events });
        return sequence;
    }

    toString(): string {
        return `EventSequence(${this.length} events)`;
    }
}

/**
 * **One event of a sequence**, as an object: a live view of the event the
 * sequence holds under its id, never a copy. Reading it asks the sequence, so
 * after a hand moves the note in the roll the object reads where it now is.
 *
 * It is made by the sequence -- {@link EventSequence.events} hands them out,
 * one object per event -- and is not built directly. Its keys read through
 * {@link SeqEvent.get}; {@link SeqEvent.event} answers a free `Event` with
 * the same keys, to play or to copy.
 *
 * An event the sequence no longer holds is **detached**:
 * {@link SeqEvent.sequence} is `null` and reading it throws. An undo that
 * brings the event back brings this same object back with it.
 */
export class SeqEvent {
    readonly #sequence: EventSequence;
    readonly #id: number;

    private constructor(sequence: EventSequence, id: number) {
        this.#sequence = sequence;
        this.#id = id;
    }

    /** @internal */
    static of(sequence: EventSequence, id: number): SeqEvent {
        return new SeqEvent(sequence, id);
    }

    #written(): Written {
        const written = this.#sequence.writtenOf(this.#id);
        if (written === null) throw new Error("the sequence no longer holds this event");
        return written;
    }

    /** The event's identity in its sequence, for the host that names it. @internal */
    get id(): number {
        return this.#id;
    }

    /** The sequence that holds the event, or `null` once it holds it no more. */
    get sequence(): EventSequence | null {
        return this.#sequence.writtenOf(this.#id) === null ? null : this.#sequence;
    }

    #held(): EventSequence {
        if (this.sequence === null) throw new Error("the sequence no longer holds this event");
        return this.#sequence;
    }

    /**
     * Where the event sits, in the sequence's beats. Setting it moves the
     * event, and it keeps its place among the events at the beat it goes to.
     */
    get at(): number {
        return Number(this.#written().at);
    }

    set at(beat: number) {
        this.#held().edit({ intent: "move", id: this.#id, at: Number(beat) }, "move an event");
    }

    /**
     * Writes one key, with its family's coherence: a moved `midinote` moves the
     * `freq`, the `degree` and the written `pitches` the event holds, and
     * written `pitches` move what it sounds.
     */
    set(key: string, value: unknown): void {
        this.#held().edit({ intent: "set", id: this.#id, key: String(key), value }, `set ${key}`);
    }

    /**
     * Removes the event from its sequence. This object is left detached, and
     * an undo that brings the event back brings it back too.
     */
    remove(): void {
        this.#held().edit({ intent: "remove", id: this.#id }, "remove an event");
    }

    /** One key of the event, or `undefined` when it has none. */
    get(key: string): unknown {
        return (this.#written().data ?? {})[key];
    }

    /** Whether the event has this key. */
    has(key: string): boolean {
        return key in (this.#written().data ?? {});
    }

    /** The names of the event's keys. */
    keys(): string[] {
        return Object.keys(this.#written().data ?? {});
    }

    /**
     * The event's keys as a free `Event` -- a copy, to play or to add
     * elsewhere; changing it changes nothing here.
     */
    get event(): Event {
        return eventOfKeys(this.#written().data ?? {});
    }

    /**
     * **The curves over this event alone** -- its own automation, as MPE gives
     * a note its bend, pressure and timbre: a live collection of `Automation`
     * views, each point's beat counted from the event's start and free to run
     * past the note's end into its release.
     */
    get automation(): SeqAutomation {
        return new SeqAutomation(this.#sequence, this.#id);
    }

    toString(): string {
        const written = this.#sequence.writtenOf(this.#id);
        if (written === null) return "SeqEvent(detached)";
        return `SeqEvent(at ${Number(written.at)} ${JSON.stringify(written.data ?? {})})`;
    }
}

/**
 * **A sequence's events, as a live collection** of {@link SeqEvent}s in beat
 * order ({@link EventSequence.events}). It reads the sequence each time it is
 * asked, so it is never out of date and never needs refreshing.
 */
export class SeqEvents {
    readonly #sequence: EventSequence;

    /** @internal */
    constructor(sequence: EventSequence) {
        this.#sequence = sequence;
    }

    #of(ids: number[]): SeqEvent[] {
        return ids.map((id) => this.#sequence.eventOf(id));
    }

    /** How many events the sequence holds. */
    get length(): number {
        return this.#sequence.length;
    }

    [Symbol.iterator](): IterableIterator<SeqEvent> {
        return this.#of(this.#sequence.idsOf())[Symbol.iterator]();
    }

    /** The event at index `i` in beat order (negative counts from the end). */
    item(i: number): SeqEvent {
        const ids = this.#sequence.idsOf();
        const id = ids.at(i);
        if (id === undefined) throw new RangeError(`the sequence holds no event at index ${i}`);
        return this.#sequence.eventOf(id);
    }

    /**
     * **Adds an event at** `beat` -- an `Event`, an object of its keys, or
     * another {@link SeqEvent}, whose keys are copied -- and answers the
     * {@link SeqEvent} that is it. It goes after every event already at that
     * beat.
     */
    add(beat: number, event: Event | EventProps | SeqEvent): SeqEvent {
        const keys = event instanceof SeqEvent ? event.event : event;
        const answer = this.#sequence.edit(
            { intent: "add", event: { at: Number(beat), data: keysOf(keys) } },
            "add an event",
        );
        return this.#sequence.eventOf(Number(answer.id));
    }

    /** The events exactly at `beat`, in the order they were placed. */
    at(beat: number): SeqEvent[] {
        return this.#of(this.#sequence.idsOf({ at: Number(beat) }));
    }

    /** The events in the half-open beat window `[t0, t1)`. */
    range(t0: number, t1: number): SeqEvent[] {
        return this.#of(this.#sequence.idsOf({ from: Number(t0), to: Number(t1) }));
    }
}

/**
 * **Curves a sequence holds, as a live collection** of `Automation` views: the
 * sequence's own ({@link EventSequence.automation}) or one event's
 * ({@link SeqEvent.automation}).
 */
export class SeqAutomation {
    readonly #sequence: EventSequence;
    readonly #event: number | null;

    /** @internal */
    constructor(sequence: EventSequence, event: number | null) {
        this.#sequence = sequence;
        this.#event = event;
    }

    #written(): Record<string, unknown>[] {
        const written = this.#sequence.curvesOf(this.#event);
        if (written === null) throw new Error("the sequence no longer holds this event");
        return written;
    }

    #views(): Automation[] {
        return this.#written().map((c) => this.#sequence.curveOf(this.#event, Number(c.id)));
    }

    /** How many curves it holds. */
    get length(): number {
        return this.#written().length;
    }

    [Symbol.iterator](): IterableIterator<Automation> {
        return this.#views()[Symbol.iterator]();
    }

    /** The curve at index `i` (negative counts from the end). */
    item(i: number): Automation {
        const curve = this.#views().at(i);
        if (curve === undefined) throw new RangeError(`no curve at index ${i}`);
        return curve;
    }

    /**
     * **Adds a curve** and answers it, held: the `Automation` that is now a
     * view of what the sequence holds.
     *
     * `target` says what it moves -- `{cc: 74}` (0 to 127), `{bend: true}`
     * (semitones), `{pressure: true}`, `{timbre: true}` (0 to 1) or
     * `{control: "cutoff"}`, with `min`/`max` to override the range and, on the
     * sequence's curves, `channel` for the one channel it acts on (counted
     * from 0, as a note's; without it, every channel). `points` are `[beat,
     * value]` pairs or the document's points; `name` labels it. Or `target` is
     * a free `Automation`, which is added as it is and becomes the view.
     *
     * Throws when the sequence's {@link EventSequence.midi} spec cannot say
     * such a curve here.
     */
    add(
        target: Record<string, unknown> | Automation,
        { points = [], name }: { points?: Iterable<PointLike>; name?: string } = {},
    ): Automation {
        let curve: Automation | null = null;
        let written: Record<string, unknown>;
        if (target instanceof Automation) {
            curve = target;
            if (curve.holder !== null) {
                throw new Error("this curve is held already: add a copy of it (Automation.read(curve.write()))");
            }
            written = { ...curve.write(), id: 0 };
        } else {
            written = { id: 0, target: { ...target }, points: new Automation({ points }).points };
            if (name !== undefined) written.name = String(name);
        }
        if (this.#event !== null) this.#written();
        const id = this.#sequence.writeCurve(this.#event, written, "add a curve");
        if (curve === null) return this.#sequence.curveOf(this.#event, id);
        this.#sequence.adoptCurve(curve, this.#event, id);
        return curve;
    }

    /**
     * **Gives one of the sequence's curves to the notes it reaches**: each note
     * on its channel (every note, for a curve that names none) takes the
     * stretch of the curve its span covers as a curve of its own -- sounding as
     * it did, since a channel reaches a note from its on to its off -- and the
     * sequence's goes, leaving `curve` detached. A note with its own curve over
     * that control keeps it; over a bend, which adds, that throws, as does a
     * curve the sequence's {@link EventSequence.midi} spec cannot say of one
     * note.
     */
    toEvents(curve: Automation): void {
        if (this.#event !== null) throw new Error("an event's curves are already its notes'");
        const holder = curve.holder;
        if (holder === null || holder[0] !== this.#sequence || holder[1] !== null) {
            throw new Error("not one of this sequence's curves");
        }
        this.#sequence.edit({ intent: "automationtoevents", curve: curve.id }, "give a curve to the notes");
    }

    /**
     * **Gathers the notes' curves over `target` into one of the sequence's** --
     * of the notes on `channel`, or of every note -- and answers it: each
     * note's curve over its span, on the channel its notes share. The notes'
     * curves go. It holds where the notes agree: two that sound at once with
     * different curves throw, since one channel cannot say both -- a chord
     * whose curve was given to its notes gives it back.
     */
    fromEvents(target: Record<string, unknown>, { channel }: { channel?: number } = {}): Automation {
        if (this.#event !== null) throw new Error("an event's curves gather from nothing");
        const intent: Record<string, unknown> = { intent: "eventstoautomation", target: { ...target } };
        if (channel !== undefined) intent.channel = Math.trunc(channel);
        const answer = this.#sequence.edit(intent, "gather the notes' curves");
        return this.#sequence.curveOf(null, Number(answer.id));
    }
}

/**
 * A destination that keeps what plays instead of sounding it: each event at
 * the beat it plays on, in the beats of the structure being rendered.
 *
 * It stands where a `Server` would -- an event plays on it through
 * `playEvent`, a raw OSC message through `sendBundle` or `sendMsg`, raw MIDI
 * through `sendMessage` -- and records each one as the event it is, with no
 * node, no latency and no server behind it.
 */
class Recorder {
    /** `[beat, keys, the event's own curves as written]`. */
    readonly events: [number, Record<string, unknown>, Record<string, unknown>[]][] = [];
    /** The channels' curves that played, as the sequence's own. */
    readonly curves: Record<string, unknown>[] = [];

    /**
     * The beat it is, in the rendered structure's beats, and the function that
     * carries a beat of the clock it was stamped on there: a child timeline
     * plays in its own beats, and a sequence is in its root's.
     */
    private now(delay = 0): [number, number, (beat: number) => number] {
        const moment = Moment.current();
        const clock = moment.clock as unknown as { rootBeat?: (beat: number) => number } | null;
        const toRoot = typeof clock?.rootBeat === "function"
            ? (beat: number) => clock.rootBeat!(beat)
            : (beat: number) => beat;
        const local = moment.beat + delay;
        return [toRoot(local), local, toRoot];
    }

    playEvent(event: Event): number | null {
        const [at, local, toRoot] = this.now();
        const keys = event.keysData();
        delete keys.node;
        delete keys.server;
        if ((keys.type ?? "note") === "note") {
            // How long it sounds, in the root's beats as well.
            keys.sustain = toRoot(local + event.sustain()) - at;
        }
        // **Its curves travel with it**: each as the note's own curve in the
        // sequence, its beats counted from the note's start in the root's.
        const given = event.get("automation") as Played | Played[] | undefined;
        const curves = given === undefined || given === null
            ? []
            : Array.isArray(given) ? given : [given];
        this.events.push([at, keys, curves.map((c) => moved(c, local, toRoot, at))]);
        return null;
    }

    /** A channel's curve, played: one of the sequence's curves, its beats the root's. */
    playAutomation(curve: Played): null {
        const [, local, toRoot] = this.now();
        this.curves.push(moved(curve, local, toRoot, 0));
        return null;
    }

    sendBundle(messages: readonly TimedMessage[], { delayBeats = 0 }: { delayBeats?: number } = {}): void {
        const [at] = this.now(delayBeats);
        for (const [addr, ...args] of messages) {
            this.events.push([at, { type: "osc", addr: String(addr), args }, []]);
        }
    }

    sendMsg(addr: string, ...args: unknown[]): void {
        this.sendBundle([[addr, ...args] as TimedMessage]);
    }

    sendMessage(message: ArrayLike<number>): void {
        this.events.push([this.now()[0], JSON.parse(coreEventOfMidi(Uint8Array.from(message))), []]);
    }
}

/** A curve that was played: what writes itself. */
interface Played {
    write(): Record<string, unknown>;
}

/**
 * A played curve as a sequence writes it: its points, counted from the beat
 * `local` it was played at on its own clock, carried to the root's beats and
 * counted from `zero` there.
 */
function moved(
    curve: Played,
    local: number,
    toRoot: (beat: number) => number,
    zero: number,
): Record<string, unknown> {
    const written = { ...curve.write(), id: 0 };
    const points = (written as { points?: { at: number }[] }).points ?? [];
    return {
        ...written,
        points: points.map((point) => ({ ...point, at: toRoot(local + Number(point.at)) - zero })),
    };
}

/**
 * Plays `start(recorder, clock)` on an offline session's clock and answers what
 * played as a sequence. `until` bounds it, in the clock's beats; with none it
 * runs until nothing is due, and an endless source is refused rather than run
 * forever.
 */
async function rendered(
    start: (recorder: Recorder, clock: TempoClock) => (() => unknown) | null,
    until: number | undefined,
    tempoMap: TempoMap | null,
): Promise<EventSequence> {
    const { Session } = await import("../session.ts");
    const { MAX_BOUNCED_EVENTS } = await import("../render.ts");
    const session = await Session.nrt();
    const recorder = new Recorder();
    session.use(() => {
        const stop = start(recorder, session.clock);
        try {
            session.clock.render(until, {
                maxSteps: until === undefined ? MAX_BOUNCED_EVENTS : undefined,
            });
        } catch (error) {
            if (until !== undefined) throw error;
            throw new Error(
                `renderEvents: it did not end after ${MAX_BOUNCED_EVENTS} events -- `
                    + "pass until to bound it",
            );
        } finally {
            stop?.();
        }
    });
    const data: Record<string, unknown> = {
        events: recorder.events.map(([at, keys, curves]) =>
            curves.length > 0 ? { at, data: keys, automation: curves } : { at, data: keys }),
    };
    if (recorder.curves.length > 0) data.automation = recorder.curves;
    if (tempoMap) data.tempo_map = JSON.parse(tempoMap.dump());
    return EventSequence.fromData(data);
}

/** What `renderTimeline` needs of a `Timeline`. @internal */
interface Rendered {
    readonly transport: unknown;
    readonly map: TempoMap;
    play(options: { at?: number; destination?: never }): unknown;
    stop(): unknown;
}

/** `Timeline.renderEvents`. @internal */
export function renderTimeline(timeline: Rendered, until?: number): Promise<EventSequence> {
    if (timeline.transport !== null) {
        throw new Error(
            "renderEvents plays a timeline on its own clock: take it off the transport "
                + "(timeline.transport = null) first",
        );
    }
    // The session's clock runs at one beat a second, so the timeline's beats
    // reach it as the seconds its own map makes of them.
    const bound = until === undefined ? undefined : timeline.map.secsAt(until);
    return rendered(
        (recorder) => {
            timeline.play({ at: 0, destination: recorder as never });
            return () => timeline.stop();
        },
        bound,
        timeline.map,
    );
}

/** `EventPattern.renderEvents`. @internal */
export function renderPattern(
    pattern: { play(destination: never, options: { clock: TempoClock }): { stop?: () => unknown } },
    until?: number,
): Promise<EventSequence> {
    return rendered(
        (recorder, clock) => {
            const player = pattern.play(recorder as never, { clock });
            return player.stop ? () => player.stop!() : null;
        },
        until,
        null,
    );
}

/** An event's keys as the document stores them. */
function keysOf(event: Event | EventProps): Record<string, unknown> {
    return (event instanceof Event ? event : new Event(event)).keysData();
}
