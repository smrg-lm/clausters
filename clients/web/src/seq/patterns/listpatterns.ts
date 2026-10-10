// Patterns that read a list (mirrors `clausters/seq/patterns/listpatterns.py`).
//
// An item may itself be a pattern, which is **embedded**: all its values come
// out in its place before the next item. A list pattern whose every item is an
// event pattern is an event pattern, and plays (`pseq([pbind(...),
// pbind(...)])` is a phrase of phrases); otherwise it is a value pattern. The
// random ones draw from the random context (see `randompatterns.ts`).

import * as builtins from "../../base/builtins.ts";
import * as rand from "../../base/rand.ts";
import { StopStream } from "../../base/stream.ts";
import type { Event } from "../event.ts";
import { INF, allEvents, asStream, embed, pattern } from "../pattern.ts";
import type { EventPattern, Pattern } from "../pattern.ts";

/** A list pattern's type: an event pattern over events, else a value pattern. */
export type ListPattern<T> = [T] extends [Event] ? EventPattern<T> : Pattern<T>;

/** Items of a list pattern: values or patterns of them. */
export type ListItems<T> = readonly (T | Pattern<T>)[];

const overEvents = (items: readonly unknown[]): boolean => allEvents(items);

/** Ends a generator quietly when one of its streams ends. */
const ended = (error: unknown): void => {
    if (!(error instanceof StopStream)) throw error;
};

