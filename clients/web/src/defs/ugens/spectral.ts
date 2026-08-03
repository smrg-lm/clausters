// The frequency-domain chain: FFT, the `pv*` transforms, IFFT (mirrors
// `clausters/defs/ugens/spectral.py`).
//
// A chain is a **frame**, not a signal: `fft` opens one, each `pv*` transforms
// it in place and `ifft` closes it back to samples. Wire them in order
// (`fft` → `pv*` → … → `ifft`). The frame is synth-private scratch (no buffer
// to allocate), which is why only `fft` names a size — the server propagates
// it down the chain.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";
import { pvTokens } from "../pv_expr.ts";
import type { PvOperand } from "../pv_expr.ts";

/** The window `fft` transforms with (`0` Hann, `1` sine, …). */
export interface FftOptions {
    /** The window size, a power of two: 256/512/1024/2048/4096. */
    fftSize?: number;
    /** The fraction of the window between frames. */
    hop?: number;
    /** The window shape. */
    wintype?: number;
}

/**
 * Opens a spectral chain: windows `source` (an audio signal) and transforms
 * it to a spectral frame once per **hop**. `active > 0` runs the transform,
 * `<= 0` holds. These size the transform, so they are static fields given
 * **only here** — the server propagates them to the rest of the chain. The
 * window is also settable live with `Server.uCmd`. Feed the result to a `pv*`
 * filter or `ifft`.
 */
export const fft = (
    source: Channel,
    active: Channel = 1.0,
    { fftSize = 1024, hop = 0.5, wintype = 0 }: FftOptions = {},
): Ugen =>
    new Ugen("FFT", [source, active], {
        static: {
            fft_size: Math.trunc(fftSize),
            hop: Number(hop),
            wintype: Math.trunc(wintype),
        },
    });

/**
 * Closes a spectral chain: inverse-transforms each fresh frame and
 * overlap-adds it back to audio (window-normalized, so a bare `fft` → `ifft`
 * reconstructs at unity gain, delayed by one window). `chain` is the output
 * of an `fft` or a `pv*` filter.
 */
export const ifft = (chain: Channel): Ugen => new Ugen("IFFT", [chain]);

/**
 * Passes only the bins whose magnitude is **above** `threshold`, zeroing the
 * rest. `chain` comes from `fft` or another `pv*`.
 */
export const pvMagAbove = (chain: Channel, threshold: Channel): Ugen =>
    new Ugen("PV_MagAbove", [chain, threshold]);

/** Passes only the bins whose magnitude is **below** `threshold`. */
export const pvMagBelow = (chain: Channel, threshold: Channel): Ugen =>
    new Ugen("PV_MagBelow", [chain, threshold]);

/**
 * Brick-wall band limit: `wipe > 0` zeroes the top fraction of bins (a low
 * pass), `wipe < 0` the bottom (a high pass), `0` passes everything (`wipe`
 * in −1..1).
 */
export const pvBrickWall = (chain: Channel, wipe: Channel): Ugen =>
    new Ugen("PV_BrickWall", [chain, wipe]);

/**
 * Limits each bin's magnitude **to** `threshold`: louder bins are scaled down
 * to it (phases kept), quieter bins pass untouched.
 */
export const pvMagClip = (chain: Channel, threshold: Channel): Ugen =>
    new Ugen("PV_MagClip", [chain, threshold]);

/**
 * Two-chain combiner: per-bin complex sum. Both inputs must be spectral
 * chains of the same `fftSize` (and distinct); the result lands in chain A,
 * which the combiner's output carries onward.
 */
export const pvAdd = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Add", [chainA, chainB]);

/** Two-chain combiner: per-bin complex product (spectral ring modulation). */
export const pvMul = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Mul", [chainA, chainB]);

/** Two-chain combiner: per bin, whichever input has the **smaller** magnitude. */
export const pvMin = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Min", [chainA, chainB]);

/** Two-chain combiner: per bin, whichever input has the **larger** magnitude. */
export const pvMax = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Max", [chainA, chainB]);

/**
 * Two-chain combiner: A's bins scaled by B's magnitudes — A's phases kept (a
 * spectral envelope transfer, the classic "vocoder" cross-synthesis).
 */
export const pvMagMul = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_MagMul", [chainA, chainB]);

/**
 * Two-chain combiner: A's magnitudes with B's phases (the complementary
 * cross-synthesis to `pvMagMul`).
 */
