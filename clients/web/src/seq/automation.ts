// Automation: a break-point control curve as a control vector (buffer).
// Mirrors `clausters/seq/automation.py`.
//
// An `Automation` places a break-point curve on the timeline that drives one or
// more `[node, control]` targets. It is rendered as a **control vector**: the
// curve is discretized on the server into a control buffer (`/buffer_gen "env"`,
// evaluated through the same envelope-shape math the `EnvGen` UGen plays), and
// a small control synth reads that buffer onto a control bus which the targets
// follow via `/node_map`. The stored curve is an `Env` — the same object the
// `bpf` editor round-trips through `envToPoints`/`pointsToEnv`.
//
// Two phases, for the same reason as in the reference client: `prepare`
// allocates and fills the buffer and allocates the bus, and is the half that
// waits (it `await`s, at setup); `play` — the timeline-item hook — only
// *schedules* the lane synth, the `/node_map`s and the `/node_free`, and never
// waits at all, so it is callable from inside a routine.
//
// The offline half of the reference module has no counterpart here: there is no
// score destination in this client, so `play` cannot self-prepare and says so
// (`clients/web/PLAN.md`, W13).

import { main } from "../base/main.ts";
import { Buffer } from "../defs/buffer.ts";
import { Bus } from "../defs/bus.ts";
import { AddAction, Node, ROOT_NODE_ID, nodeId } from "../defs/node.ts";
import type { NodeLike } from "../defs/node.ts";
import type { Server } from "../defs/server/index.ts";
import { SynthDef } from "../defs/synthdef.ts";
import {
    bufFrames,
    control,
    envToPoints,
    outCtl,
    playBuf,
    pointsToEnv,
    resolveCurve,
    sampleRate,
} from "../defs/ugens/index.ts";
import type { Env } from "../defs/ugens/index.ts";
import type { MsgArg, TimedMessage } from "../base/osc.ts";
import { Moment } from "../base/moment.ts";

/** The internal control-synth def name. */
export const LANE_DEF = "clausters.auto_lane";
/** The default curve resolution, in buffer frames. */
export const DEFAULT_FRAMES = 1024;

/**
 * The internal control synth that plays a control buffer onto a control bus
 * over `dur` seconds. The playback rate is derived from the buffer length and
 * the engine sample rate, so the whole buffer spans `dur` whatever the sample
 * rate is; the client passes only `buf`, `bus` and `dur`.
 */
export function autoLaneDef(): SynthDef {
    const buf = control("buf", 0.0, { rate: "ir" });
    const bus = control("bus", 0.0, { rate: "ir" });
    const dur = control("dur", 1.0, { rate: "ir" });
    const rate = bufFrames(buf).div(dur.mul(sampleRate()));
    const sig = playBuf(buf, 0.0, rate, 0.0);
    return new SynthDef(LANE_DEF, outCtl(bus, sig));
}

/** The servers the lane def has been sent to, so it is sent once each. */
const registered = new WeakSet<Server>();

/** Registers the automation lane def on `server` once (idempotent). */
export async function addAutomationDef(
    server: Server,
    { wait = true }: { wait?: boolean } = {},
): Promise<void> {
    if (registered.has(server)) return;
    registered.add(server);
    await autoLaneDef().send(server, { wait });
}

/** One `[node, control]` pair the curve drives. */
export type AutomationTarget = [NodeLike, string];

/** A single target or a list of them. */
export type AutomationTargets = AutomationTarget | readonly AutomationTarget[];

function normTargets(target?: AutomationTargets | null): AutomationTarget[] {
    if (target === null || target === undefined) return [];
    const first = (target as readonly unknown[])[0];
    if (target.length === 2 && !Array.isArray(first)) {
        return [target as AutomationTarget];
    }
    return (target as readonly AutomationTarget[]).map((t) => [t[0], t[1]]);
}

/**
 * The flat `/buffer_gen "env"` argument list: `level0`, then a
 * `(level, time, shape, curve)` quad per segment. The times are relative —
 * only their proportions matter, playback maps them onto real time.
 *
 * Tagged rather than inferred, so the bytes are the reference client's: a
 * shape is an int and everything else a float, where the inference rule would
 * send a whole-numbered level as an int.
 *
 * @internal
 */
