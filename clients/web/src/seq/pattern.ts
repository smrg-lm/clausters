// Patterns: what a pattern is, and how one is written (mirrors
// `clausters/seq/pattern.py`).
//
// A **pattern** is a reusable, lazy description of a sequence of values -- the
// definition of a generator. Iterating it starts a fresh **stream**, which
// yields the values: `for (const x of pseq([1, 2, 3]))` loops over 1, 2, 3, and
// a second loop starts again from 1. A pattern holds no state of its own;
// whatever a walk needs to remember (a position, a random walk's value, a seed)
// lives in that walk.
//
// **Value patterns and event patterns** are the two kinds, and they differ in
// what their stream yields. A value pattern yields values -- numbers, arrays,
// anything a key takes. An event pattern yields `Event`s, and it is the only
// kind that plays: `EventPattern.play` runs it on a clock and `renderEvents`
// renders it into an `EventSequence`. `pbind` is the event pattern that builds
// events out of value patterns, one per key. A list pattern (`pseq`, `prand`,
// `pn`) whose every element is an event pattern is one too; which kind a
// pattern is, is decided when it is built.
//
// **A pattern is written as a generator function** under `pattern`, as a UGen
// is a function returning a graph node. The function receives the **incoming
// value** first -- `inval`, the event a `pbind` is building when the pattern is
// one of its keys, else `undefined` -- and then its own arguments; each `yield`
// gives a value and receives the next incoming one:
//
//     const pcount = pattern(function* pcount(inval: unknown, n: number) {
//         for (let i = 0; i < n; i++) inval = yield i;
//         return inval;
//     });
//
// A sub-pattern is embedded with `inval = yield* embed(item, inval)`, which
// passes the incoming values through and hands back the last one.
//
// **A pattern composes with the operators** a UGen does, by the same names:
// `pseq([1, 2, 3]).add(60)` is a pattern of 61, 62, 63, and `.midicps()`,
// `.min(x)` or `.mod(12)` compose the same way. Each value is computed by the
// `base/builtins` function of that name, so a pattern, an array and a signal
// answer an operator with one function. An operator applies to each value a
// stream yields; a function over a whole list (`normalize`) is not an
// operator, and is applied to the list before it becomes a pattern. An event
// pattern yields events, which have no arithmetic, and refuses an operator.
//
// The patterns themselves are in `seq/patterns/`, by family.

import { AbstractObject } from "../base/absobject.ts";
import type { Composed, Fan } from "../base/absobject.ts";
import { Stream, StopStream } from "../base/stream.ts";
import { valueOp } from "../base/builtins.ts";
import { main } from "../base/main.ts";
import type { Event, EventDestination } from "./event.ts";
import { EventStreamPlayer } from "./eventstream.ts";
import type { TempoClock } from "../base/clock.ts";

/** An endless pattern's length. */
export const INF = Infinity;

/** The generator a pattern is written as: values out, incoming values in. */
export type PatternGenerator<T> = Generator<T, unknown, unknown>;

// ---- streams over a pattern ----

/**
 * A walk over a pattern: one generator, started on the first `next`.
 *
 * `next` takes the incoming value (`inval`) and returns the pattern's next
 * value, or throws `StopStream` at its end. `reset` starts the walk again.
 */
export class PatternStream<T = unknown> extends Stream {
    readonly pattern: Pattern<T>;
    private gen: PatternGenerator<T> | null = null;
    private done = false;

    constructor(pattern: Pattern<T>) {
        super();
        this.pattern = pattern;
    }

    next(inval?: unknown): T {
        if (this.done) throw new StopStream();
        let step: IteratorResult<T, unknown>;
        if (this.gen === null) {
            this.gen = this.pattern.embed(inval);
            step = this.gen.next();
        } else {
            step = this.gen.next(inval);
        }
        if (step.done) {
            this.done = true;
            throw new StopStream();
        }
        return step.value;
    }

    reset(): void {
        this.gen = null;
        this.done = false;
    }
}

/** A bare value as a stream: the value, forever. */
class ConstStream extends Stream {
    readonly value: unknown;

    constructor(value: unknown) {
        super();
        this.value = value;
    }

    next(): unknown {
        return this.value;
    }
}

/**
 * Yields `value`, or every value of it if it is a pattern; returns the last
 * incoming value. Inside a pattern: `inval = yield* embed(x, inval)`.
 */
