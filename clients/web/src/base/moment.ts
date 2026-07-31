// When something is happening: a clock and an exact beat on it.
//
// The one place that answers "what time is it *for this event*". A running
// routine carries the beat the clock stamped on it (yield-accumulated, not
// wall-clock now), so everything emitted from one wake shares a single instant
// and inter-event timing stays exact. Outside any routine there is no clock,
// and physical time stands in for logical time rather than opening a second
// code path.
//
// `Moment` only reads a clock. What a destination *does* with the instant is
// the destination's (`base/destination.ts`).

import type { TempoClock } from "./clock.ts";
import { currentRoutine } from "./context.ts";

/**
 * A clock and an exact beat on it.
 *
 * `clock` is `null` outside any routine; beats then read as seconds
 * (tempo 1.0), which is what lets a bare `new Event().play(server)` use the
 * same machinery as one inside a routine.
 */
export class Moment {
    readonly clock: TempoClock | null;
    readonly beat: number;

    constructor(clock: TempoClock | null, beat: number) {
        this.clock = clock;
        this.beat = beat;
    }

    /**
     * The ambient moment.
     *
     * Inside a routine, the exact beat the clock stamped on it. That beat
     * belongs to *its* clock, so an explicit `clock` that is not the one the
     * routine plays on is asked for its own `beats()` instead. With no clock
     * in either place, the clockless moment.
     */
    static current(clock?: TempoClock | null): Moment {
        const routine = currentRoutine();
        const on = clock ?? routine?.clock ?? null;
        if (!on) return new Moment(null, 0);
        if (routine?.clock === on) return new Moment(on, routine.logicalBeat);
        return new Moment(on, on.beats());
    }

    /** This moment moved `deltaBeats` later on the same clock. */
    at(deltaBeats = 0): Moment {
        return new Moment(this.clock, this.beat + deltaBeats);
    }

    /**
     * Seconds on the clock's own axis (measured from its beat zero), or the
     * beat itself when there is no clock (tempo 1.0).
     */
    secs(): number {
        return this.clock ? this.clock.beats2secs(this.beat) : this.beat;
    }

    /**
     * Unix seconds — what an OSC timetag is made of. With no clock, or before
     * the clock's first `start` placed its wall-clock origin, this is now plus
     * whatever the moment carries.
     */
    instant(): number {
        const start = this.clock?.startTime ?? null;
        return start === null ? Date.now() / 1000 + this.secs() : start + this.secs();
    }
}
