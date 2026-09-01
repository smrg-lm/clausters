// The TempoClock: musical time, and the driver that resumes routines on it
// (mirrors `clausters/base/clock.py`).
//
// The seam between the shared core and the host language. The clock owns the
// scheduling queue and the beat/second arithmetic — both of them
// `clausters-core`'s, reached through the wasm door, so timing matches the
// server's own sample clock. The queue holds **routines** (and one-shot
// callables); resuming a routine (the `yield` driver) stays in TypeScript.
//
// The defining property is that the **logical beat advances only by the
// routines' yields**, never by wall-clock drift: a routine that yields `0.25`
// is resumed exactly a quarter-beat later, whatever the browser's timers do.
// That is what makes inter-event timing exact — and, with a `SampleClockTimebase`,
// sample-accurate. The wake-up only has to arrive within the emission
// headroom (`Server.latency`); the exactness rides on the timetag, not on the
// wake-up.
//
// The clock does **not** talk to the server. It only schedules and reports
// time (`beats`, `beats2secs`, `startTime`); emitting belongs to `Server`,
// which reads the clock of the routine it is resuming. Anchoring to a
// server's sample clock is likewise the Server's job — `server.sampleTimebase()`
// hands back a timebase this clock merely reads, and `joinTransport` reads a
// server's shared grid **once**, at the join, keeping three numbers: after it
// the clock is as offline as before.

import { Scheduler, TempoMap } from "../core/clausters_core_web.js";
import { setCurrentRoutine } from "./context.ts";
import { BEATS, LINEAR, tempoEnv } from "./time.ts";
import type { CurveSpec, TempoEnvelope, TimeUnit } from "./time.ts";
import { Routine, Stream, StopStream } from "./stream.ts";
import {
    MonotonicTimebase,
    SampleClockTimebase,
    bar,
    beatInBar,
    quantDelay,
    samplesToSecs,
} from "./timebase.ts";
import { main } from "./main.ts";
import type { SessionLike } from "./main.ts";
import type { Server } from "../defs/server/index.ts";
import type { Timebase } from "./timebase.ts";
import type { TickReply, TickRequest } from "./tick-worker.ts";

/**
 * Anything the clock can resume: a stream (a `Routine`), or a plain callable
 * for a one-shot. A callable that returns a number is rescheduled by that
 * many beats; one returning nothing runs once.
 */
export type Schedulable = Stream | (() => number | void);

// ---- the pacing seam ----

/**
 * How the clock is woken. One wake is pending at a time: scheduling again
 * replaces it, which is all a single-queue driver needs.
 */
export interface Ticker {
    /** Wakes `callback` in `seconds`, replacing any pending wake. */
    schedule(seconds: number, callback: () => void): void;
    /** Drops the pending wake, if any. */
    cancel(): void;
    /** Releases whatever the ticker holds. */
    close(): void;
}

/**
 * The page-thread ticker. Correct everywhere, but clamped when nested and
 * throttled to about a second in a background tab — which is why the browser
 * default is the worker one.
 */
export function timerTicker(): Ticker {
    let pending: ReturnType<typeof setTimeout> | null = null;
    const cancel = () => {
        if (pending !== null) clearTimeout(pending);
        pending = null;
    };
    return {
        schedule(seconds, callback) {
            cancel();
            pending = setTimeout(callback, Math.max(seconds, 0) * 1000);
        },
        cancel,
        close: cancel,
    };
}

/**
 * A ticker driven by hand: what tests wake with, so the same driver the
 * browser runs advances deterministically and instantly.
 */
export interface ManualTicker extends Ticker {
    /** The seconds the clock last asked to sleep, or `null` when it is idle. */
    readonly pending: number | null;
    /** Runs the pending wake. */
    fire(): void;
}

export function manualTicker(): ManualTicker {
    let seconds: number | null = null;
    let callback: (() => void) | null = null;
    return {
        get pending() {
            return seconds;
        },
        schedule(s, cb) {
            seconds = s;
            callback = cb;
        },
        cancel() {
            seconds = null;
            callback = null;
        },
        close() {
            this.cancel();
        },
        fire() {
            const run = callback;
            seconds = null;
            callback = null;
            run?.();
        },
    };
}

