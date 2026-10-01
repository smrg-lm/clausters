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
// What a render logged -- a node the engine rejected, a `Poll`'s line -- goes
// to the console at its level (`INFO` to `console.debug`). A UGen that refuses
// what it was given (a convolution kernel it cannot use whole) fails the
// render with the reason instead.
//
// **Where the audio goes.** With no `path` the samples come back in
// `stats.samples`; with one the take is written to that file and
// `stats.samples` is `null` -- the path chooses where the output goes, not
// whether there is one, as in the reference client. The file is where every
// path this client takes is: the disk under node, the page's own storage
// (`opfs`) in a tab, which is also what `Buffer.read` and the server's
// `/buffer_allocRead` read there. The framing and the int16/int24 conversion
// are the server crate's (`engine/codec.ts`), so the file is the one a native
// render writes. What a page cannot do is stream while rendering: its
// renderer is the wasm engine in this tab, so the samples exist in full before
// the file is written. `readSoundfile` reads a file back through the server's
// own decoder.
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
import type { OscNrtInterface } from "./base/connection.ts";
import { LogicalTimebase } from "./base/timebase.ts";
import { EventPattern, Pattern } from "./seq/pattern.ts";
import type { Event } from "./seq/event.ts";
import { Timeline } from "./seq/timeline.ts";
import type { PlayDestination } from "./seq/timeline.ts";
import { channelStats } from "./data/analysis.ts";
import { renderScoreBytes } from "./engine/render.ts";
import { decodeSoundfile, encodeWav } from "./engine/codec.ts";
import { readFileAt, writeFileAt } from "./base/files.ts";

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
    /**
     * Where the audio goes. Absent, the samples come back in `stats.samples`;
     * given, the take is written to this file -- on the disk under node, in the
     * page's own storage (`opfs`) in a tab -- and `stats.samples` is `null`.
     */
    path?: string;
    /** The file's sample format beside a `path`: `"float"` (the default), `"int24"` or `"int16"`. */
    sampleFormat?: "int16" | "int24" | "float";
}

/** What a render did -- the one thing every render resolves with. */
export interface RenderStats {
    /** Frames produced (per channel). */
    frames: number;
    /** Interleaved channel count. */
    channels: number;
    sampleRate: number;
    /** Score events the render ran; 0 for a file read back. */
    events: number;
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
    /** The file the take was written to, or `null` when it was kept in memory. */
    path: string | null;
    /** The audio, interleaved -- `null` when a `path` sent it to a file instead. */
    samples: Float32Array | null;
}

/** A finished render's samples, or why there are none. */
function samplesOf(stats: RenderStats): Float32Array {
    if (stats.samples === null) {
        throw new Error(`clausters: the take went to ${stats.path}: read it with readSoundfile`);
    }
    return stats.samples;
}

/**
 * Splits interleaved `samples` into `count` per-channel arrays.
 *
 * Interleaved is the currency everywhere in Clausters -- it is the server's own
 * buffer layout (`/buffer_getRange` indexes `frame * channels + channel`), so
 * audio *going to* the server needs no conversion. Deinterleaving is for
 * analysis on this side. `interleave` is the inverse.
 */
export function channels(samples: ArrayLike<number>, count: number): Float32Array[] {
    const frames = count > 0 ? Math.floor(samples.length / count) : 0;
    const out = Array.from({ length: count }, () => new Float32Array(frames));
    for (let i = 0; i < frames; i++) {
        for (let c = 0; c < count; c++) out[c]![i] = samples[i * count + c]!;
    }
    return out;
}

/**
 * Weaves per-channel arrays back into one interleaved `Float32Array` -- the
 * inverse of `channels`, and the layout the server wants. Throws when the
 * channels differ in length.
 */
export function interleave(...chans: ArrayLike<number>[]): Float32Array {
    if (chans.length === 0) return new Float32Array(0);
    const n = chans[0]!.length;
    if (chans.some((c) => c.length !== n)) {
        throw new Error("every channel must have the same length");
    }
    const out = new Float32Array(n * chans.length);
    chans.forEach((c, k) => {
        for (let i = 0; i < n; i++) out[i * chans.length + k] = c[i]!;
    });
    return out;
}

