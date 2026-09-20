// Patterns (mirrors `clausters/seq/pattern.py`).
//
// A `Pattern` is a reusable, lazy description of a value sequence -- the
// definition of a generator: iterating it yields the values (a fresh walk each
// time), and it does not play. Value patterns (`Pseq`, `Pwhite`, ...) feed
// `Pbind`, which combines per-key value patterns into a stream of `Event`s. An
// `EventPattern` -- `Pbind`, and a list pattern whose every element is an event
// pattern -- is what plays, on a clock, with `EventPattern.play` (see
// `EventStreamPlayer`).
//
// Patterns are plain generators underneath, so nesting and composition are
// natural; a sub-pattern used as a value is embedded (iterated) in place.

import { FunctionStream, StopStream } from "../base/stream.ts";
import type { Stream } from "../base/stream.ts";
import * as rand from "../base/rand.ts";
import { main } from "../base/main.ts";
import { Event } from "./event.ts";
import type { EventDestination } from "./event.ts";
import { EventStreamPlayer } from "./eventstream.ts";
import type { TempoClock } from "../base/clock.ts";

/** An endless pattern's length. */
export const INF = Infinity;

/** Yields a value, or iterates it if it is itself a pattern. */
function* embed<T>(value: T | Pattern<T>): Generator<T, void, undefined> {
    if (value instanceof Pattern) yield* value;
    else yield value;
}

/** A bare value used where a pattern is expected becomes an endless constant. */
export const asPattern = <T>(value: T | Pattern<T>): Pattern<T> =>
    value instanceof Pattern ? value : new Pconst(value);

/**
 * The definition of a generator: walking it yields its values. It has no
 * `play` -- what plays is an {@link EventPattern}. `render` of a value pattern
 * generates its values.
 */
export abstract class Pattern<T = unknown> {
    abstract [Symbol.iterator](): Generator<T, void, undefined>;

    /** A `Stream` over this pattern -- the form the clock can resume. */
    stream(): Stream {
        const it = this[Symbol.iterator]();
        return new FunctionStream(() => {
            const step = it.next();
            if (step.done) throw new StopStream();
            return step.value;
        });
    }
}

/** What an event pattern carries, so a list pattern built over events is one. */
const EVENTS = Symbol("clausters.eventPattern");

/** Plays `pattern`'s events on `clock`, sending to `destination`. */
function playEvents(
    pattern: Pattern<unknown>,
    destination: EventDestination | undefined,
    { clock, quant }: { clock?: TempoClock; quant?: number },
): EventStreamPlayer {
    const target = destination ?? (main.resolveServer() as unknown as EventDestination);
    return new EventStreamPlayer(pattern, target).play(clock, quant);
}

/**
 * A pattern whose values are events, and so a pattern that plays.
 *
 * `Pbind` is one, and so is a list pattern (`Pseq`, `Prand`, `Pn`) whose every
 * element is one: it resolves what it is when it is built, and
 * `instanceof EventPattern` answers for both.
 */
export abstract class EventPattern<T = Event> extends Pattern<T> {
    readonly [EVENTS] = true;

    static override [Symbol.hasInstance](value: unknown): boolean {
        if (this !== EventPattern) return Function.prototype[Symbol.hasInstance].call(this, value);
        return value instanceof Pattern && (value as { [EVENTS]?: boolean })[EVENTS] === true;
    }

    /**
     * Plays this event pattern on `clock`, sending to `destination`.
     *
     * Both are optional and resolve against the ambient context (the running
     * session, else the active one, else the default session): an omitted
     * `destination` takes that environment's server, and an omitted `clock`
     * the running routine's or, outside one, the default session's -- created
     * and started on first use. So `new Pbind(...).play()` sounds with a
     * `Session` opened somewhere and nothing else wired.
     */
    play(
        destination?: EventDestination,
        options: { clock?: TempoClock; quant?: number } = {},
    ): EventStreamPlayer {
        return playEvents(this as Pattern<unknown>, destination, options);
    }
}

/** Whether every element of a list pattern is an event pattern. */
const allEvents = (items: readonly unknown[]): boolean =>
    items.length > 0 && items.every((item) => item instanceof EventPattern);

// ---- value patterns ----

/** A constant value, `length` times (endless by default). */
export class Pconst<T> extends Pattern<T> {
    readonly value: T;
    readonly length: number;

    constructor(value: T, length: number = INF) {
        super();
        this.value = value;
        this.length = length;
    }

    *[Symbol.iterator](): Generator<T, void, undefined> {
        for (let i = 0; i < this.length; i++) yield this.value;
    }
}

/**
 * The items in order, `repeats` times (sub-patterns are embedded). Over event
 * patterns only, it is an {@link EventPattern}.
 */
export class Pseq<T> extends Pattern<T> {
    readonly items: readonly (T | Pattern<T>)[];
    readonly repeats: number;

    constructor(items: readonly (T | Pattern<T>)[], repeats: number = 1) {
        super();
        this.items = [...items];
        this.repeats = repeats;
        if (new.target === Pseq && allEvents(this.items)) {
            return new EventPseq(this.items, repeats);
        }
    }

    *[Symbol.iterator](): Generator<T, void, undefined> {
        for (let i = 0; i < this.repeats; i++) {
            for (const item of this.items) yield* embed(item);
        }
    }
}

/** The items in order, yielding exactly `length` values (cycling). */
export class Pser<T> extends Pattern<T> {
    readonly items: readonly T[];
    readonly length: number;

    constructor(items: readonly T[], length: number) {
        super();
        this.items = [...items];
        this.length = length;
    }

