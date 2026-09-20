// The measurements a view is drawn from.
//
// Pure functions over samples, with no state and no server: hand them a tap
// window, a slice of a buffer, anything. Each is `clausters-core`'s own -- the
// function the GUI host's meter and phasescope draw from -- so a figure
// measured here and the same figure drawn by the host are the same number, not
// two implementations that agree today.
//
// What is *not* here is anything of the **screen**: a decibel curve, an
// oscilloscope's framing and trigger, a row of pixel columns. Those are
// drawing; the host is what draws, and a script that wants to see a signal
// names a view (`scope`, `plot`, a widget in a GuiDef) instead of computing
// one. Nor is anything with memory across frames -- the exponential averaging
// and peak hold of a spectrum display, the rolling history of a scope: how
// long a trace remembers is a look, not a measurement.

import {
    channel_stats,
    loudness as coreLoudness,
    true_peak,
    correlation as coreCorrelation,
    lissajous as coreLissajous,
} from "../core/clausters_core_web.js";

/**
 * The stereo **correlation** (Pearson's r) of two equal-length channels, in
 * `[-1, 1]`: `+1` the same signal, `0` unrelated, `-1` one the other's
 * inverse -- the bar under a phasescope.
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
 * The **true peak** of one channel of an interleaved buffer, in linear
 * amplitude -- the reconstructed peak rather than the largest sample.
 *
 * A signal whose samples all read below full scale can still reconstruct above
 * it, by up to about 3 dB, and every converter sees that peak. The filter is
 * the one ITU-R BS.1770-4 Annex 2 specifies, at 4*, which is what makes a
 * reading dBTP -- so this is the number a delivery specification means when it
 * asks for one, and it is never below {@link channelStats}'s peak. `-1` for a
 * channel the buffer does not have.
 */
export function truePeak(
    samples: Float32Array,
    channels: number,
    channel: number,
): number {
    return true_peak(samples, channels, channel);
}

/**
 * The **peak and RMS** of one channel of an interleaved buffer, as
 * `[peak, rms]` -- what a render reports about what it produced.
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

/**
 * What a loudness measurement reports, as {@link loudness} returns it.
 */
export interface Loudness {
    /** The gated integrated loudness, in LUFS: the programme's loudness. */
    integrated: number;
    /** The loudness range, in LU: how far the short-term loudness spreads. */
    range: number;
    /** The loudest 400 ms, in LUFS. */
    momentaryMax: number;
    /** The loudest 3 s, in LUFS. */
    shortTermMax: number;
}

/**
 * The **loudness** of an interleaved buffer at `rate` Hz, as ITU-R BS.1770 and
 * EBU R 128 define it.
 *
 * A peak says how close a signal came to full scale; loudness says how loud it
 * sounds, which is the number a delivery specification asks for (EBU R 128
 * targets -23 LUFS, streaming services around -14). Each channel is K-weighted
 * -- a high shelf for the head and a high-pass under 38 Hz -- and its mean square
 * summed with the channel weights.
 *
 * The **integrated** loudness is gated at -70 LUFS and 10 LU under what that
 * leaves; the **range** is EBU Tech 3342's spread of the 3 s loudness between
 * its 10th and 95th percentiles; the maxima are the loudest **momentary**
 * (400 ms) and **short-term** (3 s) readings. Silence reads `-Infinity`, and a
 * range with no spread `0`.
 *
 * `weights` is one per channel -- `0` leaves one out, `1.41` is a surround -- or
 * absent for the weights BS.1770 gives a layout known by its count: mono,
 * stereo, L R C, L R Ls Rs, L R C Ls Rs, and L R C LFE Ls Rs for six or more.
 * `undefined` for a request that cannot be met: no channels, a rate under
 * 10 Hz, or weights that are not one per channel.
 */
export function loudness(
    samples: Float32Array,
    channels: number,
    rate: number,
    weights?: readonly number[],
): Loudness | undefined {
    const measured = coreLoudness(
        samples,
        channels,
        rate,
        weights === undefined ? undefined : Float64Array.from(weights),
    );
    if (measured.length !== 4) {
        return undefined;
    }
    const [integrated, range, momentaryMax, shortTermMax] = measured;
    return { integrated, range, momentaryMax, shortTermMax };
}