/**
 * The page's one tick worker, shared by every clock on it (a worker per clock
 * would give nothing: the work is a `setTimeout`).
 */
let sharedWorker: Worker | null = null;
let nextTickerId = 1;
const tickCallbacks = new Map<number, () => void>();

function tickWorker(): Worker {
    if (sharedWorker === null) {
        sharedWorker = new Worker(new URL("./tick-worker.js", import.meta.url), {
            type: "module",
        });
        sharedWorker.onmessage = (event: MessageEvent<TickReply>) => {
            const callback = tickCallbacks.get(event.data.id);
            tickCallbacks.delete(event.data.id);
            callback?.();
        };
    }
    return sharedWorker;
}

/**
 * The browser ticker: the wake-up is timed in a worker, so a background tab's
 * timer throttling cannot starve the schedule.
 */
export function workerTicker(): Ticker {
    const worker = tickWorker();
    const id = nextTickerId++;
    const post = (msg: TickRequest) => worker.postMessage(msg);
    return {
        schedule(seconds, callback) {
            tickCallbacks.set(id, callback);
            post({ id, delayMs: Math.max(seconds, 0) * 1000 });
        },
        cancel() {
            tickCallbacks.delete(id);
            post({ id, cancel: true });
        },
        close() {
            this.cancel();
        },
    };
}

/**
 * The default ticker for this environment: the worker where there is one, the
 * page timer otherwise (node, and any environment without `Worker`).
 */
export function defaultTicker(): Ticker {
    return typeof Worker === "undefined" ? timerTicker() : workerTicker();
}

// ---- the clock ----

/**
 * One queued item and how many times it is currently queued (the same
 * routine may sit in the queue more than once).
 */
interface Entry {
    item: Schedulable;
    queued: number;
}

export interface TempoClockOptions {
    /**
     * The pacing source. Defaults to the page's monotonic clock; pass a
     * `SampleClockTimebase` from `Server.sampleTimebase()` to pace against a
     * server's own sample counter.
     */
    timebase?: Timebase;
    /** How the clock is woken. Defaults to `defaultTicker()`. */
    ticker?: Ticker;
}

/** What {@link TempoClock.setTempo} takes beside the tempo. */
export interface SetTempoOptions {
    /** How far the change is spread; omitted, the change is a step. */
    over?: number;
    /** What `over` (or an envelope's times) measures. */
    unit?: TimeUnit;
    /** The shape of the change. An envelope carries its own. */
    curve?: CurveSpec;
}

/** A scheduler that keeps musical time in beats and resumes routines on it. */
export class TempoClock {
    /**
     * The piece's beat→second map, and the clock's whole relation to time. It
     * starts as one constant-tempo segment, which computes exactly the affine
     * expression this clock always used; {@link TempoClock.setTempo} records a
     * breakpoint on it instead of overwriting the one anchor there used to be,
     * so what a tempo change moved stays knowable afterwards.
     *
     * It is a pure function of a beat — it knows nothing of *now* — which is
     * what lets an editor draw the piece from the same one the clock plays by.
     *
     * Assigning it hands the clock a piece's own tempo: the map is **copied**,
     * so later edits on either side stay apart, and the pacing picks it up from
     * the next wait. Do it before `start` — replacing the map under a running
     * clock moves every beat that has not fired yet, which is a seek and not a
     * tempo change.
     */
    get map(): TempoMap {
        return this.tempoMapHeld;
    }

    set map(tempoMap: TempoMap) {
        this.tempoMapHeld = tempoMap.copy();
        this.syncMap(true);
    }

    private tempoMapHeld: TempoMap;
    /**
     * The pacing source — *only* used to decide how long to sleep between
     * items, and read by `Server` to choose how to stamp what it emits.
     */
    timebase: Timebase;
    /**
     * The environment this clock belongs to.
     *
     * **A clock built while a session is ambient adopts it**, and the session
     * keeps it and closes it. That is not a convenience: it is the
     * back-reference ambient resolution follows, and a play running on this
     * clock resolves *that* session's server and random root — which is what
     * keeps several sessions on one page isolated from each other. A clock
     * built with no session ambient has `null` here and resolves against the
     * default session.
     *
     * The clock still never talks to a server — this is a field it is read
     * through, not a collaborator it calls.
     */
    session: SessionLike | null = null;

