// Static timelines and a playhead (mirrors `clausters/seq/timeline.py`).
//
// The counterpart to the generative layer (`Routine`, `Pbind`). A routine is a
// forward-only generator: its musical state lives in the generator's locals,
// so it cannot be *seeked*. A `Timeline` is the opposite — a **static,
// editable list of timed items kept sorted by beat**, with random access by
// time (`indexAt`, `range`). That is what makes DAW-style transport controls
// possible: a `Playhead` scans the timeline forward as the clock advances, and
// play / stop / locate / loop re-seek the cursor by time at the boundaries.
//
// An *item* is anything that can render itself on a destination — it has a
// `play(destination)` method. `Event` already is one, so a timeline of events
// renders to whatever destination the playhead holds, exactly like the rest of
// the client. `OscEvent` wraps a raw OSC message, so a timeline can also be a
// plain, editable OSC score.
//
// This layer is **client-side**: each playhead has its own local transport
// over its own timeline, and several clients phase-align through `quant` and
// the shared `/transport_set` grid. A playhead can also *follow* the server's
// transport (`followTransport`), which is one conductor's play/stop/locate
// driving every client — the same local transport, driven from outside.

import { TempoClock } from "../base/clock.ts";
import { ManualTimebase } from "../base/timebase.ts";
import { currentRoutine } from "../base/context.ts";
import { Routine } from "../base/stream.ts";
import { Event } from "./event.ts";
import type { EventDestination } from "./event.ts";
import type { Pattern } from "./pattern.ts";
import type { Server, TimedMessage } from "../defs/server/index.ts";
import type { MsgArg } from "../base/osc.ts";
import { OscFunc } from "../responders.ts";

/** What a timeline can hold: anything that renders itself on a destination. */
export interface TimelineItem {
    play(destination: PlayDestination): unknown;
}

/** The destination a playhead renders on. `Server` satisfies it. */
export interface PlayDestination extends EventDestination {
    sendBundle(
        messages: readonly TimedMessage[],
        options?: { delayBeats?: number; clock?: TempoClock },
    ): void;
}

/**
 * One timed item. A stable object, so it can be removed or moved by identity
 * after other edits have shifted positions.
 */
export class Entry {
    beat: number;
    readonly item: unknown;

    constructor(beat: number, item: unknown) {
        this.beat = beat;
        this.item = item;
    }
}

/**
 * A raw OSC message as a timeline item: rendering it sends the message at the
 * playhead's current logical beat.
 */
export class OscEvent {
    readonly message: TimedMessage;

    constructor(addr: string, ...args: MsgArg[]) {
        this.message = [addr, ...args];
    }

    play(destination: PlayDestination): void {
        destination.sendBundle([this.message]);
    }
}

/**
 * A static, editable sequence of `(beat, item)` kept sorted by beat, with
 * random access by time.
 *
 * Items stay in beat order, and a stable insert preserves the order of items
 * added at the same beat (a note-off before a re-trigger). `add` returns a
 * handle you pass back to `remove`/`move`, so edits stay correct as other
 * inserts shift indices.
 */
export class Timeline {
    private entries: Entry[] = [];

    constructor(items?: Iterable<readonly [number, unknown]>) {
        if (items) for (const [beat, item] of items) this.add(beat, item);
    }

    // ---- editing ----

    /** Inserts `item` at `beat` (kept sorted); returns the entry handle. */
    add(beat: number, item: unknown): Entry {
        const entry = new Entry(beat, item);
        this.entries.splice(this.insertIndex(beat), 0, entry);
        return entry;
    }

    /** Removes an entry returned by `add` (by identity). */
    remove(entry: Entry): this {
        const i = this.entries.indexOf(entry);
        if (i >= 0) this.entries.splice(i, 1);
        return this;
    }

    /** Moves an entry to `newBeat`, keeping the timeline sorted. */
    move(entry: Entry, newBeat: number): Entry {
        this.remove(entry);
        entry.beat = newBeat;
        this.entries.splice(this.insertIndex(newBeat), 0, entry);
        return entry;
    }

