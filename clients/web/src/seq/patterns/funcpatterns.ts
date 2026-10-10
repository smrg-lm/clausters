// Patterns whose values a function computes when the walk asks for them
// (mirrors `clausters/seq/patterns/funcpatterns.py`).
//
// A function given to one is called as late as possible -- when the value is
// needed -- and with the incoming value when it declares an argument (the
// event a `pbind` is building, when the pattern is one of its keys).

import { StopStream } from "../../base/stream.ts";
import { INF, asStream, embed, pattern } from "../pattern.ts";
import type { Pattern, PatternGenerator } from "../pattern.ts";

/** A function of the incoming value, or of nothing. */
export type ValueFunc<T> = (inval?: unknown) => T;

const call = <T>(func: ValueFunc<T>, inval: unknown): T => (func.length >= 1 ? func(inval) : func());

/**
 * `func()` for each value, `length` times: a value computed at the moment it is
 * needed (`func(inval)` when it takes the incoming value).
 */
export const pfunc = pattern(function* pfunc(inval: unknown, func: ValueFunc<unknown>, length: number = INF) {
    for (let i = 0; i < length; i++) inval = yield call(func, inval);
    return inval;
}) as <T>(func: ValueFunc<T>, length?: number) => Pattern<T>;

/**
 * `func`'s value `repeats` times, each embedded -- a function that returns a
 * pattern plays it (sclang's `Pfuncn`, once by default).
 */
export const pfuncn = pattern(function* pfuncn(inval: unknown, func: ValueFunc<unknown>, repeats: number = 1) {
    for (let i = 0; i < repeats; i++) inval = yield* embed(call(func, inval), inval);
    return inval;
}) as <T>(func: ValueFunc<T | Pattern<T>>, repeats?: number) => Pattern<T>;

/**
 * A generator function as a pattern: its `yield`s are the values, and each
 * `yield` receives the next incoming value. `func` takes the first incoming
 * value when it declares a parameter -- a routine's body, walked as a pattern.
 */
export const prout = pattern(function* prout(
    inval: unknown,
    func: (inval?: unknown) => Generator<unknown, unknown, unknown>,
) {
    const gen = func.length >= 1 ? func(inval) : func();
    let step = gen.next();
    while (!step.done) {
        inval = yield step.value;
        step = gen.next(inval);
    }
    return step.value ?? inval;
}) as <T>(func: (inval?: unknown) => Generator<T, unknown, unknown>) => Pattern<T>;

/**
 * The pattern `func` returns, built when the walk starts and embedded: a
 * pattern decided at the moment it is played.
 */
export const plazy = pattern(function* plazy(inval: unknown, func: ValueFunc<unknown>) {
    return yield* embed(call(func, inval), inval);
}) as <T>(func: ValueFunc<T | Pattern<T>>) => Pattern<T>;

/**
 * The next value of `iftrue` where `condition` is true, of `iffalse` where it
 * is false; it ends when the condition does or the side it asks does.
 */
export const pif = pattern(function* pif(
    inval: unknown,
    condition: unknown,
    iftrue: unknown,
    iffalse: unknown = null,
) {
    const tests = asStream(condition);
    const [yes, no] = [asStream(iftrue), asStream(iffalse)];
    try {
        for (;;) {
            const side = tests.next(inval) ? yes : no;
            inval = yield side.next(inval);
        }
    } catch (error) {
        if (!(error instanceof StopStream)) throw error;
    }
    return inval;
}) as <T>(condition: boolean | Pattern<boolean>, iftrue: T | Pattern<T>, iffalse?: T | Pattern<T>) => Pattern<T>;

/**
 * Every combination of the values of `patterns`, the last one changing fastest
 * -- nested loops, one per pattern -- each as `func` of the array of values
 * (the array itself when `func` is `null`).
 */
export const pproduct = pattern(function* pproduct(
    inval: unknown,
    func: ((values: unknown[]) => unknown) | null,
    patterns: readonly unknown[],
) {
    const values: unknown[] = new Array(patterns.length).fill(null);
    function* level(i: number, inval: unknown): PatternGenerator<unknown> {
        const stream = asStream(patterns[i]);
        try {
            for (;;) {
                values[i] = stream.next(inval);
                if (i < patterns.length - 1) inval = yield* level(i + 1, inval);
                else inval = yield func === null ? [...values] : func([...values]);
            }
        } catch (error) {
            if (!(error instanceof StopStream)) throw error;
        }
        return inval;
    }
    if (patterns.length === 0) return inval;
    return yield* level(0, inval);
}) as <T, U = T[]>(func: ((values: T[]) => U) | null, patterns: readonly (T | Pattern<T>)[]) => Pattern<U>;
