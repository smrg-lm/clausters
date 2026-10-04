/**
 * The curves of played events: what sends them (mirrors
 * `clausters/seq/curves.py`).
 *
 * An `Event` carries its curves (`automation: [...]`), and a channel's curve is
 * a playable of its own (`Automation.play`). Neither builds anything on the
 * server: a note is the plain synth it always was, and the controls its curves
 * drive are **set**, one value every block, in timed bundles.
 * {@link CurveEmitter} is what sends them -- one per `Server`, reached as
 * `server.curves`.
 *
 * **What each value is is the shared core's** (`editingEventCurves`): a note's
 * own curve wins over its channel's on the same control, a bend adds the two
 * onto the frequency the note was started with, and a channel's curve is
 * glided as the server's event lane glides it. What is here is what a language
 * owns: the list of what sounds, the clock it sounds on, and the wake that
 * sends the next stretch.
 *
 * **It stays a little ahead of the clock, and no further.** The server keeps a
 * bounded queue of timed bundles and drops what does not fit, and a curve is
 * hundreds of values a second, so a whole curve is never sent at once: the
 * emitter wakes every {@link WINDOW} and sends the instants up to two windows
 * ahead, one bundle per instant holding every curve that sounds.
 *
 * **A curve reaches a note from its start to its off**; through its release
 * the note keeps the last value. The off is the last instant a client knows
 * the node is there, and a `/node_set` to a node that is gone is a `/fail` --
 * and, offline, the end of the render.
 *
 * **A channel's curve reaches the notes played on the clock it was played
 * on.** Each clock is an axis of its own here, since a curve is placed by the
 * beat it was played at and a beat belongs to one clock.
 *
 * @module
 */

import { editingEventCurves as coreEventCurves } from "../core/clausters_core_web.js";
import type { TempoClock } from "../base/clock.ts";
import { Moment } from "../base/moment.ts";
import type { TimedMessage } from "../base/osc.ts";
import { SampleClockTimebase } from "../base/timebase.ts";
import type { Server } from "../defs/server/index.ts";
import type { Event } from "./event.ts";

/**
 * How often an emitter wakes, in seconds. It sends up to two of these ahead of
 * its clock, so a wake that comes late by less than one is not heard.
 */
export const WINDOW = 0.05;

/** The sample rate a curve is stepped at when nothing says the server's. */
const RATE = 48_000;

/** A curve as the core reads it: what it drives and its points. */
interface Written {
    target: unknown;
    points: unknown[];
}

/**
 * A sounding note: where it starts and ends on its axis, and its own curves
 * as written.
 */
interface Note {
    node: number;
    start: number;
    /** The beat it started on -- its curves' zero. Seconds with no clock. */
    beat: number;
    off: number | null;
    channel: number;
    freq: number;
    curves: Written[];
}

/** A channel's curve, playing: its zero, and the glide where the last window left it. */
interface Curve {
    id: number;
    start: number;
    beat: number;
    target: unknown;
    points: unknown[];
    state: number | null;
}

/** What one window of the core's rule answers. */
interface Answer {
    bundles?: [number, [number, string, number][]][];
    states?: [number, number][];
    over?: number[];
    idle?: boolean;
}

/** What an `automation` entry of an event answers: an `Automation`. */
interface Playable {
    target: unknown;
    enabled?: boolean;
    write(): { points?: unknown[] };
}

/**
 * `[node, control, value]` triples as `/node_set` messages, one per node, in
 * the order the nodes first appear.
 */
function messages(sets: readonly [number, string, number][]): TimedMessage[] {
    const byNode = new Map<number, TimedMessage>();
    for (const [node, control, value] of sets) {
        let message = byNode.get(node);
        if (message === undefined) {
            message = ["/node_set", ["i", node]];
            byNode.set(node, message);
        }
        message.push(["s", String(control)], ["f", Number(value)]);
    }
    return [...byNode.values()];
}