    /** Drops every item. */
    clear(): this {
        this.entries = [];
        return this;
    }

    /**
     * Snaps every placement to the nearest multiple of `grid` beats; a zero or
     * negative grid is a no-op.
     */
    quantize(grid: number): this {
        if (grid <= 0) return this;
        for (const entry of this.entries) {
            entry.beat = Math.max(0, Math.round(entry.beat / grid) * grid);
        }
        this.entries.sort((a, b) => a.beat - b.beat);
        return this;
    }

    // ---- random access by time ----

    /**
     * The index the *last* item at `beat` would be inserted after (a stable
     * insert), and the cursor of the first item strictly after it.
     */
    private insertIndex(beat: number): number {
        let lo = 0;
        let hi = this.entries.length;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (this.entries[mid]!.beat <= beat) lo = mid + 1;
            else hi = mid;
        }
        return lo;
    }

    /**
     * The cursor (index) of the first item at or after `beat` — the seek
     * primitive a playhead starts and locates with.
     */
    indexAt(beat: number): number {
        let lo = 0;
        let hi = this.entries.length;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (this.entries[mid]!.beat < beat) lo = mid + 1;
            else hi = mid;
        }
        return lo;
    }

    /** The `(beat, item)` pairs in the half-open beat window `[t0, t1)`. */
    range(t0: number, t1: number): [number, unknown][] {
        return this.entries
            .slice(this.indexAt(t0), this.indexAt(t1))
            .map((e) => [e.beat, e.item] as [number, unknown]);
    }

    /** The items exactly at `beat`. */
    at(beat: number): unknown[] {
        return this.entries.filter((e) => e.beat === beat).map((e) => e.item);
    }

    /** The beat of the last item (0 when empty) — the timeline's length. */
    duration(): number {
        return this.entries.at(-1)?.beat ?? 0;
    }

    get length(): number {
        return this.entries.length;
    }

    /** The `(beat, item)` pair at index `i`. */
    get(i: number): [number, unknown] | undefined {
        const entry = this.entries[i];
        return entry ? [entry.beat, entry.item] : undefined;
    }

    *[Symbol.iterator](): Generator<[number, unknown], void, undefined> {
        for (const entry of this.entries) yield [entry.beat, entry.item];
    }

    // ---- capture a pattern into a timeline ----

    /**
     * Bounces an event pattern into a static timeline by running it with no
     * pacing and recording each event at its logical beat. `dur` bounds an
     * open-ended pattern (in beats); leave it out to drain a finite one.
     *
     * The run uses the clock's own seams — a hand-driven timebase and ticker —
     * so it is the same driver live playback uses, only advanced as fast as
     * the loop can go.
     */
    static fromPattern(
        pattern: Pattern<unknown>,
        { dur, tempo = 1.0 }: { dur?: number; tempo?: number } = {},
    ): Timeline {
        const timeline = new Timeline();
        const recorder: EventDestination = {
            playEvent(event: Event) {
                timeline.add(currentRoutine()?.logicalBeat ?? 0, event);
                return null;
            },
            sendMsg() {},
        };
        // The offline drive, which is what a bounce is: no wall clock, no
        // ticker, no sleeping. The clock is deliberately **not started** —
        // `render` walks the queue in beat order itself, so the pattern is
        // queued and then drained, which is the same pair of calls the Python
        // client makes (`pattern.play(clock, recorder)`; `clock.render(dur)`).
        // Driving a `manualTicker` by hand here was a second driver for a job
        // this one already does.
        const clock = new TempoClock(tempo, { timebase: new ManualTimebase(0) });
        pattern.play(recorder, { clock });
        clock.render(dur);
        clock.close();
        return timeline;
    }
}

/**
 * A transport over a `Timeline`: play / stop / locate / loop, and a song
 * `position`.
 *
 * The playhead scans the timeline forward as a clock advances, rendering each
 * item on a destination. The forward scan is what `play` runs; the random
 * access lives at the boundaries — `play({ at })` and `locate(beat)` re-seek
 * the cursor by time, which a forward-only routine could never do.
 *
 * Timing rides the clock's logical time like everything else, so a playhead
 * inherits `quant` and a sample-exact timebase for free.
 *
 * A pass ends on its own when the scan reaches the end of the timeline:
 * `playing` goes false and `finished` says the end is why, so a transport
 * reads the end off the playhead instead of timing it.
 */
