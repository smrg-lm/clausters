// Patterns whose values a function computes when the walk asks for them
// (mirrors `clausters/seq/patterns/funcpatterns.py`).

import { INF, pattern } from "../pattern.ts";
import type { Pattern } from "../pattern.ts";

/**
 * `func()` for each value, `length` times: a value computed at the moment it is
 * needed.
 */
export const pfunc = pattern(function* pfunc(
    inval: unknown,
    func: () => unknown,
    length: number = INF,
) {
    for (let i = 0; i < length; i++) inval = yield func();
    return inval;
}) as <T>(func: () => T, length?: number) => Pattern<T>;