    // The last segment as an affine triple, refreshed on every edit: the anchor
    // `tempo = x` re-hangs the map from, so assigning a tempo keeps the beat the
    // clock is on where it already was. `tempoCache` is that segment's tempo --
    // the *destination* while a ramp is running; the tempo actually sounding is
    // the `tempo` getter, read from the map.
    private baseBeats = 0;
    private baseSecs = 0;
    private tempoCache = 1.0;
    /** The joined `/transport_set` grid, or `null` on this clock's own beats. */
    private transport: { kind: "sample" | "wall"; origin: number; tempo: number } | null = null;
    private readonly queue = new Scheduler();
    private readonly items = new Map<number, Entry>();
    private readonly ids = new WeakMap<object, number>();
    private nextId = 1;
    private readonly ticker: Ticker;
    private running = false;
    private mode: "rt" | "nrt" | "stopped" = "stopped";
    /** The yield-driven beat while an item is being resumed. */
    private logicalBeat = 0;
    private monoStart: number | null = null;
    private unixStart: number | null = null;
    /** The timebase instant `freeze` held the beat at, or `null` when running. */
    private frozenAt: number | null = null;
    private pumping = false;

    constructor(tempo = 1.0, { timebase, ticker }: TempoClockOptions = {}) {
        this.tempoMapHeld = new TempoMap(tempo);
        this.timebase = timebase ?? new MonotonicTimebase();
        this.ticker = ticker ?? defaultTicker();
        main.currentSession?.adopt?.(this);
    }

    // ---- beat/second math (through the core) ----

    /**
     * Beats per second **at the beat the clock is on** — the tempo that is
     * sounding, read from the map (`map.tempoAt(beats())`).
     *
     * Under a constant tempo, and after a ramp has finished, this is the last
     * change's tempo. *Inside* a ramp it is the tempo reached so far, not the
     * one being ramped to: the destination is `map.last()`, and a piece whose
     * map has changes still ahead of the playhead has not reached them.
     *
     * Assigning it changes the slope without pinning the instant, which is what
     * setting the grid does; {@link TempoClock.setTempo} is the musical gesture
     * (it keeps the current beat on the second it already fell on).
     */
    get tempo(): number {
        return this.tempoMapHeld.tempoAt(this.beats());
    }

    set tempo(tempo: number) {
        this.tempoMapHeld = TempoMap.anchored(tempo, this.baseBeats, this.baseSecs)
            ?? new TempoMap(tempo);
        this.syncMap();
    }

    /**
     * Re-reads the map's last segment into the affine cache, and (for an edit)
     * wakes the driver, which may be asleep on a wait the edit just moved.
     */
    private syncMap(wake = false): void {
        const [beats, secs, tempo] = this.tempoMapHeld.last();
        this.baseBeats = beats;
        this.baseSecs = secs;
        this.tempoCache = tempo;
        if (wake) this.wakeSoon();
    }

    /**
     * A beat position in seconds, through the piece's time map. Under one tempo
     * this is the affine conversion it has always been; across a tempo change it
     * is the integral, so a beat before the change still reports the second it
     * actually fell on.
     */
    beats2secs(beats: number): number {
        return this.tempoMapHeld.secsAt(beats);
    }

    /** Seconds as a beat position — the inverse of {@link TempoClock.beats2secs}. */
    secs2beats(secs: number): number {
        return this.tempoMapHeld.beatsAt(secs);
    }

    /**
     * The clock's current beat: the paced elapsed beat while running (what
     * scheduling relative to "now" reads), else the yield-driven logical beat
     * — before the first `start`, and after a `stop`, which holds the beat it
     * reached.
     */
    beats(): number {
        if (!this.running || this.monoStart === null) return this.logicalBeat;
        if (this.frozenAt !== null) {
            return this.secs2beats(this.frozenAt - this.monoStart);
        }
        return this.secs2beats(this.timebase.now() - this.monoStart);
    }

    // ---- the freeze gate (a server transport's pause, reaching the clock) ----

    /** Whether the beat is held where `freeze` left it. */
    get frozen(): boolean {
        return this.frozenAt !== null;
    }

