// Streams and routines (mirrors `clausters/base/stream.py`).
//
// The coroutine layer. A `Routine` wraps a **generator function**; driving it
// resumes the generator, and the value it yields is a *time to wait* (in
// beats) before the next resumption. What resumes routines on a schedule is
// the clock (`base/clock.ts`); here we only define the protocol. This is the
// part that stays in the host language — `yield` is JavaScript control flow
// and never moves to Rust.
//
// **A routine must never block, and must never `await`.** It runs on the
// page's one thread: blocking it stalls every other routine, the timeline and
// the rendering. Cede time by yielding instead. In particular, to send a def
// from inside a routine use the non-waiting form (`def.send(server, // { wait: false })`) and yield enough time before the `/s_new` that depends on
// it — never `await server.sync()`. That is also why the driver takes
// `function*` and not `async function*`: the ambient "what is running right
// now" (`base/context.ts`) is only sound while a wake runs to completion.

import { currentRoutine } from "./context.ts";
import { spawnRng } from "./rand.ts";
import type { Rng } from "./rand.ts";
import type { TempoClock } from "./clock.ts";

/** Thrown (and caught by the clock) to end a stream normally. */
export class StopStream extends Error {
    constructor(message = "the stream ended") {
        super(message);
        this.name = "StopStream";
    }
}

/**
 * A lazy sequence: `next()` produces values until it throws `StopStream`.
 *
 * Concrete streams carry their own random generator (`rng`), derived from the
 * creating context at construction (see `base/rand.ts`): random values drawn
 * while a stream runs come from *its* stream, so one root seed reproduces a
 * whole script and concurrent routines stay reproducible per routine.
 */
export abstract class Stream {
    /** The stream's own random generator. */
    rng: Rng | null = null;
    /**
     * The clock currently driving this stream, set by the clock on each wake;
     * `null` when it is not playing. A `Server` reads it to find the logical
     * time of what it is emitting.
     */
    clock: TempoClock | null = null;
    /**
     * The exact logical beat at which the clock last resumed this stream
     * (yield-accumulated, never wall-clock). The Server stamps from it.
     */
    logicalBeat = 0;

    /**
     * Produces the next value, optionally fed `inval`; throws `StopStream` to
     * end.
     */
    abstract next(inval?: unknown): unknown;

    /**
     * Returns the stream to its initial state so iteration restarts. A no-op
     * on the base; stateful subclasses override it.
     */
    reset(): void {}

    /** Iterating a stream runs it to its end (a `StopStream` closes the loop). */
    *[Symbol.iterator](): Generator<unknown, void, undefined> {
        for (;;) {
            let value: unknown;
            try {
                value = this.next();
            } catch (error) {
                if (error instanceof StopStream) return;
                throw error;
            }
            yield value;
        }
    }
}

/** Wraps a plain callable: each `next` calls it with `inval`. */
export class FunctionStream extends Stream {
    private readonly func: (inval?: unknown) => unknown;
    private readonly resetFunc?: () => void;

    constructor(func: (inval?: unknown) => unknown, resetFunc?: () => void) {
        super();
        this.func = func;
        this.resetFunc = resetFunc;
        this.rng = spawnRng();
    }

    next(inval?: unknown): unknown {
        return this.func(inval);
    }

    override reset(): void {
        this.resetFunc?.();
    }
}

/**
 * What a routine's generator function looks like: it may take the initial
 * `inval`, and each `yield` is a delay in beats.
 */
export type RoutineFunc = (
    inval?: unknown,
) => Generator<number | undefined, unknown, unknown>;

/**
 * The state a routine is in. `paused` is a routine held out of the queue
 * without being finished — `pause` puts it there, `play` resumes it.
 */
export type RoutineState = "init" | "running" | "done" | "paused";

/**
 * Wraps a generator function into a resumable timeline.
 *
 * Each `next` resumes it; a yielded number is the delay in beats before the
 * routine should be resumed again. The generator's own locals are its musical
 * state, which is why a routine is forward-only — the seekable counterpart is
 * `seq/timeline.ts`.
 */
export class Routine extends Stream {
    readonly func: RoutineFunc;
    state: RoutineState = "init";
    private gen: Generator<number | undefined, unknown, unknown> | null = null;

    constructor(func: RoutineFunc) {
        super();
        this.func = func;
        // Its own stream, seeded by the context that creates it (sclang-style
        // inheritance): everything random drawn while this routine runs comes
        // from here, so one root seed reproduces it.
        this.rng = spawnRng();
    }

    /**
     * Discards the running generator and returns to `init`, so the next
     * `next` or `play` starts the generator function afresh.
     */
    override reset(): void {
        this.gen = null;
        this.state = "init";
    }

    /**
     * Resumes the generator once (sending it `inval`) and returns the value it
     * yields — a delay in beats — or throws `StopStream` when it finishes.
     * The clock calls this on each wake; you rarely call it yourself.
     */
    next(inval?: unknown): unknown {
        if (this.state === "done") throw new StopStream();
        this.state = "running"; // also what resumes a paused routine
        if (this.gen === null) {
            this.gen = this.func(inval);
            const first = this.gen.next();
            if (first.done) {
                this.state = "done";
                throw new StopStream();
            }
            return first.value;
        }
        const step = this.gen.next(inval);
        if (step.done) {
            this.state = "done";
            throw new StopStream();
        }
        return step.value;
    }

    /**
     * Schedules this routine to start on `clock`; returns itself. Inside a
     * running routine the clock defaults to the one driving it.
     */
    play(clock?: TempoClock, quant?: number): this {
        const target = clock ?? resolveClock();
        this.clock = target; // known from scheduling, not only from waking
        target.play(this, quant);
        return this;
    }

    /**
     * Takes this routine off its clock, keeping its position; returns itself.
     * The generator is untouched, so a later `play` resumes it at the very
     * `yield` it was paused on — the counterpart of `reset`, which throws that
     * position away. Pausing a routine that is not scheduled does nothing.
     */
    pause(): this {
        this.clock?.unsched(this);
        if (this.state === "running") this.state = "paused";
        return this;
    }

    /**
     * Takes this routine off its clock *and* rewinds it; returns itself.
     * `pause` followed by `reset`: a later `play` starts the generator function
     * afresh, from the top. This is a routine's own transport, not
     * `TempoClock.stop`, which halts the *clock* and every routine on it.
     */
    stop(): this {
        this.pause();
        this.reset();
        return this;
    }
}

/**
 * The clock driving the routine that is running right now. Throws where there
 * is none, because "later" has no meaning without one.
 */
function resolveClock(): TempoClock {
    const running = currentRoutine();
    if (running?.clock) return running.clock;
    throw new Error(
        "no clock to play on: pass one, or play from inside a routine already " +
            "running on a TempoClock",
    );
}