export function* embed<T>(value: T | Pattern<T>, inval?: unknown): PatternGenerator<T> {
    if (value instanceof Pattern) return yield* value.embed(inval);
    return yield value as T;
}

/**
 * A stream over `value`: a fresh walk of a pattern, the stream itself, or a
 * bare value repeated forever -- how an argument that may be either is read one
 * value at a time.
 */
export function asStream(value: unknown): Stream {
    if (value instanceof Pattern) return value.stream();
    if (value instanceof Stream) return value;
    return new ConstStream(value);
}

// ---- the pattern ----

/**
 * An argument as a pattern keeps it: an array, a plain object or any other
 * iterable is copied when the pattern is built, so editing the array
 * afterwards does not change the pattern, and a one-shot iterable (a
 * generator) is read once rather than consumed by the first walk.
 */
function held(value: unknown): unknown {
    if (value === null || typeof value !== "object") return value;
    if (value instanceof Pattern || value instanceof Stream) return value;
    if (Array.isArray(value)) return [...value];
    if (Object.getPrototypeOf(value) === Object.prototype) return { ...value };
    if (Symbol.iterator in value) return Array.from(value as Iterable<unknown>);
    return value;
}

/** How an argument is written in a pattern's description. */
function described(value: unknown): string {
    if (value instanceof Pattern) return value.toString();
    if (typeof value === "string") return JSON.stringify(value);
    if (typeof value === "function") return value.name || "function";
    if (Array.isArray(value)) return `[${value.map(described).join(", ")}]`;
    if (value !== null && typeof value === "object") {
        const entries = Object.entries(value).map(([k, v]) => `${k}: ${described(v)}`);
        return `{ ${entries.join(", ")} }`;
    }
    return String(value);
}

function refuseEvents(...operands: unknown[]): void {
    for (const operand of operands) {
        if (operand instanceof EventPattern) {
            throw new TypeError(
                `${operand} is an event pattern: it yields events, which have no ` +
                    "arithmetic -- an operator composes a value pattern",
            );
        }
    }
}

/**
 * The definition of a generator: iterating it starts a fresh walk that yields
 * its values. It has no `play` -- what plays is an {@link EventPattern};
 * `render` of a value pattern generates its values.
 *
 * Made by a function written under {@link pattern}; `new Pattern(gen, args)`
 * is what that builds.
 */
export class Pattern<T = unknown> extends AbstractObject<Pattern<number>, unknown> {
    private readonly gen: (inval: unknown, ...args: never[]) => PatternGenerator<T>;
    private readonly args: readonly unknown[];
    private readonly label: string;

    constructor(
        gen: (inval: unknown, ...args: never[]) => PatternGenerator<T>,
        args: readonly unknown[] = [],
        name?: string,
    ) {
        super();
        this.gen = gen;
        this.args = args;
        this.label = name ?? (gen.name || "pattern");
    }

    /** The generator that walks this pattern, fed `inval` first. */
    embed(inval?: unknown): PatternGenerator<T> {
        return this.gen(inval, ...(this.args as never[]));
    }

    [Symbol.iterator](): Iterator<T> {
        return this.stream()[Symbol.iterator]() as Iterator<T>;
    }

    /** A fresh `PatternStream` over this pattern. */
    stream(): PatternStream<T> {
        return new PatternStream(this);
    }

    toString(): string {
        return `${this.label}(${this.args.map(described).join(", ")})`;
    }

    // ---- AbstractObject: an operator composes a pattern ----

    protected binop<U>(selector: string, other: U): Composed<Pattern<number>, U, Fan<never, never>> {
        refuseEvents(this, other);
        return opPattern("pbinop", selector, [this, other]) as Composed<Pattern<number>, U, Fan<never, never>>;
    }

    protected unop(selector: string): Pattern<number> {
        refuseEvents(this);
        return opPattern("punop", selector, [this]);
    }
}

/**
 * What an operator composes: `punop`, `pbinop`, `pnarop`
 * (`seq/patterns/oppatterns.ts`). @internal
 */
export function opPattern(
    name: string,
    selector: string,
    operands: readonly unknown[],
): Pattern<number> {
    return new Pattern(applyOp as never, [selector, ...operands], name);
}

