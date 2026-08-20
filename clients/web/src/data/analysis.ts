// The measurements a view is drawn from.
//
// Pure functions over samples, with no state and no server: hand them a tap
// window, a slice of a buffer, anything. Each is `clausters-core`'s own — the
// function the GUI host's meter and phasescope draw from — so a figure
// measured here and the same figure drawn by the host are the same number, not
// two implementations that agree today.
//
// What is *not* here is anything of the **screen**: a decibel curve, an
// oscilloscope's framing and trigger, a row of pixel columns. Those are
// drawing; the host is what draws, and a script that wants to see a signal
// names a view (`scope`, `plot`, a widget in a GuiDef) instead of computing
// one. Nor is anything with memory across frames — the exponential averaging
// and peak hold of a spectrum display, the rolling history of a scope: how
// long a trace remembers is a look, not a measurement.

import {
    channel_stats,
    correlation as coreCorrelation,
    lissajous as coreLissajous,
} from "../core/clausters_core_web.js";

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