    /**
     * Holds the logical beat where it is, without stopping the clock.
     *
     * This is how a server transport's pause reaches a client
     * (`Server.transportStop` on a governed group). The timebase only decides
     * how long to sleep between events and how to stamp one, so a page whose
     * server froze would otherwise keep advancing beats and scheduling events
     * ahead — running away from a piece that is not moving. Freezing stops the
     * beat instead of stopping the playhead: what was already scheduled stays
     * scheduled, and the server's frozen queue holds it.
     *
     * The client's reaction does not have to be precise. Between the server's
     * stop and this call a little look-ahead has already gone out; it lands in
     * the server's frozen queue and fires on resume in its exact relative
     * place. The exactness is the engine's, not the page's.
     *
     * Idempotent: freezing an already frozen clock keeps the first freeze's
     * position.
     */
    freeze(): this {
        if (this.frozenAt === null) this.frozenAt = this.timebase.now();
        return this;
    }

    /**
     * Whether the beat advances **by itself**: the real-time driver is running
     * (`start`, not yet `stop`ped).
     *
     * False before the first `start` and during an offline render, whose beat is
     * the queue's position and not the wall's — the distinction a caller needs
     * before treating `beats()` as a thing that moves while it waits (a
     * transport sweeping a cursor over the last item's tail). Freezing does not
     * change it: a frozen clock is rolling and held.
     */
    get rolling(): boolean {
        return this.running && this.mode === "rt";
    }

    /**
     * Resumes from where `freeze` left the beat.
     *
     * The pacing origin shifts by the time spent frozen, so those seconds are
     * not part of the piece: the beat picks up where it stopped rather than
     * jumping forward by the length of the pause.
     */
    thaw(): this {
        if (this.frozenAt !== null) {
            if (this.monoStart !== null) {
                this.monoStart += this.timebase.now() - this.frozenAt;
            }
            this.frozenAt = null;
        }
        return this;
    }

    /**
     * The wall-clock origin (Unix seconds) of the current beat axis — the
     * instant beat 0 falls on — or `null` before the first `start`. The
     * Server turns a logical beat into a timetag from it: the **wall** clock,
     * kept apart from the monotonic pacing source so timetags stay valid Unix
     * time. A `stop` leaves it in place (it is the axis a later `start`
     * resumes); a `start` re-places it so the held beat maps to now.
     */
    get startTime(): number | null {
        return this.unixStart;
    }

    /**
     * The timebase value of the current beat axis' zero, placed by `start`.
     * For a sample timebase this is `sampleOrigin / sampleRate`, which the
     * Server turns into the absolute sample for `/sched_at`.
     */
    get pacingOrigin(): number | null {
        return this.monoStart;
    }

    /** How many items are queued. */
    get queued(): number {
        return this.queue.len;
    }

    /**
     * **The tempo gesture**, from the beat the clock is on.
     *
     * With no `over` it is a **step**: the tempo changes from here, pinning the
     * current instant, so the beat the clock is on keeps mapping to the second
     * it already mapped to and nothing already scheduled jumps.
     *
     * With `over` it is a **shape written over a stretch** — an accelerando or
     * a ritardando reaching `tempo` and holding it. And `tempo` may be an
     * envelope (anything with `levels`, `times` and `curves`), in which case
     * the whole shape is written in one call and `over` is not needed: its own
     * times are the extents.
     *
     * `unit` says what `over` (or an envelope's times) measures — `BEATS` or
     * `SECONDS`. In seconds the width in beats is solved exactly, so an
     * accelerando can be asked for by how long it lasts rather than by how many
     * beats it covers. `curve` is the shape: `LINEAR`, `EXPONENTIAL` or a
     * numeric curvature (0 is linear, positive starts slow, negative starts
     * fast); an envelope carries its own and this is ignored.
     *
     * A tempo envelope is of **finite duration** — after its last segment the
     * tempo it reached holds. A sustain or a loop point is refused rather than
     * ignored: those are a gate's ideas, and a piece's tempo has no gate.
     *
     * A change is **recorded** rather than overwriting what came before, so the
     * beats before it stay convertible afterwards.
     */
    setTempo(
        tempo: number | TempoEnvelope,
        { over, unit = BEATS, curve = LINEAR }: SetTempoOptions = {},
    ): this {
        const at = this.beats();
        if (typeof tempo === "number") {
            if (over === undefined) {
                // A breakpoint, not an overwrite. The map keeps the second `at`
                // already fell on, which is what makes the change free of a
                // discontinuity — and, unlike the single anchor this used to
                // be, it also keeps every earlier tempo, so the beats before
                // the change stay convertible.
                this.tempoMapHeld.push(at, tempo);
            } else {
                // The tempo the shape departs from is the one sounding at `at`,
                // not the affine cache's, which is the last segment's and would
                // be a shape's destination.
                tempoEnv(this.tempoMapHeld, at,
                         [this.tempoMapHeld.tempoAt(at), tempo], [over], curve, unit);
            }
        } else {
            if (tempo.releaseNode != null || tempo.loopNode != null) {
                throw new Error(
                    "a tempo envelope is of finite duration: it has no gate, so a "
                    + "releaseNode or a loopNode has nothing to mean",
                );
            }
            tempoEnv(this.tempoMapHeld, at, tempo.levels, tempo.times,
                     tempo.curves ?? curve, unit);
        }
        this.syncMap(true);
        return this;
    }

