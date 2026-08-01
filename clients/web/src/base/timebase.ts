// Time units, and the pacing source a clock measures its sleeps against
// (mirrors `clausters/base/timebase.py`).
//
// Two halves, both about *time*:
//
// - the **conversions** — beats to seconds, seconds to samples, the bar grid,
//   the NTP timetag. Every one of them is `clausters-core`'s own function
//   reached through the wasm door, so a beat resolves to the same second here,
//   in the Python client and in the server. Nothing in this package computes a
//   time by hand.
// - the **timebases** — what a running `TempoClock` reads to decide how long
//   to sleep, and what a `Server` reads to decide how to stamp what it emits.
//
// A `TempoClock`'s logical beat advances only by the routines' yields; the
// timebase never moves it. It only paces, and anchors the emission:
//
// - `MonotonicTimebase` (the default) — `performance.now()`. Events go out as
//   NTP-timetagged bundles; the drift between the page's clock and the
//   server's is small but real.
// - `SampleTimebase` — seconds derived from the server's **sample counter**
//   (`sample() / sampleRate`). The client paces against the server's own
//   clock and the `Server` emits `/sched_at <absolute sample>` instead of a
//   wall-clock timetag, so there is no inter-clock drift and the timing is
//   exact at the sample. Build one with `Server.sampleTimebase()`, which
//   knows how to reach the counter over each carrier.
//
// The core wasm must be loaded first (`await loadCore()`), as everywhere else
// in the package.

import {
    bar as coreBar,
    beat_in_bar as coreBeatInBar,
    beats_to_secs as coreBeatsToSecs,
    quant_delay as coreQuantDelay,
    samples_to_secs as coreSamplesToSecs,
    secs_to_beats as coreSecsToBeats,
    secs_to_samples as coreSecsToSamples,
    unix_to_ntp as coreUnixToNtp,
    unix_to_sample as coreUnixToSample,
} from "../core/clausters_core_web.js";

// ---- the conversions ----

/**
 * Seconds at `beats` on the affine clock `(tempo, baseBeats, baseSecs)` —
 * the pair `(baseBeats, baseSecs)` is the instant a tempo change pinned.
 */
export const beatsToSecs = (
    tempo: number,
    baseBeats: number,
    baseSecs: number,
    beats: number,
): number => coreBeatsToSecs(tempo, baseBeats, baseSecs, beats);

/** Beats at `secs` on the same affine clock. */
export const secsToBeats = (
    tempo: number,
    baseBeats: number,
    baseSecs: number,
    secs: number,
): number => coreSecsToBeats(tempo, baseBeats, baseSecs, secs);

/** Seconds → sample count at `rate` (ties to even, the server's rounding). */
export const secsToSamples = (secs: number, rate: number): number =>
    coreSecsToSamples(secs, rate);

/** Sample count → seconds at `rate`. */
export const samplesToSecs = (samples: number, rate: number): number =>
    coreSamplesToSecs(samples, rate);

/**
 * Beats to wait so a routine starts on the next `quant` boundary of the grid
 * (`quant` 0 or negative starts now).
 */
export const quantDelay = (pos: number, quant: number): number =>
    coreQuantDelay(pos, quant);

/** The 0-based bar index `beats` falls in, on a grid of `quant` beats per bar. */
export const bar = (beats: number, quant: number): number => coreBar(beats, quant);

/** The beat within its bar, in `[0, quant)`. */
export const beatInBar = (beats: number, quant: number): number =>
    coreBeatInBar(beats, quant);

/**
 * A Unix timestamp → the 64 NTP timetag bits. A `bigint`: the wire value is a
 * full 64-bit word and a JS number would drop its low bits.
 */
export const unixToNtp = (unixSecs: number): bigint => coreUnixToNtp(unixSecs);

/**
 * A Unix timestamp → the server's absolute sample, through a `/clock_query` anchor
 * and the measured rate.
 */
export const unixToSample = (
    unixSecs: number,
    anchorUnix: number,
    anchorSample: number,
    rate: number,
): number => coreUnixToSample(unixSecs, anchorUnix, anchorSample, rate);

// ---- the pacing sources ----

/**
 * What a clock paces against: seconds that only move forward. `kind` is what
 * a `Server` reads to decide how to stamp an emission.
 */
export interface Timebase {
    readonly kind: string;
    now(): number;
}

/**
 * The page's monotonic clock. Unaffected by wall-clock steps, and the default
 * for a clock that has not been anchored to a server.
 */
export class MonotonicTimebase implements Timebase {
    readonly kind = "monotonic";

    now(): number {
        return performance.now() / 1000;
    }
}

/**
 * Seconds from a server's sample counter: `sample() / sampleRate`.
 *
 * `sample` is any callable returning the current counter — the page engine's
 * audio clock, or a `/clock_query`-anchored model against a remote server. It must
 * be **synchronous**: the clock reads it on every scheduling turn.
 */
export class SampleTimebase implements Timebase {
    readonly kind = "sample";
    readonly sampleRate: number;
    private readonly sample: () => number;

    constructor(sample: () => number, sampleRate: number) {
        this.sample = sample;
        this.sampleRate = sampleRate;
    }

    now(): number {
        return this.sample() / this.sampleRate;
    }

    /** The server's current sample counter. */
    currentSample(): number {
        return Math.trunc(this.sample());
    }

    /**
     * The absolute sample for a time in *this timebase's* seconds (the core's
     * rounding, shared with the server).
     */
    sampleAt(seconds: number): number {
        return secsToSamples(seconds, this.sampleRate);
    }
}

/**
 * A timebase driven by hand: what tests pace with, so the same code path the
 * browser runs advances deterministically and instantly.
 */
export class ManualTimebase implements Timebase {
    readonly kind = "manual";
    private seconds: number;

    constructor(start = 0) {
        this.seconds = start;
    }

    now(): number {
        return this.seconds;
    }

    /** Moves time forward by `secs` (never backwards). */
    advance(secs: number): void {
        this.seconds += Math.max(secs, 0);
    }

    /** Places time at an absolute second. */
    set(secs: number): void {
        this.seconds = secs;
    }
}
