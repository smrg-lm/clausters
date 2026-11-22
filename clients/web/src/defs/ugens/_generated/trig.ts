// The `trig` builders that mirror the wire, re-exported by `trig.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/trig.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * Holds the **level the input had at the trigger** for `dur` seconds,
 * then 0. Its value carries information, which is what makes it a sampler as
 * well as a gate; use `trig1` when all you want is a 1.
 */
export const trig = (signal: Channel, dur: Channel = 0.1): Ugen =>
    new Ugen("Trig", [signal, dur]);

/**
 * Holds 1 for `dur` seconds after each trigger, whatever level triggered
 * it.
 */
export const trig1 = (signal: Channel, dur: Channel = 0.1): Ugen =>
    new Ugen("Trig1", [signal, dur]);

/**
 * One sample of 1, `dur` seconds after each trigger. A trigger arriving
 * while one is already in flight is **dropped**, not queued, so a burst
 * cannot pile up.
 */
export const tDelay = (signal: Channel, dur: Channel = 0.1): Ugen =>
    new Ugen("TDelay", [signal, dur]);

/**
 * Sample and hold: takes one sample of `signal` at each rising edge of
 * `trig` and holds it until the next one.
 */
export const latch = (signal: Channel, trig: Channel = 0.0): Ugen =>
    new Ugen("Latch", [signal, trig]);

/**
 * Passes `signal` while `trig` is above zero and **freezes** at the
 * last value when it is not. Unlike `latch` it is transparent for as long as
 * the gate is open.
 */
export const gate = (signal: Channel, trig: Channel = 0.0): Ugen =>
    new Ugen("Gate", [signal, trig]);

/**
 * A comparator with hysteresis: 1 once `signal` rises past `hi`, 0 once
 * it falls past `lo`, unchanged in between. The gap is what keeps a noisy
 * input from chattering the way a plain `signal > threshold` would.
 */
export const schmidt = (signal: Channel, lo: Channel = 0.0, hi: Channel = 1.0): Ugen =>
    new Ugen("Schmidt", [signal, lo, hi]);

/**
 * Flips between 0 and 1 on each trigger -- a divider by two of the
 * *triggers*, not of the signal.
 */
export const toggleFf = (trig: Channel = 0.0): Ugen => new Ugen("ToggleFF", [trig]);

/**
 * 1 from the first `trig`, 0 from the next `reset`. Both on the same
 * sample leaves it at 0: reset is applied second.
 */
export const setResetFf = (trig: Channel = 0.0, reset: Channel = 0.0): Ugen =>
    new Ugen("SetResetFF", [trig, reset]);

/** Counts triggers, from 1; a rising `reset` puts it back to 0. */
export const pulseCount = (trig: Channel = 0.0, reset: Channel = 0.0): Ugen =>
    new Ugen("PulseCount", [trig, reset]);

/**
 * One trigger out for every `div` in. `start` is where the counter
 * begins, read once -- set it to `div - 1` to fire on the very first
 * trigger, which is how two dividers are phased against each other.
 */
export const pulseDivider = (
    trig: Channel = 0.0,
    div: Channel = 2.0,
    start: Channel = 0.0,
): Ugen => new Ugen("PulseDivider", [trig, div, start]);

/**
 * A counter that walks `[min, max]` -- **both ends included** -- one
 * `step` per trigger, wrapping. It sits at `resetval` until the first
 * trigger, which lands on `resetval + step`: a stepper is defined by its
 * transitions. A negative `step` walks the same ring backwards.
 */
export const stepper = (
    trig: Channel = 0.0,
    reset: Channel = 0.0,
    min: Channel = 0.0,
    max: Channel = 7.0,
    step: Channel = 1.0,
    resetval: Channel = 0.0,
): Ugen => new Ugen("Stepper", [trig, reset, min, max, step, resetval]);

/**
 * The time in seconds between the last two triggers, held between them
 * (0 until there are two). The crossing is interpolated, so a period measured
 * off a slow oscillator is not rounded to the sample.
 */
