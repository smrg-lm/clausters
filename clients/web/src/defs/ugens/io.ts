// What a graph reads from and writes to: buses, replies, feedback (mirrors
// `clausters/defs/ugens/io.py`).
//
// The bus pair (`in_`/`out`, with their control-rate and replacing forms), the
// side-effect UGens that emit an OSC reply or a console post instead of audio
// (a def may hold only these and no `out` at all), the streaming disk pair,
// and the `localIn`/`localOut` feedback pair.

import { ChannelList, SynthLeaf, Ugen, channelBinop, isList } from "./graph.ts";
import type { Channel } from "./graph.ts";
export * from "./_generated/io.ts";

/**
 * One writer per channel on consecutive buses (`bus`, `bus+1`, ...) -- the
 * point where a channel list becomes buses. A constant `bus` is offset here;
 * a control (or any single-channel node) takes the first channel itself and
 * an `Add` of the offset for each one after, which the server reads as a bus
 * it can still order the node by.
 */
function outChannels(
    kind: string,
    bus: Channel,
    signal: ChannelList | readonly Channel[],
): ChannelList {
    if (typeof bus !== "number" && !(bus instanceof SynthLeaf)) {
        throw new TypeError(
            `a multichannel ${kind} lays its channels on consecutive buses ` +
                "from one bus -- a number or a single-channel node",
        );
    }
    const sig = new ChannelList(signal);
    return new ChannelList(
        sig.items.map(
            (s, i) => new Ugen(kind, [i === 0 ? bus : channelBinop(bus, "add", i), s]),
        ),
    );
}

/**
 * Sums `signal` into the audio `bus` (output happens only here). A channel
 * list writes its channels to consecutive buses: `out(0, dup(sig))` is a
 * stereo output, and so is `out(control("out", 0.0), dup(sig))`, which
 * follows the control wherever a note sets it.
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

/**
 * Overwrites the audio `bus` with `signal` instead of summing. A channel list
 * overwrites consecutive buses, from a number or from a control alike.
 */
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
 * Writes `signal`'s latest per-block value to a **control** `bus` -- the
 * write side of `inCtl`. Passes `signal` through as its output. A channel
 * list writes its channels to consecutive buses, from a number or from a
 * control alike.
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
 * On each trigger of `trig`, sends the OSC message `cmd nodeID replyId
 * value...` to `/server_notify` clients. Output is silence; pass it as a root.
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
export interface PollOptions {
    /** The name printed beside the value. A **static** field. */
    label?: string;
}

export const poll = (
    trig: Channel,
    signal: Channel,
    trigId: Channel = -1,
    { label = "poll" }: PollOptions = {},
): Ugen => new Ugen("Poll", [trig, signal, trigId], { label });

// --- streaming disk I/O ---
//
// These two read and write **the server's filesystem**, whichever that is: a
// native server's disk over a socket, and the page's own storage (`opfs`) in a
// tab. The paths there are `/`-separated under the origin's root.
//
// It used to say a tab had no path to stream from or to, and that a def naming
// one was rejected. Both are false now -- but the browser's streaming is not the
// native one either, and the difference is written down rather than left to be
// heard: the reader is a Worker handing spans across a message port instead of
// a thread sharing a ring, so a stream starts after a longer lead and an
// underrun is silence. **A tab streams WAV only**: a span of a compressed file
// is not a file, and decoding one whole is what a buffer is for.
// `clients/web/docs/src/platform.md` carries both.

/**
 * Streams a file from disk, one file frame per server sample (no resampling --
 * pitch follows the sample-rate ratio). Mono per UGen: `chan` picks the
 * channel, a stereo file is two `diskIn`s. `start` is the frame of the file
 * the stream begins at (its top by default), and `loop` restarts at the file's
 * top at the end of the stream. `path`, `loop` and `start` are **static**
 * fields and ride in the options object, so the positional parameter is the
 * one input the wire has. For a handful of streams, not per-voice (each spawns
 * its own I/O thread natively, and its own reader in a page).
 */
export interface DiskInOptions {
    /** The file to stream, on the server's filesystem. A **static** field. */
    path: string;
    /** Restart at the file's top at the end of the stream. A **static** field. */
    loop?: boolean;
    /** The frame of the file the stream begins at, its top by default. A **static** field. */
    start?: number;
}

export const diskIn = (chan: Channel = 0.0, { path, loop = false, start = 0 }: DiskInOptions): Ugen => {
    const fields: Record<string, unknown> = { path: String(path), loop: Boolean(loop) };
    if (start) fields.start = Math.trunc(start);
    return new Ugen("DiskIn", [chan], { static: fields });
};

/**
 * Streams `signal` to a mono sound file at `path` (`format` is `"int16"`,
 * `"int24"` or `"float"`) and passes `signal` through as its output: a WAV,
 * closed as an RF64 if the take outgrew 4 GB, or a W64 or an RF64 when the
 * path ends in `.w64` or `.rf64`. Record stereo with two
 * `diskOut`s.
 *
 * It delivers audio out of the graph, so it is a valid def root on its own:
 * `play(diskOut(sig, { path }))` records **without sounding**. To record and
 * hear the same take, route it yourself -- `out(0, diskOut(sig, { path }))`, which is
 * what the pass-through output is for.
 */
export interface DiskOutOptions {
    /** The file to write, on the server's filesystem. A **static** field. */
    path: string;
    /** The sample format written. A **static** field. */
    format?: "int16" | "int24" | "float";
}

export const diskOut = (signal: Channel, { path, format = "int16" }: DiskOutOptions): Ugen =>
    new Ugen("DiskOut", [signal], { static: { path: String(path), format: String(format) } });
