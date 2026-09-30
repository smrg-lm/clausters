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
import { midiReadSmf, midiWriteSmfTempo, requireCore } from "../base/core.ts";
import { Moment } from "../base/moment.ts";
import type { TempoClock } from "../base/clock.ts";
import { TempoMap } from "../base/time.ts";
import type { TimedMessage } from "../base/osc.ts";
import { Event, eventOfKeys } from "./event.ts";
import type { EventProps } from "./event.ts";

/** One event of a sequence as the document writes it. */
interface Written {
    id: number;
    at: number;
    data?: Record<string, unknown>;
}

/**
 * A sequence of events in beats, each with an id.
 *
 * Built from `[beat, event]` pairs, like a `Timeline`; an event is an `Event` or
 * an object of its keys. Iterating yields `[beat, Event]` pairs in beat order;
 * {@link EventSequence.entries} adds each one's id, which is what the edits name
 * an event by.
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
        const sequence = Object.create(EventSequence.prototype) as EventSequence;
        requireCore("EventSequence.fromData");
        (sequence as unknown as { seq: JsEventSequence }).seq =
            new JsEventSequence(JSON.stringify(data));
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
     * its lanes -- what a session stores and {@link EventSequence.fromData}
     * reads.
     */
    data(): Record<string, unknown> {
        return this.call("state");
    }

    // ---- reading ----

    /** How many events it holds. */
    get length(): number {
        return Number(this.call("len").len);
    }

    *[Symbol.iterator](): IterableIterator<[number, Event]> {
        for (const [, beat, event] of this.entries()) yield [beat, event];
    }

    /** Every event as `[id, beat, Event]`, in beat order. */
    entries(): [number, number, Event][] {
        const events = (this.data().events ?? []) as Written[];
        return events.map((e) => [e.id, Number(e.at), eventOfKeys(e.data ?? {})]);
    }

    /** The event with this id as `[beat, Event]`; throws when there is none. */
    get(id: number): [number, Event] {
        const event = this.call("event", { id }) as Written | null;
        if (event === null) throw new RangeError(`the sequence holds no event ${id}`);
        return [Number(event.at), eventOfKeys(event.data ?? {})];
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
        this.apply({ intent: "tempo", tempo_map: written });
    }

    // ---- editing ----

    /**
     * Applies one edit in the sequence's vocabulary (`add`, `remove`, `move`,
     * `set`, `keys`, `setevents`, `tempo`, `lane`, `removelane`, `expression`,
     * `removeexpression`, `restore`) and answers `{applied, current}` --
     * `current` the edit that puts it back, read before this one landed -- with
     * `id` for an add, a lane or an expression. Throws when refused.
     */
    apply(intent: Record<string, unknown>): { applied: boolean; current: unknown; id?: number } {
        return this.call("apply", { intent });
    }

    /** Adds an event at `beat`; its new id. */
    add(beat: number, event: Event | EventProps): number {
        const answer = this.apply({ intent: "add", event: { at: Number(beat), data: keysOf(event) } });
        return Number(answer.id);
    }

    /** Removes the event with this id. */
    remove(id: number): void {
        this.apply({ intent: "remove", id });
    }

    /** Moves the event with this id to `beat`. */
    move(id: number, beat: number): void {
        this.apply({ intent: "move", id, at: Number(beat) });
    }

    /**
     * Writes one key of an event, with its family's coherence: a moved
     * `midinote` moves the `freq` and the `degree` the event holds.
     */
    set(id: number, key: string, value: unknown): void {
        this.apply({ intent: "set", id, key, value });
    }

    // ---- curves ----

    /**
     * Adds a curve over the whole sequence -- a lane -- and answers its id.
     * `target` says what it moves: `{cc: 74}` (0 to 127), `{bend: true}`
     * (semitones), `{pressure: true}`, `{timbre: true}` (0 to 1) or
     * `{control: "cutoff"}`, with `min`/`max` to override the range and
     * `channel` for the one channel it acts on (counted from 0, as a note's;
     * without it, every channel). `points` are `[beat, value]` pairs; `name`
     * labels it. The notes editor draws it as a row under the roll.
     */
    addLane(
        target: Record<string, unknown>,
        { points = [], name }: { points?: readonly (readonly [number, number])[]; name?: string } = {},
    ): number {
        return this.curve({ intent: "lane" }, target, points, name);
    }

    /**
     * Adds a curve over the event with this id -- its own expression, as MPE
     * gives a note its bend, pressure and timbre -- and answers its id. `target`
     * as for {@link EventSequence.addLane}; `points` are `[beat, value]` pairs,
     * each beat counted from the event's start, and free to run past the
     * note's end into its release. The notes editor draws it inside the note,
     * and a bend in the plane over the pitches it spans.
     */
    addExpression(
        id: number,
        target: Record<string, unknown>,
        { points = [], name }: { points?: readonly (readonly [number, number])[]; name?: string } = {},
    ): number {
        return this.curve({ intent: "expression", id }, target, points, name);
    }

    /** Removes the lane with this id. */
    removeLane(lane: number): void {
        this.apply({ intent: "removelane", lane });
    }

    /** Removes curve `lane` from the event with this id. */
    removeExpression(id: number, lane: number): void {
        this.apply({ intent: "removeexpression", id, lane });
    }

    private curve(
        intent: Record<string, unknown>,
        target: Record<string, unknown>,
        points: readonly (readonly [number, number])[],
        name: string | undefined,
    ): number {
        const automation: Record<string, unknown> = {
            id: 0,
            target: { ...target },
            points: points.map(([at, value]) => ({ at: Number(at), value: Number(value) })),
        };
        if (name !== undefined) automation.name = String(name);
        return Number(this.apply({ ...intent, automation }).id);
    }

    // ---- MIDI files ----

    /**
     * The sequence as a Standard MIDI File, at `ppq` ticks per beat: every
     * event's MIDI messages -- a note as its on and off, a `"midi"` event as its
     * message -- and the tempo map as the file's tempo. An `"osc"` event has no
     * MIDI spelling and is left out; a tempo ramp is written as the step at its
     * breakpoint, since a file's tempo only steps.
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
     * note-off that closes it -- and its other messages as `"midi"` events, in
     * beats, with the file's tempo as the tempo map (its default 120 quarter
     * notes a minute when it states none).
     */
    static fromSmf(data: Uint8Array): EventSequence {
        const read = JSON.parse(midiReadSmf(data));
        if (read.error) throw new Error(read.error);
        const sequence = new EventSequence();
        sequence.call("loadmidi", { ppq: read.ppq, events: read.events, tempo: read.tempo });
        return sequence;
    }

    toString(): string {
        return `EventSequence(${this.length} events)`;
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
    readonly events: [number, Record<string, unknown>][] = [];

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
        this.events.push([at, keys]);
        return null;
    }

    sendBundle(messages: readonly TimedMessage[], { delayBeats = 0 }: { delayBeats?: number } = {}): void {
        const [at] = this.now(delayBeats);
        for (const [addr, ...args] of messages) {
            this.events.push([at, { type: "osc", addr: String(addr), args }]);
        }
    }

    sendMsg(addr: string, ...args: unknown[]): void {
        this.sendBundle([[addr, ...args] as TimedMessage]);
    }

    sendMessage(message: ArrayLike<number>): void {
        this.events.push([this.now()[0], JSON.parse(coreEventOfMidi(Uint8Array.from(message)))]);
    }
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
        events: recorder.events.map(([at, keys]) => ({ at, data: keys })),
    };
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