export class Playhead {
    readonly timeline: Timeline;
    readonly clock: TempoClock;
    readonly destination: PlayDestination;

    private running = false;
    private ended = false;
    private epoch = 0;
    private routine: Routine | null = null;
    private loopWindow: [number, number] | null = null;
    private startBeat = 0;
    private posBeat = 0;
    private posClock: number | null = null;
    /** The responder obeying a server's transport, while following one. */
    private following: OscFunc | null = null;

    constructor(timeline: Timeline, clock: TempoClock, destination: PlayDestination) {
        this.timeline = timeline;
        this.clock = clock;
        this.destination = destination;
    }

    // ---- transport ----

    /**
     * Starts (or restarts) playback from beat `at`, snapping the start to a
     * `quant` boundary of the clock's grid. Re-seeks the cursor to `at`, so it
     * doubles as a locate-and-play.
     */
    play({ at = 0, quant }: { at?: number; quant?: number } = {}): this {
        this.startBeat = at;
        this.posBeat = at;
        this.posClock = null;
        this.running = true;
        this.ended = false;
        this.epoch += 1;
        const epoch = this.epoch;
        if (this.routine !== null) this.clock.unsched(this.routine);
        this.routine = new Routine(() => this.feed(epoch));
        this.clock.play(this.routine, quant);
        return this;
    }

    /**
     * Halts the playhead. Items already rendered keep sounding (their releases
     * are scheduled); no further items are played.
     */
    stop(): this {
        this.posBeat = this.position();
        this.running = false;
        this.ended = false; // halted by hand, not ended
        this.posClock = null;
        this.epoch += 1;
        if (this.routine !== null) {
            this.clock.unsched(this.routine);
            this.routine = null;
        }
        return this;
    }

    /**
     * Seeks to `beat`. While playing, restarts the scan from there (random
     * access); while stopped, sets where the next `play` begins.
     */
    locate(beat: number): this {
        if (this.running) {
            this.play({ at: beat });
        } else {
            this.startBeat = beat;
            this.posBeat = beat;
            this.ended = false; // seeking away from the end leaves it behind
        }
        return this;
    }

    /**
     * Loops the half-open beat window `[start, end)`: when the scan reaches
     * `end` it wraps back to `start`. Set before or during play.
     */
    loop(start: number, end: number): this {
        this.loopWindow = [start, end];
        return this;
    }

    /** Stops looping; the scan plays through to the end. */
    unloop(): this {
        this.loopWindow = null;
        return this;
    }

    // ---- following the server's shared transport ----

    /**
     * Makes this playhead obey a `server`'s shared transport: when a conductor
     * calls `transportPlay` / `transportStop` / `transportLocate`, the server
     * broadcasts the new state and this playhead rolls / halts / seeks to
     * match — so several clients run in lockstep on one grid.
     *
     * It registers for the server's pushes (`notify`, which `boot`/`attach`
     * already does) and subscribes to the `/transport_query.reply` broadcasts
     * through `server.onReply`, then applies the current state once. `quant`
     * snaps each rolling start to a beat boundary, so every follower lands
     * together — with the clock joined to the same grid
     * (`TempoClock.joinTransport`) that boundary is the *shared* bar line.
     * Release with `unfollowTransport`.
     *
     * Beat-aligned in plain wall-clock mode; sample-exact when the clock is
     * also locked to the server (`Session.lockToServer`).
     *
     * The responder is an `OscFunc` on the server's receiver, as in the
     * reference client — with the receiver the page already has (the server's
     * connection) rather than a socket opened for the purpose, which a browser
     * has no way to bind.
     */
    async followTransport(
        server: Server,
        { quant, timeout }: { quant?: number; timeout?: number } = {},
    ): Promise<this> {
        this.unfollowTransport();
        await server.notify(true, timeout);
        this.following = new OscFunc(
            (msg) => {
                // /transport_query.reply originSample tempo defined playing position ...
                if (msg.length < 7 || !Number(msg[3])) return;
                const position = Number(msg[5]);
                if (Number(msg[4])) {
                    this.play({ at: position, quant });
                } else {
                    this.stop();
                    this.locate(position);
                }
            },
            "/transport_query.reply",
            { recv: server.receiver },
        );
        const state = await server.transportState(timeout);
        // Gated on the **grid**, not on the state: the state is always there
        // now, but a playhead runs on beats, and `position` is 0 until a grid
        // says what a beat is. Applying that would locate to 0 on a server
        // whose transport is being driven in samples.
        if (state.tempo !== null) {
            if (state.playing) this.play({ at: state.position, quant });
            else this.locate(state.position);
        }
        return this;
    }

