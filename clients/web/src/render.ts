// The free-standing `render` -- one verb for the change of state to sound
// (mirrors `clausters/render.py`).
//
// `render` is the third ambient verb, next to `play` and `plot`: it turns a
// **generator** thing (an algorithm that describes sound) into a **generated**
// one (samples -- random-access audio). It dispatches by kind:
//
// - a binary **score** (`Uint8Array`) -> the offline renderer, as is;
// - a **def** (`SynthDef` / `FaustDef` / `GraphDef`) or a bare **expression**
//   (a UGen graph, a `ChannelList`, a Faust `Signal` -- coerced through
//   `defs/asdef.ts`) -> instanced offline for `dur` seconds, the audible
//   sibling of `plot(def)`;
// - a `Timeline`, an `EventPattern`, a `Routine`/`Stream` or a bare
//   **generator** -> an **offline bounce**: an offline session plays it and
//   the drained score is rendered. An endless source needs `until` (the
//   bounce would never drain);
// - a **value pattern** (a `Pattern` whose values are not events) -> the values
//   it generates, as an array. An endless one needs `count`.
//
// **An offline bounce runs in an offline session**, and the tempo is the
// clock's: `clock` is the one it plays on, as it is for `play`. It must be a
// clock of an offline session (`Session.nrt`), on its `LogicalTimebase`, and
// the render is that session's; with no `clock` the render makes an offline
// session of its own and plays on its clock, at tempo 1.0.
//
// Every path but a value pattern's resolves with a `RenderStats`: the frame,
// channel and event counts, per-channel peak and RMS, the seed the take used,
// and the samples themselves (interleaved `Float32Array`).
//
// **Where this client stops, and why.** The reference client's verb also
// writes a file, through the server's own `--nrt` renderer: it hands a score to
// a process that streams straight to disk, so a long bounce never builds
// millions of floats just to be written out. A page has no such process -- its
// renderer is the same wasm engine that makes its sound, and what it produces
// is a `Float32Array` in this tab.
//
// It is not that a page cannot write a file: it has OPFS, and `Buffer.write`
// goes out to it. What it cannot do is write to a *path the caller names* --
// OPFS is the page's own store, not the machine's -- nor stream while
// rendering, since the samples exist in full before anything can be written.
// So `path` and the `sampleFormat` that only means anything beside it stay
// out, and `wavBytes(stats)` is the browser's version of the same intent: a
// finished render as WAV bytes, which the page then downloads, writes to OPFS,
// or feeds back into a buffer.
//
// `workers` stays out for a harder reason: the wasm entry point renders on the
// calling thread (`workers: 0`, fixed in `crates/clausters-web`), and wasm
// threads need cross-origin isolation the embedding page has to grant. A count
// this client could not honour would be a worse surface than none.
//
// ```ts
// const stats = await render(sine(440).mul(0.2), { dur: 2.0 });
// const bounced = await render(new Pbind({ degree: new Pseq([0, 2, 4]), dur: 0.5 }));
// const values = await render(new Pseq([1, 2, 3], 2));   // [1, 2, 3, 1, 2, 3]
// ```

import { Routine, Stream } from "./base/stream.ts";
import { asDef, exprChannels, isExpr } from "./defs/asdef.ts";
import type { Expr } from "./defs/asdef.ts";
import { FaustDef } from "./defs/faustdef.ts";
import { GraphDef } from "./defs/graphdef.ts";
import { Group, Synth } from "./defs/node.ts";
import type { Controls } from "./defs/node.ts";
import { SynthDef } from "./defs/synthdef.ts";
import type { TempoClock } from "./base/clock.ts";
import { LogicalTimebase } from "./base/timebase.ts";
import { EventPattern, Pattern } from "./seq/pattern.ts";
import type { Event } from "./seq/event.ts";
import { Timeline } from "./seq/timeline.ts";
import type { PlayDestination } from "./seq/timeline.ts";
import { channelStats } from "./data/analysis.ts";
import { renderScoreBytes } from "./engine/render.ts";

/**
 * How many events -- or values -- a render takes before it decides its source
 * is endless, when no `until` (or `count`) bounds it.
 *
 * A render holds what it generated in memory, so a million is already past any
 * real run and nowhere near a legitimate one -- which is what makes the cap
 * honest *here* and wrong inside `TempoClock.render`, where a long offline
 * render of a real score is exactly the thing that runs for a very long time on
 * purpose.
 */
