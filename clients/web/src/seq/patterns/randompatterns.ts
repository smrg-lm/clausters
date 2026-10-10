// Value patterns of random values (mirrors
// `clausters/seq/patterns/randompatterns.py`).
//
// Every draw comes from the **random context** (the running routine's stream,
// or the root outside one -- see `base/rand.ts`): `seed(n)` reproduces them
// along with everything else in the script. There is no per-pattern seed --
// independent seeds would break whole-script consistency. Each yields `length`
// values (endless by default), and a bound or a parameter may itself be a
// pattern, read one value per step. The distributions are the core's, drawn by
// the names `base/rand` gives them.

import * as builtins from "../../base/builtins.ts";
import * as rand from "../../base/rand.ts";
import { StopStream } from "../../base/stream.ts";
import { INF, asStream, pattern } from "../pattern.ts";
import type { NumArg } from "./valuepatterns.ts";

/** `draw` of the next value of each argument, `length` times. */
function* draws(
    inval: unknown,
    length: number,
    draw: (...values: number[]) => number,
    ...args: NumArg[]
): Generator<number, unknown, unknown> {
    const streams = args.map(asStream);
    try {
        for (let i = 0; i < length; i++) {
            const values = streams.map((s) => s.next(inval) as number);
            inval = yield draw(...values);
        }
    } catch (error) {
        if (!(error instanceof StopStream)) throw error;
    }
    return inval;
}

/** Uniform random numbers in `[lo, hi)`. */
export const pwhite = pattern(function* pwhite(
    inval: unknown, lo: NumArg = 0, hi: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.rrand, lo, hi);
});

/**
 * Exponentially distributed between `lo` and `hi` (same sign, nonzero): equal
 * chances per octave, the distribution a frequency or a duration wants.
 */
export const pexprand = pattern(function* pexprand(
    inval: unknown, lo: NumArg = 0.0001, hi: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.exprand, lo, hi);
});

/** Between `lo` and `hi`, more likely toward `lo`: the lower of two uniform draws. */
export const plprand = pattern(function* plprand(
    inval: unknown, lo: NumArg = 0, hi: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.lprand, lo, hi);
});

/** Between `lo` and `hi`, more likely toward `hi`: the higher of two uniform draws. */
export const phprand = pattern(function* phprand(
    inval: unknown, lo: NumArg = 0, hi: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.hprand, lo, hi);
});

/** Between `lo` and `hi`, more likely toward the middle: the mean of two uniform draws. */
export const pmeanrand = pattern(function* pmeanrand(
    inval: unknown, lo: NumArg = 0, hi: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.meanrand, lo, hi);
});

/**
 * The beta distribution between `lo` and `hi`: `prob1` and `prob2` weigh the
 * two ends -- below 1 a value gathers at that end, above 1 it leaves it, and
 * both 1 is uniform.
 */
export const pbeta = pattern(function* pbeta(
    inval: unknown,
    lo: NumArg = 0,
    hi: NumArg = 1,
    prob1: NumArg = 1,
    prob2: NumArg = 1,
    length: number = INF,
) {
    const draw = (lo: number, hi: number, p1: number, p2: number): number =>
        rand.betarand(p1, p2) * (hi - lo) + lo;
    return yield* draws(inval, length, draw, lo, hi, prob1, prob2);
});

/** Cauchy around `mean`: mostly near it, `spread` wide, with tails that now and then reach far. */
export const pcauchy = pattern(function* pcauchy(
    inval: unknown, mean: NumArg = 0, spread: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.cauchy, mean, spread);
});

/** Gaussian around `mean` with standard deviation `dev`. */
export const pgauss = pattern(function* pgauss(
    inval: unknown, mean: NumArg = 0, dev: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.gauss, mean, dev);
});

/**
 * A whole number of events, `mean` of them on average (Poisson): how many
 * notes in a beat, how many voices enter.
 */
export const ppoisson = pattern(function* ppoisson(
    inval: unknown, mean: NumArg = 1, length: number = INF,
) {
    return yield* draws(inval, length, rand.poisson, mean);
});

function* walk(
    inval: unknown,
    lo: NumArg,
    hi: NumArg,
    step: NumArg,
    length: number,
    move: (value: number, step: number) => number,
): Generator<number, unknown, unknown> {
    const [los, his, steps] = [asStream(lo), asStream(hi), asStream(step)];
    try {
        let [l, h] = [los.next(inval) as number, his.next(inval) as number];
        steps.next(inval);
        let value = rand.rrand(l, h);
        for (let i = 0; i < length; i++) {
            [l, h] = [los.next(inval) as number, his.next(inval) as number];
            const s = steps.next(inval) as number;
            value = builtins.fold(move(value, s), l, h) as number;
            inval = yield value;
        }
    } catch (error) {
        if (!(error instanceof StopStream)) throw error;
    }
    return inval;
}

/**
 * A random walk between `lo` and `hi`: each value moves from the last by up
 * to `step` either way, folded back at the bounds. It starts at a uniform draw
 * between them.
 */
export const pbrown = pattern(function* pbrown(
    inval: unknown, lo: NumArg = 0, hi: NumArg = 1, step: NumArg = 0.125, length: number = INF,
) {
    return yield* walk(inval, lo, hi, step, length, (v, s) => v + rand.rand2(s));
});

/**
 * {@link pbrown} by factors: each value is the last times up to `1 + step` or
 * `1 - step` -- the walk a frequency takes, even per octave.
 */
export const pgbrown = pattern(function* pgbrown(
    inval: unknown, lo: NumArg = 0, hi: NumArg = 1, step: NumArg = 0.125, length: number = INF,
) {
    return yield* walk(inval, lo, hi, step, length, (v, s) => v * (1 + rand.rand2(s)));
});

/**
 * Values between `lo` and `hi` drawn with `distribution` as their probability
 * shape: a list of relative chances across the range, from `lo` to `hi`, read
 * as a table of `tableSize` points (at least 64).
 */
export const pprob = pattern(function* pprob(
    inval: unknown,
    distribution: readonly number[],
    lo: NumArg = 0,
    hi: NumArg = 1,
    tableSize: number | null = null,
    length: number = INF,
) {
    const table = builtins.asrandomtable(distribution, tableSize ?? Math.max(64, distribution.length));
    const draw = (lo: number, hi: number): number => rand.tablerand(table) * (hi - lo) + lo;
    return yield* draws(inval, length, draw, lo, hi);
});
