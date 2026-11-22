// The `demand` builders that mirror the wire, re-exported by `demand.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/demand.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * An arithmetic sequence: `start`, `start + step`, ... The step is read
 * on every item, so it may itself be a stream.
 */
export const dseries = (
    repeats: Channel = 0.0,
    start: Channel = 0.0,
    step: Channel = 1.0,
): Ugen => new Ugen("Dseries", [repeats, start, step], { rate: "dr" });

/** A geometric sequence: `start`, `start * grow`, ... */
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
 * A random walk of at most `step` per item, **folded** into
 * `[lo, hi]` -- it turns around at a bound rather than piling up against
 * it.
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
 * Repeats each item of the `value` stream `repeats` times. The count
 * is pulled per item, so it may vary.
 */
export const dstutter = (repeats: Channel, value: Channel): Ugen =>
    new Ugen("Dstutter", [repeats, value], { rate: "dr" });

/**
 * Repeats each item of the `value` stream `repeats` times: sclang's
 * newer name for `dstutter`, and the same stream.
 */
export const ddup = (repeats: Channel, value: Channel): Ugen =>
    new Ugen("Ddup", [repeats, value], { rate: "dr" });

/**
 * Yields the `signal` stream until its items add up to `sum`, the
 * last one cut to land on it -- durations that fill a bar exactly. Within
 * `tolerance` of the sum counts as there.
 */
export const dconst = (sum: Channel, signal: Channel, tolerance: Channel = 0.001): Ugen =>
    new Ugen("Dconst", [sum, signal, tolerance], { rate: "dr" });

/**
 * Passes the `signal` stream through, and restarts it when the `reset`
 * stream rises. The restart shows on the next item.
 */
export const dreset = (signal: Channel, reset: Channel = 0.0): Ugen =>
    new Ugen("Dreset", [signal, reset], { rate: "dr" });

/**
 * Reads the buffer frame the `phase` stream names -- a `dseries` phase
 * walks it as a step sequence. Out of range it wraps when `loop` is set and
 * clamps when it is not.
 */
export const dbufrd = (
    bufnum: Channel,
    phase: Channel,
    loop: Channel = 1.0,
    channel: Channel = 0.0,
): Ugen => new Ugen("Dbufrd", [bufnum, phase, loop, channel], { rate: "dr" });

/**
 * Writes each item of the `signal` stream into the buffer frame the
 * `phase` stream names, and yields it -- the writing twin of `dbufrd`. Out of
 * range the frame wraps when `loop` is set and clamps when it is not.
 */
export const dbufwr = (
    bufnum: Channel,
    phase: Channel,
    signal: Channel,
    loop: Channel = 1.0,
    channel: Channel = 0.0,
): Ugen => new Ugen("Dbufwr", [bufnum, phase, signal, loop, channel], { rate: "dr" });

/**
 * Demand driver: pulls the next value from `source` on each rising edge
 * of `trig` and holds it between triggers; a rising `reset` restarts the
 * stream. Once the stream ends the last value is held.
 */
export const demand = (trig: Channel, reset: Channel, source: Channel): Ugen =>
    new Ugen("Demand", [trig, reset, source]);

/**
 * Demand driver with a clock of its own: pulls one `level` every
 * `dur` seconds and holds it.
 *
 * Both `dur` and `level` are pulled, which is what makes a sequencer of
 * it -- a stream of durations against a stream of pitches, the two free to be
 * different lengths. When either ends, `doneAction` fires (see
 * `DoneAction`).
 */
export const duty = (
    dur: Channel,
    reset: Channel = 0.0,
    level: Channel = 1.0,
    doneAction: Channel = 0.0,
): Ugen => new Ugen("Duty", [dur, reset, level, doneAction]);

/**
 * `duty` emitting each level on its own sample and nothing in between -- a
 * trigger stream whose amplitudes are the levels. With `gapFirst` the
 * first duration is spent before the first level, so the stream opens with a
 * gap instead of a trigger.
 */
export const tduty = (
    dur: Channel,
    reset: Channel = 0.0,
    level: Channel = 1.0,
    doneAction: Channel = 0.0,
    gapFirst: Channel = 0.0,
): Ugen => new Ugen("TDuty", [dur, reset, level, doneAction, gapFirst]);

/**
 * An envelope whose breakpoints are streams: each segment pulls its
 * duration, shape, curvature and end level, and glides from wherever the
 * output is. `shape` and `curve` are `envGen`'s.
 *
 * A `gate` at or above one runs; between zero and one releases at the end
 * of the segment playing (holding there, and firing `doneAction`, until the
 * gate is back at one); at or below zero holds. A rising `reset` restarts the
 * streams: up to one it glides on from the current level, above one it jumps
 * to the first. When the duration or the level stream ends, the envelope holds
 * its last level and `doneAction` fires.
 */
export const demandEnvGen = (
    level: Channel,
    dur: Channel,
    shape: Channel = 1.0,
    curve: Channel = 0.0,
    gate: Channel = 1.0,
    reset: Channel = 1.0,
    levelScale: Channel = 1.0,
    levelBias: Channel = 0.0,
    timeScale: Channel = 1.0,
    doneAction: Channel = 0.0,
): Ugen => new Ugen("DemandEnvGen", [level, dur, shape, curve, gate, reset, levelScale, levelBias, timeScale, doneAction]);
