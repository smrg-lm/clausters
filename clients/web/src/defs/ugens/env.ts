// Envelopes: the breakpoint builder, the generator, and what `done` frees
// (mirrors `clausters/defs/ugens/env.py`).
//
// `Env` is the shape, `envGen` plays it, and a `DoneAction` says what becomes
// of the node when it ends — the one place in the package where a UGen's
// completion reaches the node tree.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * The action `envGen` takes when its envelope finishes — scsynth's full
 * done-action set (0–15). The relative actions act on the synth's neighbours
 * in its group; a paused node is resumed with `Server.run` (`/node_run`).
 */
export const DoneAction = {
    /** Do nothing; the envelope just holds its final level. */
    NONE: 0,
    /** Pause the synth (stops processing; it stays in the tree). */
    PAUSE_SELF: 1,
    /** Free the synth — the usual choice for a one-shot or a released note. */
    FREE_SELF: 2,
    FREE_SELF_AND_PREV: 3,
    FREE_SELF_AND_NEXT: 4,
    FREE_SELF_AND_FREE_ALL_IN_PREV: 5,
    FREE_SELF_AND_FREE_ALL_IN_NEXT: 6,
    FREE_SELF_TO_HEAD: 7,
    FREE_SELF_TO_TAIL: 8,
    FREE_SELF_PAUSE_PREV: 9,
    FREE_SELF_PAUSE_NEXT: 10,
    FREE_SELF_AND_DEEP_FREE_PREV: 11,
    FREE_SELF_AND_DEEP_FREE_NEXT: 12,
    FREE_ALL_IN_GROUP: 13,
    /** Free the synth's whole enclosing group. */
    FREE_GROUP: 14,
    FREE_SELF_RESUME_NEXT: 15,
} as const;

export type DoneAction = (typeof DoneAction)[keyof typeof DoneAction];

/**
 * Envelope shape name → the server's shape number. A numeric curve value
 * maps to the custom-curvature shape (5) instead.
 */
const SHAPE_NUMBERS: Record<string, number> = {
    step: 0,
    lin: 1,
    linear: 1,
    exp: 2,
    exponential: 2,
    sin: 3,
    sine: 3,
    wel: 4,
    welch: 4,
    sqr: 6,
    squared: 6,
    cub: 7,
    cubed: 7,
    hold: 8,
};

/**
 * A segment shape: a name, or a numeric curvature (0 linear, positive starts
 * slow, negative starts fast).
 */
export type Curve = string | number;

/**
 * A shape name (`"lin"`, `"exp"`, `"sin"`, …) or a numeric curvature as the
 * wire's `[shape, curve]` pair. A number selects the custom-curvature shape,
 * so a drawn segment and a played one agree by construction — which is why
 * the GuiDef `bpf`/`clip` builders resolve their break-points through here.
 */
export function resolveCurve(spec: Curve): [number, number] {
    if (typeof spec === "string") {
        const shape = SHAPE_NUMBERS[spec];
        if (shape === undefined) {
            throw new TypeError(
                `unknown envelope shape '${spec}'; use one of ` +
                    `${[...new Set(Object.keys(SHAPE_NUMBERS))].sort().join(", ")} ` +
                    "or a numeric curvature",
            );
        }
        return [shape, 0.0];
    }
    return [5, Number(spec)];
}

/**
 * A breakpoint envelope: `levels` (one more than `times`), the segment
 * `times` in seconds, and a `curve` per segment (a shape name, a numeric
 * curvature, or an array of either, one per segment).
 *
 * `releaseNode` is the index into `levels` where the envelope sustains while
 * the gate is held (`undefined` = no sustain, plays straight through). Feed
 * it to `envGen`.
 */
export class Env {
    readonly levels: number[];
    readonly times: number[];
    readonly curves: Curve[];
    readonly releaseNode?: number;
    readonly loopNode?: number;

    constructor(
        levels: readonly number[],
        times: readonly number[],
        curve: Curve | readonly Curve[] = "lin",
        options: { releaseNode?: number; loopNode?: number } = {},
    ) {
        this.levels = levels.map(Number);
        this.times = times.map(Number);
        if (this.levels.length !== this.times.length + 1) {
            throw new TypeError(
                `levels (${this.levels.length}) must be one longer than ` +
                    `times (${this.times.length})`,
            );
        }
        if (Array.isArray(curve)) {
            if (curve.length !== this.times.length) {
                throw new TypeError(
                    `curve list (${curve.length}) must match the number of ` +
                        `segments (${this.times.length})`,
                );
            }
            this.curves = [...(curve as readonly Curve[])];
        } else {
            this.curves = this.times.map(() => curve as Curve);
        }
        this.releaseNode = options.releaseNode;
        this.loopNode = options.loopNode;
    }

