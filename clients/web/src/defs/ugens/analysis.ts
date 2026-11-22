// Analysis: the measures read off an FFT chain, the onset and beat trackers,
// and the pitch follower (mirrors `clausters/defs/ugens/analysis.py`).
//
// The measures of a frame (`specCentroid`, `loudness`, `mfcc`, ...) and the
// onset and beat detectors read a chain an `fft` opens -- they leave it as it
// was, so a chain can feed any number of them and an `ifft` besides -- and
// hold their value between frames. `pitch` reads a signal and `beatTrack2`
// features a def writes to control buses. A UGen with several outputs is a
// node a channel, and its builder returns them as a `ChannelList`.

import { ChannelList, Ugen, chans } from "./graph.ts";
import type { Channel } from "./graph.ts";
export * from "./_generated/analysis.ts";

const range = (n: number) => Array.from({ length: n }, (_, i) => i);

/**
 * The spectrum's mel-frequency cepstral coefficients, from the first: the
 * power in 42 bands evenly spaced in mels from 80 Hz to 18 kHz, as dB,
 * through a cosine transform, scaled into about 0 to 1. `numcoeff` (1 to 42)
 * is how many, a constant. Returns `numcoeff` channels.
 */
export const mfcc = (chain: Channel, numcoeff = 13): ChannelList => {
    if (!Number.isInteger(numcoeff) || numcoeff < 1 || numcoeff > 42) {
        throw new RangeError(`mfcc needs 1 to 42 coefficients, got ${numcoeff}`);
    }
    return chans(range(numcoeff).map((c) => new Ugen("MFCC", [chain, numcoeff, c])));
};

/**
 * The beat of the audio `chain` analyses, by Davies and Plumbley's
 * context-dependent tracker: a trigger on each beat, on each half beat and on
 * each quarter, and the tempo in beats a second. While `lock` is above 0.5
 * the outputs keep their period and phase and the tracker goes on following.
 * Tuned to a 1024-point chain at a half-frame hop at 44.1 or 48 kHz (about
 * 11 ms a frame). Returns the four channels.
 */
export const beatTrack = (chain: Channel, lock: Channel = 0.0): ChannelList =>
    chans(range(4).map((c) => new Ugen("BeatTrack", [chain, lock, c])));

/**
 * The beat of `numfeatures` features (onset strengths, say) on the control
 * buses from `busindex`: twice a second their last `windowsize` seconds are
 * matched against a template of four events a beat, straight and swung, at
 * each tempo from 60 to 179 beats a minute and every phase to
 * `phaseaccuracy` seconds. `weightingscheme` is -2.5 for flat weights, -1.5
 * to compensate each tempo for the events that fit the window, or a buffer
 * of 120 weights, one a tempo. `numfeatures`, `windowsize`, `phaseaccuracy`
 * and `weightingscheme` are constants; `lock` above 0.5 holds the outputs.
 * Returns six channels: the beat, half-beat and quarter-beat triggers, the
 * tempo in beats a second, the beat's phase (0 to 1) and the groove (0
 * straight, 1 swung).
 */
export const beatTrack2 = (
    busindex: Channel,
    numfeatures: Channel,
    windowsize: Channel = 2.0,
    phaseaccuracy: Channel = 0.02,
    lock: Channel = 0.0,
    weightingscheme: Channel = -2.1,
): ChannelList =>
    chans(
        range(6).map(
            (c) =>
                new Ugen("BeatTrack2", [busindex, numfeatures, windowsize, phaseaccuracy, lock, weightingscheme, c]),
        ),
    );

/**
 * The frequency of `signal` by autocorrelation, and whether it has one.
 * `execFreq` times a second, if `signal` reaches `ampThreshold`, the first
 * autocorrelation peak above `peakThreshold` of its zero lag (searched at
 * `maxBinsPerOctave` lags an octave) is the period; a frequency between
 * `minFreq` and `maxFreq` is taken, through a median of the last `median`.
 * `downSample` reads every so many samples. Every argument but `signal` is a
 * constant. Returns two channels: the frequency (`initFreq` before the
 * first), and 1 when the last run found one (with `clar > 0` the peak's
 * clarity, 0 to 1) or 0.
 */
export const pitch = (
    signal: Channel,
    initFreq: Channel = 440.0,
    minFreq: Channel = 60.0,
    maxFreq: Channel = 4000.0,
    execFreq: Channel = 100.0,
    maxBinsPerOctave: Channel = 16,
    median: Channel = 1,
    ampThreshold: Channel = 0.01,
    peakThreshold: Channel = 0.5,
    downSample: Channel = 1,
    clar: Channel = 0,
): ChannelList => {
    const params = [
        initFreq,
        minFreq,
        maxFreq,
        execFreq,
        maxBinsPerOctave,
        median,
        ampThreshold,
        peakThreshold,
        downSample,
        clar,
    ];
    return chans(range(2).map((c) => new Ugen("Pitch", [signal, ...params, c])));
};
