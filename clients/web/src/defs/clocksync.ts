// Track the server's sample clock: over a socket, or read directly in this
// process (mirrors `clausters/defs/clocksync.py`).
//
// Two ways to feed a `SampleTimebase`, one per carrier:
//
// - `WsSampleClock` — over a socket, where the client cannot read the sample
//   counter directly, it queries the server's `/clock_query` and models the
//   counter (below). The Python client's `UdpSampleClock`, over the carrier a
//   browser has.
// - `EmbedSampleClock` — for an in-page engine, whose connection exposes the
//   counter itself: no round trips, no model — every read *is* the counter. It
//   mirrors the tracker's surface so `Server.sampleTimebase` treats both alike.
//   The Python client's class of the same name, and named for the same
//   property: what makes the counter readable is that the server is **embedded
//   in this process** (the wasm engine is the `synth,embed` build, reached
//   through the embed door), not that there is one of it per page — a page
//   opens as many engines as it wants to.
//
// The socket tracker models
//
//     sample(t_local) = a + b * t_local
//
// from `(local time, counter)` anchor pairs, a least-squares line over a
// sliding window. The fit itself lives in the shared core
// (`SampleClockModel` over `clausters_core::clocksync`), reached here through
// the wasm door the way the Python client reaches it through ctypes, so every
// client predicts the same sample from the same anchors. The `TempoClock` then
// paces against this and the `Server` schedules every event by absolute sample
// with `/sched_at` — drift-free.
//
// Query latency does not accumulate: an anchor is paired with the *midpoint*
// of its round trip, whose half-width is a bounded uncertainty that only
// shifts the whole grid by a constant. Relative timing stays sample-exact by
// construction.
//
// **One difference from the Python client, and it is the carrier's.** There a
// tracker opens its **own** UDP socket, so `/clock_query` round trips never
// contend with the Server's command socket. A browser client has one
// WebSocket to a given server and cannot open a second cheaply, so this tracker rides the
// `Server`'s connection through its ordinary request path. Nothing about the
// model changes — the anchor is still the midpoint of a measured round trip —
// only that the round trip shares a queue with everything else the client
// sends.

import { ReplyTimeout } from "../errors.ts";
import { SampleClockModel } from "../core/clausters_core_web.js";
import { SampleTimebase } from "../base/timebase.ts";
import type { SampleClock } from "../base/connection.ts";
import type { Server } from "./server/index.ts";

/** One `/clock_query` observation, and the local time it is centred on. */
export interface Anchor {
    local: number;
    sample: number;
    rate: number;
    /** The anchor's uncertainty: half the round trip, in seconds. */
    uncertainty: number;
}

/**
 * What `Server.sampleTimebase` builds and keeps: the surface both carriers
 * answer to, so the resolver never branches past the constructor.
 */
export interface ServerSampleClock {
    /** One observation; resolves with the anchor's uncertainty in seconds. */
    anchor(): Promise<number>;
    /** A few anchors to seed the model; resolves with the worst uncertainty. */
    warmup(anchors?: number, gap?: number): Promise<number>;
    /** Re-anchor in the background forever (keeps the slope fresh). */
    track(interval?: number): this;
    untrack(): void;
    /** The predicted current value of the server's sample counter. */
    now(): number;
    readonly rate: number;
    /** The measured drift in parts per million, or `null` with no model. */
    readonly driftPpm: number | null;
    /** A `SampleTimebase` reading this clock. */
    timebase(): SampleTimebase;
    close(): void;
}

/**
 * Tracks a server's sample clock over a socket and yields a timebase.
 *
 * Build one through `Server.sampleTimebase()`, which resolves the carrier;
 * the first `anchor` has to succeed for the model to exist at all, which is
 * why the constructor takes one already made.
 */
export class WsSampleClock implements ServerSampleClock {
    private readonly model: SampleClockModel;
    private readonly server: Server;
    private readonly timeout: number | undefined;
    private readonly nominalRate: number;
    private tracker: ReturnType<typeof setInterval> | null = null;

    constructor(server: Server, first: Anchor, { timeout, window = 64 }: {
        timeout?: number;
        window?: number;
    } = {}) {
        this.server = server;
        this.timeout = timeout;
        this.nominalRate = first.rate;
        this.model = new SampleClockModel(first.rate, window);
        this.model.addAnchor(first.local, first.sample, first.rate);
    }

