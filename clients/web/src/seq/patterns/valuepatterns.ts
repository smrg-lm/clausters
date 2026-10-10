// Value patterns computed from their arguments: a constant, a series (mirrors
// `clausters/seq/patterns/valuepatterns.py`).
//
// Each is a value pattern: its stream yields values, `length` of them (endless
// by default).

import { INF, Pattern, pattern } from "../pattern.ts";

/** `value`, `length` times (endless by default). */
export const pconst = pattern(function* pconst<T>(inval: unknown, value: T, length: number = INF) {
    for (let i = 0; i < length; i++) inval = yield value;
    return inval;
}) as <T>(value: T, length?: number) => Pattern<T>;

/** `value` if it is a pattern, else an endless {@link pconst} of it. */
export const asPattern = <T>(value: T | Pattern<T>): Pattern<T> =>
    value instanceof Pattern ? value : pconst(value);

/** The arithmetic series `start, start + step, ...` (`length` values). */
export const pseries = pattern(function* pseries(
    inval: unknown,
    start: number = 0,
    step: number = 1,
    length: number = INF,
) {
    let value = start;
    for (let i = 0; i < length; i++) {
        inval = yield value;
        value += step;
    }
    return inval;
});

/** The geometric series `start, start * grow, ...` (`length` values). */
export const pgeom = pattern(function* pgeom(
    inval: unknown,
    start: number = 1,
    grow: number = 2,
    length: number = INF,
) {
    let value = start;
    for (let i = 0; i < length; i++) {
        inval = yield value;
        value *= grow;
    }
    return inval;
});