    *[Symbol.iterator](): Generator<T, void, undefined> {
        for (let i = 0; i < this.length; i++) yield this.items[i % this.items.length]!;
    }
}

/**
 * Random items, `length` values, drawn from the **random context** (the
 * running routine's stream, or the root outside one -- see `base/rand.ts`):
 * `seed(n)` reproduces the choices along with everything else in the script.
 * There is no per-pattern seed -- independent seeds would break whole-script
 * consistency. Over event patterns only, it is an {@link EventPattern}.
 */
export class Prand<T> extends Pattern<T> {
    readonly items: readonly (T | Pattern<T>)[];
    readonly length: number;

    constructor(items: readonly (T | Pattern<T>)[], length: number = INF) {
        super();
        this.items = [...items];
        this.length = length;
        if (new.target === Prand && allEvents(this.items)) {
            return new EventPrand(this.items, length);
        }
    }

    *[Symbol.iterator](): Generator<T, void, undefined> {
        for (let i = 0; i < this.length; i++) yield* embed(rand.choice(this.items));
    }
}

/**
 * Uniform random numbers in `[lo, hi)`, `length` values, from the random
 * context (see `Prand` on seeding).
 */
export class Pwhite extends Pattern<number> {
    readonly lo: number;
    readonly hi: number;
    readonly length: number;

    constructor(lo = 0, hi = 1, length: number = INF) {
        super();
        this.lo = lo;
        this.hi = hi;
        this.length = length;
    }

    *[Symbol.iterator](): Generator<number, void, undefined> {
        for (let i = 0; i < this.length; i++) yield rand.uniform(this.lo, this.hi);
    }
}

/** Arithmetic series `start, start + step, ...` (`length` values). */
export class Pseries extends Pattern<number> {
    readonly start: number;
    readonly step: number;
    readonly length: number;

    constructor(start = 0, step = 1, length: number = INF) {
        super();
        this.start = start;
        this.step = step;
        this.length = length;
    }

    *[Symbol.iterator](): Generator<number, void, undefined> {
        let value = this.start;
        for (let i = 0; i < this.length; i++) {
            yield value;
            value += this.step;
        }
    }
}

/** Geometric series `start, start * grow, ...` (`length` values). */
export class Pgeom extends Pattern<number> {
    readonly start: number;
    readonly grow: number;
    readonly length: number;

    constructor(start = 1, grow = 2, length: number = INF) {
        super();
        this.start = start;
        this.grow = grow;
        this.length = length;
    }

    *[Symbol.iterator](): Generator<number, void, undefined> {
        let value = this.start;
        for (let i = 0; i < this.length; i++) {
            yield value;
            value *= this.grow;
        }
    }
}

/** Calls `func()` for each value (`length` values). */
export class Pfunc<T> extends Pattern<T> {
    readonly func: () => T;
    readonly length: number;

    constructor(func: () => T, length: number = INF) {
        super();
        this.func = func;
        this.length = length;
    }

    *[Symbol.iterator](): Generator<T, void, undefined> {
        for (let i = 0; i < this.length; i++) yield this.func();
    }
}

/** Repeats `pattern` `n` times. Over an event pattern, it is an {@link EventPattern}. */
export class Pn<T> extends Pattern<T> {
    readonly pattern: T | Pattern<T>;
    readonly n: number;

    constructor(pattern: T | Pattern<T>, n: number = INF) {
        super();
        this.pattern = pattern;
        this.n = n;
        if (new.target === Pn && pattern instanceof EventPattern) {
            return new EventPn(pattern, n);
        }
    }

    *[Symbol.iterator](): Generator<T, void, undefined> {
        for (let i = 0; i < this.n; i++) yield* embed(this.pattern);
    }
}

// ---- event pattern ----

/**
 * The keys a `Pbind` binds: each one a pattern, or a constant held for every
 * event.
 */
export type Bindings = Record<string, unknown>;

/**
 * Binds keys to value patterns; yields an `Event` per step, stopping when any
 * key's walk stops. Constants are held; sub-patterns advance one value per
 * event.
 */
export class Pbind extends EventPattern<Event> {
    readonly patterns: Bindings;

    constructor(patterns: Bindings) {
        super();
        this.patterns = { ...patterns };
    }

    *[Symbol.iterator](): Generator<Event, void, undefined> {
        const walks = Object.entries(this.patterns).map(
            ([key, value]) =>
                [key, asPattern(value)[Symbol.iterator]()] as const,
        );
        for (;;) {
            const props: Record<string, unknown> = {};
            for (const [key, walk] of walks) {
                const step = walk.next();
                if (step.done) return;
                props[key] = step.value;
            }
            yield new Event(props);
        }
    }
}

// ---- list patterns over events ----
//
// What a list pattern is made as when every element is an event pattern: the
// same pattern, and playable. `new Pseq([new Pbind(...), new Pbind(...)])` is one.

class EventPseq<T> extends Pseq<T> {
    readonly [EVENTS] = true;

    play(destination?: EventDestination, options: { clock?: TempoClock; quant?: number } = {}): EventStreamPlayer {
        return playEvents(this as Pattern<unknown>, destination, options);
    }
}

class EventPrand<T> extends Prand<T> {
    readonly [EVENTS] = true;

    play(destination?: EventDestination, options: { clock?: TempoClock; quant?: number } = {}): EventStreamPlayer {
        return playEvents(this as Pattern<unknown>, destination, options);
    }
}

class EventPn<T> extends Pn<T> {
    readonly [EVENTS] = true;

    play(destination?: EventDestination, options: { clock?: TempoClock; quant?: number } = {}): EventStreamPlayer {
        return playEvents(this as Pattern<unknown>, destination, options);
    }
}