    /**
     * A fixed-duration percussive hit: 0 → `level` → 0. No sustain, so a
     * rising gate triggers the whole thing.
     */
    static perc(attack = 0.01, release = 1.0, level = 1.0, curve: Curve = -4.0): Env {
        return new Env([0.0, level, 0.0], [attack, release], curve);
    }

    /**
     * The classic attack/decay/sustain/release. Sustains at `peak * sustain`
     * (the release node) until the gate falls.
     */
    static adsr(
        attack = 0.01,
        decay = 0.3,
        sustain = 0.5,
        release = 1.0,
        peak = 1.0,
        curve: Curve = -4.0,
    ): Env {
        return new Env(
            [0.0, peak, peak * sustain, 0.0],
            [attack, decay, release],
            curve,
            { releaseNode: 2 },
        );
    }

    /** Attack to `sustain`, hold there until release, then fall to 0. */
    static asr(attack = 0.01, sustain = 1.0, release = 1.0, curve: Curve = -4.0): Env {
        return new Env([0.0, sustain, 0.0], [attack, release], curve, {
            releaseNode: 1,
        });
    }

    /**
     * A step sequence: **each value held for its duration** — `levels` and
     * `times` have the *same* length, unlike the constructor.
     */
    static step(
        levels: readonly number[],
        times: readonly number[],
        options: { releaseNode?: number; loopNode?: number } = {},
    ): Env {
        if (levels.length !== times.length) {
            throw new TypeError(
                `Env.step: levels (${levels.length}) and times ` +
                    `(${times.length}) must have the same length`,
            );
        }
        if (levels.length === 0) throw new TypeError("Env.step needs at least one level");
        return new Env([levels[0]!, ...levels], times, "step", options);
    }

    /**
     * The envelope as the flat number list `envGen` appends after its fixed
     * inputs: `initLevel, numSegments, releaseNode, loopNode` then `target,
     * duration, shape, curve` per segment.
     */
    toInputs(): number[] {
        const n = this.times.length;
        const rel = this.releaseNode ?? -1.0;
        const loop = this.loopNode ?? -1.0;
        const out: number[] = [this.levels[0]!, n, rel, loop];
        for (let i = 0; i < n; i++) {
            const [shape, cval] = resolveCurve(this.curves[i]!);
            out.push(this.levels[i + 1]!, this.times[i]!, shape, cval);
        }
        return out;
    }
}

/**
 * An `Env` (levels / segment times / curves) as the flat `bpf` breakpoint
 * list `[t, v, shape, curve, …]`, with absolute times starting at `timeAt`.
 * The last point carries a linear placeholder (no segment leaves it). Feed
 * the result to the `bpf` widget or to a live `points` set.
 */
export function envToPoints(env: Env, { timeAt = 0.0 }: { timeAt?: number } = {}): number[] {
    const out: number[] = [];
    let t = timeAt;
    for (let i = 0; i < env.levels.length; i++) {
        const [shape, curve] =
            i < env.times.length ? resolveCurve(env.curves[i]!) : [1, 0.0];
        out.push(t, env.levels[i]!, shape, curve);
        if (i < env.times.length) t += env.times[i]!;
    }
    return out;
}

/**
 * A `bpf` breakpoint list — the flat `t v shape curve …` quads a `"points"`
 * event carries — as an `Env`: absolute times become segment durations and
 * each segment keeps its shape (the numeric curvature for the custom shape,
 * the shape name otherwise).
 *
 * A first breakpoint later than `timeAt` (default `0.0`) is a drawn initial
 * delay, encoded as a leading `hold` segment (the first level held for that
 * duration) so what was drawn and what plays stay identical. `releaseNode`
 * and `loopNode` pass through to the `Env`.
 */