    /**
     * The 0-based bar index the clock's current beat (or an explicit `beats`)
     * falls in, on a grid of `quant` beats per bar.
     */
    bar(quant: number, beats?: number): number {
        return bar(beats ?? this.beats(), quant);
    }

    /** The beat within its bar, in `[0, quant)`. */
    beatInBar(quant: number, beats?: number): number {
        return beatInBar(beats ?? this.beats(), quant);
    }

    // ---- the master clock (drift-free timing) ----

    /**
     * Lock this clock to a master `server`'s sample clock, so events schedule
     * on the server's own sample axis (drift-free) instead of a wall-clock
     * timetag. Resolves with `this`.
     *
     * Opt-in: a plain clock paces against wall-clock time, which works
     * standalone and across a socket. This switches it to the server's counter
     * — over the in-page engine that counter is read directly, over a socket it
     * is tracked through `/clock_query` anchors. The switch is **graceful**: a
     * score server, or a master that does not answer, leaves the clock on
     * wall-clock time, so a page with no reachable server keeps working.
     *
     * **Idempotent**: on a clock already on a sample timebase it is a no-op,
     * which is what makes it safe to call after a `Session.embed()`/`live()`
     * that anchored by default. Release it with {@link TempoClock.unlock}.
     *
     * The reference client's `TempoClock.lock_to`, and the verb
     * `Session.lockToServer` is the session-wide spelling of.
     */
    async lockTo(
        server: Server,
        { warmup = true, timeout }: { warmup?: boolean; timeout?: number } = {},
    ): Promise<this> {
        if (this.timebase instanceof SampleClockTimebase) return this;
        this.timebase = await server.sampleTimebase({
            ...(timeout === undefined ? {} : { timeout }),
            warmup,
        });
        return this;
    }

    /**
     * Undo a {@link TempoClock.lockTo}: back to wall-clock time. Returns
     * `this`.
     */
    unlock(): this {
        this.timebase = new MonotonicTimebase();
        return this;
    }

    // ---- the shared transport (phase alignment) ----

    /**
     * Adopts a master `server`'s shared `/transport_set` beat grid as this
     * clock's tempo and grid, so a `quant`-ed routine starts on the **same**
     * beat as every other client joined to it — a page opened halfway through
     * a bar still lands on the next bar line the conductor and every other
     * client land on.
     *
     * Reads the transport once; a server with none defined leaves the clock on
     * its own grid (no-op). A clock on a `SampleClockTimebase` (`lockToServer`)
     * aligns **sample-exactly**, since the grid is defined on the very counter
     * it paces against; a wall-clock one aligns to beats through the server's
     * `/clock_query` anchor (drift-bounded, and re-joining re-anchors it).
     *
     * The rule the clock never bends holds here too: it does not *talk* to a
     * server — this reads the grid once, off a handle you pass, and keeps
     * three numbers. Nothing about a joined clock is asynchronous afterwards.
     */
    async joinTransport(server: Server, timeout?: number): Promise<this> {
        const grid = await server.transport(timeout);
        if (grid === null) return this;
        // The shared grid is affine by construction (`/transport_set` is an
        // origin and one tempo), so joining one *declares the piece affine*: the
        // map is replaced by that single segment rather than gaining a
        // breakpoint. A piece with a tempo curve phase-aligns by sample instead.
        this.tempoMapHeld = new TempoMap(grid.tempo);
        this.syncMap(true);
        if (this.timebase instanceof SampleClockTimebase) {
            this.transport = { kind: "sample", origin: grid.originSample, tempo: grid.tempo };
            return this;
        }
        // The grid's origin is a sample; a wall-clock clock cannot read that
        // axis, so the `/clock_query` anchor maps it to Unix time — the same
        // core conversion the server uses, so both grids are one grid.
        const anchor = await server.request("/clock_query", [], {
            expect: ["/clock_query.reply"],
            timeout,
        });
        const sample0 = Number(anchor.args[0]);
        const rate = Number(anchor.args[1]);
        const unix0 = Number(anchor.args[2]);
        this.transport = {
            kind: "wall",
            origin: unix0 + samplesToSecs(grid.originSample - sample0, rate),
            tempo: grid.tempo,
        };
        return this;
    }

