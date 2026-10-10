// What an operator composes: a pattern of a pattern's values, operated on
// (mirrors `clausters/seq/patterns/oppatterns.py`).
//
// `pseq([1, 2, 3]).add(60)` is a `pbinop`, `pwhite().sqrt()` a `punop`; they
// are what the operators build and are rarely written by name. Each step takes
// one value from every operand (a bare operand is the same value every time)
// and applies the `base/builtins` function of the selector; the pattern ends
// when any operand ends. They are value patterns: an event pattern refuses an
// operator.

import { opPattern } from "../pattern.ts";
import type { Pattern } from "../pattern.ts";

/** The unary operator `selector` on each value of `a`. */
export const punop = (selector: string, a: unknown): Pattern<number> =>
    opPattern("punop", selector, [a]);

/** The binary operator `selector` on each pair of values of `a` and `b`. */
export const pbinop = (selector: string, a: unknown, b: unknown): Pattern<number> =>
    opPattern("pbinop", selector, [a, b]);

/**
 * The n-ary operator `selector` (a range map, `clip`) on each value of `a` and
 * of each of `args`, which a range map ends with its `clip` mode.
 */
export const pnarop = (selector: string, a: unknown, ...args: unknown[]): Pattern<number> =>
    opPattern("pnarop", selector, [a, ...args]);
