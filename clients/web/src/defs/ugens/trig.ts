// Triggers and control flow (mirrors `clausters/defs/ugens/trig.py`).
//
// A **trigger** is a signal crossing from <= 0 up to > 0 — one definition
// shared by every function here, so the same crossing means the same thing
// whatever produced it.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * Holds the **level the input had at the trigger** for `dur` seconds, then
 * 0. Use `trig1` when all you want is a 1.
 */
export const trig = (signal: Channel, dur: Channel = 0.1): Ugen =>
    new Ugen("Trig", [signal, dur]);

/** Holds 1 for `dur` seconds after each trigger, whatever level triggered it. */
export const trig1 = (signal: Channel, dur: Channel = 0.1): Ugen =>
    new Ugen("Trig1", [signal, dur]);

/**
 * One sample of 1, `dur` seconds after each trigger. A trigger arriving
 * while one is already in flight is **dropped**, not queued.
 */
export const tDelay = (signal: Channel, dur: Channel = 0.1): Ugen =>
    new Ugen("TDelay", [signal, dur]);

/**
 * Sample and hold: takes one sample of `signal` at each rising edge of
 * `trig` and holds it until the next one.
 */
export const latch = (signal: Channel, trigger: Channel = 0.0): Ugen =>
    new Ugen("Latch", [signal, trigger]);

/**
 * Passes `signal` while `trigger` is above zero and **freezes** at the last
 * value when it is not — transparent for as long as the gate is open.
 */
export const gate = (signal: Channel, trigger: Channel = 0.0): Ugen =>
    new Ugen("Gate", [signal, trigger]);

/**
 * A comparator with hysteresis: 1 once `signal` rises past `hi`, 0 once it
 * falls past `lo`, unchanged in between.
 */
export const schmidt = (
    signal: Channel,
    lo: Channel = 0.0,
    hi: Channel = 1.0,
): Ugen => new Ugen("Schmidt", [signal, lo, hi]);

/**
 * Flips between 0 and 1 on each trigger — a divider by two of the
 * *triggers*, not of the signal.
 */
export const toggleFf = (trigger: Channel = 0.0): Ugen =>
    new Ugen("ToggleFF", [trigger]);

/**
 * 1 from the first `trigger`, 0 from the next `reset`. Both on the same
 * sample leaves it at 0: reset is applied second.
 */
export const setResetFf = (
    trigger: Channel = 0.0,
    reset: Channel = 0.0,
): Ugen => new Ugen("SetResetFF", [trigger, reset]);

/** Counts triggers, from 1; a rising `reset` puts it back to 0. */
export const pulseCount = (
    trigger: Channel = 0.0,
    reset: Channel = 0.0,
): Ugen => new Ugen("PulseCount", [trigger, reset]);

/**
 * One trigger out for every `div` in. `start` is where the counter begins,
 * read once — set it to `div - 1` to fire on the very first trigger.
 */
export const pulseDivider = (
    trigger: Channel = 0.0,
    div: Channel = 2.0,
    start: Channel = 0.0,
): Ugen => new Ugen("PulseDivider", [trigger, div, start]);

/**
 * A counter that walks `[min, max]` — **both ends included** — one `step`
 * per trigger, wrapping. It sits at `resetval` until the first trigger,
 * which lands on `resetval + step`.
 */
export const stepper = (
    trigger: Channel = 0.0,
    reset: Channel = 0.0,
    min: Channel = 0.0,
    max: Channel = 7.0,
    step: Channel = 1.0,
    resetval: Channel = 0.0,
): Ugen => new Ugen("Stepper", [trigger, reset, min, max, step, resetval]);

/** The time in seconds between the last two triggers, held between them. */
export const timer = (trigger: Channel = 0.0): Ugen =>
    new Ugen("Timer", [trigger]);

/**
 * A ramp rising at `rate` per second, restarted at each trigger. It is
 * already running before the first one, so `sweep(0, 1)` is the node's age.
 */
export const sweep = (
    trigger: Channel = 0.0,
    rate: Channel = 1.0,
): Ugen => new Ugen("Sweep", [trigger, rate]);

/**
 * 1 on any sample where `signal` moved by more than `threshold`. It compares
 * the **halved** difference, `|(x[n] − x[n−1]) / 2|`, matching sclang's
 * `HPZ1`-derived definition.
 */
export const changed = (
    signal: Channel,
    threshold: Channel = 0.0,
): Ugen => new Ugen("Changed", [signal, threshold]);

/**
 * Turns each impulse into an exponential falling 60 dB in `decaytime`. Its
 * attack is instantaneous, which clicks — see `decay2`.
 */
export const decay = (signal: Channel, decaytime: Channel = 1.0): Ugen =>
    new Ugen("Decay", [signal, decaytime]);

/** `decay` minus a second, faster decay, which rounds the attack. */
export const decay2 = (
    signal: Channel,
    attacktime: Channel = 0.01,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("Decay2", [signal, attacktime, decaytime]);