    /**
     * Stops following a server transport (see `followTransport`): drops the
     * subscription, leaving the playhead wherever the last broadcast left it.
     */
    unfollowTransport(): this {
        this.following?.free();
        this.following = null;
        return this;
    }

    /**
     * The current song position, in beats. Interpolated from the clock between
     * items while playing; the start or last-seek beat while stopped.
     */
    /**
     * The **clock** beat the scan last woke on — the origin `position`
     * interpolates from, and, once the scan has drained, the beat at which its
     * last item was rendered. `null` before the first wake.
     *
     * A transport reads it to keep a cursor moving after the scan is over: the
     * piece ends where the last item does, which is a stretch of time later.
     */
    get scannedAt(): number | null {
        return this.posClock;
    }

    position(): number {
        if (!this.running || this.posClock === null) return this.posBeat;
        let pos = this.posBeat + (this.clock.beats() - this.posClock);
        if (this.loopWindow !== null) {
            const [start, end] = this.loopWindow;
            const span = end - start;
            if (span > 0 && pos >= end) pos = start + ((pos - start) % span);
        }
        return pos;
    }

    /**
     * Whether the scan is running. It goes false on `stop` **and** when the
     * scan reaches the end of the timeline, so a transport polls this one flag
     * instead of comparing `position` against a length of its own.
     */
    get playing(): boolean {
        return this.running;
    }

    /**
     * Whether the scan ran off the end, as opposed to being halted by hand or
     * still playing. It is the *scan* that ended: a loop never ends, and the
     * last item keeps sounding for its own length — the playhead schedules
     * items, it does not wait for them.
     */
    get finished(): boolean {
        return this.ended;
    }

    // ---- the feeder: a cursor walk fed to the clock ----

    private *feed(epoch: number): Generator<number | undefined, void, unknown> {
        const tl = this.timeline;
        let cursor = tl.indexAt(this.startBeat);
        let prev = this.startBeat;
        while (this.running && epoch === this.epoch) {
            this.posBeat = prev;
            this.posClock = this.clock.beats();
            if (this.loopWindow !== null) {
                const [start, end] = this.loopWindow;
                const next = tl.get(cursor);
                if (next === undefined || next[0] >= end) {
                    const tail = end - prev;
                    if (tail > 0) yield tail;
                    cursor = tl.indexAt(start);
                    prev = start;
                    continue;
                }
            }
            const entry = tl.get(cursor);
            if (entry === undefined) {
                // Drained: the pass is over, and the transport driving it has
                // to know without polling a length of its own. The feeder runs
                // on the clock, so it records the end rather than announcing
                // it — `playing` goes false, `position` freezes on the last
                // item.
                this.running = false;
                this.ended = true;
                return;
            }
            const [beat, item] = entry;
            const wait = beat - prev;
            if (wait > 0) {
                yield wait;
                if (!(this.running && epoch === this.epoch)) return;
                prev = beat;
                this.posBeat = prev;
                this.posClock = this.clock.beats();
            }
            (item as { play(destination: PlayDestination): unknown }).play(
                this.destination,
            );
            cursor += 1;
        }
    }
}
