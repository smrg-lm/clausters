// The measurements a view is drawn from.
//
// Pure functions over samples, with no state and no server: hand them a tap
// window, a slice of a buffer, anything. Each is `clausters-core`'s own — the
// function the GUI host's meter, phasescope and spectrum draw from — so a
// figure computed here and the same figure drawn by the host are the same
// number, not two implementations that agree today.
//
// What is *not* here is everything with memory across frames: the exponential
// averaging and peak hold of a spectrum display, the rolling history of a
// scope. Those are display smoothing, and belong to whoever draws — how long
// a trace remembers is a look, not a measurement.

import {
    channel_stats,
    correlation as coreCorrelation,
    lissajous as coreLissajous,
    spectrum_db,
} from "../core/clausters_core_web.js";

/** The analysis windows the FFT applies, by name (the wire's `wintype`). */
export type WindowShape =
    | "rectangular"
    | "hann"
    | "sine"
    | "welch"
    | "hamming"
    | "blackman";

const WINTYPE: Record<WindowShape, number> = {
    rectangular: -1,
    hann: 0,
    sine: 1,
    welch: 2,
    hamming: 3,
    blackman: 4,
};

/**
 * The stereo **correlation** (Pearson's r) of two equal-length channels, in
 * `[-1, 1]`: `+1` the same signal, `0` unrelated, `-1` one the other's
 * inverse — the bar under a phasescope.
 *
 * `undefined` where it is undefined: a length mismatch, an empty pair, or a
 * constant channel (silence has no correlation with anything).
 */
export function correlation(
    left: Float32Array,
    right: Float32Array,
): number | undefined {
    return coreCorrelation(left, right);
}

/**
 * The **Lissajous** (goniometer) projection of a stereo pair: one `[x, y]`
 * point per frame, `x` the side signal and `y` the mid, interleaved. Mono
 * draws a vertical line, anti-phase a horizontal one, a wide field fills the
 * lozenge. Empty when the channels differ in length.
 */
export function lissajous(left: Float32Array, right: Float32Array): Float32Array {
    return coreLissajous(left, right);
}

/**
 * One spectrum frame: `samples` windowed, transformed, and scaled to decibels
 * — `fftSize / 2` bins, a full-scale sine reading about 0 dB at its bin and
 * silence sitting at the -120 dB reference floor. Bin `b` is at
 * `b * sampleRate / fftSize` hertz.
 *
 * `samples` should be exactly one `fftSize` window (a shorter one is
 * zero-padded); `fftSize` must be a supported power of two, or the result is
 * empty.
 */
export function spectrumDb(
    samples: Float32Array,
    {
        fftSize = 1024,
        window = "hann",
    }: { fftSize?: number; window?: WindowShape } = {},
): Float32Array {
    return spectrum_db(samples, fftSize, WINTYPE[window] ?? 0);
}

/**
 * The **peak and RMS** of one channel of an interleaved buffer, as
 * `[peak, rms]` — what a render reports about what it produced.
 *
 * The stride walk measures without deinterleaving first, so these are the same
 * two numbers the server and the Python client report for the same audio. An
 * empty pair for a channel the buffer does not have.
 */
export function channelStats(
    samples: Float32Array,
    channels: number,
    channel: number,
): number[] {
    return [...channel_stats(samples, channels, channel)];
}
