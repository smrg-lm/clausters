// Envelopes: the breakpoint builder, the generator, and what `done` frees
// (mirrors `clausters/defs/ugens/env.py`).
//
// `Env` is the shape, `envGen` plays it, and a `DoneAction` says what becomes
// of the node when it ends -- the one place in the package where a UGen's
// completion reaches the node tree.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";
import type { MsgArg } from "../../base/osc.ts";

/**
 * The action `envGen` takes when its envelope finishes -- scsynth's full
 * done-action set (0-15). The relative actions act on the synth's neighbours
 * in its group; a paused node is resumed with `Server.run` (`/node_run`).
 */
export const DoneAction = {
    /** Do nothing; the envelope just holds its final level. */
    NONE: 0,
    /** Pause the synth (stops processing; it stays in the tree). */
    PAUSE_SELF: 1,
    /** Free the synth -- the usual choice for a one-shot or a released note. */
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
 * Envelope shape name -> the server's shape number. A numeric curve value
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
 * A shape name (`"lin"`, `"exp"`, `"sin"`, ...) or a numeric curvature as the
 * wire's `[shape, curve]` pair. A number selects the custom-curvature shape,
 * so a drawn segment and a played one agree by construction -- which is why
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
    levels: number[];
    times: number[];
    curves: Curve[];
    releaseNode?: number;
    loopNode?: number;

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
     * A fixed-duration percussive hit: 0 -> `level` -> 0. No sustain, so a
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
     * A step sequence: **each value held for its duration** -- `levels` and
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
     * This envelope in the **other basis**: the flat `[t, v, shape, curve,
     * ...]` break points a {@link Bpf} is written with, absolute from
     * `timeAt`. The conversion is {@link envToPoints}.
     */
    toPoints({ timeAt = 0.0 }: { timeAt?: number } = {}): number[] {
        return envToPoints(this, { timeAt });
    }

    /**
     * Take this envelope's shape from a break-point list, in place.
     *
     * The other half of {@link Env.toPoints}, and what makes an `Env` editable
     * by anything that draws break points. `releaseNode` and `loopNode`
     * survive when they still index a level and are dropped when they do not --
     * a curve redrawn with fewer points has no say about where the old sustain
     * was.
     */
    setPoints(
        points: PointsLike,
        { timeAt = 0.0, curve }: { timeAt?: number; curve?: Curve | readonly Curve[] } = {},
    ): this {
        const fresh = pointsToEnv(flatQuads(quads(points, curve)), { timeAt });
        this.levels = fresh.levels;
        this.times = fresh.times;
        this.curves = fresh.curves;
        if (this.releaseNode !== undefined && this.releaseNode >= this.levels.length) {
            this.releaseNode = undefined;
        }
        if (this.loopNode !== undefined && this.loopNode >= this.levels.length) {
            this.loopNode = undefined;
        }
        return this;
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
 * list `[t, v, shape, curve, ...]`, with absolute times starting at `timeAt`.
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
 * A `bpf` breakpoint list -- the flat `t v shape curve ...` quads a `"points"`
 * event carries -- as an `Env`: absolute times become segment durations and
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
    // First name wins for the aliased numbers ("lin"/"exp"/... come before
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
 * Plays an `Env` or a {@link Bpf} -- the same envelope in either basis. A
 * rising `gate` (re)triggers from the start; while the
 * gate is held the envelope sustains at the env's release node; when the
 * gate falls it plays the release segments. `levelScale`/`levelBias` affine
 * the output, `timeScale` stretches every segment. `doneAction` is taken
 * when the envelope finishes.
 */
export function envGen(
    env: Env | Bpf,
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
 * 1 once `signal` has stayed within +/-`amp` for `time` seconds, with the
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
 * A single ramp from `start` to `end` over `dur` seconds, then held -- an
 * `envGen` with one linear segment, taking the same `DoneAction` set.
 */
export const line = (
    start: Channel = 0.0,
    end: Channel = 1.0,
    dur: Channel = 1.0,
    doneAction: number = DoneAction.NONE,
): Ugen => new Ugen("Line", [start, end, dur, Number(doneAction)]);

/**
 * `line` in equal *ratios* rather than equal steps -- the shape that reads as
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
 * through unchanged -- the trigger-driven counterpart of a `DoneAction`.
 */
export const freeSelf = (signal: Channel): Ugen => new Ugen("FreeSelf", [signal]);

/**
 * Pauses the enclosing synth while `signal` is greater than zero, passing it
 * through. Resume with `Server.run`.
 */
export const pauseSelf = (signal: Channel): Ugen => new Ugen("PauseSelf", [signal]);

/**
 * 1 once `source` has finished, 0 before -- a trigger the rest of the graph
 * can read. `source` must be a UGen that *can* finish (`envGen`, `line`,
 * `xLine`); the server rejects the def by name otherwise.
 */
export const done = (source: Channel): Ugen => new Ugen("Done", [source]);

/**
 * Passes `source` through and frees the synth once it has finished -- the
 * idiom for an envelope whose own `doneAction` is `NONE` because something
 * else in the graph still needs it.
 */
export const freeSelfWhenDone = (source: Channel): Ugen =>
    new Ugen("FreeSelfWhenDone", [source]);

/**
 * A break point, in any of the spellings {@link quads} reads: `[at, value]`,
 * `[at, value, curve]` with a shape name or a numeric curvature, or the
 * resolved `[at, value, shape, curve]` the `bpf` widget sends.
 */
export type Point = readonly [number, number] | readonly [number, number, Curve] |
    readonly [number, number, number, number];

/** A break-point list: points, or the flat `t v shape curve ...` numbers. */
export type PointsLike = readonly Point[] | readonly number[];

/** A resolved break point: the shape number and the curvature the wire carries. */
export type Quad = [number, number, number, number];

/**
 * A break-point list as `[at, value, shape, curve]` quads.
 *
 * Every spelling of a break point is read here, and a point is written the way
 * an {@link Env} segment is -- **with the curve's name**, not with the server's
 * shape number: `[at, value]` is linear or whatever `curve` says, `[at, value,
 * "exp"]` names its own shape (any value `Env`'s `curve` takes), `[at, value,
 * shape, curve]` is the resolved pair, and a flat number array is that same
 * resolved form, dropping a trailing partial quad rather than guessing at it.
 *
 * `curve` is the shape for the points that do not name one: one for all of
 * them, or an array with one per **segment** (one fewer than the points, as
 * `Env` counts them). A point that names its own shape keeps it.
 */
export function quads(points: PointsLike, curve?: Curve | readonly Curve[]): Quad[] {
    let read: (readonly (number | string)[])[];
    if (points.length > 0 && !Array.isArray(points[0])) {
        read = [];
        const flat = points as readonly number[];
        for (let i = 0; i + 4 <= flat.length; i += 4) read.push(flat.slice(i, i + 4));
    } else {
        read = (points as readonly Point[]).map((point) => [...point]);
    }
    let curves: Curve[];
    if (Array.isArray(curve)) {
        const segments = Math.max(read.length - 1, 0);
        if (curve.length !== segments) {
            throw new TypeError(
                `curve list (${curve.length}) must match the number of ` +
                    `segments (${segments})`,
            );
        }
        curves = [...(curve as readonly Curve[]), "lin"];
    } else {
        // The last point's shape is a placeholder -- no segment leaves it -- so
        // a blanket `curve` stops one short, which is what `envToPoints` writes
        // and what keeps a round trip identical.
        curves = new Array(Math.max(read.length - 1, 0)).fill(
            (curve ?? "lin") as Curve,
        );
        curves.push("lin");
    }
    return read.map((point, i) => {
        let shape: number;
        let curvature: number;
        if (point.length === 2) {
            [shape, curvature] = resolveCurve(curves[i]!);
        } else if (point.length === 3) {
            [shape, curvature] = resolveCurve(point[2] as Curve);
        } else if (point.length === 4) {
            shape = Math.trunc(point[2] as number);
            curvature = Number(point[3]);
        } else {
            throw new TypeError(
                "a break point is [at, value], [at, value, curve] or " +
                    `[at, value, shape, curve]; got ${point.length} numbers`,
            );
        }
        return [Number(point[0]), Number(point[1]), shape, curvature] as Quad;
    });
}

/** Resolved quads back as the flat list the `bpf` widget and a `"points"` event speak. */
export function flatQuads(points: readonly Quad[]): number[] {
    return points.flatMap((point) => [...point]);
}

/**
 * An envelope as the flat `/buffer_gen "env"` argument list: `level0`, then a
 * `(level, time, shape, curve)` quad per segment.
 *
 * The server evaluates it with the same shape maths {@link envGen} plays
 * (`clausters_core::envshape`), so a buffer filled this way and an `EnvGen`
 * reading the same envelope agree. Segment times are **relative** here -- only
 * their proportions matter, since what maps the buffer onto real time is
 * whatever reads it.
 *
 * Tagged rather than inferred, so the bytes are the reference client's: a shape
 * is an int and everything else a float, where the inference rule would send a
 * whole-numbered level as an int.
 */
export function envGenArgs(env: Env | Bpf): MsgArg[] {
    const shape = env instanceof Env ? env : env.toEnv();
    const args: MsgArg[] = [["f", shape.levels[0]!]];
    for (let k = 0; k < shape.times.length; k++) {
        const [number_, curvature] = resolveCurve(shape.curves[k]!);
        args.push(["f", shape.levels[k + 1]!], ["f", shape.times[k]!],
            ["i", number_], ["f", curvature]);
    }
    return args;
}

/**
 * A break-point curve: the same envelope an {@link Env} is, in **absolute
 * coordinates**.
 *
 * One datum, two bases. An `Env` says `levels`, segment `times` and a `curve`
 * per segment, which is what {@link envGen} and `/buffer_gen "env"` read; a
 * `Bpf` says `[at, value, shape, curve]` per point, with `at` an absolute
 * second and the curve belonging to the segment that *leaves* that point, which
 * is what a drawn curve is and what the `bpf` widget sends. The conversion both
 * ways is {@link envToPoints} / {@link pointsToEnv}, and the two are
 * interchangeable wherever a curve is asked for: `envGen` plays either, `plot`
 * draws either, and `gui.edit` opens either.
 *
 * It is a **specification and nothing else** -- no buffer, no bus, no server.
 * Rendering a control curve to something a node can read is the multitrack's
 * job and it is done in the shared crate (`clausters_core::mixer`), once, for
 * every client.
 *
 * ```ts
 * const curve = new Bpf([[0.0, 200.0], [2.0, 4000.0]], { curve: "exp" });
 * play(new Synth("filter", { cutoff: envGen(curve) }));
 * ```
 */
export class Bpf {
    /**
     * The unit this object's length is in -- **seconds**, like the `Env` it is
     * another spelling of.
     */
    static readonly durationUnit = "seconds";

    points: Quad[];
    /**
     * The index of the point the curve sustains at while a gate is held, and
     * the one a loop returns to -- an `Env`'s two, in this basis, since a point
     * here is a level there.
     */
    releaseNode?: number;
    loopNode?: number;

    constructor(
        points: PointsLike,
        {
            curve,
            releaseNode,
            loopNode,
        }: {
            curve?: Curve | readonly Curve[];
            releaseNode?: number;
            loopNode?: number;
        } = {},
    ) {
        this.points = quads(points, curve);
        if (this.points.length < 2) {
            throw new TypeError("a curve needs at least two break points");
        }
        this.releaseNode = releaseNode;
        this.loopNode = loopNode;
    }

    /** The same curve, read out of an `Env`. */
    static fromEnv(env: Env, { timeAt = 0.0 }: { timeAt?: number } = {}): Bpf {
        return new Bpf(envToPoints(env, { timeAt }), {
            releaseNode: env.releaseNode,
            loopNode: env.loopNode,
        });
    }

    /**
     * The same curve as an `Env`, which is what plays it.
     *
     * A first point later than `timeAt` is a drawn initial delay and becomes a
     * leading `hold` segment ({@link pointsToEnv} says why), so the sustain and
     * loop indices move with it.
     */
    toEnv({ timeAt = 0.0 }: { timeAt?: number } = {}): Env {
        const env = pointsToEnv(this.toPoints(), { timeAt });
        const shift = env.levels.length - this.points.length;
        env.releaseNode = this.releaseNode === undefined ? undefined : this.releaseNode + shift;
        env.loopNode = this.loopNode === undefined ? undefined : this.loopNode + shift;
        return env;
    }

    /**
     * The curve as the flat `[t, v, shape, curve, ...]` list the `bpf` widget
     * and a `"points"` event both speak.
     */
    toPoints(): number[] {
        return flatQuads(this.points);
    }

    /**
     * Take this curve's points from a break-point list, in place -- the other
     * half of {@link Bpf.toPoints}, and what an editor writes back through.
     * Reads every spelling {@link quads} does.
     */
    setPoints(points: PointsLike, curve?: Curve | readonly Curve[]): this {
        const fresh = quads(points, curve);
        if (fresh.length < 2) {
            throw new TypeError("a curve needs at least two break points");
        }
        this.points = fresh;
        if (this.releaseNode !== undefined && this.releaseNode >= this.points.length) {
            this.releaseNode = undefined;
        }
        if (this.loopNode !== undefined && this.loopNode >= this.points.length) {
            this.loopNode = undefined;
        }
        return this;
    }

    /**
     * The flat inputs `envGen` appends -- {@link Env.toInputs} of
     * {@link Bpf.toEnv}, so a `Bpf` is played wherever an `Env` is.
     */
    toInputs(): number[] {
        return this.toEnv().toInputs();
    }

    /**
     * The span the curve covers, in **seconds**: its last point's time less its
     * first's.
     */
    duration(): number {
        return this.points[this.points.length - 1]![0] - this.points[0]![0];
    }
}