export const pvCopyPhase = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_CopyPhase", [chainA, chainB]);

/**
 * While `freeze <= 0` stores each frame's magnitudes and passes through;
 * while `> 0` rescales every bin to the stored magnitudes — the spectral
 * envelope holds while the phases keep running.
 */
export const pvMagFreeze = (chain: Channel, freeze: Channel = 0.0): Ugen =>
    new Ugen("PV_MagFreeze", [chain, freeze]);

/**
 * Averages each bin's magnitude over `bins` neighbors on each side (`0` is
 * transparent), phases untouched — a spectral blur.
 */
export const pvMagSmear = (chain: Channel, bins: Channel = 0.0): Ugen =>
    new Ugen("PV_MagSmear", [chain, bins]);

/**
 * Remaps bin `b` to `round(b * stretch + shift)`: colliding bins sum,
 * out-of-range bins are dropped. `stretch = 1, shift = 0` is transparent; a
 * positive `shift` moves every partial up by `shift` bin widths.
 */
export const pvBinShift = (
    chain: Channel,
    stretch: Channel = 1.0,
    shift: Channel = 0.0,
): Ugen => new Ugen("PV_BinShift", [chain, stretch, shift]);

/**
 * The `pvBinShift` remap applied to the magnitude envelope only, laid over
 * the frame's original phases.
 */
export const pvMagShift = (
    chain: Channel,
    stretch: Channel = 1.0,
    shift: Channel = 0.0,
): Ugen => new Ugen("PV_MagShift", [chain, stretch, shift]);

/** The per-bin expressions and extra inputs `pvKernel` applies to a frame. */
export interface PvKernelOptions {
    /** The bin's new magnitude; omitted, the identity. */
    mag?: PvOperand;
    /** The bin's new phase; omitted, each bin's phase is kept *exactly*. */
    phase?: PvOperand;
    /** Extra signal inputs the expressions read as `param(0)`, `param(1)`, … */
    params?: readonly Channel[];
}

/**
 * The general per-frame mechanism: applies user-written **bin expressions** to
 * every bin of each fresh frame. `mag` and `phase` are symbolic per-bin
 * expressions built from `defs/pv_expr`'s terms (its `mag`/`phase`/
 * `binIndex`/`nbins`/`binfreq`/`param`) with the math methods; each maps one
 * bin's values to that bin's new magnitude / phase. An omitted expression is
 * the identity — and an identity `phase` keeps each bin's phase *exactly*
 * (the cheap path: pure magnitude maps skip the polar conversion).
 *
 * `params` are extra signal inputs (controls, LFOs, constants) the
 * expressions read as `param(0)`, `param(1)`, … — sampled once per hop.
 *
 * An expression is a **pure per-bin map**: no state across bins or frames, no
 * reading other bins. Gates, tilts, masks and magnitude algebra belong here;
 * freeze/smear (cross-frame state) and shift (bin remaps) stay with the
 * dedicated `pv*` filters. The server validates the program at `/def_send
 * synth` (stack discipline, parameter arity, unknown words) and rejects a bad
 * def with `/fail`.
 *
 * Note that `mag` is a raw transform magnitude — it scales with the input
 * level, the window and the `fftSize`, it is **not** normalized to 0..1 — so
 * thresholds and constants must be calibrated to the material (probe a
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
 * Partitioned convolution: convolves `source` with a **prepared** kernel — a
 * buffer written by `dest.gen("prepare_partconv", fftSize, irBufnum)` (size
 * `dest` with `partconvFrames`). The IR's spectra are computed once, off the
 * audio thread; the UGen's steady per-block cost is flat (the partition
 * products are spread across the hop).
 *
 * `fftSize` is the transform size; the partition length — and the intrinsic
 * latency — is `fftSize / 2` samples. `partitions` caps the kernel length
 * this instance accepts (its pre-allocated state). Moving `kernel` to a
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

/**
 * Frames a kernel buffer needs to hold `irFrames` of impulse response
 * prepared at `fftSize` (partitions of `fftSize / 2`, plus the two-sample
 * header) — the size to `Buffer.alloc` before
 * `buf.gen("prepare_partconv", fftSize, irBufnum)`.
 */
export function partconvFrames(irFrames: number, fftSize = 1024): number {
    const part = Math.floor(fftSize / 2);
    const parts = Math.ceil(Math.trunc(irFrames) / part);
    return 2 + parts * Math.trunc(fftSize);
}
