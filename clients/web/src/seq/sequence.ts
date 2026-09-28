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

import { JsEventSequence } from "../core/clausters_core_web.js";
import { requireCore } from "../base/core.ts";
import { TempoMap } from "../base/time.ts";
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
    private readonly seq: JsEventSequence;

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
     * `set`, `keys`, `setevents`, `tempo`, `restore`) and answers
     * `{applied, current}` -- `current` the edit that puts it back, read before
     * this one landed -- with `id` for an add. Throws when refused.
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

    toString(): string {
        return `EventSequence(${this.length} events)`;
    }
}

/** An event's keys as the document stores them. */
function keysOf(event: Event | EventProps): Record<string, unknown> {
    return (event instanceof Event ? event : new Event(event)).keysData();
}