export const MAX_BOUNCED_EVENTS = 1_000_000;

/** The render's own settings -- what the offline server is configured with. */
export interface RenderOptions {
    /** Render sample rate, in Hz. */
    sampleRate?: number;
    /** Interleaved output channel count. */
    channels?: number;
    /**
     * Starting seed for the render's stochastic UGens. Absent, the render
     * draws a fresh one -- so anything with noise in it is a new take every
     * call -- and reports it in `stats.seed`; passing that back replays the
     * take exactly.
     */
    seed?: number | bigint;
}

/** What a render did -- the one thing every render resolves with. */
export interface RenderStats {
    /** Frames produced (per channel). */
    frames: number;
    /** Interleaved channel count. */
    channels: number;
    sampleRate: number;
    /** Length in seconds. */
    duration: number;
    /** Peak magnitude per channel, in channel order. */
    peak: number[];
    /** RMS per channel, in channel order. */
    rms: number[];
    /**
     * The seed this take started from. Unless you asked for one you got a
     * fresh one, so **this is how you get a take back**.
     */
    seed: bigint;
    /** The audio, interleaved. */
    samples: Float32Array;
}

/** One channel of a finished render, deinterleaved. */
export function channel(stats: RenderStats, index: number): Float32Array {
    const out = new Float32Array(stats.frames);
    for (let i = 0; i < stats.frames; i++) {
        out[i] = stats.samples[i * stats.channels + index]!;
    }
    return out;
}

/**
 * Renders a binary score -- the bytes a `ScoreConnection` accumulated -- and
 * measures what came out.
 *
 * This is the one place samples are produced: every other path in this module
 * builds a score and ends here.
 */
export async function renderScore(
    score: Uint8Array,
    { sampleRate = 48_000.0, channels = 2, seed }: RenderOptions = {},
): Promise<RenderStats> {
    const { samples, seed: used } = await renderScoreBytes(score, sampleRate, channels, seed);
    const frames = channels > 0 ? Math.floor(samples.length / channels) : 0;
    const peak: number[] = [];
    const rms: number[] = [];
    for (let ch = 0; ch < channels; ch++) {
        const [p = 0, r = 0] = channelStats(samples, channels, ch);
        peak.push(p);
        rms.push(r);
    }
    return {
        frames,
        channels,
        sampleRate,
        duration: sampleRate > 0 ? frames / sampleRate : 0,
        peak,
        rms,
        seed: used,
        samples,
    };
}

/** What `render` takes on top of the render's own settings. */
export interface RenderVerbOptions extends RenderOptions {
    /** Seconds a def or expression is held before it is freed. */
    dur?: number;
    /** Controls (ports, for a `GraphDef`) the instance is started with. */
    controls?: Controls;
    /**
     * Extra defs the render needs first -- a `GraphDef`'s member defs, or the
     * instrument a bounced pattern, timeline or routine names. Every offline
     * path starts from an **empty** ephemeral session, so whatever the
     * samples names has to ride along.
     */
    defs?: readonly (SynthDef | FaustDef | GraphDef)[];
    /**
     * The clock it plays on, as for `play`: a clock of an offline session,
     * whose render this is. Omitted, the render makes an offline session of
     * its own, at tempo 1.0.
     */
    clock?: TempoClock;
    /**
     * Stop the offline bounce at this beat of `clock` -- required for an
     * endless source, which never drains on its own (an event pattern with no
     * bound throws after `MAX_BOUNCED_EVENTS` events).
     */
    until?: number;
    /**
     * How many values a value pattern generates -- required for an endless
     * one, which otherwise throws after `MAX_BOUNCED_EVENTS`.
     */
    count?: number;
}

/** Anything `render` knows how to turn into samples. */
export type Renderable =
    | Uint8Array
    | SynthDef
    | FaustDef
    | GraphDef
    | Expr
    | Timeline
    | Pattern<unknown>
    | Stream
    | Generator<number | undefined, unknown, unknown>
    | (() => Generator<number | undefined, unknown, unknown>);

/**
 * Renders `obj` offline and resolves with a `RenderStats` -- or, for a value
 * pattern, with the values it generates.
 *
 * Everything here is offline by nature: a pattern or a routine is
 * forward-only, and sounding one live is `play`'s job.
 */
