// The `io` builders that mirror the wire, re-exported by `io.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/io.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/** Reads an audio bus (sampled per block). */
export const in_ = (bus: Channel = 0.0): Ugen => new Ugen("In", [bus]);

/** Reads a control-bus value, constant over the block. */
export const inCtl = (bus: Channel = 0.0): Ugen => new Ugen("InCtl", [bus]);

/**
 * A **level with the ballistics a person can read**: instantaneous attack,
 * a fall of `decay` decibels per second, and a peak held `hold` seconds
 * before it starts falling.
 *
 * One block is one measurement -- the block's own peak in, the whole block out
 * at what the meter now reads -- which is what a meter is and what a control
 * bus carries, so a finer answer would be samples nothing can read. The rules
 * are the shared crate's (`measure::Ballistics`), so every meter drawn
 * anywhere falls at the same rate.
 *
 * It **emits** the value rather than writing it anywhere, so what to do with
 * it stays yours: a control bus per channel, a `sendReply`, or a signal that
 * drives something.
 */
export const meter = (
    signal: Channel,
    decay: Channel = 20.0,
    hold: Channel = 0.0,
): Ugen => new Ugen("Meter", [signal, decay, hold]);

/**
 * **How many times the signal was flattened**, counted since the pass
 * began: a run of `run` consecutive samples at or over `ceiling` is one
 * over, however long it goes on.
 *
 * The count and not a flag, because the flag is a *reader's* state -- a meter's
 * red lamp stays lit until a hand puts it out, and two windows watching one
 * bus each have their own -- while the count is the signal's. Pair it with
 * `outCtl` and hand the bus to a `meter` widget's `clip`: the widget
 * differences the count and lights up on an over it has not seen.
 *
 * A single sample at full scale is **not** clipping. The engine runs in
 * floating point, so a sample at 1.0 destroys nothing until the signal is
 * converted or reaches a converter; what a red lamp reports is a waveform that
 * was flattened, and the signature of that is a run. Three is the field's
 * number, and `run: 1` is the pessimistic meter that lights on any sample
 * touching full scale.
 *
 * It runs per **sample**, unlike `meter`: the run is the whole of what
 * distinguishes a flattened peak from a peak that legitimately reached full
 * scale, and a block peak has already lost it.
 */
export const clipCount = (
    signal: Channel,
    ceiling: Channel = 1.0,
    run: Channel = 3.0,
): Ugen => new Ugen("ClipCount", [signal, ceiling, run]);

/**
 * A **meter's level over the reconstructed signal**: the same ballistics as
 * `meter` -- instantaneous attack, a fall of `decay` decibels per second, a
 * peak held `hold` seconds -- but what goes in is the block's **true peak**
 * rather than its largest sample.
 *
 * The signal between two samples can reach past both: a tone sampled so that
 * every sample sits at full scale can be three decibels over it in between,
 * and every converter sees that. The reconstruction is the filter ITU-R
 * BS.1770-4 Annex 2 specifies, so the reading is in **dBTP** and is never
 * below what `meter` reads off the same signal. Hand the bus to a `meter`
 * widget with `rate: "control"` and `peak: "true"`, which reads it against
 * the -1 dBTP ceiling rather than full scale.
 */
export const truePeak = (
    signal: Channel,
    decay: Channel = 20.0,
    hold: Channel = 0.0,
): Ugen => new Ugen("TruePeak", [signal, decay, hold]);

/**
 * On each trigger of `trig`, sends `/nodeTrigger nodeID id value` to `/serverNotify`
 * clients. Output is silence; pass it as a `SynthDef` root.
 */
export const sendTrig = (trig: Channel, id: Channel = 0.0, value: Channel = 0.0): Ugen =>
    new Ugen("SendTrig", [trig, id, value]);

/**
 * Reads synth-private feedback channel `channel` (a constant); pairs with
 * `localOut` for one-block feedback. `LocalIn` must precede its
 * `LocalOut` -- the `SynthDef`'s topological order does that as long
 * as the output graph reaches the `localIn` before the `localOut`.
 */
export const localIn = (channel: Channel = 0.0): Ugen => new Ugen("LocalIn", [channel]);

/**
 * Writes `signal` into synth-private feedback channel `channel` (a
 * constant); also passes `signal` through as its output (so it can be a
 * SynthDef output to keep the write in the graph).
 */
export const localOut = (channel: Channel, signal: Channel): Ugen =>
    new Ugen("LocalOut", [channel, signal]);

/**
 * The class of every sample of `signal`: 0 for an ordinary number (zero
 * included), 1 for a NaN, 2 for an infinity, 3 for a denormal. `post` says when
 * it is also written to the server's console: 0 never, 1 at every bad sample,
 * 2 whenever the class changes, with how long the previous one lasted. `id`
 * tells one check from another there.
 */
export const checkBadValues = (
    signal: Channel,
    id: Channel = 0.0,
    post: Channel = 2.0,
): Ugen => new Ugen("CheckBadValues", [signal, id, post]);

/**
 * `signal` with every NaN, infinity and denormal replaced by `replace`:
 * a guard before a filter whose state one NaN would ruin for good.
 */
export const sanitize = (signal: Channel, replace: Channel = 0.0): Ugen =>
    new Ugen("Sanitize", [signal, replace]);

/**
 * Reads an audio bus whatever the order of the nodes: this block's samples
 * if the bus has been written this block, else the last block's -- the
 * feedback between synths `in_` cannot read.
 */
export const inFeedback = (bus: Channel = 0.0): Ugen => new Ugen("InFeedback", [bus]);

/**
 * A control bus as a trigger: its value in the block it was set (the same
 * value included), 0 otherwise.
 */
export const inTrig = (bus: Channel = 0.0): Ugen => new Ugen("InTrig", [bus]);

/** A control bus through a one-pole lag of `lag` seconds to -60 dB. */
export const lagIn = (bus: Channel = 0.0, lag: Channel = 0.1): Ugen =>
    new Ugen("LagIn", [bus, lag]);

/**
 * Reads a control bus: the server's control buses are the memory a client
 * in another process maps, scsynth's shared controls.
 */
export const sharedIn = (bus: Channel = 0.0): Ugen => new Ugen("SharedIn", [bus]);
