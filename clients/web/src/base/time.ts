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
 * **The shapes, and their closed forms.** Every shape is written over the
 * segment's *normalised position* `u = (b - b0) / db`, which is what makes the
 * rest of this work. Writing `T0` and `T1` for the tempos at the two ends:
 *
 * | shape | tempo `T(u)` |
 * | --- | --- |
 * | {@link STEP} | `T0` |
 * | {@link LINEAR} | `T0 + (T1 - T0)*u` |
 * | {@link EXPONENTIAL} | `T0 * (T1/T0)**u` |
 * | a curvature `c` | `A + B*exp(c*u)`, with `B = -(T1-T0)/(1-exp(c))` and `A = T0 + (T1-T0)/(1-exp(c))` |
 *
 * A curvature of 0 **is** linear — the knob is continuous through its middle
 * rather than a shape apart — positive starts slow and negative starts fast.
 * That is `Env`'s own convention, and these are `Env`'s own shape numbers, so
 * one vocabulary spells a tempo curve and an amplitude curve.
 *
 * **The seconds** are the integral of `1/T` over the beat axis. Per unit of
 * beat, `K` is that integral from `u = 0` to `u = 1`:
 *
 * | shape | `K` |
 * | --- | --- |
 * | {@link STEP} | `1/T0` |
 * | {@link LINEAR} | `log(T1/T0) / (T1 - T0)` |
 * | {@link EXPONENTIAL} | `(1/T0 - 1/T1) / log(T1/T0)` |
 * | a curvature `c` | `(1 - log((A + B*exp(c))/(A + B))/c) / A` |
 *
 * so a stretch `db` beats wide lasts `db * K` seconds. **This is where an
 * average of the two tempos goes wrong**: over eight beats from 1 to 2 beats a
 * second the true length is `log(2)/0.125 = 5.545` s and the average says
 * `8/1.5 = 5.333` s — a fifth of a second, audible and, drawn, visible.
 *
 * **The extent in seconds** follows from the same `K`, and it is why the shapes
 * are written over `u` rather than over beats: `K` does not depend on how wide
 * the segment is, so asking for a change that lasts `dt` seconds is one
 * division — `db = dt / K` — exact for every shape and never a search. For a
 * straight ramp that makes `db` the logarithmic mean of the two tempos times
 * the seconds.
 *
 * **The inverse** — the beat falling on a second, which a running clock reads
 * on every `TempoClock.beats` — is closed for {@link LINEAR}
 * (`u = T0*(exp(k*s) - 1)/k`, `k = T1 - T0`) and for {@link EXPONENTIAL}
 * (`u = -log(1 - s*T0*log(T1/T0))/log(T1/T0)`). A curvature mixes `u` and
 * `exp(c*u)` and has **no** closed inverse, so the core solves it with a
 * safeguarded Newton iteration — one implementation, so every client inverts to
 * the same place. It is also why `Env`'s `sin` and `wel` are **not** tempo
 * shapes: they integrate in closed form but invert transcendentally, and
 * inverting is the operation a clock cannot pay for on every read.
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

/** An extent is a stretch of the **beat** axis. */
export const BEATS = "beats";
/** An extent is a stretch of wall clock. */
export const SECONDS = "seconds";
/** What an extent measures — a stretch of beats and a stretch of seconds are
 * different stretches under any tempo but a constant one. */
export type TimeUnit = typeof BEATS | typeof SECONDS;

/** A segment's tempo is constant. */
export const STEP = "step";
/** A segment's tempo ramps linearly (in beats) to the next breakpoint. */
export const LINEAR = "linear";
/** A segment's tempo ramps geometrically — equal *ratios* over equal stretches
 * of beat, so 60→120 and 120→240 are the same move. */
export const EXPONENTIAL = "exponential";

/**
 * The shape of a tempo curve: a name, or a numeric curvature where 0 is
 * linear, positive starts slow and negative starts fast.
 */
export type CurveSpec = string | number;

/** An envelope of tempos, of finite duration — `Env`'s shape, without a gate. */
export interface TempoEnvelope {
    /** The tempos, one more than `times`. */
    levels: number[];
    /** The extents, one per segment. */
    times: number[];
    /** The shape of each segment, or one for all of them. */
    curves?: CurveSpec[] | CurveSpec;
    /** Refused for a tempo: a piece's tempo has no gate to sustain on. */
    releaseNode?: number | null;
    /** Refused for a tempo, for the same reason. */
    loopNode?: number | null;
}

// The shapes a tempo curve can take, as the envelope shape numbers the core
// reads. They are `Env`'s own numbers, so one vocabulary spells a tempo curve
// and an amplitude curve -- but only these, plus a numeric curvature, are tempo
// shapes: `sin` and `wel` integrate in closed form and invert transcendentally,
// and inverting is what a running clock does on every read.
const SHAPES: Record<string, number> = {
    step: 0, lin: 1, linear: 1, exp: 2, exponential: 2,
};

/**
 * A shape name or a numeric curvature as the `[number, curvature]` pair the
 * core reads.
 */
export function tempoShape(spec: CurveSpec): [number, number] {
    if (typeof spec === "number") return [5, spec];
    const number = SHAPES[spec];
    if (number === undefined) {
        throw new Error(
            `unknown tempo shape ${JSON.stringify(spec)}; use one of `
            + `${Object.keys(SHAPES).sort().join(", ")} or a numeric curvature`,
        );
    }
    return [number, 0];
}

/**
 * **Writes a whole tempo envelope on `map` from beat `at`** — one more tempo
 * than extents, one shape per segment (one shape for all of them, or a list).
 *
 * The Python client spells this `TempoMap.env(...)`; the map is a generated
 * wasm class here, which cannot grow a method, so the wrapper is a function
 * taking the map. Same arguments, same order, same result.
 *
 * The envelope is of **finite duration**: after its last segment the tempo it
 * reached holds. `unit` says what the extents measure — in `SECONDS` each
 * segment's width in beats is solved exactly rather than searched for.
 */
export function tempoEnv(
    map: TempoMap,
    at: number,
    tempos: number[],
    extents: number[],
    curves: CurveSpec[] | CurveSpec = LINEAR,
    unit: TimeUnit = BEATS,
): void {
    const list = Array.isArray(curves) ? curves : extents.map(() => curves);
    if (list.length !== extents.length) {
        throw new Error(
            `curves (${list.length}) must be one per extent (${extents.length})`,
        );
    }
    const pairs = list.map(tempoShape);
    const ok = map.env(
        at,
        new Float64Array(tempos),
        new Float64Array(extents),
        new Uint32Array(pairs.map((p) => p[0])),
        new Float64Array(pairs.map((p) => p[1])),
        unit === SECONDS,
    );
    if (!ok) {
        throw new Error(
            "an envelope needs one more tempo than extents, every tempo finite "
            + "and > 0, every extent > 0, and shapes a tempo curve has",
        );
    }
}

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