export function pointsToEnv(
    points: readonly number[],
    {
        timeAt = 0.0,
        releaseNode,
        loopNode,
    }: { timeAt?: number; releaseNode?: number; loopNode?: number } = {},
): Env {
    const quads: number[][] = [];
    for (let i = 0; i + 4 <= points.length; i += 4) {
        quads.push(points.slice(i, i + 4) as number[]);
    }
    if (quads.length < 2) {
        throw new TypeError("an envelope needs at least two breakpoints");
    }
    // First name wins for the aliased numbers ("lin"/"exp"/… come before
    // their long forms in the table).
    const names = new Map<number, string>();
    for (const [name, num] of Object.entries(SHAPE_NUMBERS)) {
        if (!names.has(num)) names.set(num, name);
    }
    const levels = quads.map((q) => q[1]!);
    const times = quads.slice(1).map((q, i) => q[0]! - quads[i]![0]!);
    const curves: Curve[] = quads
        .slice(0, -1)
        .map((q) => (Math.trunc(q[2]!) === 5 ? q[3]! : (names.get(Math.trunc(q[2]!)) ?? "lin")));
    const delay = quads[0]![0]! - timeAt;
    if (delay > 1e-9) {
        levels.unshift(levels[0]!);
        times.unshift(delay);
        curves.unshift("hold");
    }
    return new Env(levels, times, curves, { releaseNode, loopNode });
}

/**
 * Plays an `Env`. A rising `gate` (re)triggers from the start; while the
 * gate is held the envelope sustains at the env's release node; when the
 * gate falls it plays the release segments. `levelScale`/`levelBias` affine
 * the output, `timeScale` stretches every segment. `doneAction` is taken
 * when the envelope finishes.
 */
export function envGen(
    env: Env,
    {
        gate: gateInput = 1.0,
        levelScale = 1.0,
        levelBias = 0.0,
        timeScale = 1.0,
        doneAction = DoneAction.NONE,
    }: {
        gate?: Channel;
        levelScale?: Channel;
        levelBias?: Channel;
        timeScale?: Channel;
        doneAction?: number;
    } = {},
): Ugen {
    return new Ugen("EnvGen", [
        gateInput,
        levelScale,
        levelBias,
        timeScale,
        Number(doneAction),
        ...env.toInputs(),
    ]);
}

/**
 * 1 once `signal` has stayed within ±`amp` for `time` seconds, with the
 * `doneAction` taken then. The counter restarts on the first sample that
 * exceeds `amp`, so what it measures is *uninterrupted* silence.
 */
export const detectSilence = (
    signal: Channel,
    amp: Channel = 0.0001,
    time: Channel = 0.1,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("DetectSilence", [signal, amp, time, Number(doneAction)]);

/**
 * A single ramp from `start` to `end` over `dur` seconds, then held — an
 * `envGen` with one linear segment, taking the same `DoneAction` set.
 */
export const line = (
    start: Channel = 0.0,
    end: Channel = 1.0,
    dur: Channel = 1.0,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("Line", [start, end, dur, Number(doneAction)]);

/**
 * `line` in equal *ratios* rather than equal steps — the shape that reads as
 * straight when it drives a frequency or a gain. `start` and `end` must be
 * non-zero and share a sign.
 */
export const xLine = (
    start: Channel = 0.01,
    end: Channel = 1.0,
    dur: Channel = 1.0,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("XLine", [start, end, dur, Number(doneAction)]);

/**
 * Frees the enclosing synth while `signal` is greater than zero, passing it
 * through unchanged — the trigger-driven counterpart of a `DoneAction`.
 */
export const freeSelf = (signal: Channel): Ugen => new Ugen("FreeSelf", [signal]);

/**
 * Pauses the enclosing synth while `signal` is greater than zero, passing it
 * through. Resume with `Server.run`.
 */
export const pauseSelf = (signal: Channel): Ugen => new Ugen("PauseSelf", [signal]);

/**
 * 1 once `source` has finished, 0 before — a trigger the rest of the graph
 * can read. `source` must be a UGen that *can* finish (`envGen`, `line`,
 * `xLine`); the server rejects the def by name otherwise.
 */
export const done = (source: Channel): Ugen => new Ugen("Done", [source]);

/**
 * Passes `source` through and frees the synth once it has finished — the
 * idiom for an envelope whose own `doneAction` is `NONE` because something
 * else in the graph still needs it.
 */
export const freeSelfWhenDone = (source: Channel): Ugen =>
    new Ugen("FreeSelfWhenDone", [source]);