export function envGenArgs(env: Env): MsgArg[] {
    const args: MsgArg[] = [["f", env.levels[0]!]];
    for (let k = 0; k < env.times.length; k++) {
        const [shape, curve] = resolveCurve(env.curves[k]!);
        args.push(
            ["f", env.levels[k + 1]!],
            ["f", env.times[k]!],
            ["i", shape],
            ["f", curve],
        );
    }
    return args;
}

/**
 * A control-automation lane: a break-point curve (`Env`) driving one or more
 * `[node, control]` targets, rendered as a control buffer read onto a control
 * bus. Editable through `toPoints`/`fromPoints` — the `bpf` widget's flat
 * `[time, value, shape, curve, …]` form: times in seconds, values in real
 * control units. Its times are an `Env`'s, so they are in **seconds**: the
 * curve is a shape in real time, and the clock's tempo enters only where the
 * lane is scheduled.
 *
 * ```ts
 * const auto = Automation.fromPoints(
 *     [[0, 200.0, "lin", 0], [2, 4000.0, "exp", 0]],
 *     [synth, "cutoff"]);
 * await auto.prepare(server);   // at setup: this is the half that waits
 * timeline.add(0, auto);        // played by the Playhead as a timeline item
 * ```
 */
export class Automation {
    /**
     * The unit this object's length is in — **seconds**, because the curve is an
     * `Env` and an envelope's segment times are real time. Read by
     * `form.Element.durationUnit`, so an element wrapping a curve is measured
     * the way the curve is.
     */
    readonly durationUnit = "seconds";
    env: Env;
    targets: AutomationTarget[];
    readonly name: string;
    readonly frames: number;
    buf: Buffer | null = null;
    bus: Bus | null = null;
    /**
     * The lane synth of the last `play` and the server it went to, so `stop`
     * can interrupt the sweep early.
     */
    node: number | null = null;
    private playingOn: Server | null = null;

    constructor(
        env: Env,
        target?: AutomationTargets | null,
        { name, frames = DEFAULT_FRAMES }: { name?: string; frames?: number } = {},
    ) {
        this.env = env;
        this.targets = normTargets(target);
        this.name = name ?? (this.targets[0]?.[1] ?? "automation");
        this.frames = Math.trunc(frames);
    }

    /**
     * Builds one from a `bpf` breakpoint list — `[[time, value, shape, curve],
     * …]`, or the flat `[t, v, shape, curve, …]` a `"points"` event carries.
     *
     * Times are in **seconds** — they are an `Env`'s segment times, which is
     * what the curve is stored as and what the envelope math on the server
     * reads — and values are in the target control's real units. The conversion
     * to the clock's beats happens where the lane is scheduled ({@link
     * Automation.play}), not in the curve.
     */
    static fromPoints(
        points: readonly number[] | readonly (readonly number[])[],
        target?: AutomationTargets | null,
        {
            name,
            frames = DEFAULT_FRAMES,
            ...envOptions
        }: {
            name?: string;
            frames?: number;
            timeAt?: number;
            releaseNode?: number;
            loopNode?: number;
        } = {},
    ): Automation {
        const flat = Array.isArray(points[0])
            ? (points as readonly (readonly number[])[]).flat()
            : (points as readonly number[]);
        return new Automation(pointsToEnv(flat, envOptions), target, { name, frames });
    }

    /** The curve as the `bpf` flat breakpoint list `[t, v, shape, curve, …]`. */
    toPoints(): number[] {
        return envToPoints(this.env);
    }

    /**
     * The curve's length in **seconds** (the sum of its segment times, which is
     * what an `Env` measures them in).
     */
    duration(): number {
        return this.env.times.reduce((sum, t) => sum + t, 0);
    }

