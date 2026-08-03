// Demand rate: streams that have a next value, not samples (mirrors
// `clausters/defs/ugens/demand.py`).
//
// A demand UGen is pulled rather than run — `demand` asks for the next value
// on a trigger and the stream advances only then — and nesting them is how a
// sequence is built.

import { ChannelList, Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

// `repeats` is how many values the stream yields before it ends: **0 means
// endlessly** (sclang writes `inf`, which a def cannot carry).

function demandValues(values: ChannelList | readonly Channel[]): Channel[] {
    const items = values instanceof ChannelList ? [...values.items] : [...values];
    if (items.length === 0) {
        throw new TypeError("a demand source needs at least one value");
    }
    return items;
}

/**
 * A demand sequence: yields `values` in order, `repeats` times (`0`
 * endlessly), then ends. A value may be another demand stream, and then it
 * is *drained* rather than taken once.
 */
export const dseq = (
    values: ChannelList | readonly Channel[],
    repeats: Channel = 0.0,
): Ugen => new Ugen("Dseq", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * `repeats` items drawn at random from `values`, each pick independent of
 * the last. Unlike `dseq`, the count is of items, not passes.
 */
export const drand = (
    values: ChannelList | readonly Channel[],
    repeats: Channel = 0.0,
): Ugen => new Ugen("Drand", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * Demand driver: pulls the next value from `source` on each rising edge of
 * `trigger` and holds it between triggers; a rising `reset` restarts the
 * stream. Once the stream ends the last value is held.
 */
export const demand = (
    trigger: Channel,
    reset: Channel,
    source: Channel,
): Ugen => new Ugen("Demand", [trigger, reset, source]);
