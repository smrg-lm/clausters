// The event-pattern player (mirrors `clausters/seq/eventstream.py`).
//
// Plays an **event pattern** (a `Pbind`) on a clock: it is a routine that, for
// each event, plays it against the destination (emitting at the routine's
// exact logical beat) and yields the event's `delta` to advance.

import { Routine } from "../base/stream.ts";
import type { TempoClock } from "../base/clock.ts";
import { Event } from "./event.ts";
import type { EventDestination } from "./event.ts";
import type { Pattern } from "./pattern.ts";

export class EventStreamPlayer {
    readonly pattern: Pattern<unknown>;
    readonly destination: EventDestination;
    routine: Routine | null = null;

    constructor(pattern: Pattern<unknown>, destination: EventDestination) {
        this.pattern = pattern;
        this.destination = destination;
    }

    /** Schedules the pattern on `clock`, snapping the start to `quant`. */
    play(clock?: TempoClock, quant?: number): this {
        const events = this.pattern[Symbol.iterator]();
        const destination = this.destination;
        this.routine = new Routine(function* () {
            for (;;) {
                const step = events.next();
                if (step.done) return;
                const event =
                    step.value instanceof Event
                        ? step.value
                        : new Event(step.value as Record<string, unknown>);
                event.play(destination); // emits at the current logical beat
                yield event.delta(); // advance by dur * stretch
            }
        });
        this.routine.play(clock, quant);
        return this;
    }

    /**
     * Ends the player. Items already played keep sounding (their releases are
     * scheduled); nothing further is played.
     */
    stop(): this {
        if (this.routine !== null) {
            this.routine.clock?.unsched(this.routine);
            this.routine.state = "done";
        }
        return this;
    }
}