    /**
     * One `/clock_query` round trip, timestamped at the midpoint of the
     * exchange — the best estimate of when the server read its own counter.
     * Static because the first one has to happen before there is a model to
     * put it in.
     */
    static async query(server: Server, timeout?: number): Promise<Anchor> {
        const sent = performance.now() / 1000;
        const msg = await server.request("/clock_query", [], {
            expect: ["/clock_query.reply"],
            timeout,
        });
        const received = performance.now() / 1000;
        return {
            local: (sent + received) / 2,
            sample: Number(msg.args[0]),
            rate: Number(msg.args[1]),
            uncertainty: (received - sent) / 2,
        };
    }

    async anchor(): Promise<number> {
        const next = await WsSampleClock.query(this.server, this.timeout);
        this.model.addAnchor(next.local, next.sample, next.rate);
        return next.uncertainty;
    }

    /**
     * Firms the model up before anything schedules against it: one anchor
     * gives an offset, several give a rate — but only if they are spread over
     * enough time. Back-to-back round trips all land inside a couple of
     * milliseconds, and a regression over that span is noise.
     */
    async warmup(anchors = 5, gap = 0.05): Promise<number> {
        let worst = 0.0;
        for (let i = 1; i < anchors; i++) {
            if (gap > 0) await new Promise((done) => setTimeout(done, gap * 1000));
            worst = Math.max(worst, await this.anchor());
        }
        return worst;
    }

    track(interval = 0.5): this {
        if (this.tracker !== null || interval <= 0) return this;
        this.tracker = setInterval(() => {
            this.anchor().catch(() => {
                /* a missed anchor is not fatal: the model holds. */
            });
        }, interval * 1000);
        return this;
    }

    untrack(): void {
        if (this.tracker !== null) clearInterval(this.tracker);
        this.tracker = null;
    }

    now(): number {
        return this.model.sampleAt(performance.now() / 1000);
    }

    get rate(): number {
        return this.nominalRate;
    }

    get driftPpm(): number | null {
        return this.model.driftPpm;
    }

    timebase(): SampleTimebase {
        return new SampleTimebase(() => this.now(), this.rate);
    }

    close(): void {
        this.untrack();
    }
}

/**
 * The in-process counterpart of `WsSampleClock`: reads the engine's sample
 * counter straight off the connection, which shares an `AudioContext` with
 * it. One per engine, not one per page — a connection carries a client over
 * one engine, and a page may hold several.
 *
 * There is nothing to track — the counter is read synchronously and exactly —
 * so `anchor`/`warmup`/`track` are trivial no-ops kept only for surface parity
 * with the socket tracker, and they never wait or time out. `close` releases
 * nothing: the clock belongs to the connection that opened it.
 */
export class EmbedSampleClock implements ServerSampleClock {
    private readonly clock: SampleClock;

    constructor(clock: SampleClock) {
        this.clock = clock;
    }

    async anchor(): Promise<number> {
        // A direct read has no round trip: probe the counter once and report
        // zero uncertainty.
        this.clock.sample();
        return 0.0;
    }

    async warmup(): Promise<number> {
        return 0.0;
    }

    track(): this {
        return this;
    }

    untrack(): void {}

    now(): number {
        return this.clock.sample();
    }

    get rate(): number {
        return this.clock.sampleRate;
    }

    /** No model, so nothing to measure: the two clocks are the same one. */
    get driftPpm(): number | null {
        return null;
    }

    timebase(): SampleTimebase {
        return new SampleTimebase(() => this.clock.sample(), this.clock.sampleRate);
    }

    close(): void {}
}

/**
 * The sample clock of whichever carrier `server` is on, seeded and tracking,
 * or `null` when the server does not answer `/clock_query` — which leaves the
 * caller on wall-clock time rather than failing.
 *
 * An embedded engine needs no warmup and no tracking, so both arguments are
 * ignored there; over a socket they size the regression (see `warmup`).
 */
export async function sampleClockFor(
    server: Server,
    { timeout, anchors = 5, gap = 0.05, trackEvery = 0.5 }: {
        timeout?: number;
        anchors?: number;
        gap?: number;
        trackEvery?: number;
    } = {},
): Promise<ServerSampleClock | null> {
    if (server.connection.sampleClock) {
        return new EmbedSampleClock(await server.connection.sampleClock());
    }
    let first: Anchor;
    try {
        first = await WsSampleClock.query(server, timeout);
    } catch (error) {
        if (!(error instanceof ReplyTimeout)) throw error;
        return null;
    }
    const clock = new WsSampleClock(server, first, { timeout });
    await clock.warmup(anchors, gap);
    return clock.track(trackEvery);
}
