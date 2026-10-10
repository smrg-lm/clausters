// Value patterns of time (mirrors `clausters/seq/patterns/timepatterns.py`):
// what a pattern yields depends on the beat the walk is at, the logical beat
// of the routine playing it.
//
// They read the clock, so they have meaning only where a clock moves -- as
// keys of an event pattern that plays, or in a routine. Walked by a plain loop
// the beat does not move: `ptime` stays at 0, and `pstep` and `pseg` hold their
// first value, endlessly.

import { Moment } from "../../base/moment.ts";
import { StopStream } from "../../base/stream.ts";
import { shapeValue } from "../../core/clausters_core_web.js";
import { resolveCurve } from "../../defs/ugens/env.ts";
import type { Curve } from "../../defs/ugens/env.ts";
import { INF, Pattern, asStream, embed, pattern } from "../pattern.ts";
import { pseq } from "./listpatterns.ts";

const beat = (): number => Moment.current().beat;

/** An array given where a pattern is expected is a `pseq` of it. */
const listed = (value: unknown): unknown =>
    Array.isArray(value) ? pseq(value) : value;

const ended = (error: unknown): void => {
    if (!(error instanceof StopStream)) throw error;
};

/** The beats since the walk started, at each step. */
export const ptime = pattern(function* ptime(inval: unknown, length: number = INF) {
    const start = beat();
    for (let i = 0; i < length; i++) inval = yield beat() - start;
    return inval;
}) as (length?: number) => Pattern<number>;

/**
 * A step function of time: each of `levels` held for its duration in `durs`
 * (beats), whatever is asked of it in between -- a value that changes on a grid
 * of its own, not the events'. `levels` and `durs` may be arrays or patterns.
 */
export const pstep = pattern(function* pstep(
    inval: unknown,
    levels: unknown,
    durs: unknown = 1,
    repeats: number = 1,
) {
    let end = beat();
    for (let r = 0; r < repeats; r++) {
        const [levelStream, durStream] = [asStream(listed(levels)), asStream(listed(durs))];
        try {
            for (;;) {
                const level = levelStream.next(inval);
                end += durStream.next(inval) as number;
                while (end > beat()) inval = yield* embed(level, inval);
            }
        } catch (error) {
            ended(error);
        }
    }
    return inval;
}) as <T>(levels: readonly (T | Pattern<T>)[] | Pattern<T>, durs?: number | readonly number[] | Pattern<number>, repeats?: number) => Pattern<T>;

/**
 * A curve of time: from each of `levels` to the next over its duration in
 * `durs` (beats), by the shape in `curves` (`"lin"`, `"exp"`, a curvature ...)
 * -- what an envelope draws, read at the beat it is asked. `levels`, `durs`
 * and `curves` may be arrays or patterns.
 */
export const pseg = pattern(function* pseg(
    inval: unknown,
    levels: unknown,
    durs: unknown = 1,
    curves: unknown = "lin",
    repeats: number = 1,
) {
    for (let r = 0; r < repeats; r++) {
        const levelStream = asStream(listed(levels));
        const durStream = asStream(listed(durs));
        const curveStream = asStream(listed(curves));
        try {
            let level = levelStream.next(inval) as number;
            let end = beat();
            for (;;) {
                const startLevel = level;
                level = levelStream.next(inval) as number;
                const dur = durStream.next(inval) as number;
                const [shape, curvature] = resolveCurve(curveStream.next(inval) as Curve);
                const start = end;
                end = end + dur;
                let now = beat();
                while (end > now) {
                    inval = yield shapeValue(shape, curvature, startLevel, level, (now - start) / dur);
                    now = beat();
                }
            }
        } catch (error) {
            ended(error);
        }
    }
    return inval;
}) as (
    levels: readonly number[] | Pattern<number>,
    durs?: number | readonly number[] | Pattern<number>,
    curves?: Curve | readonly Curve[] | Pattern<Curve>,
    repeats?: number,
) => Pattern<number>;
