// The spectrogram cache: a take analyzed once, as the bytes a view reads.
//
// A spectrogram is drawn from a short-time Fourier transform -- a windowed
// FFT every `hop` samples, the magnitudes normalized over a fixed decibel
// range -- and the transform is a **cache**, like the peak pyramid: computed
// once and handed on. The analysis is `clausters_core::stft`, reached through
// the core's wasm door: the same code the GUI host analyzes with, and the
// same format it reads and the Python client writes. A cache built here and
// the transform the host computes from the samples are the same bytes.
//
// **The cache is data; the picture is the host's.** The frequency scale, the
// decibel window, the colors and how much of a transform is on a card at once
// are decided where the drawing is, and the drawing is never here: a
// `spectrogram` widget names the cache and the host draws it.

import { stftCache as coreStftCache } from "../core/clausters_core_web.js";

/** The options of {@link stftCache}. */
export interface StftOptions {
    /** The analysis window, in samples: a power of two from 256 to 4096. */
    windowSize?: number;
    /** The samples between two columns. */
    hop?: number;
    /** The rate of the samples, which places the frequency axis. */
    sampleRate?: number;
}

/**
 * The **spectrogram cache** of mono `samples`: the transform a `spectrogram`
 * widget draws, as the bytes its `cache` names -- so a host draws a take it
 * never analyzes.
 *
 * A Hann window of `windowSize` samples every `hop`. The hop is raised only
 * for a take longer than a transform keeps (some six minutes at the
 * defaults). One channel a cache: de-interleave a multichannel take and build
 * one per channel.
 *
 * Requires a prior `loadCore()`, like everything core-backed. Throws for a
 * window the FFT has no size for, or a hop under 1.
 */
export function stftCache(
    samples: ArrayLike<number>,
    { windowSize = 1024, hop = 512, sampleRate = 48000 }: StftOptions = {},
): Uint8Array {
    const flat = samples instanceof Float32Array ? samples : Float32Array.from(samples);
    const bytes = coreStftCache(flat, windowSize, hop, sampleRate);
    if (!bytes) {
        throw new RangeError(
            `no spectrogram of windowSize ${windowSize} and hop ${hop}: the window ` +
                "is a power of two from 256 to 4096 and the hop is at least 1",
        );
    }
    return bytes;
}
