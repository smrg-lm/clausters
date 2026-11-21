// The `spectral` builders that mirror the wire, re-exported by `spectral.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/spectral.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * Closes a spectral chain: inverse-transforms each fresh frame and
 * overlap-adds it back to audio (window-normalized, so a bare `fft`->`ifft`
 * reconstructs at unity gain, delayed by one window). `chain` is the output
 * of an `fft` or a `pv_*` filter.
 */
export const ifft = (chain: Channel): Ugen => new Ugen("IFFT", [chain]);

/**
 * Passes only the bins whose magnitude is **above** `threshold`, zeroing
 * the rest. `chain` comes from `fft` or another `pv_*`.
 */
export const pvMagAbove = (chain: Channel, threshold: Channel): Ugen =>
    new Ugen("PV_MagAbove", [chain, threshold]);

/** Passes only the bins whose magnitude is **below** `threshold`. */
export const pvMagBelow = (chain: Channel, threshold: Channel): Ugen =>
    new Ugen("PV_MagBelow", [chain, threshold]);

/**
 * Brick-wall band limit: `wipe > 0` zeroes the top fraction of bins (a low
 * pass), `wipe < 0` the bottom (a high pass), `0` passes everything
 * (`wipe` in -1..1).
 */
export const pvBrickWall = (chain: Channel, wipe: Channel): Ugen =>
    new Ugen("PV_BrickWall", [chain, wipe]);

/**
 * Limits each bin's magnitude **to** `threshold`: louder bins are scaled
 * down to it (phases kept), quieter bins pass untouched.
 */
export const pvMagClip = (chain: Channel, threshold: Channel): Ugen =>
    new Ugen("PV_MagClip", [chain, threshold]);

/**
 * Two-chain combiner: per-bin complex sum. Both inputs must be spectral
 * chains of the same `fftSize`, `hop` and `wintype` (and distinct); the
 * result lands in chain A, which the combiner's output carries onward.
 */
export const pvAdd = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Add", [chainA, chainB]);

/** Two-chain combiner: per-bin complex product (spectral ring modulation). */
export const pvMul = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Mul", [chainA, chainB]);

/**
 * Two-chain combiner: per bin, whichever input has the **smaller**
 * magnitude.
 */
export const pvMin = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Min", [chainA, chainB]);

/**
 * Two-chain combiner: per bin, whichever input has the **larger**
 * magnitude.
 */
export const pvMax = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Max", [chainA, chainB]);

/**
 * Two-chain combiner: A's bins scaled by B's magnitudes -- A's phases kept
 * (a spectral envelope transfer, the classic "vocoder" cross-synthesis).
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
 * while `> 0` rescales every bin to the stored magnitudes -- the spectral
 * envelope holds while the phases keep running.
 */
export const pvMagFreeze = (chain: Channel, freeze: Channel = 0.0): Ugen =>
    new Ugen("PV_MagFreeze", [chain, freeze]);

/**
 * Averages each bin's magnitude over `bins` neighbors on each side
 * (`0` is transparent), phases untouched -- a spectral blur.
 */
export const pvMagSmear = (chain: Channel, bins: Channel = 0.0): Ugen =>
    new Ugen("PV_MagSmear", [chain, bins]);

/**
 * Remaps bin `b` to `round(b * stretch + shift)`: colliding bins sum,
 * out-of-range bins are dropped. `stretch: 1, shift: 0` is transparent; a
 * positive `shift` moves every partial up by `shift` bin widths.
 */
export const pvBinShift = (
    chain: Channel,
    stretch: Channel = 1.0,
    shift: Channel = 0.0,
): Ugen => new Ugen("PV_BinShift", [chain, stretch, shift]);

/**
 * The `pvBinShift` remap applied to the magnitude envelope only, laid
 * over the frame's original phases.
 */
export const pvMagShift = (
    chain: Channel,
    stretch: Channel = 1.0,
    shift: Channel = 0.0,
): Ugen => new Ugen("PV_MagShift", [chain, stretch, shift]);
