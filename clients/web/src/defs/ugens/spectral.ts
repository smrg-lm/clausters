// The frequency-domain chain: FFT, the `pv*` transforms, IFFT (mirrors
// `clausters/defs/ugens/spectral.py`).
//
// A chain is a **frame**, not a signal: `fft` opens one, each `pv*` transforms
// it in place and `ifft` closes it back to samples. Wire them in order
// (`fft` -> `pv*` -> ... -> `ifft`). The frame is synth-private scratch (no buffer
// to allocate), which is why only `fft` names a size -- the server propagates
// it down the chain.

import { ChannelList, Ugen, chans } from "./graph.ts";
import type { Channel } from "./graph.ts";
import { pvTokens } from "../pv_expr.ts";
import type { PvOperand } from "../pv_expr.ts";
export * from "./_generated/spectral.ts";

/** The frame an `fft` or `fftTrigger` takes. */
export interface FftOptions {
    /** The transform size, a power of two from 64 to 32768. */
    fftSize?: number;
    /**
     * The fraction of the frame between frames: in (0, 1], and at least one
     * 64-sample block (0.25 of 256).
     */
    hop?: number;
    /** The window shape (`0` Hann, `1` sine, ...). */
    wintype?: number;
    /**
     * The frame of signal windowed, when shorter than the transform: at least
     * one 64-sample block, padded with zeros to `fftSize`.
     */
    winsize?: number;
}

const frameFields = ({ fftSize = 1024, hop = 0.5, wintype = 0, winsize }: FftOptions) => {
    const fields: Record<string, number> = {
        fft_size: Math.trunc(fftSize),
        hop: Number(hop),
        wintype: Math.trunc(wintype),
    };
    if (winsize !== undefined) fields.winsize = Math.trunc(winsize);
    return fields;
};

/**
 * Opens a spectral chain: windows `source` (an audio signal) and transforms
 * it to a spectral frame once per **hop**. `active` gates the input, per
 * sample: `<= 0` is analysed as silence, so the round trip is the gated input
 * exactly, and a frame whose whole window was gated is not taken -- the chain
 * does no work while it stays off. `fftSize` is the transform size,
 * `winsize` the frame of signal windowed when shorter (the transform pads it
 * with zeros, a finer grid of bins over the same stretch of time). The
 * options size the transform, so they are static fields given **only here**
 * -- the server propagates them to the rest of the chain. The chain's window
 * is also settable live with `Server.uCmd` addressed to this `fft` (the
 * `ifft` follows it). Feed the result to a `pv*` filter or `ifft`.
 */
export const fft = (source: Channel, active: Channel = 1.0, options: FftOptions = {}): Ugen =>
    new Ugen("FFT", [source, active], { static: frameFields(options) });

/**
 * Opens a spectral chain whose frames are **written**, not analysed: it marks
 * the frame fresh once per hop and leaves its bins to what follows --
 * `packFft` setting them, or a `pvKernel` program computing them -- and an
 * `ifft` resynthesizes them like any other frame. The options are `fft`'s.
 * The frame starts silent and keeps what was written into it from one hop to
 * the next. Its frames carry no analysis window, so the `ifft` normalizes
 * their overlap by the synthesis window alone: a bin of magnitude `m` comes
 * back as a partial of amplitude `2 * m / fftSize`.
 */
export const fftTrigger = (options: FftOptions = {}): Ugen =>
    new Ugen("FFTTrigger", [], { static: frameFields(options) });

/**
 * Writes a run of bins from signals: `magsPhases` is the flat list `[mag0,
 * phase0, mag1, phase1, ...]`, pair `j` setting bin `frombin + j` (up to
 * `tobin`, by default the last pair's), read at each frame. With `zeroothers`
 * above 0 every other bin is zeroed. DC and Nyquist, which are real, take
 * `mag * cos(phase)`. Usually on an `fftTrigger` chain, so the frame is what
 * the def writes; on an analysed one it replaces the bins it names.
 */
export function packFft(
    chain: Channel,
    magsPhases: readonly Channel[],
    frombin: Channel = 0,
    tobin?: Channel,
    zeroothers: Channel = 0,
): Ugen {
    const last = tobin ?? Number(frombin) + Math.floor(magsPhases.length / 2) - 1;
    return new Ugen("PackFFT", [chain, frombin, last, zeroothers, ...magsPhases]);
}

/** The per-bin expressions and extra inputs `pvKernel` applies to a frame. */
export interface PvKernelOptions {
    /** The bin's new magnitude; omitted, the identity. */
    mag?: PvOperand;
    /** The bin's new phase; omitted, each bin's phase is kept *exactly*. */
    phase?: PvOperand;
    /** Extra signal inputs the expressions read as `param(0)`, `param(1)`, ... */
    params?: readonly Channel[];
}

