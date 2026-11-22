// Oscillators and noise: what a graph starts from (mirrors
// `clausters/defs/ugens/osc.py`).
//
// The band-limited pair (`saw`, `pulse`) beside the naive `lf*` forms, the
// noise family by spectrum, and `phasor` as the ramp the table readers run on.

export * from "./_generated/osc.ts";

import { ChannelList, Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * `freqs`, `amps` (ones unless given) and a third list (`fill` unless given),
 * as the flat triples a bank takes.
 */
export function partials(
    freqs: readonly Channel[],
    amps: readonly Channel[] | null,
    third: readonly Channel[] | null,
    fill: number,
): Channel[] {
    const a = amps ?? freqs.map(() => 1.0);
    const t = third ?? freqs.map(() => fill);
    if (a.length !== freqs.length || t.length !== freqs.length) {
        throw new RangeError("a bank's lists must have one entry per partial");
    }
    return freqs.flatMap((f, i) => [f, a[i]!, t[i]!]);
}

/**
 * A bank of fixed sines: one per entry of `freqs`, at `amps` (1 unless given)
 * and starting at `phases` (cycles, 0 unless given), the frequencies times
 * `freqscale` plus `freqoffset`. Read at the synth's start: a fixed spectrum,
 * as cheap as sines get.
 */
export const klang = (
    freqs: readonly Channel[],
    amps: readonly Channel[] | null = null,
    phases: readonly Channel[] | null = null,
    freqscale: Channel = 1.0,
    freqoffset: Channel = 0.0,
): Ugen => new Ugen("Klang", [freqscale, freqoffset, ...partials(freqs, amps, phases, 0.0)]);

/** The channels of a granular UGen's rows, one a channel. */
function channels(numchans: number): number[] {
    if (!Number.isInteger(numchans) || numchans < 1) {
        throw new TypeError(
            `a granular UGen needs a channel count of at least 1, got ${String(numchans)}`,
        );
    }
    return Array.from({ length: numchans }, (_unused, c) => c);
}

/**
 * At each rising `trigger`, a grain `dur` seconds long of a sine at `freq`,
 * under the Hann (or the window in buffer `envbufnum`), panned at `pan` across
 * `numchans` channels: -1 to 1 left to right on a pair, a turn round a ring. At
 * most `maxGrains` (a constant) at once.
 */
export const grainSin = (
    numchans = 1,
    trigger: Channel = 0.0,
    dur: Channel = 1.0,
    freq: Channel = 440.0,
    pan: Channel = 0.0,
    envbufnum: Channel = -1.0,
    maxGrains = 512,
): ChannelList =>
    new ChannelList(
        channels(numchans).map(
            (c) => new Ugen("GrainSin", [trigger, dur, freq, pan, envbufnum, maxGrains, numchans, c]),
        ),
    );

/**
 * `grainSin` of an FM pair: a carrier at `carfreq` deviated by
 * `index * modfreq` at `modfreq`.
 */
export const grainFm = (
    numchans = 1,
    trigger: Channel = 0.0,
    dur: Channel = 1.0,
    carfreq: Channel = 440.0,
    modfreq: Channel = 200.0,
    index: Channel = 1.0,
    pan: Channel = 0.0,
    envbufnum: Channel = -1.0,
    maxGrains = 512,
): ChannelList =>
    new ChannelList(
        channels(numchans).map(
            (c) =>
                new Ugen("GrainFM", [
                    trigger, dur, carfreq, modfreq, index, pan, envbufnum, maxGrains, numchans, c,
                ]),
        ),
    );

/**
 * `grainSin` of a mono buffer: each grain read from `pos` (0 to 1 of the
 * buffer) at `rate` times its own rate, wrapping round it, with no
 * interpolation (`interp` 1), linear (2) or cubic (4).
 */
export const grainBuf = (
    numchans: number,
    trigger: Channel,
    dur: Channel,
    sndbuf: Channel,
    rate: Channel = 1.0,
    pos: Channel = 0.0,
    interp: Channel = 2.0,
    pan: Channel = 0.0,
    envbufnum: Channel = -1.0,
    maxGrains = 512,
): ChannelList =>
    new ChannelList(
        channels(numchans).map(
            (c) =>
                new Ugen("GrainBuf", [
                    trigger, dur, sndbuf, rate, pos, interp, pan, envbufnum, maxGrains, numchans, c,
                ]),
        ),
    );

/** `grainSin` of `signal`: each grain the input while it sounds. */
export const grainIn = (
    numchans: number,
    trigger: Channel,
    dur: Channel,
    signal: Channel,
    pan: Channel = 0.0,
    envbufnum: Channel = -1.0,
    maxGrains = 512,
): ChannelList =>
    new ChannelList(
        channels(numchans).map(
            (c) => new Ugen("GrainIn", [trigger, dur, signal, pan, envbufnum, maxGrains, numchans, c]),
        ),
    );

/**
 * At each rising `trigger`, a grain of a mono buffer centred on `centerPos`
 * seconds into it, `dur` long under the Hann, read at `rate`, at `amp`, panned
 * at `pan` as `grainSin`'s.
 */
export const tGrains = (
    numchans: number,
    trigger: Channel = 0.0,
    bufnum: Channel = 0.0,
    rate: Channel = 1.0,
    centerPos: Channel = 0.0,
    dur: Channel = 0.1,
    pan: Channel = 0.0,
    amp: Channel = 0.1,
    interp: Channel = 4.0,
): ChannelList =>
    new ChannelList(
        channels(numchans).map(
            (c) =>
                new Ugen("TGrains", [
                    trigger, bufnum, rate, centerPos, dur, pan, amp, interp, numchans, c,
                ]),
        ),
    );

/**
 * A buffer stretched in time, `numchans` of its channels: grains `windowSize`
 * long, `overlaps` at once, each read from `pointer` (0 to 1 of the buffer) at
 * `freqScale` times its pitch. The length moves at random by up to
 * `windowRandRatio` of itself.
 */
export const warp1 = (
    numchans: number,
    bufnum: Channel = 0.0,
    pointer: Channel = 0.0,
    freqScale: Channel = 1.0,
    windowSize: Channel = 0.2,
    envbufnum: Channel = -1.0,
    overlaps: Channel = 8.0,
    windowRandRatio: Channel = 0.0,
    interp: Channel = 1.0,
): ChannelList =>
    new ChannelList(
        channels(numchans).map(
            (c) =>
                new Ugen("Warp1", [
                    bufnum, pointer, freqScale, windowSize, envbufnum, overlaps, windowRandRatio,
                    interp, c,
                ]),
        ),
    );

