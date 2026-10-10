// Value patterns of random values (mirrors
// `clausters/seq/patterns/randompatterns.py`).
//
// Every draw comes from the **random context** (the running routine's stream,
// or the root outside one -- see `base/rand.ts`): `seed(n)` reproduces them
// along with everything else in the script. There is no per-pattern seed --
// independent seeds would break whole-script consistency.

import * as rand from "../../base/rand.ts";
import { INF, pattern } from "../pattern.ts";

/** Uniform random numbers in `[lo, hi)`, `length` values. */
export const pwhite = pattern(function* pwhite(
    inval: unknown,
    lo: number = 0,
    hi: number = 1,
    length: number = INF,
) {
    for (let i = 0; i < length; i++) inval = yield rand.rrand(lo, hi);
    return inval;
});