/**
 * The general per-frame mechanism: applies user-written **bin expressions** to
 * every bin of each fresh frame. `mag` and `phase` are symbolic per-bin
 * expressions built from `defs/pv_expr`'s terms (its `mag`/`phase`/
 * `binIndex`/`nbins`/`binfreq`/`param`) with the math methods; each maps one
 * bin's values to that bin's new magnitude / phase. An omitted expression is
 * the identity -- and an identity `phase` keeps each bin's phase *exactly*
 * (the cheap path: pure magnitude maps skip the polar conversion).
 *
 * `params` are extra signal inputs (controls, LFOs, constants) the
 * expressions read as `param(0)`, `param(1)`, ... -- sampled once per hop.
 *
 * An expression is a **pure per-bin map**: no state across bins or frames, no
 * reading other bins. Gates, tilts, masks and magnitude algebra belong here;
 * freeze/smear (cross-frame state) and shift (bin remaps) stay with the
 * dedicated `pv*` filters. The server validates the program at `/def_send
 * synth` (stack discipline, parameter arity, unknown words) and rejects a bad
 * def with `/fail`.
 *
 * Note that `mag` is a raw transform magnitude -- it scales with the input
 * level, the window and the `fftSize`, it is **not** normalized to 0..1 -- so
 * thresholds and constants must be calibrated to the signal (probe a
 * render, or `poll` a reference).
 *
 * ```ts
 * import { mag, binIndex, nbins, param } from "clausters/defs/pv_expr.js";
 * // A tilted spectral gate: the threshold rises with frequency.
 * const tilt = param(0).mul(binIndex.div(nbins).mul(4).add(1));
 * chain = pvKernel(chain, {
 *     mag: mag.mul(mag.ge(tilt)),
 *     params: [control("thresh", 2.0)],
 * });
 * ```
 */
export function pvKernel(
    chain: Channel,
    { mag, phase, params = [] }: PvKernelOptions = {},
): Ugen {
    const staticFields: Record<string, unknown> = {};
    if (mag !== undefined) staticFields.mag_expr = pvTokens(mag);
    if (phase !== undefined) staticFields.phase_expr = pvTokens(phase);
    return new Ugen("PV_Kernel", [chain, ...params], {
        static: Object.keys(staticFields).length > 0 ? staticFields : undefined,
    });
}

/** The transform size and kernel capacity of a `conv`. */
export interface ConvOptions {
    /** The transform size (a supported power of two). */
    fftSize?: number;
    /** How long a kernel this instance accepts, in partitions. */
    partitions?: number;
}

/**
 * Partitioned convolution: convolves `source` with a **prepared** kernel -- a
 * buffer written by `dest.gen("prepare_partconv", fftSize, irBufnum)` (size
 * `dest` with `partconvFrames`). The IR's spectra are computed once, off the
 * audio thread; the UGen's steady per-block cost is flat (the partition
 * products are spread across the hop).
 *
 * `fftSize` is the transform size; the partition length -- and the intrinsic
 * latency -- is `fftSize / 2` samples. `partitions` is the most partitions a
 * kernel may have (its pre-allocated state, up to 256): a longer kernel plays
 * silence and is reported -- the server logs it, an offline render fails --
 * rather than being cut short. Moving `kernel` to a
 * *different* prepared buffer crossfades over one partition; regenerating the
 * same buffer switches hard.
 */
export const conv = (
    source: Channel,
    kernel: Channel,
    { fftSize = 1024, partitions = 16 }: ConvOptions = {},
): Ugen =>
    new Ugen("Conv", [source, kernel], {
        static: {
            fft_size: Math.trunc(fftSize),
            partitions: Math.trunc(partitions),
        },
    });

/** The kernel capacity of a `partConv`. */
export interface PartConvOptions {
    /** How long a kernel this instance accepts, in partitions. */
    partitions?: number;
}

/**
 * `conv` with scsynth's `PartConv` inputs: `fftsize` (a constant, a supported
 * power of two) and `irbufnum`, a kernel prepared with
 * `dest.gen("prepare_partconv", fftsize, irBufnum)`. `partitions` is the most
 * partitions a kernel may have, `conv`'s.
 */
export const partConv = (
    source: Channel,
    fftsize: Channel,
    irbufnum: Channel,
    { partitions = 16 }: PartConvOptions = {},
): Ugen =>
    new Ugen("PartConv", [source, fftsize, irbufnum], {
        static: { partitions: Math.trunc(partitions) },
    });

/**
 * `convolution2L` with a kernel per channel: `source` convolved with the
 * samples of buffer `kernelL` on the left and `kernelR` on the right, both
 * read at the start and again at each `trigger` and faded in over
 * `crossfade` frames. Returns the two channels.
 */
export const stereoConvolution2L = (
    source: Channel,
    kernelL: Channel,
    kernelR: Channel,
    trigger: Channel = 0.0,
    framesize: Channel = 2048,
    crossfade: Channel = 1.0,
): ChannelList =>
    chans(
        [0.0, 1.0].map(
            (c) =>
                new Ugen("StereoConvolution2L", [
                    source,
                    kernelL,
                    kernelR,
                    trigger,
                    framesize,
                    crossfade,
                    c,
                ]),
        ),
    );

/**
 * Frames a kernel buffer needs to hold `irFrames` of impulse response
 * prepared at `fftSize` (partitions of `fftSize / 2`, plus the two-sample
 * header) -- the size to `Buffer.alloc` before
 * `buf.gen("prepare_partconv", fftSize, irBufnum)`.
 */
export function partconvFrames(irFrames: number, fftSize = 1024): number {
    const part = Math.floor(fftSize / 2);
    const parts = Math.ceil(Math.trunc(irFrames) / part);
    return 2 + parts * Math.trunc(fftSize);
}
