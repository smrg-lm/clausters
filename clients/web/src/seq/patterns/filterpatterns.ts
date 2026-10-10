// Patterns made of another pattern (mirrors
// `clausters/seq/patterns/filterpatterns.py`).
//
// A filter pattern reads the pattern it filters and changes what comes out:
// how often, how many, which, or what each value becomes. It is the kind of the
// pattern it filters -- over an event pattern it is an event pattern, and plays
// -- except the ones that make values of values (`pclump`, `pflatten`, `pdiff`,
// `pwrap`, `pprorate`, `pavaroh`), and the ones that work on events' time
// (`pdur`, `pdelta`), which are event patterns by what they do.

import * as builtins from "../../base/builtins.ts";
import { StopStream } from "../../base/stream.ts";
import { Event, rest } from "../event.ts";
import { EventPattern, INF, asStream, embed, pattern, traceValues } from "../pattern.ts";
import type { Pattern } from "../pattern.ts";

/** A filter pattern's type: an event pattern over one, else a value pattern. */
export type FilterPattern<T> = [T] extends [Event] ? EventPattern<T> : Pattern<T>;

const overEvent = (source: unknown): boolean => source instanceof EventPattern;
const overSecond = (_first: unknown, source: unknown): boolean => source instanceof EventPattern;

/** Ends a generator quietly when one of its streams ends. */
const ended = (error: unknown): void => {
    if (!(error instanceof StopStream)) throw error;
};

/**
 * `func(value)`, or `func(value, inval)` when it declares the incoming value
 * too -- the two forms sclang's function filters accept.
 */
const call = (func: (...args: never[]) => unknown, value: unknown, inval: unknown): unknown =>
    func.length >= 2
        ? (func as (v: unknown, i: unknown) => unknown)(value, inval)
        : (func as (v: unknown) => unknown)(value);

/** A copy of a value a pattern repeats, so an event yielded twice is two events. */
const copied = (value: unknown): unknown => (value instanceof Event ? new Event({ ...value.props }) : value);

/** Marks the incoming event with `key` in place, making one if there is none. */
const mark = (inval: unknown, key: string, value: boolean): unknown => {
    if (inval instanceof Event) inval.props[key] = value;
    else if (inval !== null && typeof inval === "object") (inval as Record<string, unknown>)[key] = value;
    else return value ? { [key]: value } : inval;
    return inval;
};

/**
 * `source` from start to end, `repeats` times. With a `key`, the event each
 * repeat starts on is marked with it as `true` -- in place, so as a key of a
 * `pbind` it marks the event being built, which a {@link pgate} later in the
 * same `pbind` reads to take a new value -- and the event after the last as
 * `false`.
 */
