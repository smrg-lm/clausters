// Demand rate: streams that have a next value, not samples (mirrors
// `clausters/defs/ugens/demand.py`).
//
// A demand UGen is pulled rather than run -- `demand` (or `duty`) asks for the
// next value on a trigger and the stream advances only then -- and nesting them
// is how a sequence is built.

import { ChannelList, Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";
import { sources } from "./pan.ts";
export * from "./_generated/demand.ts";

// A demand UGen is a *stream*: it has no samples, only a next value, and it
// yields one each time a driver asks. Its inputs may be streams too -- that is
// what makes a sequence of phrases, rather than of numbers, expressible -- so
// every builder below accepts another `d*` anywhere it accepts a number.
//
// `repeats` is how many the stream yields before it ends: **0 means
// endlessly**. sclang writes `inf` there, which a def cannot carry (the wire
// rejects a non-finite constant, and JSON has no spelling for one), so the
// count of none is the endless one. For a list source it counts *passes over
// the list*; for a random pick it counts *items* -- scsynth's own asymmetry,
// and the useful reading of each.

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
 * `drand` that never picks the value it just used -- the same list without
 * immediate repetition.
 */
export const dxrand = (
    values: ChannelList | readonly Channel[],
    repeats: Channel = 0.0,
): Ugen => new Ugen("Dxrand", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * `values` shuffled **once** and then replayed in that order, `repeats`
 * times. The shuffle is redrawn on a reset, not on each pass -- that is what
 * separates it from `drand`.
 */
export const dshuf = (
    values: ChannelList | readonly Channel[],
    repeats: Channel = 0.0,
): Ugen => new Ugen("Dshuf", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * Takes **one** item from the stream `which` picks, then picks again.
 *
 * Unlike `dseq`, an unselected stream is not advanced and the selected one is
 * not drained -- the `1` is the count. The index wraps into range. Accepts the
 * sources as arguments or as one list.
 */
export const dswitch1 = (
    which: Channel,
    ...items: readonly Channel[] | [ChannelList | readonly Channel[]]
): Ugen => new Ugen("Dswitch1", [which, ...sources(items)], { rate: "dr" });

