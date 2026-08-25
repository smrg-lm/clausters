// The free-standing `play` — one verb for everything playable (mirrors
// `clausters/play.py`).
//
// `play` is the interactive front door: it plays whatever you hand it against
// the ambient context, so you never spell out a server or a clock for a quick
// take. Like SuperCollider's `play` (and the Python client's), it dispatches
// by kind:
//
// - an `Event` — or a plain **object** of event keys — → a note (immediate
//   outside a clock, timetagged inside one);
// - an event `Pattern` (a `Pbind`) → an `EventStreamPlayer` on a clock;
// - a `Routine`/`Stream`, or a bare **generator** (object or function) →
//   scheduled on a clock;
// - a **def** (`SynthDef` / `FaustDef` / `GraphDef`) → sent and instanced on
//   the server. Returns the node handle — it plays until you free it;
// - a bare **expression** (a UGen graph, a `ChannelList`, a Faust `Signal`) →
//   the same, through the ephemeral-def coercion (`defs/asdef.ts`), so
//   `play(sine(440).mul(0.5))` sounds a def it wrapped for you;
// - a `Timeline` → a `Playhead` over the ambient clock and server;
// - an `Automation` → its lane synth triggered and its targets mapped
//   (`await auto.prepare(server)` first — see below);
// - a `Buffer` → sounded through the stock playbuf instrument (a buffer
//   sounds through an instrument; here the verb provides the default one —
//   `rate`/`amp` controls, freed when the take ends);
// - anything else following the **timeline-item protocol**
//   (`play(destination)` — an `OscEvent`, …) → dispatched to it with the
//   ambient server.
//
// Everything resolves against the ambient environment (the running session,
// else the default one): the server defaults to the ambient session's and the
// clock to the running routine's or, outside one, the default session's clock,
// created and started on first use.
//
// ```ts
// const s = (await Session.embed()).adoptDefault();
// play(new Event({ degree: 0 }));                          // one note, now
// play(new Pbind({ degree: new Pseq([0, 2, 4]), dur: 0.5 })); // a phrase
// ```
//
// **One difference.** The reference verb prepares an
// unprepared `Automation` on the spot, blocking off the clock thread; a
// synchronous verb in a page cannot, so an unprepared one is refused by name
// and `await auto.prepare(server)` is the door.

import { main } from "./base/main.ts";
import { Routine, Stream } from "./base/stream.ts";
import type { TempoClock } from "./base/clock.ts";
import { asDef, isExpr } from "./defs/asdef.ts";
import type { Expr } from "./defs/asdef.ts";
import { Buffer } from "./defs/buffer.ts";
import { FaustDef } from "./defs/faustdef.ts";
import { GraphDef } from "./defs/graphdef.ts";
import { Group, Synth } from "./defs/node.ts";
import type { Controls, Node } from "./defs/node.ts";
import type { Server } from "./defs/server/index.ts";
import { SynthDef } from "./defs/synthdef.ts";
import { bufSampleRate, control, out, playBuf, sampleRate } from "./defs/ugens/index.ts";
import { Event } from "./seq/event.ts";
import type { EventDestination } from "./seq/event.ts";
import type { EventStreamPlayer } from "./seq/eventstream.ts";
import { Pattern } from "./seq/pattern.ts";
import { Automation } from "./seq/automation.ts";
import { Playhead, Timeline } from "./seq/timeline.ts";
import type { PlayDestination } from "./seq/timeline.ts";

/** Anything `play` knows how to start. */
export type Playable =
    | Event
    | Record<string, unknown>
    | Pattern<unknown>
    | Stream
    | Generator<number | undefined, unknown, unknown>
    | (() => Generator<number | undefined, unknown, unknown>)
    | SynthDef
    | FaustDef
    | GraphDef
    | Expr
    | Timeline
    | Buffer
    | Automation
    | { play(destination: unknown): unknown };

export interface PlayOptions {
    /** The destination server; the ambient one by default. */
    server?: Server;
    /**
     * The clock to schedule on (patterns, routines, timelines); the running
     * routine's by default, else the default session's, started on first use.
     * Ignored by a bare event played immediately, and by a def or a buffer.
     */
    clock?: TempoClock;
    /** Start quantization for a pattern, routine or timeline. */
    quant?: number;
    /**
     * The controls (ports, for a `GraphDef`) a def is instanced with; for a
     * `Buffer`, the stock instrument's `rate` (a musical ratio) and `amp`.
     */
    controls?: Controls;
}

/**
 * Plays `playable` against the ambient context.
 *
 * Returns something that knows how to end what just started: the completed
 * event for an event or object (`free()` / `release()`), the
 * `EventStreamPlayer` for a pattern (`stop()`), the routine for a routine, the
 * node handle for a def or a buffer (`free()`), the `Playhead` for a timeline
 * (`stop()`) and the `Automation` itself (`stop()`).
 */