export async function render(obj: Pattern<Event>, options?: RenderVerbOptions): Promise<RenderStats>;
export async function render<T>(obj: Pattern<T>, options?: RenderVerbOptions): Promise<T[]>;
export async function render(obj: Renderable, options?: RenderVerbOptions): Promise<RenderStats>;
export async function render(
    obj: Renderable,
    options: RenderVerbOptions = {},
): Promise<RenderStats | unknown[]> {
    const {
        dur = 1.0, controls, defs = [], until, count, clock, ...cfg
    } = options;

    if (obj instanceof Uint8Array) return renderScore(obj, cfg);

    if (
        obj instanceof SynthDef || obj instanceof FaustDef || obj instanceof GraphDef
        || isExpr(obj)
    ) {
        checkExprWidth(obj, cfg.channels ?? 2);
        return bounceDef(asDef(obj), { dur, controls, defs, ...cfg });
    }

    if (obj instanceof Timeline) {
        return bounce(
            (session) =>
                obj.play({ destination: session.server as unknown as PlayDestination }),
            { clock, until, defs, ...cfg },
        );
    }

    if (obj instanceof EventPattern) {
        return bounce(
            (session, on) => obj.play(session.server, { clock: on }),
            { clock, until, defs, guard: "event pattern", ...cfg },
        );
    }

    if (obj instanceof Pattern) return values(obj, count);

    const playable = obj instanceof Stream ? obj : asRoutine(obj);
    if (playable === null) {
        throw new TypeError(
            "don't know how to render this; expected a score (Uint8Array), a def "
                + "(SynthDef/FaustDef/GraphDef), a bare expression, a Timeline, a "
                + "pattern, or a Routine/Stream/generator",
        );
    }
    return bounce((_session, on) => playable.play(on), {
        clock, until, defs, ...cfg,
    });
}

/**
 * The values a value pattern generates: `count` of them, or all of them for a
 * finite one -- refused past `MAX_BOUNCED_EVENTS` with no `count`.
 */
function values<T>(pattern: Pattern<T>, count: number | undefined): T[] {
    const out: T[] = [];
    const limit = count ?? MAX_BOUNCED_EVENTS;
    for (const value of pattern) {
        if (out.length === limit) {
            if (count === undefined) {
                throw new Error(
                    `render: the ${pattern.constructor.name} did not end after `
                    + `${MAX_BOUNCED_EVENTS} values -- pass count to take a number of them`,
                );
            }
            break;
        }
        out.push(value);
    }
    return out;
}

/**
 * Renders a def offline: an ephemeral NRT session, the `defs` it needs plus
 * the def itself at score time 0, one instance with `controls`, freed at `dur`
 * seconds.
 *
 * The shared change of state -- `render` returns the stats, `plot` draws their
 * samples -- so what you see and what you hear come from one render.
 */
export async function bounceDef(
    def: SynthDef | FaustDef | GraphDef,
    {
        dur = 1.0,
        controls,
        defs = [],
        ...cfg
    }: RenderOptions & {
        dur?: number;
        controls?: Controls;
        defs?: readonly (SynthDef | FaustDef | GraphDef)[];
    } = {},
): Promise<RenderStats> {
    const { Session } = await import("./session.ts");
    const session = await Session.nrt(); // its clock at tempo 1.0: beats == seconds
    const server = session.server;
    for (const extra of defs) await extra.send(server);
    await def.send(server);
    const node = def instanceof GraphDef
        ? Group.graph(def.name, controls, { server })
        : new Synth(def.name, controls, { server });
    server.sendBundleAfter(dur, [["/node_free", ["i", node.id]]]);
    return session.render(cfg);
}

/**
 * An offline session: the `defs` first, then `start(session, clock)` schedules
 * the source on the clock it plays on and the session's server, and the drained
 * score is rendered.
 *
 * The session is `clock`'s, which has to be an offline one; with no `clock` it
 * is a new one, which starts **empty** -- it is not the one the caller has been
 * working in -- so a pattern naming an instrument of its own has to bring it
 * along, exactly as a def bounce does.
 *
 * `guard` names what an unbounded render is refused for after
 * `MAX_BOUNCED_EVENTS` events.
 */