    /**
     * Allocates and fills the control buffer and allocates the bus. Call once,
     * at setup — this is the half that waits, which is why it is not `play`'s
     * job: a routine must never `await` the server.
     */
    async prepare(
        server?: Server,
        { wait = true, timeout }: { wait?: boolean; timeout?: number } = {},
    ): Promise<this> {
        const on = main.resolveServer(server);
        await addAutomationDef(on, { wait });
        if (this.buf === null) {
            this.buf = await Buffer.alloc(this.frames, 1, { server: on, timeout });
        }
        await this.buf.gen("env", envGenArgs(this.env), { wait, timeout });
        if (this.bus === null) this.bus = Bus.control(1, { server: on });
        return this;
    }

    /**
     * Re-fill the control buffer from the curve **as it now stands**.
     *
     * `prepare` fills it once, at setup; an edit to `env` afterwards changes
     * what the next render *schedules* and not what the lane synth *reads*, so
     * without this the curve you draw is not the curve you hear. Anything that
     * rewrites the envelope of a prepared automation calls it — the multitrack
     * editor does, on every break-point edit.
     *
     * Not awaited by default: it is called from an event handler, and the fill
     * is one command the server applies in the order it arrived, ahead of the
     * synth that reads it. Does nothing before `prepare` — there is no buffer to
     * fill, and the first `prepare` will fill it from the same envelope.
     */
    refill({ wait = false }: { wait?: boolean } = {}): this {
        if (this.buf === null) return this;
        void this.buf.gen("env", envGenArgs(this.env), { wait });
        return this;
    }

    /**
     * Timeline-item hook and interactive trigger: schedules the lane synth,
     * `/node_map`s the targets, and frees the synth after the curve's
     * duration. Nothing here waits.
     *
     * Two timing regimes, chosen by context as an `Event`'s are. **Inside a
     * routine** everything goes out as timed bundles at the routine's exact
     * logical beat. **Outside any clock** (an interactive `play(auto)`) the
     * lane starts immediately and frees itself on wall time, the curve's beats
     * reading as seconds (tempo 1.0).
     *
     * Returns the lane synth's node id.
     */
    play(destination?: Server): number {
        if (this.buf === null || this.bus === null) {
            throw new Error(
                "Automation.play: await prepare(server) first — allocating and " +
                    "filling a buffer waits on the server, which a routine must not",
            );
        }
        const server = main.resolveServer(destination);
        const clock = Moment.current().clock;
        const durSecs = this.duration();
        const durBeats = clock === null ? durSecs : durSecs * clock.tempo;

        const node = server.nodes.alloc();
        this.node = node;
        this.playingOn = server;
        const sNew: TimedMessage = [
            "/synth_new",
            LANE_DEF,
            ["i", node],
            ["i", AddAction.HEAD],
            ["i", ROOT_NODE_ID],
            "buf",
            ["f", this.buf.bufnum],
            "bus",
            ["f", this.bus.index],
            "dur",
            ["f", durSecs],
        ];
        const maps: TimedMessage[] = this.targets.map(([target, ctl]) => [
            "/node_map",
            ["i", nodeId(target)],
            ctl,
            ["i", this.bus!.index],
        ]);
        if (clock === null) {
            // No clock in context: an immediate lane, self-freeing on wall time.
            server.sendMsg(...(sNew as [string, ...MsgArg[]]));
            for (const m of maps) server.sendMsg(...(m as [string, ...MsgArg[]]));
            server.sendBundleAfter(durSecs, [["/node_free", ["i", node]]]);
            return node;
        }
        server.sendBundle([sNew]);
        for (const m of maps) server.sendBundle([m]);
        server.sendBundle([["/node_free", ["i", node]]], { delayBeats: durBeats });
        return node;
    }

    /**
     * Interrupts the sweep **now**: frees the lane synth of the last `play`, so
     * the curve stops advancing and the mapped controls hold their last value.
     * A no-op when nothing is playing; the end-of-curve free already scheduled
     * still arrives and is harmless.
     */
    stop(): void {
        if (this.node !== null && this.playingOn !== null) {
            new Node(this.node, this.playingOn).free();
        }
        this.node = null;
        this.playingOn = null;
    }

    /** Returns the buffer and the bus to their allocators. */
    free(): void {
        if (this.buf !== null) {
            this.buf.free();
            this.buf = null;
        }
        if (this.bus !== null) {
            this.bus.free();
            this.bus = null;
        }
    }
}