export const timer = (trig: Channel = 0.0): Ugen => new Ugen("Timer", [trig]);

/**
 * A ramp rising at `rate` per second, restarted at each trigger. It is
 * already running before the first one, so `sweep(0, 1)` is simply the
 * node's age in seconds.
 */
export const sweep = (trig: Channel = 0.0, rate: Channel = 1.0): Ugen =>
    new Ugen("Sweep", [trig, rate]);

/**
 * 1 on any sample where `signal` moved by more than `threshold`.
 *
 * It compares the **halved** difference, `|(x[n] - x[n-1]) / 2|`, because
 * sclang builds this out of `HPZ1` whose gain is 0.5 and a ported def must
 * not change value: a step of 0.2 registers against 0.09, not against 0.19.
 */
export const changed = (signal: Channel, threshold: Channel = 0.0): Ugen =>
    new Ugen("Changed", [signal, threshold]);

/**
 * Turns each impulse into an exponential falling 60 dB in `decaytime`
 * (`y[n] = x[n] + b*y[n-1]`). Its attack is instantaneous, which clicks --
 * see `decay2`.
 */
export const decay = (signal: Channel, decaytime: Channel = 1.0): Ugen =>
    new Ugen("Decay", [signal, decaytime]);

/**
 * `decay` minus a second, faster decay, which rounds the attack. Its peak
 * is lower than 1 and sits where the two exponentials' slopes match.
 */
export const decay2 = (
    signal: Channel,
    attacktime: Channel = 0.01,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("Decay2", [signal, attacktime, decaytime]);

/**
 * At each rising edge of `trig`, frees node `id` (any node, this one
 * included); passes `trig` through.
 */
export const free = (trig: Channel, id: Channel): Ugen => new Ugen("Free", [trig, id]);

/**
 * Runs node `id` while `gate` is not 0 and pauses it while it is, acting
 * when the gate changes; passes `gate` through.
 */
export const pause = (gate: Channel, id: Channel): Ugen => new Ugen("Pause", [gate, id]);

/**
 * The largest `|signal|` since the last rising edge of `trig`, which starts
 * it again from the sample it rises on.
 */
export const peak = (signal: Channel, trig: Channel = 0.0): Ugen =>
    new Ugen("Peak", [signal, trig]);

/** The largest `signal` since the last rising edge of `trig`. */
export const runningMax = (signal: Channel, trig: Channel = 0.0): Ugen =>
    new Ugen("RunningMax", [signal, trig]);

/** The smallest `signal` since the last rising edge of `trig`. */
export const runningMin = (signal: Channel, trig: Channel = 0.0): Ugen =>
    new Ugen("RunningMin", [signal, trig]);

/**
 * `|signal|` held at each new peak and falling back towards it by `decay` a
 * sample.
 */
export const peakFollower = (signal: Channel, decay: Channel = 0.999): Ugen =>
    new Ugen("PeakFollower", [signal, decay]);

/** The sum of the last `numsamp` samples, its window sized at the start. */
export const runningSum = (signal: Channel, numsamp: Channel = 40.0): Ugen =>
    new Ugen("RunningSum", [signal, numsamp]);

/** The value `signal` had before it last moved by at least `diff`. */
export const lastValue = (signal: Channel, diff: Channel = 0.01): Ugen =>
    new Ugen("LastValue", [signal, diff]);

/** Whichever of `a` and `b` moved more since the last sample. */
export const mostChange = (a: Channel, b: Channel): Ugen =>
    new Ugen("MostChange", [a, b]);

/** Whichever of `a` and `b` moved less since the last sample. */
export const leastChange = (a: Channel, b: Channel): Ugen =>
    new Ugen("LeastChange", [a, b]);

/**
 * The frequency of `signal`'s rising zero crossings, each placed within its
 * sample, held from one to the next.
 */
export const zeroCrossing = (signal: Channel): Ugen => new Ugen("ZeroCrossing", [signal]);