/**
 * One clock's time, and what sounds on it. `clock` is `null` for the notes
 * played outside any clock, whose time is the wall's.
 */
class Axis {
    readonly emitter: CurveEmitter;
    readonly clock: TempoClock | null;
    readonly notes = new Map<number, Note>();
    /**
     * The channels' curves, newest first: the first that names a control is
     * the one a note reads.
     */
    channels: Curve[] = [];
    /** The second, on this axis, everything has been sent up to. */
    until: number | null = null;
    awake = false;
    /** When the wake last ran, in the page's own milliseconds. */
    woken = 0;

    constructor(emitter: CurveEmitter, clock: TempoClock | null) {
        this.emitter = emitter;
        this.clock = clock;
    }

    // ---- time ----

    now(): number {
        const clock = this.clock;
        return clock === null ? this.emitter.wall() : clock.beats2secs(clock.beats());
    }

    /**
     * A moment as `[seconds on this axis, the beat a curve played there counts
     * from]`. With no clock both are the wall's seconds, from the instant
     * `pin` the caller read it at.
     */
    placed(moment: Moment, pin: number | null = null): [number, number] {
        if (this.clock === null) {
            const secs = (pin ?? this.emitter.wall()) + moment.beat;
            return [secs, secs];
        }
        return [moment.secs(), moment.beat];
    }

    /**
     * Where a curve whose zero is `beat` stands at `start`, and how fast it
     * advances up to `to`: the tempo, read off the clock for this stretch.
     */
    private position(beat: number, start: number, to: number): { at: number; rate: number } {
        const clock = this.clock;
        if (clock === null) return { at: start - beat, rate: 1.0 };
        const at = clock.secs2beats(start);
        const rate = to > start ? (clock.secs2beats(to) - at) / (to - start) : clock.tempo;
        return { at: at - beat, rate };
    }

    /**
     * One window of the core's rule over `notes` and this axis's channels.
     * `fresh` hands the channels' curves with no glide, for a stretch that is
     * not the one after the last.
     */
    private ask(
        start: number,
        to: number,
        notes: readonly Note[],
        { first = false, fresh = false }: { first?: boolean; fresh?: boolean } = {},
    ): Answer {
        const request = {
            sample_rate: this.emitter.sampleRate(this.clock),
            from: start,
            to,
            start: first,
            notes: notes.map((n) => ({
                node: n.node,
                start: n.start,
                off: n.off,
                channel: n.channel,
                freq: n.freq,
                curves: n.curves,
                ...this.position(n.beat, start, to),
            })),
            channels: this.channels.map((c) => ({
                id: c.id,
                start: c.start,
                target: c.target,
                points: c.points,
                state: fresh ? null : c.state,
                ...this.position(c.beat, start, to),
            })),
        };
        return JSON.parse(coreEventCurves(JSON.stringify(request))) as Answer;
    }

    private send(bundles: Answer["bundles"]): void {
        const { server } = this.emitter;
        const clock = this.clock;
        for (const [secs, sets] of bundles ?? []) {
            if (clock === null) {
                // The wall's own second, not a delay from a now read again.
                server.sendAt(new Moment(null, 0), messages(sets), { keep: false, pin: secs });
            } else {
                server.sendAt(new Moment(clock, clock.secs2beats(secs)), messages(sets), {
                    keep: false,
                });
            }
        }
    }

    // ---- a note ----

    /**
     * Takes `note` in, and answers what its controls are set to as it is
     * made: its curves' first values, as messages for the bundle that makes
     * it.
     */
    begin(note: Note): TimedMessage[] {
        this.notes.set(note.node, note);
        if (note.curves.length === 0 && this.channels.length === 0) return [];
        // The channels' curves where they stand at the note's own start, not
        // where the wake, which runs ahead, has left their glide.
        const answer = this.ask(note.start, note.start, [note], { first: true, fresh: true });
        const bundles = answer.bundles ?? [];
        return bundles.length > 0 ? messages(bundles[0][1]) : [];
    }