export function play(playable: Playable, options: PlayOptions = {}): unknown {
    const { server, clock, quant, controls } = options;

    if (playable instanceof Event) {
        return playable.play(destinationFor(server));
    }
    if (playable instanceof Stream) {
        return playable.play(clock ?? ambientClock(), quant);
    }
    if (playable instanceof Pattern) {
        return playable.play(destinationFor(server), { clock, quant }) as EventStreamPlayer;
    }
    if (isGenerator(playable) || isGeneratorFunction(playable)) {
        return asRoutine(playable).play(clock ?? ambientClock(), quant);
    }
    if (
        playable instanceof SynthDef
        || playable instanceof FaustDef
        || playable instanceof GraphDef
    ) {
        return playDef(playable, main.resolveServer(server), controls);
    }
    if (isExpr(playable)) {
        // A bare expression: coerced into an ephemeral def and played like
        // any other. The def is named `tmp_*`, so the server never persists
        // it, and it is this call's alone.
        return playDef(asDef(playable), main.resolveServer(server), controls);
    }
    if (playable instanceof Timeline) {
        const playhead = new Playhead(
            playable,
            clock ?? ambientClock(),
            main.resolveServer(server) as unknown as PlayDestination,
        );
        playhead.play({ quant });
        return playhead;
    }
    if (playable instanceof Buffer) {
        return playBuffer(playable, main.resolveServer(server), controls);
    }
    if (playable instanceof Automation) {
        playable.play(main.resolveServer(server));
        return playable;
    }
    if (typeof playable === "object" && typeof (playable as Playable & {
        play?: unknown;
    }).play === "function") {
        // The timeline-item protocol (`OscEvent`, and anything else a
        // Playhead could play): play(destination).
        return (playable as { play(destination: unknown): unknown })
            .play(main.resolveServer(server));
    }
    if (typeof playable === "object" && playable !== null) {
        return new Event(playable as Record<string, unknown>).play(destinationFor(server));
    }
    throw new TypeError(
        `don't know how to play ${String(playable)}; expected an Event or event ` +
            "object, an event Pattern (Pbind), a Routine/Stream or generator, a " +
            "def (SynthDef/FaustDef/GraphDef) or a bare expression, a Timeline, a " +
            "Buffer, an " +
            "Automation, or anything with play(destination)",
    );
}

/** The ambient server as an event destination. */
function destinationFor(server?: Server): EventDestination {
    return main.resolveServer(server) as unknown as EventDestination;
}

/** The clock an ambient play schedules on, created and started on first use. */
function ambientClock(): TempoClock {
    return main.resolveClock() ?? main.getDefaultClock();
}

function isGenerator(value: unknown): value is Generator<number | undefined, unknown, unknown> {
    return (
        typeof value === "object" && value !== null
        && typeof (value as { next?: unknown }).next === "function"
        && typeof (value as { [Symbol.iterator]?: unknown })[Symbol.iterator] === "function"
    );
}

function isGeneratorFunction(value: unknown): boolean {
    return typeof value === "function"
        && value.constructor?.name === "GeneratorFunction";
}

/**
 * A `Routine` over a generator: a generator *function* is wrapped directly; an
 * already-created generator object is played through once (a `reset` cannot
 * restart it — pass the function to keep it re-runnable).
 */
function asRoutine(playable: unknown): Routine {
    if (isGeneratorFunction(playable)) {
        return new Routine(playable as () => Generator<number | undefined, unknown, unknown>);
    }
    return new Routine(() => playable as Generator<number | undefined, unknown, unknown>);
}

/**
 * Sends `def` (any family) and instances it: `/graph_new` for a `GraphDef`,
 * `/synth_new` otherwise. Returns the node handle.
 *
 * The send is not awaited — `play` is a synchronous verb — but the carrier is
 * ordered, so the def is on its way before the creation that names it.
 */
function playDef(
    def: SynthDef | FaustDef | GraphDef,
    server: Server,
    controls?: Controls,
): Node {
    void def.send(server, { wait: false });
    if (def instanceof GraphDef) {
        return Group.graph(def.name, controls, { server });
    }
    return new Synth(def.name, controls, { server });
}

/**
 * A buffer sounds through an instrument (see `docs/decisions.md`); here the
 * verb provides the stock one — one `playBuf` lane per channel, with `rate`
 * and `amp` controls — and frees it when the take ends (the buffer's frames
 * over its rate). Returns the `Synth`.
 */
function playBuffer(buffer: Buffer, server: Server, controls?: Controls): Synth {
    if (!buffer.frames) {
        throw new Error(
            "cannot play a buffer of unknown length; read its shape first "
                + "(await buffer.info())",
        );
    }
    const channels = Math.max(1, buffer.channels);
    const buf = control("buf", 0.0);
    // `rate` is a musical ratio: the def rescales it by the buffer's own
    // sample rate, so a take plays at pitch on any engine rate.
    const rate = control("rate", 1.0).mul(bufSampleRate(buf)).div(sampleRate());
    const amp = control("amp", 1.0);
    const def = new SynthDef(
        `_playbuf${channels}`,
        ...Array.from({ length: channels }, (_, ch) =>
            out(ch, playBuf(buf, ch, rate).mul(amp))),
    );
    void def.send(server, { wait: false });
    const given = controls ? Object.fromEntries(Object.entries(controls)) : {};
    const node = new Synth(def.name, { buf: buffer.bufnum, ...given }, { server });
    const fileRate = buffer.sampleRate || 48_000.0;
    const speed = Number((given as Record<string, number>).rate ?? 1.0);
    server.sendBundleAfter(buffer.frames / fileRate / speed, [
        ["/node_free", ["i", node.id]],
    ]);
    return node;
}
