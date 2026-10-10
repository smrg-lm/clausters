// Patterns made of another pattern (mirrors
// `clausters/seq/patterns/filterpatterns.py`).
//
// A filter pattern is the kind of the pattern it filters: over an event pattern
// it is an event pattern, and plays.

import type { Event } from "../event.ts";
import { EventPattern, INF, embed, pattern } from "../pattern.ts";
import type { Pattern } from "../pattern.ts";

/** A filter pattern's type: an event pattern over one, else a value pattern. */
export type FilterPattern<T> = [T] extends [Event] ? EventPattern<T> : Pattern<T>;

/** `source` from start to end, `repeats` times. */
export const pn = pattern(
    function* pn(inval: unknown, source: unknown, repeats: number = INF) {
        for (let i = 0; i < repeats; i++) inval = yield* embed(source, inval);
        return inval;
    },
    { events: (source: unknown) => source instanceof EventPattern },
) as <T>(source: T | Pattern<T>, repeats?: number) => FilterPattern<T>;