/** The items in order, `repeats` times, starting at `offset`. */
export const pseq = pattern(
    function* pseq(inval: unknown, items: readonly unknown[], repeats: number = 1, offset: number = 0) {
        const n = items.length;
        for (let r = 0; r < repeats; r++) {
            for (let i = 0; i < n; i++) inval = yield* embed(items[(i + offset) % n], inval);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, repeats?: number, offset?: number) => ListPattern<T>;

/**
 * The items in order, cycling from `offset`, until exactly `length` items
 * have come out.
 */
export const pser = pattern(
    function* pser(inval: unknown, items: readonly unknown[], length: number = 1, offset: number = 0) {
        for (let i = 0; i < length; i++) {
            inval = yield* embed(items[(i + offset) % items.length], inval);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, length?: number, offset?: number) => ListPattern<T>;

/** A random item, `length` times. */
export const prand = pattern(
    function* prand(inval: unknown, items: readonly unknown[], length: number = INF) {
        for (let i = 0; i < length; i++) inval = yield* embed(rand.choose(items), inval);
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, length?: number) => ListPattern<T>;

/** A random item, `length` times, never the same one twice in a row. */
export const pxrand = pattern(
    function* pxrand(inval: unknown, items: readonly unknown[], length: number = INF) {
        const n = items.length;
        let index = rand.irand(n);
        for (let i = 0; i < length; i++) {
            index = (index + rand.irand(n - 1) + 1) % n;
            inval = yield* embed(items[index], inval);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, length?: number) => ListPattern<T>;

/**
 * A random item, `length` times, chosen with the relative `weights` (they need
 * not sum to 1); `weights` may be a pattern of weight arrays.
 */
export const pwrand = pattern(
    function* pwrand(
        inval: unknown,
        items: readonly unknown[],
        weights: readonly number[] | Pattern<readonly number[]>,
        length: number = INF,
    ) {
        const weightLists = asStream(weights);
        try {
            for (let i = 0; i < length; i++) {
                const w = weightLists.next(inval) as readonly number[];
                inval = yield* embed(rand.wchoose(items, w), inval);
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(
    items: ListItems<T>,
    weights: readonly number[] | Pattern<readonly number[]>,
    length?: number,
) => ListPattern<T>;

/** The items in one random order, the same order `repeats` times. */
export const pshuffle = pattern(
    function* pshuffle(inval: unknown, items: readonly unknown[], repeats: number = 1) {
        const order = rand.scramble(items);
        for (let r = 0; r < repeats; r++) {
            for (const item of order) inval = yield* embed(item, inval);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, repeats?: number) => ListPattern<T>;

/**
 * `repeats` segments of `length` items, each starting `step` items after the
 * last, from `start`: a window sliding along the list. Past the end it wraps,
 * or with `wrap` false it stops.
 */
export const pslide = pattern(
    function* pslide(
        inval: unknown,
        items: readonly unknown[],
        repeats: number = 1,
        length: number | Pattern<number> = 3,
        step: number | Pattern<number> = 1,
        start: number = 0,
        wrap: boolean = true,
    ) {
        const [lengths, steps] = [asStream(length), asStream(step)];
        const n = items.length;
        let pos = start;
        try {
            for (let r = 0; r < repeats; r++) {
                const len = Math.trunc(lengths.next(inval) as number);
                for (let j = 0; j < len; j++) {
                    let item: unknown;
                    if (wrap) item = items[(((pos + j) % n) + n) % n];
                    else if (pos + j >= 0 && pos + j < n) item = items[pos + j];
                    else return inval;
                    inval = yield* embed(item, inval);
                }
                pos += Math.trunc(steps.next(inval) as number);
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(
    items: ListItems<T>,
    repeats?: number,
    length?: number | Pattern<number>,
    step?: number | Pattern<number>,
    start?: number,
    wrap?: boolean,
) => ListPattern<T>;

/**
 * A walk over the list: from `start`, each item moves the position by the next
 * of `steps` (by default one either way, at random), times the direction. At
 * an end the next of `directions` sets the direction: `1` keeps it and wraps
 * around, and `pseq([1, -1], INF)` bounces.
 */
export const pwalk = pattern(
    function* pwalk(
        inval: unknown,
        items: readonly unknown[],
        steps: number | Pattern<number> | null = null,
        directions: number | Pattern<number> = 1,
        start: number = 0,
    ) {
        const stepStream = asStream(steps ?? prand([-1, 1]));
        const directionStream = asStream(directions);
        const n = items.length;
        let index = start;
        try {
            let direction = directionStream.next(inval) as number;
            for (;;) {
                let step = stepStream.next(inval) as number;
                inval = yield* embed(items[Math.trunc(index)], inval);
                step *= direction;
                if (index + step < 0 || index + step >= n) {
                    direction = directionStream.next(inval) as number;
                    step = Math.abs(step) * (builtins.sign(direction) as number);
                }
                index = (((index + step) % n) + n) % n;
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(
    items: ListItems<T>,
    steps?: number | Pattern<number> | null,
    directions?: number | Pattern<number>,
    start?: number,
) => ListPattern<T>;

/**
 * The items in order, `repeats` times, where an item that is an array gives
 * one of its elements per pass: `place([1, [2, 3]], 4)` is `1 2 1 3 1 2 1 3`
 * -- arrays laced into a line.
 */
export const place = pattern(function* place(
    inval: unknown,
    items: readonly unknown[],
    repeats: number = 1,
    offset: number = 0,
) {
    const n = items.length;
    for (let j = 0; j < repeats; j++) {
        for (let i = 0; i < n; i++) {
            let item = items[(i + offset) % n];
            if (Array.isArray(item)) item = item[j % item.length];
            inval = yield* embed(item, inval);
        }
    }
    return inval;
}) as <T>(items: readonly (T | Pattern<T> | readonly (T | Pattern<T>)[])[], repeats?: number, offset?: number) => Pattern<T>;

/**
 * The items' values interlaced: one value of each pattern in turn, `repeats`
 * rounds; an item that has ended is skipped, and the pattern ends when every
 * one has.
 */
export const placep = pattern(
    function* placep(inval: unknown, items: readonly unknown[], repeats: number = 1, offset: number = 0) {
        const streams = items.map(asStream);
        const n = streams.length;
        let endedCount = 0;
        let index = 0;
        let repeat = 0;
        while (repeat < repeats && endedCount < n) {
            let value: unknown;
            let got = true;
            try {
                value = streams[(offset + index) % n]!.next(inval);
            } catch (error) {
                ended(error);
                got = false;
            }
            if (got) {
                endedCount = 0;
                inval = yield* embed(value, inval);
            } else {
                endedCount += 1;
            }
            index += 1;
            if (index === n) {
                index = 0;
                repeat += 1;
            }
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, repeats?: number, offset?: number) => ListPattern<T>;

/**
 * An array per step of the next value of each item, until any ends; `repeats`
 * times -- the items played side by side as one value.
 */
export const ptuple = pattern(function* ptuple(inval: unknown, items: readonly unknown[], repeats: number = 1) {
    for (let r = 0; r < repeats; r++) {
        const streams = items.map(asStream);
        try {
            for (;;) inval = yield streams.map((s) => s.next(inval));
        } catch (error) {
            ended(error);
        }
    }
    return inval;
}) as <T>(items: ListItems<T>, repeats?: number) => Pattern<T[]>;

/** The item each value of `which` indexes, embedded whole (wrapping past the end). */
export const pswitch = pattern(
    function* pswitch(inval: unknown, items: readonly unknown[], which: number | Pattern<number> = 0) {
        const indices = asStream(which);
        const n = items.length;
        try {
            for (;;) {
                const i = Math.trunc(indices.next(inval) as number);
                inval = yield* embed(items[((i % n) + n) % n], inval);
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: ListItems<T>, which?: number | Pattern<number>) => ListPattern<T>;

/**
 * One value of the item each value of `which` indexes: the items are walked
 * side by side, and `which` says which one speaks.
 */
export const pswitch1 = pattern(function* pswitch1(
    inval: unknown,
    items: readonly unknown[],
    which: number | Pattern<number> = 0,
) {
    const streams = items.map(asStream);
    const indices = asStream(which);
    const n = streams.length;
    try {
        for (;;) {
            const i = Math.trunc(indices.next(inval) as number);
            inval = yield streams[((i % n) + n) % n]!.next(inval);
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as <T>(items: ListItems<T>, which?: number | Pattern<number>) => Pattern<T>;

/**
 * A finite state machine: `states` is the array of entry states, then for each
 * state its item and the states it may go to next, `[entry, item0, next0,
 * item1, next1, ...]`; each step goes to one of the next states at random and
 * embeds its item. A state whose item is `null` ends a pass.
 */
export const pfsm = pattern(function* pfsm(inval: unknown, states: readonly unknown[], repeats: number = 1) {
    const last = Math.floor((states.length - 1) / 2) - 1;
    for (let r = 0; r < repeats; r++) {
        let index = 0;
        for (;;) {
            const next = builtins.clip(rand.choose(states[index] as readonly number[]), 0, last) as number;
            index = Math.trunc(next) * 2 + 2;
            const item = states[index - 1];
            if (item === null || item === undefined) break;
            inval = yield* embed(item, inval);
        }
    }
    return inval;
}) as <T>(states: readonly unknown[], repeats?: number) => Pattern<T>;

/** What a state of {@link pdfsm} maps a signal to: the next state and what to embed. */
export type DfsmState = Record<string | number, readonly [number | null, unknown]>;

/**
 * A deterministic finite state machine: `states[0]` is a pattern of signals,
 * and each state after it maps a signal (or `"default"`) to the next state and
 * what to embed, `{ [signal]: [nextState, item] }`. It ends when the signals do
 * or a next state is `null` or past the last.
 */
export const pdfsm = pattern(function* pdfsm(
    inval: unknown,
    states: readonly unknown[],
    startState: number = 0,
    repeats: number = 1,
) {
    const count = states.length - 1;
    for (let r = 0; r < repeats; r++) {
        let current: number | null = startState;
        const signals = asStream(states[0]);
        try {
            for (;;) {
                const signal = signals.next(inval) as string | number;
                const state = states[(current as number) + 1] as DfsmState;
                const [nextState, item]: readonly [number | null, unknown] =
                    signal in state ? state[signal]! : state["default"]!;
                current = nextState;
                if (current === null || current >= count) break;
                inval = yield* embed(item, inval);
            }
        } catch (error) {
            ended(error);
        }
    }
    return inval;
}) as <T>(states: readonly unknown[], startState?: number, repeats?: number) => Pattern<T>;
