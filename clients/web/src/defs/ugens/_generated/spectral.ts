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
 * Brick-wall band limit: `wipe > 0` zeroes that fraction of the bins from
 * the bottom up, DC included (a high pass), `wipe < 0` from the top down,
 * Nyquist included (a low pass), `0` passes everything (`wipe` in -1..1).
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
 * envelope holds while the phases keep running. The first frame is always
 * stored, so a chain frozen from its start holds its first spectrum.
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
 * positive `shift` moves every partial up by `shift` bin widths. With
 * `interp` above 0 a bin is split between the two bins around
 * `b * stretch + shift`, in proportion to how near it lands to each.
 */
export const pvBinShift = (
    chain: Channel,
    stretch: Channel = 1.0,
    shift: Channel = 0.0,
    interp: Channel = 0.0,
): Ugen => new Ugen("PV_BinShift", [chain, stretch, shift, interp]);

/**
 * The `pvBinShift` remap applied to the magnitude envelope only, laid
 * over the frame's original phases.
 */
export const pvMagShift = (
    chain: Channel,
    stretch: Channel = 1.0,
    shift: Channel = 0.0,
): Ugen => new Ugen("PV_MagShift", [chain, stretch, shift]);

/**
 * Squares every bin's magnitude, phases kept: the loud partials stand out
 * further from the rest (and the frame's level rises with its square).
 */
export const pvMagSquared = (chain: Channel): Ugen => new Ugen("PV_MagSquared", [chain]);

/**
 * Multiplies every bin by its own random factor in `[-1, 1)`, drawn again
 * each frame: the spectrum's shape turned to noise.
 */
export const pvMagNoise = (chain: Channel): Ugen => new Ugen("PV_MagNoise", [chain]);

/**
 * The complex conjugate of every bin: each phase negated, which plays each
 * frame backwards in time (DC and Nyquist, which are real, stay).
 */
export const pvConj = (chain: Channel): Ugen => new Ugen("PV_Conj", [chain]);

/** Turns every bin's phase a quarter turn forward (DC and Nyquist stay). */
export const pvPhaseShift90 = (chain: Channel): Ugen =>
    new Ugen("PV_PhaseShift90", [chain]);

/** Turns every bin's phase a quarter turn back (DC and Nyquist stay). */
export const pvPhaseShift270 = (chain: Channel): Ugen =>
    new Ugen("PV_PhaseShift270", [chain]);

/**
 * Turns every bin's phase by `shift` radians. With `integrate` above 0 the
 * shift adds up from frame to frame, so a constant `shift` keeps turning.
 */
export const pvPhaseShift = (
    chain: Channel,
    shift: Channel,
    integrate: Channel = 0.0,
): Ugen => new Ugen("PV_PhaseShift", [chain, shift, integrate]);

/**
 * Two-chain combiner: per-bin complex division, A / B (a deconvolution).
 * A bin where B is silent is 0.
 */
export const pvDiv = (chainA: Channel, chainB: Channel): Ugen =>
    new Ugen("PV_Div", [chainA, chainB]);

/**
 * Two-chain combiner: A's magnitudes divided by B's, A's phases kept; each
 * B magnitude is taken as at least `zeroed`, so a quiet bin of B does not
 * blow up.
 */
export const pvMagDiv = (
    chainA: Channel,
    chainB: Channel,
    zeroed: Channel = 0.0001,
): Ugen => new Ugen("PV_MagDiv", [chainA, chainB, zeroed]);

/**
 * Two-chain combiner: A with a run of B's bins in it. `wipe > 0` takes that
 * fraction of B's bins from the bottom up, `wipe < 0` from the top down;
 * `0` is A, `1` or `-1` is B.
 */
export const pvBinWipe = (chainA: Channel, chainB: Channel, wipe: Channel = 0.0): Ugen =>
    new Ugen("PV_BinWipe", [chainA, chainB, wipe]);

/**
 * A comb over the bins: `numTeeth` teeth across the spectrum, the comb
 * starting at `phase` and each tooth `width` of a period wide (both
 * fractions of a period); the bins between the teeth are zeroed.
 */
export const pvRectComb = (
    chain: Channel,
    numTeeth: Channel = 0.0,
    phase: Channel = 0.0,
    width: Channel = 0.5,
): Ugen => new Ugen("PV_RectComb", [chain, numTeeth, phase, width]);

/**
 * Two-chain combiner: `pvRectComb`'s comb, with the bins between the
 * teeth taken from B instead of zeroed.
 */
export const pvRectComb2 = (
    chainA: Channel,
    chainB: Channel,
    numTeeth: Channel = 0.0,
    phase: Channel = 0.0,
    width: Channel = 0.5,
): Ugen => new Ugen("PV_RectComb2", [chainA, chainB, numTeeth, phase, width]);

