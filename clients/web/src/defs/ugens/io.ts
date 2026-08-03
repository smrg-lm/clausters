// What a graph reads from and writes to: buses, replies, feedback (mirrors
// `clausters/defs/ugens/io.py`).
//
// The bus pair (`in_`/`out`, with their control-rate and replacing forms), the
// side-effect UGens that emit an OSC reply or a console post instead of audio
// (a def may hold only these and no `out` at all), and the `localIn`/
// `localOut` feedback pair. Streaming disk I/O has no builder here yet.

import { ChannelList, Ugen, isList } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * Reads an audio bus (sampled per block). Named `in_` because `in` is a
 * reserved word — the wire name is still `In`.
 */
export const in_ = (bus: Channel = 0.0): Ugen => new Ugen("In", [bus]);

/** Reads a control-bus value, constant over the block. */
export const inCtl = (bus: Channel = 0.0): Ugen => new Ugen("InCtl", [bus]);

/**
 * One writer per channel on consecutive buses (`bus`, `bus+1`, …) — the
 * point where a channel list becomes buses. The base `bus` must be a number:
 * a signal bus cannot be offset per channel client-side.
 */
function outChannels(
    kind: string,
    bus: Channel,
    signal: ChannelList | readonly Channel[],
): ChannelList {
    if (typeof bus !== "number") {
        throw new TypeError(
            `a multichannel ${kind} needs a constant bus to lay channels on ` +
                "consecutive buses",
        );
    }
    const sig = new ChannelList(signal);
    return new ChannelList(sig.items.map((s, i) => new Ugen(kind, [bus + i, s])));
}

/**
 * Sums `signal` into the audio `bus` (output happens only here). A channel
 * list writes its channels to consecutive buses: `out(0, dup(sig))` is a
 * stereo output.
 */
export function out(bus: Channel, signal: Channel): Ugen;
export function out(bus: Channel, signal: ChannelList | readonly Channel[]): ChannelList;
export function out(
    bus: Channel,
    signal: Channel | ChannelList | readonly Channel[],
): Ugen | ChannelList {
    if (isList(signal)) return outChannels("Out", bus, signal);
    return new Ugen("Out", [bus, signal]);
}

/** Overwrites the audio `bus` with `signal` instead of summing. */
export function replaceOut(bus: Channel, signal: Channel): Ugen;
export function replaceOut(
    bus: Channel,
    signal: ChannelList | readonly Channel[],
): ChannelList;
export function replaceOut(
    bus: Channel,
    signal: Channel | ChannelList | readonly Channel[],
): Ugen | ChannelList {
    if (isList(signal)) return outChannels("ReplaceOut", bus, signal);
    return new Ugen("ReplaceOut", [bus, signal]);
}

/**
 * Writes `signal`'s latest per-block value to a **control** `bus` — the
 * write side of `inCtl`. Passes `signal` through as its output.
 */
export function outCtl(bus: Channel, signal: Channel): Ugen;
export function outCtl(
    bus: Channel,
    signal: ChannelList | readonly Channel[],
): ChannelList;
export function outCtl(
    bus: Channel,
    signal: Channel | ChannelList | readonly Channel[],
): Ugen | ChannelList {
    if (isList(signal)) return outChannels("OutCtl", bus, signal);
    return new Ugen("OutCtl", [bus, signal]);
}

// --- side-effect UGens: reply / observe, no `out` required ---
//
// These emit OSC replies or console posts on a trigger instead of audio. A
// SynthDef may contain only these and no `out(...)` at all; pass them as
// roots of the `SynthDef` (nothing else would reach them).

/**
 * On each trigger of `trig`, sends `/node_trigger nodeID id value` to `/server_notify`
 * clients. Output is silence; pass it as a `SynthDef` root.
 */
export const sendTrig = (
    trig: Channel,
    id: Channel = 0,
    value: Channel = 0.0,
): Ugen => new Ugen("SendTrig", [trig, id, value]);

/**
 * On each trigger of `trig`, sends the OSC message `cmd nodeID replyId
 * value…` to `/server_notify` clients. Output is silence; pass it as a root.
 */
export const sendReply = (
    trig: Channel,
    values: readonly Channel[] = [],
    { cmd = "/reply", replyId = -1 }: { cmd?: string; replyId?: number } = {},
): Ugen => new Ugen("SendReply", [trig, replyId, ...values], { label: cmd });

/**
 * On each trigger of `trig`, posts `label: value` to the server console and,
 * when `trigId >= 0`, also sends `/node_trigger nodeID trigId value`. `signal` passes
 * through the output, so `poll` can sit mid-chain.
 */
export const poll = (
    trig: Channel,
    signal: Channel,
    label = "poll",
    trigId: Channel = -1,
): Ugen => new Ugen("Poll", [trig, signal, trigId], { label });

// --- synth-private feedback ---

// --- synth-private feedback ---

/**
 * Reads synth-private feedback channel `channel` (a constant); pairs with
 * `localOut` for one-block feedback. `LocalIn` must precede its `LocalOut`
 * — the `SynthDef`'s topological order does that as long as the output
 * graph reaches the `localIn` before the `localOut`.
 */
export const localIn = (channel: Channel = 0.0): Ugen =>
    new Ugen("LocalIn", [channel]);

/**
 * Writes `signal` into synth-private feedback channel `channel`; also passes
 * `signal` through as its output (so it can be a SynthDef root, which keeps
 * the write in the graph).
 */
export const localOut = (channel: Channel, signal: Channel): Ugen =>
    new Ugen("LocalOut", [channel, signal]);
