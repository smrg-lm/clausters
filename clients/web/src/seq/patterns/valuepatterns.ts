// Value patterns computed from their arguments: a constant, a series (mirrors
// `clausters/seq/patterns/valuepatterns.py`).
//
// Each is a value pattern: its stream yields values, `length` of them (endless
// by default). An argument may itself be a pattern, read one value per step
// (`pseries(0, pseq([1, 2]))` steps by 1, then 2, then ends) -- the arguments
// sclang reads as streams.

import { StopStream } from "../../base/stream.ts";
import { INF, Pattern, asStream, embed, pattern } from "../pattern.ts";

/** A number, or a pattern of them read one per step. */
export type NumArg = number | Pattern<number>;

/** `value`, `length` times (endless by default). */
export const pconst = pattern(function* pconst<T>(inval: unknown, value: T, length: number = INF) {
    for (let i = 0; i < length; i++) inval = yield value;
    return inval;
}) as <T>(value: T, length?: number) => Pattern<T>;

/** `value` if it is a pattern, else an endless {@link pconst} of it. */
export const asPattern = <T>(value: T | Pattern<T>): Pattern<T> =>
    value instanceof Pattern ? value : pconst(value);

/**
 * `value` once -- every value of it, if it is a pattern: one item in a
 * pattern's place.
 */
export const pvalue = pattern(function* pvalue(inval: unknown, value: unknown) {
    return yield* embed(value, inval);
}) as <T>(value: T | Pattern<T>) => Pattern<T>;

/**
 * Steps a series: `next` reads the step, and `apply` moves the value by it.
 * @internal
 */
function* series(
    inval: unknown,
    start: number,
    step: NumArg,
    length: number,
    apply: (value: number, step: number) => number,
): Generator<number, unknown, unknown> {
    const steps = asStream(step);
    let value = start;
    try {
        for (let i = 0; i < length; i++) {
            const by = steps.next(inval) as number;
            inval = yield value;
            value = apply(value, by);
        }
    } catch (error) {
        if (!(error instanceof StopStream)) throw error;
    }
    return inval;
}

/**
 * The arithmetic series `start, start + step, ...` (`length` values); `step`
 * may be a pattern.
 */
export const pseries = pattern(function* pseries(
    inval: unknown,
    start: number = 0,
    step: NumArg = 1,
    length: number = INF,
) {
    return yield* series(inval, start, step, length, (v, s) => v + s);
});

/**
 * The geometric series `start, start * grow, ...` (`length` values); `grow`
 * may be a pattern.
 */
export const pgeom = pattern(function* pgeom(
    inval: unknown,
    start: number = 1,
    grow: NumArg = 2,
    length: number = INF,
) {
    return yield* series(inval, start, grow, length, (v, g) => v * g);
});