    /**
     * The note is made: sends what was already sent for the others, and sees
     * that the wake runs.
     */
    follow(note: Note): void {
        if (note.curves.length === 0 && this.channels.length === 0) return;
        if (this.until !== null && this.until > note.start) {
            this.send(this.ask(note.start, this.until, [note], { fresh: true }).bundles);
        }
        this.wake(note.start);
    }

    // ---- the wake ----

    /**
     * Sees that the periodic wake runs, sending from `since` on when it was
     * not.
     */
    wake(since: number): void {
        const stale = this.clock !== null && !this.emitter.offline
            && performance.now() - this.woken > 8 * WINDOW * 1000;
        if (this.awake && !stale) return;
        const begin = Math.min(since, this.now());
        if (this.until === null || this.until < begin) {
            // A stretch nothing was sent over: a glide left there is stale.
            this.until = begin;
            for (const curve of this.channels) curve.state = null;
        }
        this.awake = true;
        this.woken = performance.now();
        if (this.clock !== null) {
            this.clock.sched(0, () => this.onClock());
        } else if (this.emitter.offline) {
            // No clock and no wall: the score takes every value now.
            while (this.window((this.until ?? 0) + 1.0)) { /* until idle */ }
        } else {
            this.onTimer();
        }
    }

    /**
     * Sends the instants up to `to`; whether anything is left to send after
     * them.
     */
    private window(to: number): boolean {
        if (this.notes.size === 0) {
            this.awake = false;
            return false;
        }
        const until = this.until ?? to;
        if (to > until) {
            const answer = this.ask(until, to, [...this.notes.values()]);
            this.send(answer.bundles);
            const states = new Map(answer.states ?? []);
            for (const curve of this.channels) {
                const state = states.get(curve.id);
                if (state !== undefined) curve.state = state;
            }
            for (const node of answer.over ?? []) this.emitter.gone(node, this);
            this.until = to;
            if (answer.idle || this.notes.size === 0) {
                this.awake = false;
                return false;
            }
        }
        return true;
    }

    private tick(): boolean {
        this.woken = performance.now();
        return this.window(this.now() + 2 * WINDOW);
    }

    /**
     * The wake as a clock's item: answers the beats to the next one, or
     * nothing when there is nothing left to send.
     */
    private onClock(): number | void {
        if (!this.tick()) return;
        const clock = this.clock!;
        const beat = clock.beats();
        return Math.max(clock.secs2beats(clock.beats2secs(beat) + WINDOW) - beat, 1e-6);
    }

    private onTimer(): void {
        if (this.tick()) setTimeout(() => this.onTimer(), WINDOW * 1000);
    }
}

/**
 * What sends the curves of the events a server plays: the list of what
 * sounds, by the clock it sounds on, and the values of the next stretch.
 *
 * Made by its `Server` (`server.curves`) and driven by it: `Server.playEvent`
 * hands it every note, and `Automation.play` a channel's curve. A page does
 * not call it.
 */
export class CurveEmitter {
    /** The server whose notes these are. */
    readonly server: Server;
    readonly #axes = new Map<TempoClock | null, Axis>();
    /** node -> the axis it sounds on. */
    readonly #notes = new Map<number, Axis>();
    /** curve id -> the axis it plays on. */
    readonly #curves = new Map<number, Axis>();
    #next = 1;

    constructor(server: Server) {
        this.server = server;
    }

    /** Whether the server is a score being written rather than one that sounds. */
    get offline(): boolean {
        return this.server.scoring;
    }

    /**
     * Now, for what plays outside any clock: Unix seconds, and the score's
     * zero offline.
     */
    wall(): number {
        return this.offline ? 0.0 : Date.now() / 1000;
    }