    /**
     * Stops following a joined transport: `quant` snaps against this clock's
     * own elapsed beats again. The tempo the grid set is kept — leaving the
     * grid is not a tempo change.
     */
    leaveTransport(): this {
        this.transport = null;
        return this;
    }

    /** Whether this clock is following a shared transport grid. */
    get joined(): boolean {
        return this.transport !== null;
    }

    /**
     * The current position, in beats, on the grid `quant` snaps to: the shared
     * transport's when joined, else this clock's own elapsed beats.
     *
     * The two are deliberately different axes. The clock's beat starts when
     * *it* starts; the shared one is the conductor's, running whether this
     * page is playing or not — which is exactly what makes two pages started
     * seconds apart agree on where the next bar falls.
     */
    gridBeat(): number {
        const grid = this.transport;
        if (grid === null) return this.beats();
        if (grid.kind === "sample") {
            // A timebase swapped out from under a joined grid (a `lockToServer`
            // after the join) leaves the sample origin meaningless; the clock's
            // own beats are the honest answer until it re-joins.
            const timebase = this.timebase;
            if (!(timebase instanceof SampleClockTimebase)) return this.beats();
            return ((timebase.currentSample() - grid.origin) * grid.tempo) / timebase.sampleRate;
        }
        return (Date.now() / 1000 - grid.origin) * grid.tempo;
    }

    // ---- scheduling ----

    /**
     * The id this item is queued under, minted on first use. The queue holds
     * flat numbers; the map back to the item lives here, which is what keeps
     * the coroutine driver in the language.
     */
    private idOf(item: Schedulable): number {
        const key = item as unknown as object;
        let id = this.ids.get(key);
        if (id === undefined) {
            id = this.nextId++;
            this.ids.set(key, id);
        }
        return id;
    }

    private push(beat: number, item: Schedulable): void {
        const id = this.idOf(item);
        const entry = this.items.get(id);
        if (entry === undefined) this.items.set(id, { item, queued: 1 });
        else entry.queued += 1;
        this.queue.push(beat, id);
    }

    /**
     * The item a popped id stands for, dropping the reference once no queued
     * entry needs it.
     */
    private take(id: number): Schedulable | null {
        const entry = this.items.get(id);
        if (entry === undefined) return null;
        entry.queued -= 1;
        if (entry.queued <= 0) this.items.delete(id);
        return entry.item;
    }

    /**
     * Schedules `item` to run `delayBeats` from the current beat. Safe from
     * inside a running routine.
     */
    sched(delayBeats: number, item: Schedulable): this {
        this.push(this.beats() + delayBeats, item);
        this.wakeSoon();
        return this;
    }

    /** Schedules `item` at an absolute `beat`. */
    schedAbs(beat: number, item: Schedulable): this {
        this.push(beat, item);
        this.wakeSoon();
        return this;
    }