/** The generator behind `punop`, `pbinop` and `pnarop`. @internal */
export function* applyOp(
    inval: unknown,
    selector: string,
    ...operands: unknown[]
): PatternGenerator<number> {
    const fn = valueOp(selector) as (...values: unknown[]) => unknown;
    const streams = operands.map(asStream);
    for (;;) {
        let values: unknown[];
        try {
            values = streams.map((s) => s.next(inval));
        } catch (error) {
            if (error instanceof StopStream) return inval;
            throw error;
        }
        inval = yield fn(...values) as number;
    }
}

/**
 * A pattern whose values are events, and so a pattern that plays.
 *
 * `pbind` is one, and so is a list pattern (`pseq`, `prand`, `pn`) whose every
 * element is one: which kind a pattern is, is decided when it is built.
 */
export class EventPattern<T = Event> extends Pattern<T> {
    /**
     * Plays this event pattern on `clock`, sending to `destination`.
     *
     * Both are optional and resolve against the ambient context (the running
     * session, else the active one, else the default session): an omitted
     * `destination` takes that environment's server, and an omitted `clock`
     * the running routine's or, outside one, the default session's -- created
     * and started on first use. So `pbind(...).play()` sounds with a `Session`
     * opened somewhere and nothing else wired.
     */
    play(
        destination?: EventDestination,
        { clock, quant }: { clock?: TempoClock; quant?: number } = {},
    ): EventStreamPlayer {
        const target = destination ?? (main.resolveServer() as unknown as EventDestination);
        return new EventStreamPlayer(this as Pattern<unknown>, target).play(clock, quant);
    }

    /**
     * **Renders the pattern into the events it plays**: an `EventSequence` in
     * the pattern's beats, each event as it played -- its derived keys written
     * in, at the beat it fell on. The render goes one way (see
     * `Timeline.renderEvents`), and is asynchronous for the reason given there.
     *
     * @param until stop at this beat. Required for an endless pattern, which is
     *     otherwise refused after `MAX_BOUNCED_EVENTS` events.
     */
    async renderEvents(until?: number): Promise<import("./sequence.ts").EventSequence> {
        const { renderPattern } = await import("./sequence.ts");
        return renderPattern(this as never, until);
    }
}

/** What kind of pattern {@link pattern} builds. */
export interface PatternOptions<A extends unknown[]> {
    /**
     * `false` (the default) a value pattern, `true` an {@link EventPattern},
     * or a function of the arguments that decides when the pattern is built (a
     * list pattern is an event pattern when every element is one).
     */
    events?: boolean | ((...args: A) => boolean);
}

/**
 * **Makes a pattern out of a generator function**: the result is a function
 * that builds the pattern, as `sine` builds a UGen.
 *
 * The generator receives the incoming value first (`inval`) and then the
 * arguments the pattern was built with; it yields values and receives the next
 * incoming value from each `yield`, and returns the last one. Every walk calls
 * it afresh, so a pattern is reusable: building one stores the function and its
 * arguments (an array or plain object argument is copied), and starts nothing.
 *
 * ```ts
 * const pcount = pattern(function* pcount(inval: unknown, n: number) {
 *     for (let i = 0; i < n; i++) inval = yield i;
 *     return inval;
 * });
 * [...pcount(3)]; // [0, 1, 2]
 * ```
 */
export function pattern<A extends unknown[], T>(
    gen: (inval: unknown, ...args: A) => PatternGenerator<T>,
    options: PatternOptions<A> & { events: true },
): (...args: A) => EventPattern<T>;
export function pattern<A extends unknown[], T>(
    gen: (inval: unknown, ...args: A) => PatternGenerator<T>,
    options?: PatternOptions<A>,
): (...args: A) => Pattern<T>;
export function pattern<A extends unknown[], T>(
    gen: (inval: unknown, ...args: A) => PatternGenerator<T>,
    { events = false }: PatternOptions<A> = {},
): (...args: A) => Pattern<T> {
    const build = (...given: A): Pattern<T> => {
        const args = given.map(held) as A;
        const isEvents = typeof events === "function" ? events(...args) : events;
        const Kind = isEvents ? EventPattern : Pattern;
        return new Kind(gen as never, args);
    };
    Object.defineProperty(build, "name", { value: gen.name });
    return build;
}

/**
 * Whether every one of `items` is an event pattern (and there is one): the
 * rule a list pattern is resolved by.
 */
export const allEvents = (items: readonly unknown[]): boolean =>
    items.length > 0 && items.every((item) => item instanceof EventPattern);
