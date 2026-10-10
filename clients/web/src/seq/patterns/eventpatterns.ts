// Event patterns: the patterns whose stream yields `Event`s (mirrors
// `clausters/seq/patterns/eventpatterns.py`).
//
// An event pattern is what plays (`EventPattern.play`) and what renders into an
// `EventSequence`. Each value pattern given to one is read once per event, and
// receives the event being built as its incoming value.

import { StopStream } from "../../base/stream.ts";
import { Event } from "../event.ts";
import { asStream, pattern } from "../pattern.ts";
import type { EventPattern } from "../pattern.ts";

/**
 * The keys a `pbind` binds: each one a pattern, or a constant held for every
 * event.
 */
export type Bindings = Record<string, unknown>;

/**
 * **Binds keys to value patterns**: an `Event` per step, each key's value the
 * next of its pattern, ending when any key's pattern ends. A bare value is held
 * for every event.
 */
export const pbind = pattern(
    function* pbind(inval: unknown, bindings: Bindings) {
        const streams = Object.entries(bindings).map(([key, value]) => [key, asStream(value)] as const);
        for (;;) {
            const props: Record<string, unknown> =
                inval instanceof Event ? { ...inval.props }
                    : inval !== null && typeof inval === "object" ? { ...(inval as object) } : {};
            try {
                for (const [key, stream] of streams) props[key] = stream.next(props);
            } catch (error) {
                if (error instanceof StopStream) return inval;
                throw error;
            }
            inval = yield new Event(props);
        }
    },
    { events: true },
) as (bindings: Bindings) => EventPattern<Event>;
