// Patterns that read a list (mirrors `clausters/seq/patterns/listpatterns.py`).
//
// An item may itself be a pattern, which is **embedded**: all its values come
// out in its place before the next item. A list pattern whose every item is an
// event pattern is an event pattern, and plays (`pseq([pbind(...),
// pbind(...)])` is a phrase of phrases); otherwise it is a value pattern.

import * as rand from "../../base/rand.ts";
import type { Event } from "../event.ts";
import { INF, allEvents, embed, pattern } from "../pattern.ts";
import type { EventPattern, Pattern } from "../pattern.ts";

/** A list pattern's type: an event pattern over events, else a value pattern. */
export type ListPattern<T> = [T] extends [Event] ? EventPattern<T> : Pattern<T>;

const overEvents = (items: readonly unknown[]): boolean => allEvents(items);

/** The items in order, `repeats` times. */
export const pseq = pattern(
    function* pseq(inval: unknown, items: readonly unknown[], repeats: number = 1) {
        for (let i = 0; i < repeats; i++) {
            for (const item of items) inval = yield* embed(item, inval);
        }
        return inval;
    },
    { events: overEvents },
) as <T>(items: readonly (T | Pattern<T>)[], repeats?: number) => ListPattern<T>;

/**
 * The items in order, cycling, until exactly `length` values have come out (an
 * item is not embedded: each counts as one value).
 */
export const pser = pattern(function* pser(
    inval: unknown,
    items: readonly unknown[],
    length: number,
) {
    for (let i = 0; i < length; i++) inval = yield items[i % items.length];
    return inval;
}) as <T>(items: readonly T[], length: number) => Pattern<T>;

/**
 * A random item, `length` times, drawn from the random context (see
 * `randompatterns.ts`).
 */
export const prand = pattern(
    function* prand(inval: unknown, items: readonly unknown[], length: number = INF) {
        for (let i = 0; i < length; i++) inval = yield* embed(rand.choose(items), inval);
        return inval;
    },
    { events: overEvents },
) as <T>(items: readonly (T | Pattern<T>)[], length?: number) => ListPattern<T>;