export const pn = pattern(
    function* pn(inval: unknown, source: unknown, repeats: number = INF, key: string | null = null) {
        for (let i = 0; i < repeats; i++) {
            if (key !== null) inval = mark(inval, key, true);
            inval = yield* embed(source, inval);
        }
        if (key !== null) inval = mark(inval, key, false);
        return inval;
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, repeats?: number, key?: string | null) => FilterPattern<T>;

/** Whether the incoming event carries `key` as true. */
const keyed = (inval: unknown, key: string | null): boolean => {
    if (key === null || inval === null || typeof inval !== "object") return false;
    const props = inval instanceof Event ? inval.props : (inval as Record<string, unknown>);
    return Boolean(props[key]);
};

/**
 * `source`'s values, a new one only when the incoming event's `key` is true and
 * the last one held otherwise: a key of a `pbind` that changes when a
 * {@link pn} with that `key` starts a repeat of the phrase.
 */
export const pgate = pattern(
    function* pgate(inval: unknown, source: unknown, repeats: number = INF, key: string | null = null) {
        for (let i = 0; i < repeats; i++) {
            const stream = asStream(source);
            let held: unknown = null;
            try {
                for (;;) {
                    if (held === null || keyed(inval, key)) held = stream.next(inval);
                    inval = yield copied(held);
                }
            } catch (error) {
                ended(error);
            }
        }
        return inval;
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, repeats?: number, key?: string | null) => FilterPattern<T>;

/** `func` of each of `source`'s values (and of the incoming value, when it takes two arguments). */
export const pcollect = pattern(
    function* pcollect(inval: unknown, func: (...args: never[]) => unknown, source: unknown) {
        const stream = asStream(source);
        try {
            for (;;) inval = yield call(func, stream.next(inval), inval);
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overSecond },
) as unknown as <T, U>(func: (value: T, inval?: unknown) => U, source: T | Pattern<T>) => FilterPattern<U>;

function* filtered(
    inval: unknown,
    func: (...args: never[]) => unknown,
    source: unknown,
    keep: boolean,
): Generator<unknown, unknown, unknown> {
    const stream = asStream(source);
    try {
        for (;;) {
            const value = stream.next(inval);
            if (Boolean(call(func, value, inval)) === keep) inval = yield value;
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}

/** `source`'s values for which `func` is true. */
export const pselect = pattern(
    function* pselect(inval: unknown, func: (...args: never[]) => unknown, source: unknown) {
        return yield* filtered(inval, func, source, true);
    },
    { events: overSecond },
) as unknown as <T>(func: (value: T, inval?: unknown) => unknown, source: T | Pattern<T>) => FilterPattern<T>;

/** `source`'s values for which `func` is false. */
export const preject = pattern(
    function* preject(inval: unknown, func: (...args: never[]) => unknown, source: unknown) {
        return yield* filtered(inval, func, source, false);
    },
    { events: overSecond },
) as unknown as <T>(func: (value: T, inval?: unknown) => unknown, source: T | Pattern<T>) => FilterPattern<T>;

/** `source` without its first `n` values. */
export const pdrop = pattern(
    function* pdrop(inval: unknown, source: unknown, n: number) {
        const stream = asStream(source);
        try {
            for (let i = 0; i < Math.trunc(n); i++) stream.next(inval);
            for (;;) inval = yield stream.next(inval);
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, n: number) => FilterPattern<T>;

/** The first `n` values of `source` (sclang's `Pfin`). */
export const plen = pattern(
    function* plen(inval: unknown, source: unknown, n: number) {
        const stream = asStream(source);
        try {
            for (let i = 0; i < Math.trunc(n); i++) inval = yield stream.next(inval);
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, n: number) => FilterPattern<T>;

/** Each of `source`'s values `n` times in a row; `n` may be a pattern, read once per value. */
export const pstutter = pattern(
    function* pstutter(inval: unknown, source: unknown, n: number | Pattern<number>) {
        const [stream, counts] = [asStream(source), asStream(n)];
        try {
            for (;;) {
                const value = stream.next(inval);
                const times = Math.abs(Math.trunc(counts.next(inval) as number));
                for (let i = 0; i < times; i++) inval = yield copied(value);
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, n: number | Pattern<number>) => FilterPattern<T>;

/**
 * `source`'s next value where `trig` is true and the last one held where it is
 * false (sclang's `Pclutch`): a latch a pattern of booleans opens.
 */
export const platch = pattern(
    function* platch(inval: unknown, source: unknown, trig: boolean | Pattern<boolean> = true) {
        const [stream, trigs] = [asStream(source), asStream(trig)];
        let held: unknown;
        let have = false;
        try {
            for (;;) {
                if (trigs.next(inval) || !have) {
                    held = stream.next(inval);
                    have = true;
                    inval = yield held;
                } else {
                    inval = yield copied(held);
                }
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, trig?: boolean | Pattern<boolean>) => FilterPattern<T>;

/** `source` again and again while `func()` is true (`func(inval)` when it takes the incoming value). */
export const pwhile = pattern(
    function* pwhile(inval: unknown, func: (...args: never[]) => unknown, source: unknown) {
        const test = (): unknown =>
            func.length >= 1 ? (func as (i: unknown) => unknown)(inval) : (func as () => unknown)();
        while (test()) inval = yield* embed(source, inval);
        return inval;
    },
    { events: overSecond },
) as unknown as <T>(func: (inval?: unknown) => unknown, source: T | Pattern<T>) => FilterPattern<T>;

/** `source`'s values wrapped into `lo`..`hi`; the bounds may be patterns. */
export const pwrap = pattern(function* pwrap(
    inval: unknown,
    source: unknown,
    lo: number | Pattern<number>,
    hi: number | Pattern<number>,
) {
    const [stream, los, his] = [asStream(source), asStream(lo), asStream(hi)];
    try {
        for (;;) {
            const [l, h] = [los.next(inval) as number, his.next(inval) as number];
            inval = yield builtins.wrap(stream.next(inval) as number, l, h) as number;
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as (source: number | Pattern<number>, lo: number | Pattern<number>, hi: number | Pattern<number>) => Pattern<number>;

/**
 * `source`'s values gathered in arrays of `n` (the last one shorter if the
 * source ends inside it); `n` may be a pattern.
 */
export const pclump = pattern(function* pclump(inval: unknown, source: unknown, n: number | Pattern<number>) {
    const [stream, sizes] = [asStream(source), asStream(n)];
    let group: unknown[] = [];
    try {
        for (;;) {
            group = [];
            const size = Math.trunc(sizes.next(inval) as number);
            for (let i = 0; i < size; i++) group.push(stream.next(inval));
            inval = yield group;
        }
    } catch (error) {
        ended(error);
        if (group.length > 0) inval = yield group;
    }
    return inval;
}) as <T>(source: T | Pattern<T>, n: number | Pattern<number>) => Pattern<T[]>;

const flatten = (value: unknown, levels: number): unknown[] =>
    levels <= 0 || !Array.isArray(value) ? [value] : value.flatMap((v) => flatten(v, levels - 1));

/**
 * `source`'s values with the arrays among them opened `n` levels deep and their
 * elements yielded one by one.
 */
export const pflatten = pattern(function* pflatten(inval: unknown, source: unknown, n: number | Pattern<number> = 1) {
    const [stream, levels] = [asStream(source), asStream(n)];
    try {
        for (;;) {
            const depth = Math.trunc(levels.next(inval) as number);
            for (const item of flatten(stream.next(inval), depth)) inval = yield item;
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as <T>(source: unknown, n?: number | Pattern<number>) => Pattern<T>;

/** The difference between each of `source`'s values and the one before it: the steps of a line. */
export const pdiff = pattern(function* pdiff(inval: unknown, source: unknown) {
    const stream = asStream(source);
    try {
        let previous = stream.next(inval) as number;
        for (;;) {
            const value = stream.next(inval) as number;
            inval = yield value - previous;
            previous = value;
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as (source: number | Pattern<number>) => Pattern<number>;

/**
 * Each of `source`'s values divided by `proportion`: `c` gives the two parts
 * `c * value` and `(1 - c) * value`, an array gives a part per element -- a
 * duration split into a rhythm.
 */
export const pprorate = pattern(function* pprorate(
    inval: unknown,
    source: unknown,
    proportion: number | readonly number[] | Pattern<number | readonly number[]> = 1,
) {
    const [stream, proportions] = [asStream(source), asStream(proportion)];
    try {
        for (;;) {
            const value = stream.next(inval) as number;
            const c = proportions.next(inval) as number | readonly number[];
            const parts = Array.isArray(c) ? c : [c as number, 1 - (c as number)];
            for (const part of parts) inval = yield part * value;
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as (
    source: number | Pattern<number>,
    proportion?: number | readonly number[] | Pattern<number | readonly number[]>,
) => Pattern<number>;

/**
 * `source`'s scale degrees as keys, read with the scale `aroh` while the line
 * rises and `avaroh` while it falls -- the ascending and descending forms of a
 * raga.
 */
export const pavaroh = pattern(function* pavaroh(
    inval: unknown,
    source: unknown,
    aroh: readonly number[],
    avaroh: readonly number[],
    steps: number | Pattern<number> = 12,
) {
    const [stream, stepStream] = [asStream(source), asStream(steps)];
    let last = 0;
    try {
        for (;;) {
            const octaveSteps = stepStream.next(inval) as number;
            const degree = stream.next(inval) as number;
            const scale = degree >= last ? aroh : avaroh;
            last = degree;
            inval = yield builtins.degkey(degree, scale, octaveSteps) as number;
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as (
    source: number | Pattern<number>,
    aroh: readonly number[],
    avaroh: readonly number[],
    steps?: number | Pattern<number>,
) => Pattern<number>;

/** A rest of `time` beats, then `source`: an event pattern started late. */
export const pdelta = pattern(
    function* pdelta(inval: unknown, time: number, source: unknown) {
        if (time > 0) inval = yield rest(time);
        return yield* embed(source, inval);
    },
    { events: true },
) as (time: number, source: Pattern<Event>) => EventPattern<Event>;

/**
 * `source`'s events until `dur` beats have passed, the last one cut to end
 * exactly there (sclang's `Pfindur`). With `quant`, a source that ends early is
 * padded with a rest to the next multiple of it.
 */
export const pdur = pattern(
    function* pdur(
        inval: unknown,
        dur: number,
        source: unknown,
        tolerance: number = 0.001,
        quant: number | null = null,
    ) {
        const stream = asStream(source);
        let elapsed = 0;
        try {
            for (;;) {
                const got = stream.next(inval);
                const event = got instanceof Event ? got : new Event(got as Record<string, unknown>);
                const delta = event.delta();
                if ((builtins.roundup(elapsed + delta, tolerance) as number) >= dur) {
                    const last = new Event({ ...event.props, delta: dur - elapsed });
                    return yield last;
                }
                elapsed += delta;
                inval = yield event;
            }
        } catch (error) {
            ended(error);
            if (quant !== null) {
                const pad = (builtins.roundup(elapsed, quant) as number) - elapsed;
                if (pad > 0) inval = yield rest(pad);
            }
        }
        return inval;
    },
    { events: true },
) as (dur: number, source: Pattern<Event>, tolerance?: number, quant?: number | null) => EventPattern<Event>;

/** `source` unchanged, each value printed as it passes, after `prefix` -- only `keys` of an event, when given. */
export const ptrace = pattern(
    function* ptrace(inval: unknown, source: unknown, prefix: string = "", keys: readonly string[] | null = null) {
        return yield* traceValues(inval, source, prefix, keys);
    },
    { events: overEvent },
) as <T>(source: T | Pattern<T>, prefix?: string, keys?: readonly string[] | null) => FilterPattern<T>;