async function bounce(
    start: (session: import("./session.ts").Session, clock: TempoClock) => unknown,
    { clock, until, defs = [], guard, ...cfg }: RenderOptions & {
        clock?: TempoClock;
        until?: number;
        defs?: readonly (SynthDef | FaustDef | GraphDef)[];
        guard?: string;
    },
): Promise<RenderStats> {
    const { Session } = await import("./session.ts");
    let session: import("./session.ts").Session;
    if (clock === undefined) {
        session = await Session.nrt();
        clock = session.clock;
    } else {
        const owner = clock.session;
        if (!(clock.timebase instanceof LogicalTimebase) || !(owner instanceof Session)) {
            throw new Error(
                `render plays on a clock of an offline session, and this clock is on a `
                + `${clock.timebase.constructor.name}${owner ? "" : " in no session"}: a clock's `
                + `timebase is fixed when it is made. Pass a clock made in Session.nrt(), or `
                + `none for a session of the render's own`,
            );
        }
        session = owner;
    }
    const on = clock;
    for (const def of defs) await def.send(session.server);
    const maxSteps = guard !== undefined && until === undefined ? MAX_BOUNCED_EVENTS : undefined;
    session.use(() => {
        start(session, on);
        try {
            on.render(until, { maxSteps });
        } catch (error) {
            if (maxSteps === undefined) throw error;
            throw new Error(
                `render: the ${guard} did not end after ${MAX_BOUNCED_EVENTS} events -- `
                + `pass until to bound it`,
            );
        }
    });
    return session.server.render(cfg);
}

/**
 * Refuses a bare expression laid past the render's outputs.
 *
 * `channels` is the offline server's output count -- how many channels the
 * render *has*, a fact about the server being configured and not about the
 * graph -- so nothing is derived from one here; this only **checks**. An
 * expression the coercion lays on more buses than that writes the surplus onto
 * internal buses, which reach no output: silently half a take. (`plot` reads
 * the same width the other way round, and configures itself from it -- the
 * split the two verbs keep on purpose.)
 */
function checkExprWidth(obj: unknown, channels: number): void {
    const width = exprChannels(obj);
    if (width !== null && width > channels) {
        throw new RangeError(
            `this expression writes ${width} channels but the render has ${channels} `
                + `output channels, so channels ${channels}..${width - 1} would land on `
                + `internal buses and reach no output; pass channels: ${width} (or mix `
                + "the expression down)",
        );
    }
}

/** A `Routine` over a generator, or `null` when `obj` is not one. */
function asRoutine(obj: unknown): Routine | null {
    if (
        typeof obj === "function"
        && (obj as { constructor?: { name?: string } }).constructor?.name
            === "GeneratorFunction"
    ) {
        return new Routine(obj as () => Generator<number | undefined, unknown, unknown>);
    }
    if (
        typeof obj === "object" && obj !== null
        && typeof (obj as { next?: unknown }).next === "function"
        && typeof (obj as { [Symbol.iterator]?: unknown })[Symbol.iterator] === "function"
    ) {
        return new Routine(() => obj as Generator<number | undefined, unknown, unknown>);
    }
    return null;
}

/**
 * A finished render as **WAV bytes** (32-bit float, the format the render is
 * already in), for a page to download or hand back to a `Buffer`.
 *
 * The browser's answer to the reference client's `path`: there is no
 * filesystem to write to and no server process to write it, so the file is a
 * blob the page decides what to do with.
 */
export function wavBytes(stats: RenderStats): Uint8Array {
    const { samples, channels, sampleRate } = stats;
    const dataBytes = samples.length * 4;
    const out = new Uint8Array(44 + dataBytes);
    const view = new DataView(out.buffer);
    const ascii = (offset: number, text: string) => {
        for (let i = 0; i < text.length; i++) out[offset + i] = text.charCodeAt(i);
    };
    ascii(0, "RIFF");
    view.setUint32(4, 36 + dataBytes, true);
    ascii(8, "WAVEfmt ");
    view.setUint32(16, 16, true); // fmt chunk size
    view.setUint16(20, 3, true); // IEEE float
    view.setUint16(22, channels, true);
    view.setUint32(24, Math.round(sampleRate), true);
    view.setUint32(28, Math.round(sampleRate) * channels * 4, true); // byte rate
    view.setUint16(32, channels * 4, true); // block align
    view.setUint16(34, 32, true); // bits per sample
    ascii(36, "data");
    view.setUint32(40, dataBytes, true);
    for (let i = 0; i < samples.length; i++) {
        view.setFloat32(44 + i * 4, samples[i]!, true);
    }
    return out;
}