    /**
     * Pumps **after** the call that scheduled has returned, never inside it.
     *
     * An item due now used to be resumed on the scheduling call's own stack,
     * so `play()` on a running clock ran the routine's first pass *before it
     * returned* — where the Python client pushes and lets its own thread pick
     * the routine up, so `play()` returns first. That is what
     * `Routine.run(function* () { … })` needs: the name is bound by the
     * assignment this call is on the right-hand side of, and a first pass that
     * runs before it exists cannot read it.
     *
     * A microtask is the whole of the fix, and it is deliberately *not* the
     * ticker: an item with no wait left is due, and making it wait for a tick
     * would be saying it is not. Nothing about the timing moves — a beat is
     * computed from the timebase, not counted in wakes — so the item still
     * runs at the beat it was scheduled for. The clock's own pacing already
     * lives off the page thread (`workerTicker`); this is about which *stack*
     * resumes a generator, which is always the page's, since a routine closes
     * over page objects a worker cannot be handed.
     *
     * Inside a wake there is nothing to defer: `pump`'s loop re-reads the queue
     * before it sleeps, so a routine scheduling from its own body is already
     * picked up by the turn it is running in.
     */
    private wakeSoon(): void {
        if (this.pumping) return;
        queueMicrotask(() => this.pump());
    }

    /**
     * Schedules a routine (or callable), snapping its start to a beat grid.
     *
     * `quant` starts it on the next beat that is a multiple of it (`4` = the
     * next bar in 4/4); 0 or undefined starts it now. The grid is the clock's
     * own elapsed beats, or a shared one once the clock has joined a transport
     * (`joinTransport`) — which is what makes several clients start together.
     */
    play<T extends Schedulable>(item: T, quant?: number): T {
        this.sched(quant ? quantDelay(this.gridBeat(), quant) : 0, item);
        return item;
    }

    /**
     * Starts this clock, lets it run for `seconds`, and stops it.
     *
     * The reference client's `TempoClock.run`, which blocks the calling thread
     * for that long; here the wait is a promise, because a page has one thread
     * and blocking it would stop the very clock this is driving. Everything
     * else is the same: a convenience over `start`/`stop` for a script whose
     * whole life is one stretch of playing.
     */
    async run(seconds: number): Promise<this> {
        this.start();
        await new Promise((resolve) => setTimeout(resolve, seconds * 1000));
        return this.stop();
    }

    /** Drops every item currently queued. */
    clear(): this {
        this.queue.clear();
        this.items.clear();
        this.ticker.cancel();
        return this;
    }

    /**
     * Removes one scheduled `item` (by identity), leaving the rest in order —
     * how a playhead stops or seeks without clearing everything else.
     */
    unsched(item: Schedulable): this {
        const id = this.ids.get(item as unknown as object);
        if (id === undefined) return this;
        const removed = this.queue.remove(id);
        const entry = this.items.get(id);
        if (entry !== undefined) {
            entry.queued -= removed;
            if (entry.queued <= 0) this.items.delete(id);
        }
        this.pump();
        return this;
    }

    // ---- driving ----

    /**
     * Begins the real-time driver. Idempotent.
     *
     * A restart resumes where `stop` left the beat, so what is still queued
     * keeps its place in the music. (The Python client restarts the beat axis
     * at zero instead, which leaves queued items an unplayable stretch in the
     * future; a browser transport is a pause button, so this one holds the
     * position.)
     */
    start(): this {
        if (this.running) return this;
        this.running = true;
        this.mode = "rt";
        // Both origins are placed so `beats()` continues from where the clock
        // was stopped. A beat's position in seconds is measured from the beat
        // axis' zero, so resuming at beat *b* puts the origins `beats2secs(b)`
        // seconds in the past — the wall one too, or the timetag of the first
        // event after a restart would be that far off.
        const held = this.beats2secs(this.logicalBeat);
        this.monoStart = this.timebase.now() - held; // the pacing origin
        this.unixStart = Date.now() / 1000 - held; // the wall origin, for timetags
        this.pump();
        return this;
    }

