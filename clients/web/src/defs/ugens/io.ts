// What a graph reads from and writes to: buses, replies, feedback (mirrors
// `clausters/defs/ugens/io.py`).
//
// The bus pair (`in_`/`out`, with their control-rate and replacing forms), the
// side-effect UGens that emit an OSC reply or a console post instead of audio
// (a def may hold only these and no `out` at all), the streaming disk pair,
// and the `localIn`/`localOut` feedback pair.

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

// --- streaming disk I/O ---
//
// These two read and write **the server's filesystem**, whichever that is: a
// native server's disk over a socket, and the page's own storage (`opfs`) in a
// tab. The paths there are `/`-separated under the origin's root.
//
// It used to say a tab had no path to stream from or to, and that a def naming
// one was rejected. Both are false now — but the browser's streaming is not the
// native one either, and the difference is written down rather than left to be
// heard: the reader is a Worker handing spans across a message port instead of
// a thread sharing a ring, so a stream starts after a longer lead and an
// underrun is silence. **A tab streams WAV only**: a span of a compressed file
// is not a file, and decoding one whole is what a buffer is for.
// `clients/web/docs/src/platform.md` carries both.

/**
 * Streams a file from disk, one file frame per server sample (no resampling —
 * pitch follows the sample-rate ratio). Mono per UGen: `chan` picks the
 * channel, a stereo file is two `diskIn`s. `loop` restarts at the end of the
 * stream. For a handful of streams, not per-voice (each spawns its own I/O
 * thread natively, and its own reader in a page).
 */
export const diskIn = (path: string, chan: Channel = 0.0, loop = false): Ugen =>
    new Ugen("DiskIn", [chan], { static: { path: String(path), loop: Boolean(loop) } });

/**
 * Streams `signal` to a mono WAV at `path` (`format` is `"int16"`, `"int24"`
 * or `"float"`) and passes `signal` through as its output. Record stereo with
 * two `diskOut`s.
 *
 * It delivers audio out of the graph, so it is a valid def root on its own:
 * `play(diskOut(path, sig))` records **without sounding**. To record and hear
 * the same take, route it yourself — `out(0, diskOut(path, sig))`, which is
 * what the pass-through output is for.
 */
export const diskOut = (
    path: string,
    signal: Channel,
    format: "int16" | "int24" | "float" = "int16",
): Ugen =>
    new Ugen("DiskOut", [signal], { static: { path: String(path), format: String(format) } });

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
