// Demand rate: streams that have a next value, not samples (mirrors
// `clausters/defs/ugens/demand.py`).
//
// A demand UGen is pulled rather than run — `demand` (or `duty`) asks for the
// next value on a trigger and the stream advances only then — and nesting them
// is how a sequence is built.

import { ChannelList, Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";
import { sources } from "./pan.ts";

// A demand UGen is a *stream*: it has no samples, only a next value, and it
// yields one each time a driver asks. Its inputs may be streams too — that is
// what makes a sequence of phrases, rather than of numbers, expressible — so
// every builder below accepts another `d*` anywhere it accepts a number.
//
// `repeats` is how many the stream yields before it ends: **0 means
// endlessly**. sclang writes `inf` there, which a def cannot carry (the wire
// rejects a non-finite constant, and JSON has no spelling for one), so the
// count of none is the endless one. For a list source it counts *passes over
// the list*; for a random pick it counts *items* — scsynth's own asymmetry,
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
 * `drand` that never picks the value it just used — the same list without
 * immediate repetition.
 */
export const dxrand = (
    values: ChannelList | readonly Channel[],
    repeats: Channel = 0.0,
): Ugen => new Ugen("Dxrand", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * `values` shuffled **once** and then replayed in that order, `repeats`
 * times. The shuffle is redrawn on a reset, not on each pass — that is what
 * separates it from `drand`.
 */
export const dshuf = (
    values: ChannelList | readonly Channel[],
    repeats: Channel = 0.0,
): Ugen => new Ugen("Dshuf", [repeats, ...demandValues(values)], { rate: "dr" });

/**
 * An arithmetic sequence: `start`, `start + step`, … The step is read on
 * every item, so it may itself be a stream.
 */
export const dseries = (
    repeats: Channel = 0.0,
    start: Channel = 0.0,
    step: Channel = 1.0,
): Ugen => new Ugen("Dseries", [repeats, start, step], { rate: "dr" });

/** A geometric sequence: `start`, `start * grow`, … */
export const dgeom = (
    repeats: Channel = 0.0,
    start: Channel = 1.0,
    grow: Channel = 2.0,
): Ugen => new Ugen("Dgeom", [repeats, start, grow], { rate: "dr" });

/** Independent uniform draws on `[lo, hi]`. */
export const dwhite = (
    repeats: Channel = 0.0,
    lo: Channel = 0.0,
    hi: Channel = 1.0,
): Ugen => new Ugen("Dwhite", [repeats, lo, hi], { rate: "dr" });

/** `dwhite` over the integers in `[lo, hi]`, both ends included. */
export const diwhite = (
    repeats: Channel = 0.0,
    lo: Channel = 0.0,
    hi: Channel = 1.0,
): Ugen => new Ugen("Diwhite", [repeats, lo, hi], { rate: "dr" });

/**
 * A random walk of at most `step` per item, **folded** into `[lo, hi]` — it
 * turns around at a bound rather than piling up against it.
 */
export const dbrown = (
    repeats: Channel = 0.0,
    lo: Channel = 0.0,
    hi: Channel = 1.0,
    step: Channel = 0.01,
): Ugen => new Ugen("Dbrown", [repeats, lo, hi, step], { rate: "dr" });

/** `dbrown` over the integers. */
export const dibrown = (
    repeats: Channel = 0.0,
    lo: Channel = 0.0,
    hi: Channel = 1.0,
    step: Channel = 1.0,
): Ugen => new Ugen("Dibrown", [repeats, lo, hi, step], { rate: "dr" });

/**
 * Repeats each item of the `value` stream `repeats` times. The count is
 * pulled per item, so it may vary.
 */
export const dstutter = (repeats: Channel, value: Channel): Ugen =>
    new Ugen("Dstutter", [repeats, value], { rate: "dr" });

/**
 * Takes **one** item from the stream `which` picks, then picks again.
 *
 * Unlike `dseq`, an unselected stream is not advanced and the selected one is
 * not drained — the `1` is the count. The index wraps into range. Accepts the
 * sources as arguments or as one list.
 */
export const dswitch1 = (
    which: Channel,
    ...items: readonly Channel[] | [ChannelList | readonly Channel[]]
): Ugen => new Ugen("Dswitch1", [which, ...sources(items)], { rate: "dr" });

/**
 * Reads the buffer frame the `phase` stream names — a `dseries` phase walks
 * it as a step sequence. Out of range it wraps when `loop` is set and clamps
 * when it is not.
 */
export const dbufrd = (
    bufnum: Channel,
    phase: Channel,
    loop: Channel = 1.0,
    channel: Channel = 0.0,
): Ugen => new Ugen("Dbufrd", [bufnum, phase, loop, channel], { rate: "dr" });

/**
 * Demand driver: pulls the next value from `source` on each rising edge of
 * `trigger` and holds it between triggers; a rising `reset` restarts the
 * stream. Once the stream ends the last value is held.
 */
export const demand = (
    trig: Channel,
    reset: Channel,
    source: Channel,
): Ugen => new Ugen("Demand", [trig, reset, source]);

/**
 * Demand driver with a clock of its own: pulls one `level` every `dur`
 * seconds and holds it.
 *
 * Both `dur` and `level` are pulled, which is what makes a sequencer of it —
 * a stream of durations against a stream of pitches, the two free to be
 * different lengths. When either ends, `doneAction` fires (see `DoneAction`).
 */
export const duty = (
    dur: Channel,
    reset: Channel = 0.0,
    level: Channel = 1.0,
    doneAction: Channel = 0,
): Ugen => new Ugen("Duty", [dur, reset, level, doneAction]);

/**
 * `duty` emitting each level on its own sample and nothing in between — a
 * trigger stream whose amplitudes are the levels. With `gapFirst` the first
 * duration is spent before the first level, so the stream opens with a gap
 * instead of a trigger.
 */
export const tduty = (
    dur: Channel,
    reset: Channel = 0.0,
    level: Channel = 1.0,
    doneAction: Channel = 0,
    gapFirst: Channel = 0.0,
): Ugen => new Ugen("TDuty", [dur, reset, level, doneAction, gapFirst]);