    /**
     * The **offline drive**: resumes everything queued in beat order, with no
     * sleeping and no wall clock at all, and returns when the queue is empty
     * (or the next item is past `untilBeat`).
     *
     * This is the same driver the real-time one runs — the same `wake`, the
     * same yield-accumulated logical beat — with the waiting taken out. What
     * the routines emit lands wherever their `Server` puts it; against a
     * score carrier that is a score, which is what makes a piece written for
     * a live take renderable without changing a line of it.
     *
     * `untilBeat` is required for an endless source: an infinite pattern
     * never drains on its own, and nothing here is watching a clock to stop
     * it.
     *
     * `maxSteps` bounds the number of **resumes**, throwing once it is passed.
     * It defaults to no bound, which is the right default: a long offline
     * render of a real score is meant to run for a long time. It is for the
     * caller who knows its source might never end — a bounce of an endless
     * pattern (`Timeline.fromPattern`) — because a routine cannot report that
     * itself: a routine that throws loses its own place and nothing else, so a
     * guard inside one is swallowed by design.
     *
     * Synchronous on purpose, where everything else in this client that waits
     * is a promise: nothing is being waited *for*. A render occupies the page
     * for as long as it takes and then returns, the way a long loop does.
     */
    render(untilBeat?: number, { maxSteps }: { maxSteps?: number } = {}): this {
        this.mode = "nrt";
        this.logicalBeat = 0;
        let steps = 0;
        try {
            for (;;) {
                const beat = this.queue.peekTime();
                if (beat === undefined) break;
                if (untilBeat !== undefined && beat > untilBeat) break;
                if (maxSteps !== undefined && ++steps > maxSteps) {
                    throw new Error(
                        `render: still going after ${maxSteps} resumes — ` +
                            "the source does not end on its own",
                    );
                }
                const due = this.queue.popDue(beat);
                if (due === undefined) break;
                const item = this.take(due[1]!);
                this.logicalBeat = due[0]!;
                if (item !== null) this.wake(item, due[0]!);
            }
        } finally {
            this.mode = "stopped";
        }
        return this;
    }

    /**
     * Stops the driver, holding the beat it reached. What is queued stays
     * queued: `stop`/`start` is a transport, not a reset — `clear` is the
     * reset.
     */
    stop(): this {
        // Freeze the beat first: from here `beats()` reports it, because the
        // clock is no longer running. The two origins are deliberately kept —
        // they stay the correct origins of the beat axis a later `start`
        // resumes, and a Server emitting one last event reads them.
        this.logicalBeat = this.beats();
        this.running = false;
        this.mode = "stopped";
        this.ticker.cancel();
        return this;
    }

    /** Stops the driver and releases the ticker (its worker slot). */
    close(): this {
        this.stop();
        this.ticker.close();
        return this;
    }

    /**
     * One turn of the driver: resume everything due, then arm the wake for
     * whatever comes next. Re-entrant calls (a routine scheduling from inside
     * its own wake) are absorbed — the loop re-reads the queue anyway.
     */
    private pump(): void {
        if (!this.running || this.pumping) return;
        this.pumping = true;
        try {
            for (;;) {
                const beat = this.queue.peekTime();
                if (beat === undefined) {
                    this.ticker.cancel();
                    return;
                }
                const wait = this.beats2secs(beat) - (this.timebase.now() - this.monoStart!);
                if (wait > 0) {
                    this.ticker.schedule(wait, () => this.pump());
                    return;
                }
                const due = this.queue.popDue(beat);
                if (due === undefined) return;
                const item = this.take(due[1]!);
                if (item !== null) this.wake(item, due[0]!);
            }
        } finally {
            this.pumping = false;
        }
    }

    /** Resumes `item` at `beat`, rescheduling it by whatever delay it asks for. */
    private wake(item: Schedulable, beat: number): void {
        const isStream = item instanceof Stream;
        const previous = setCurrentRoutine(isStream ? item : null);
        this.logicalBeat = beat;
        if (isStream) {
            item.clock = this; // the running stream carries its clock (sc3)
            item.logicalBeat = beat; // ...and its exact logical time
        }
        let delta: unknown;
        try {
            delta = isStream ? item.next(this) : item();
        } catch (error) {
            if (error instanceof StopStream) return;
            // A raising routine loses its place in the schedule -- and only its
            // own place. The driver must survive it: it wakes every other
            // routine, and an error thrown from here would leave `pump` without
            // arming the next wake, a clock that reports itself running and
            // never fires again. Report it and drop this one.
            if (item instanceof Routine) item.state = "done";
            console.error("routine dropped after throwing:", error);
            return;
        } finally {
            setCurrentRoutine(previous);
        }
        if (typeof delta === "number" && Number.isFinite(delta)) {
            this.push(beat + delta, item);
        }
    }
}