/**
 * Zeroes a random `wipe` fraction of the bins, chosen in an order drawn at
 * the first frame and again at each `trig`; at `wipe: 1` every bin is zeroed.
 */
export const pvRandComb = (
    chain: Channel,
    wipe: Channel = 0.0,
    trig: Channel = 0.0,
): Ugen => new Ugen("PV_RandComb", [chain, wipe, trig]);

/**
 * Two-chain combiner: a random `wipe` fraction of the bins taken from B, in
 * an order drawn at the first frame and again at each `trig`; `0` is A,
 * `1` is B.
 */
export const pvRandWipe = (
    chainA: Channel,
    chainB: Channel,
    wipe: Channel = 0.0,
    trig: Channel = 0.0,
): Ugen => new Ugen("PV_RandWipe", [chainA, chainB, wipe, trig]);

/**
 * Moves a random `wipe` fraction of the bins to random places nearby: each
 * comes from up to `width` (a fraction of the bins) away. The arrangement
 * is drawn at the first frame and again at each `trig`.
 */
export const pvBinScramble = (
    chain: Channel,
    wipe: Channel = 0.0,
    width: Channel = 0.2,
    trig: Channel = 0.0,
): Ugen => new Ugen("PV_BinScramble", [chain, wipe, width, trig]);

/**
 * Turns each bin's phase by its own random amount, drawn at the first
 * frame and again at each `trig`: the magnitudes stay, the transients
 * smear.
 */
export const pvDiffuser = (chain: Channel, trig: Channel = 0.0): Ugen =>
    new Ugen("PV_Diffuser", [chain, trig]);

/**
 * Keeps the bins louder than both neighbors and at least `threshold`,
 * zeroing the rest: the spectrum's peaks alone.
 */
export const pvLocalMax = (chain: Channel, threshold: Channel = 0.0): Ugen =>
    new Ugen("PV_LocalMax", [chain, threshold]);

/**
 * Maps each bin `z` to `(z - a) / (1 - conj(a) z)`, with `a` the complex
 * number `areal + i aimag`: a Moebius map of the complex plane, a spectral
 * distortion with no counterpart in time.
 */
export const pvConformalMap = (
    chain: Channel,
    areal: Channel = 0.0,
    aimag: Channel = 0.0,
): Ugen => new Ugen("PV_ConformalMap", [chain, areal, aimag]);

/**
 * A copy of `chain` as a chain of its own, so one analysis feeds two
 * branches: what the def does to one after the copy leaves the other as
 * it was.
 */
export const pvCopy = (chain: Channel): Ugen => new Ugen("PV_Copy", [chain]);

/**
 * One bin of a frame as a control signal: its magnitude (`whichmeasure: 0`)
 * or its phase (`1`), updated at each frame and held between them. It
 * reads the chain and leaves it as it was.
 */
export const unpack1Fft = (
    chain: Channel,
    binindex: Channel,
    whichmeasure: Channel = 0.0,
): Ugen => new Ugen("Unpack1FFT", [chain, binindex, whichmeasure]);

/**
 * Convolves two signals: each `framesize` samples of `source` with the
 * same samples of `kernel`, so the response changes as the kernel plays --
 * a cross-synthesis. `framesize` (a power of two, a constant) is also the
 * latency.
 */
export const convolution = (
    source: Channel,
    kernel: Channel,
    framesize: Channel = 512.0,
): Ugen => new Ugen("Convolution", [source, kernel, framesize]);

/**
 * Convolves `source` with the samples in buffer `kernel` (channel 0, at
 * most `framesize` of them), read at the start and again at each
 * `trigger`, switching at the next frame. `framesize` (a power of two, a
 * constant) is the latency; for a longer kernel use `partConv`.
 */
export const convolution2 = (
    source: Channel,
    kernel: Channel,
    trigger: Channel = 0.0,
    framesize: Channel = 2048.0,
): Ugen => new Ugen("Convolution2", [source, kernel, trigger, framesize]);

/**
 * `convolution2`, fading to a kernel read at a `trigger` over `crossfade`
 * frames instead of switching.
 */
export const convolution2L = (
    source: Channel,
    kernel: Channel,
    trigger: Channel = 0.0,
    framesize: Channel = 2048.0,
    crossfade: Channel = 1.0,
): Ugen => new Ugen("Convolution2L", [source, kernel, trigger, framesize, crossfade]);

/**
 * Convolves `source` with the samples in buffer `kernel` (channel 0, at
 * most `framesize` of them) directly in time: no latency, and a cost per
 * sample of the kernel's length, so for short kernels. The kernel is read
 * at the start and again at each `trigger`.
 */
export const convolution3 = (
    source: Channel,
    kernel: Channel,
    trigger: Channel = 0.0,
    framesize: Channel = 2048.0,
): Ugen => new Ugen("Convolution3", [source, kernel, trigger, framesize]);
