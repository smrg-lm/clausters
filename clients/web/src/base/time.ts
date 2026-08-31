/**
 * Time: the piece's beat↔second map, and the questions it answers.
 *
 * A **beat is not a unit of time**. It is a logical coordinate, and what turns
 * one into a second is the tempo — which can change along the piece. So the two
 * things the word "tempo" covers are kept apart here:
 *
 * - the **tempo function**, what a user writes: the tempo at a beat, and how it
 *   moves from there (a step, a ramp);
 * - the **time map** ({@link TempoMap}), what everything queries: the second a
 *   beat falls on. It is the integral of `1 / tempo` over the beat axis, and it
 *   is computed once, in the native core, so every client and the editor answer
 *   from one implementation.
 *
 * The rule that follows, and the reason this module exists rather than a
 * `beats / tempo` in each caller: **a length in beats is not a duration**. The
 * same four beats last different seconds depending on where they sit, so
 * seconds come from two *positions* (`TempoMap.spanSecs`), never from a beat
 * count and a tempo.
 *
 * A {@link TempoClock} holds a map and reads it to pace and to stamp; this
 * module is the other half — the same map read as a **question about the
 * piece**, with no clock running and nothing playing:
 *
 * ```ts
 * const tempo = new TempoMap(1.0);        // one beat a second
 * tempo.ramp(8.0, 16.0, 1.0, 2.0);        // accelerate over bars 3-4
 * tempo.secsAt(16.0);                     // when does bar 5 arrive?   13.545...
 * tempo.spanSecs(8.0, 16.0);              // how long is the accelerando? 5.545...
 * tempo.spanBeats(0.0, 30.0);             // what fits in 30 seconds?  48.909...
 * ```
 *
 * The free conversions beside it are the rest of the time seam every client
 * shares — the beat grid (`bar`, `beatInBar`, `quantDelay`) and the sample axis
 * (`secsToSamples`, `samplesToSecs`) — re-exported here so the whole of "what
 * time is it, in which unit" reads from one import.
 */

import {
    bar as coreBar,
    beat_in_bar as coreBeatInBar,
    quant_delay as coreQuantDelay,
    samples_to_secs as coreSamplesToSecs,
    secs_to_samples as coreSecsToSamples,
    TempoMap,
} from "../core/clausters_core_web.js";

export { TempoMap };

/** A segment's tempo is constant. */
export const STEP = "step";
/** A segment's tempo ramps linearly (in beats) to the next breakpoint. */
export const LINEAR = "linear";

/**
 * The bar a beat position falls in, on a grid of `quant` beats per bar
 * (0-based; `quant <= 0` → bar 0).
 *
 * A bar count is a reading of the *beat* axis, so it needs no map: bars are
 * beats grouped, not seconds grouped.
 */
export function bar(beats: number, quant: number): number {
    return coreBar(beats, quant);
}

/**
 * The beat within its bar, on a grid of `quant` beats per bar (0-based). The
 * other half of {@link bar}.
 */
export function beatInBar(beats: number, quant: number): number {
    return coreBeatInBar(beats, quant);
}

/**
 * Beats to wait from `pos` for the next `quant` boundary (a position already on
 * one waits 0; `quant <= 0` → now).
 *
 * The shared quantization rule every client applies, and what `play`'s `quant`
 * argument is computed with.
 */
export function quantDelay(pos: number, quant: number): number {
    return coreQuantDelay(pos, quant);
}

/**
 * Seconds → a sample count at `sampleRate`, rounded the way the server rounds.
 * A length of audio crosses on this and never on a tempo: its seconds were
 * fixed before any tempo was.
 */
export function secsToSamples(secs: number, sampleRate: number): number {
    return coreSecsToSamples(secs, sampleRate);
}

/**
 * A sample count → seconds at `sampleRate` — the inverse of
 * {@link secsToSamples}.
 */
export function samplesToSecs(samples: number, sampleRate: number): number {
    return coreSamplesToSecs(samples, sampleRate);
}