/** One channel of a finished render, deinterleaved. */
export function channel(stats: RenderStats, index: number): Float32Array {
    const out = new Float32Array(stats.frames);
    for (let i = 0; i < stats.frames; i++) {
        out[i] = samplesOf(stats)[i * stats.channels + index]!;
    }
    return out;
}

/**
 * Renders a binary score -- the bytes a `OscNrtInterface` accumulated -- and
 * measures what came out.
 *
 * This is the one place samples are produced: every other path in this module
 * builds a score and ends here.
 */
export async function renderScore(
    score: Uint8Array,
    { sampleRate = 48_000.0, channels = 2, seed, path, sampleFormat = "float" }: RenderOptions = {},
): Promise<RenderStats> {
    const { samples, seed: used, events } = await renderScoreBytes(score, sampleRate, channels, seed);
    if (path !== undefined) {
        await writeFileAt(path, await encodeWav(samples, channels, sampleRate, sampleFormat));
    }
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
        events,
        duration: sampleRate > 0 ? frames / sampleRate : 0,
        peak,
        rms,
        seed: used,
        path: path ?? null,
        samples: path === undefined ? samples : null,
    };
}

/**
 * Reads a soundfile through **the server's own decoder** -- WAV, FLAC,
 * OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and the rest -- from the disk under
 * node or the page's own storage (`opfs`) in a tab: `frames` frames from
 * `start` (`-1`, the default, to the end), interleaved `float32` scaled to
 * `[-1, 1]` at the file's own rate. The same decoder `/buffer_allocRead` uses,
 * so the samples are the ones a server buffer holds. Resolves with a
 * `RenderStats` whose `samples` are filled.
 */
export async function readSoundfile(
    path: string,
    { start = 0, frames = -1 }: { start?: number; frames?: number } = {},
): Promise<RenderStats> {
    const decoded = await decodeSoundfile(await readFileAt(path), path, start, frames);
    const peak: number[] = [];
    const rms: number[] = [];
    for (let ch = 0; ch < decoded.channels; ch++) {
        const [p = 0, r = 0] = channelStats(decoded.samples, decoded.channels, ch);
        peak.push(p);
        rms.push(r);
    }
    return {
        frames: decoded.frames,
        channels: decoded.channels,
        sampleRate: decoded.sampleRate,
        events: 0,
        duration: decoded.sampleRate > 0 ? decoded.frames / decoded.sampleRate : 0,
        peak,
        rms,
        seed: 0n,
        path,
        samples: decoded.samples,
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
    /**
     * Seconds the offline bounce goes on **after its last event**, so what that
     * event set going is heard to its end -- the last note's release, an echo.
     * A bounce otherwise stops on the last event, which for a pattern is the
     * gate closing on its final note: the release is never rendered and the
     * take ends on a step. The server cannot know how long a tail should be,
     * so it is yours to say; `0` ends on the last event. Default `1.0`. A def
     * or expression is not affected (`dur` is its length), nor is a binary
     * score, which says its own length with its last bundle.
     */
    tail?: number;
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
        dur = 1.0, controls, defs = [], until, count, tail = 1.0, clock, ...cfg
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
            { clock, until, tail, defs, ...cfg },
        );
    }

    if (obj instanceof EventPattern) {
        return bounce(
            (session, on) => obj.play(session.server, { clock: on }),
            { clock, until, tail, defs, guard: "event pattern", ...cfg },
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
        clock, until, tail, defs, ...cfg,
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
 *
 * The score ends on an **empty closing bundle** `tail` seconds after its last
 * one -- the renderer ends a score on its last bundle, and a bundle with
 * nothing in it is how a length is said without doing anything.
 */
async function bounce(
    start: (session: import("./session.ts").Session, clock: TempoClock) => unknown,
    { clock, until, tail = 1.0, defs = [], guard, ...cfg }: RenderOptions & {
        clock?: TempoClock;
        until?: number;
        tail?: number;
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
    if (tail < 0) throw new RangeError(`tail is a duration, and ${tail} is negative`);
    const connection = session.server.connection as OscNrtInterface;
    const end = connection.score.end;
    if (end !== null && tail > 0) connection.addBundle(end + tail, []);
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