    /**
     * The rate a curve is stepped at: the clock's, when it is on the server's
     * samples, else the engine's default.
     *
     * @internal
     */
    sampleRate(clock: TempoClock | null): number {
        const timebase = clock?.timebase;
        return timebase instanceof SampleClockTimebase ? timebase.sampleRate : RATE;
    }

    /** Whether nothing is being followed. @internal */
    get empty(): boolean {
        return this.#axes.size === 0;
    }

    #axis(clock: TempoClock | null): Axis {
        let axis = this.#axes.get(clock);
        if (axis === undefined) {
            axis = new Axis(this, clock);
            this.#axes.set(clock, axis);
        }
        return axis;
    }

    /** @internal */
    gone(node: number, axis: Axis): void {
        axis.notes.delete(node);
        this.#notes.delete(node);
        if (axis.notes.size === 0 && axis.channels.length === 0) this.#axes.delete(axis.clock);
    }

    // ---- notes ----

    /**
     * Takes in the note `event` is about to be, on `node`, played at `when`
     * for `sustain` beats -- `pin` being the wall-clock instant a clockless
     * `when` counts from. Answers the `/node_set` messages that go in the
     * bundle that makes it: its curves' first values.
     */
    note(
        node: number,
        when: Moment,
        event: Event,
        sustain: number,
        pin: number | null = null,
    ): TimedMessage[] {
        const given = event.get("automation") as Playable | Playable[] | undefined;
        const curves = given === undefined || given === null
            ? []
            : Array.isArray(given) ? given : [given];
        const written = curves
            .filter((c) => c.enabled !== false)
            .map((c) => ({ target: c.target, points: [...(c.write().points ?? [])] }));
        const axis = this.#axis(when.clock);
        if (axis.notes.size > 256) {
            // Nothing woke to say so: the notes whose off has passed.
            const now = axis.now();
            for (const old of [...axis.notes.values()]) {
                if (old.off !== null && old.off < now) this.gone(old.node, axis);
            }
        }
        const [start, beat] = axis.placed(when, pin);
        const off = when.clock === null ? start + sustain : when.at(sustain).secs();
        const note: Note = {
            node,
            start,
            beat,
            off: Number.isFinite(off) ? off : null,
            channel: Math.trunc(Number(event.get("channel") ?? 0)) || 0,
            freq: event.freq(),
            curves: written,
        };
        this.#notes.set(node, axis);
        return axis.begin(note);
    }

    /** The bundle that makes `node` is sent: its curves follow. */
    follow(node: number): void {
        const axis = this.#notes.get(node);
        const note = axis?.notes.get(node);
        if (axis !== undefined && note !== undefined) axis.follow(note);
    }

    /**
     * `node` is gone -- freed, released by hand, or ended: nothing more is
     * sent to it.
     */
    forget(node: number): void {
        const axis = this.#notes.get(node);
        if (axis !== undefined) this.gone(node, axis);
    }

    // ---- a channel's curve ----

    /**
     * Plays a channel's curve from `when` on: it reaches the notes of its
     * channel on that clock, sounding and to come, and holds its last value
     * past its end. Answers what {@link CurveEmitter.stop} takes.
     */
    play(target: unknown, points: readonly unknown[], when: Moment): number {
        const axis = this.#axis(when.clock);
        const [start, beat] = axis.placed(when);
        const curve: Curve = { id: this.#next++, start, beat, target, points: [...points], state: null };
        axis.channels.unshift(curve);
        this.#curves.set(curve.id, axis);
        if (axis.notes.size > 0) axis.wake(start);
        return curve.id;
    }

    /**
     * Stops the channel's curve {@link CurveEmitter.play} answered `curve`
     * for: the notes it reached keep the last value it set.
     */
    stop(curve: number): void {
        const axis = this.#curves.get(curve);
        if (axis === undefined) return;
        this.#curves.delete(curve);
        axis.channels = axis.channels.filter((c) => c.id !== curve);
        if (axis.notes.size === 0 && axis.channels.length === 0) this.#axes.delete(axis.clock);
    }
}
