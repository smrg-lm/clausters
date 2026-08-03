// The driver, advanced by hand.
//
// The `Ticker` and `Timebase` seams are what the browser fills with a worker
// and the page's monotonic clock; here they are filled with manual
// implementations, so these tests exercise **the same driver** the browser
// runs, deterministically and with no timers. That is also how the property
// this layer exists for is asserted: the logical beat advances only by the
// routines' yields, so a wake-up that arrives late does not shift the music.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import type { ManualTicker } from "../src/base/clock.ts";
import { ManualTimebase, SampleTimebase } from "../src/base/timebase.ts";
import type { Server } from "../src/defs/server/index.ts";
import { Routine } from "../src/base/stream.ts";

await loadCore(
    await readFile(
        new URL("../dist/core/clausters_core_web_bg.wasm", new URL(".", import.meta.url)),
    ),
);

/**
 * A clock on the manual seams, plus a `run(seconds)` that advances time and
 * fires whatever the clock asked for along the way.
 */
function harness(tempo = 1.0) {
    const timebase = new ManualTimebase(1000);
    const ticker = manualTicker();
    const clock = new TempoClock(tempo, { timebase, ticker });
    const run = (seconds: number, { late = 0 } = {}) => {
        const target = timebase.now() + seconds;
        // Fire every wake the clock arms until the target time, optionally
        // arriving `late` seconds after it asked to be woken.
        for (;;) {
            const pending = (ticker as ManualTicker).pending;
            if (pending === null) break;
            const at = timebase.now() + pending + late;
            if (at > target) break;
            timebase.set(at);
            ticker.fire();
        }
        timebase.set(target);
    };
    return { clock, timebase, ticker, run };
}

test("a routine is resumed at the beats it yields", () => {
    const { clock, run } = harness(1.0);
    const at: number[] = [];
    const routine = new Routine(function* () {
        for (let i = 0; i < 4; i++) {
            at.push(routine.logicalBeat);
            yield 0.25;
        }
    });
    clock.start().play(routine);
    run(2);
    assert.deepEqual(at, [0, 0.25, 0.5, 0.75]);
});

test("late wake-ups do not shift the music", () => {
    const { clock, run } = harness(2.0);
    const at: number[] = [];
    const routine = new Routine(function* () {
        for (let i = 0; i < 6; i++) {
            at.push(routine.logicalBeat);
            yield 0.5;
        }
    });
    clock.start().play(routine);
    // Every wake arrives 40 ms after it was due — jitter the emission headroom
    // absorbs. The logical beats must be untouched by it.
    run(3, { late: 0.04 });
    assert.deepEqual(at, [0, 0.5, 1, 1.5, 2, 2.5]);
});

test("a one-shot callable runs once; returning a number reschedules it", () => {
    const { clock, run } = harness();
    let once = 0;
    let repeating = 0;
    clock.start();
    clock.sched(0, () => {
        once += 1;
    });
    clock.sched(0, () => {
        repeating += 1;
        return repeating < 3 ? 1 : undefined;
    });
    run(5);
    assert.equal(once, 1);
    assert.equal(repeating, 3);
});

test("quant snaps a start to the next boundary of the grid", () => {
    const { clock, timebase, run } = harness(1.0);
    clock.start();
    timebase.advance(2.3); // now at beat 2.3
    let started: number | null = null;
    const routine = new Routine(function* () {
        started = routine.logicalBeat;
        yield 1;
    });
    clock.play(routine, 4);
    run(4);
    assert.equal(started, 4);
});

test("unsched removes one routine and leaves the rest queued", () => {
    const { clock, run } = harness();
    const beats: string[] = [];
    const keep = new Routine(function* () {
        for (;;) {
            beats.push(`keep@${keep.logicalBeat}`);
            yield 1;
        }
    });
    const drop = new Routine(function* () {
        for (;;) {
            beats.push(`drop@${drop.logicalBeat}`);
            yield 1;
        }
    });
    clock.start();
    clock.play(keep);
    clock.play(drop);
    run(1.5);
    clock.unsched(drop);
    run(2);
    assert.deepEqual(beats, ["keep@0", "drop@0", "keep@1", "drop@1", "keep@2", "keep@3"]);
    assert.equal(clock.queued, 1);
});

test("clear drops everything queued", () => {
    const { clock, run } = harness();
    let woke = 0;
    clock.start().play(
        new Routine(function* () {
            for (;;) {
                woke += 1;
                yield 1;
            }
        }),
    );
    run(1.5);
    clock.clear();
    run(5);
    assert.equal(woke, 2);
    assert.equal(clock.queued, 0);
});

test("a tempo change pins the instant, so the timeline does not jump", () => {
    const { clock, timebase } = harness(2.0);
    clock.start();
    timebase.advance(4); // 4 s at 2 beats/s = beat 8
    assert.equal(clock.beats(), 8);
    const secondsOfBeat8 = clock.beats2secs(8);
    clock.setTempo(1.0);
    assert.equal(clock.beats2secs(8), secondsOfBeat8);
    assert.equal(clock.beats(), 8);
    // ...and from there the new tempo governs.
    timebase.advance(2);
    assert.equal(clock.beats(), 10);
});

test("the bar grid reads the clock's own position", () => {
    const { clock, timebase } = harness(1.0);
    clock.start();
    timebase.advance(5.5);
    assert.equal(clock.bar(4), 1);
    assert.equal(clock.beatInBar(4), 1.5);
    assert.equal(clock.bar(4, 9), 2);
});

test("a routine started from inside another runs on the same clock", () => {
    const { clock, run } = harness();
    let innerBeat: number | null = null;
    const inner = new Routine(function* () {
        innerBeat = inner.logicalBeat;
        yield 1;
    });
    const outer = new Routine(function* () {
        yield 1;
        inner.play(); // no clock argument: the running one
        yield 1;
    });
    clock.start().play(outer);
    run(4);
    assert.equal(innerBeat, 1);
});

test("stop holds the beat and start resumes from it", () => {
    const { clock, ticker, run } = harness();
    let woke = 0;
    clock.start();
    clock.play(
        new Routine(function* () {
            for (;;) {
                woke += 1;
                yield 1;
            }
        }),
    );
    run(0.5);
    assert.equal(woke, 1);

    clock.stop();
    assert.equal((ticker as ManualTicker).pending, null);
    assert.equal(clock.beats(), 0.5, "the beat it reached is held");
    run(5);
    assert.equal(woke, 1, "a stopped clock resumes nothing");

    // Restarting picks the music up where it stopped: the routine's next wake
    // is half a beat away, not a whole one.
    clock.start();
    assert.equal(clock.beats(), 0.5);
    run(0.75);
    assert.equal(woke, 2);
    run(1);
    assert.equal(woke, 3);
});

test("pause keeps a routine's place; stop rewinds it", () => {
    const { clock, run } = harness();
    const seen: number[] = [];
    const routine = new Routine(function* () {
        for (let i = 0; i < 4; i++) {
            seen.push(i);
            yield 1;
        }
    });
    clock.start().play(routine);
    run(1.5);
    assert.deepEqual(seen, [0, 1]);

    routine.pause();
    run(5);
    assert.deepEqual(seen, [0, 1], "a paused routine is resumed by nobody");
    assert.equal(routine.state, "paused");

    routine.play(clock); // resumes at the yield it was paused on
    run(2.5);
    assert.deepEqual(seen, [0, 1, 2, 3]);

    seen.length = 0;
    routine.stop();
    assert.equal(routine.state, "init", "stop rewinds: the next play starts over");
    routine.play(clock);
    run(4);
    assert.deepEqual(seen, [0, 1, 2, 3]);
});

test("a routine that throws is dropped, and the clock keeps driving the rest", () => {
    const { clock, run } = harness();
    const survivor: number[] = [];
    const bad = new Routine(function* () {
        yield 1;
        throw new Error("the routine's problem, not the clock's");
    });
    const good = new Routine(function* () {
        for (let i = 0; i < 3; i++) {
            survivor.push(i);
            yield 1;
        }
    });
    clock.start();
    clock.play(bad);
    clock.play(good);
    const errors = console.error;
    console.error = () => {};
    try {
        run(3.5);
    } finally {
        console.error = errors;
    }
    assert.deepEqual(survivor, [0, 1, 2], "the other routine ran to its end");
    assert.equal(bad.state, "done", "...and the raising one lost its place");
});

test("freeze holds the beat, and thaw does not charge the piece for the pause", () => {
    const { clock, run } = harness();
    clock.start();
    run(0.5);
    assert.equal(clock.beats(), 0.5);

    // A governed transport stopped on the server: the page holds its beat
    // rather than running away from a piece that is not moving.
    clock.freeze();
    assert.equal(clock.frozen, true);
    run(2);
    assert.equal(clock.beats(), 0.5, "the beat is held where the freeze left it");

    // Freezing twice keeps the first freeze's position.
    clock.freeze();
    assert.equal(clock.beats(), 0.5);

    clock.thaw();
    assert.equal(clock.frozen, false);
    assert.equal(clock.beats(), 0.5, "the frozen seconds are not part of the piece");
    run(0.25);
    assert.ok(Math.abs(clock.beats() - 0.75) < 1e-9);
});

// ---- the shared transport grid ----

/**
 * A fake server that answers the two reads a join makes: the grid itself, and
 * the `/clock_query` anchor a wall-clock clock maps it through. Nothing else
 * of `Server` is reached, which is the point — the clock keeps three numbers
 * and never talks to it again.
 */
function transportServer(
    grid: { originSample: number; tempo: number } | null,
    anchor: { sample: number; rate: number; unix: number } = {
        sample: 0,
        rate: 48000,
        unix: 0,
    },
): Server {
    return {
        transport: async () => grid,
        request: async () => ({
            addr: "/clock_query.reply",
            args: [anchor.sample, anchor.rate, anchor.unix],
        }),
    } as unknown as Server;
}

/**
 * One sample counter, and clocks pacing against it — two independent clients
 * on one server's clock, as far as this layer can tell. The ticker records
 * the absolute instant it was armed at, so the driver can fire several clocks
 * in the order their wakes fall due.
 */
function sampleGrid(rate = 48000) {
    let sample = 0;
    const now = () => sample / rate;
    const clocks: { clock: TempoClock; due: () => number | null; fire: () => void }[] = [];

    const makeClock = (tempo = 1.0) => {
        let at: number | null = null;
        let callback: (() => void) | null = null;
        const ticker = {
            schedule(seconds: number, cb: () => void) {
                at = now() + Math.max(seconds, 0);
                callback = cb;
            },
            cancel() {
                at = null;
                callback = null;
            },
            close() {
                this.cancel();
            },
        };
        const clock = new TempoClock(tempo, {
            timebase: new SampleTimebase(() => sample, rate),
            ticker,
        });
        clocks.push({
            clock,
            due: () => at,
            fire: () => {
                const run = callback;
                at = null;
                callback = null;
                run?.();
            },
        });
        return clock;
    };

    // Advances the counter to `seconds`, firing each armed wake at its own
    // instant and in order — the one property the ordering of two clients
    // depends on.
    const runTo = (seconds: number) => {
        for (;;) {
            const next = clocks
                .map((c) => c.due())
                .filter((d): d is number => d !== null && d <= seconds)
                .sort((a, b) => a - b)[0];
            if (next === undefined) break;
            sample = Math.round(next * rate);
            for (const c of clocks) if (c.due() === next) c.fire();
        }
        sample = Math.round(seconds * rate);
    };

    return { makeClock, runTo, at: (seconds: number) => (sample = Math.round(seconds * rate)) };
}

test("a joined grid is the conductor's, not the clock's own beats", async () => {
    const { makeClock, at } = sampleGrid();
    at(10); // the page opens ten seconds into the server's life
    const clock = makeClock(1.0);
    clock.start();

    assert.equal(clock.joined, false);
    assert.equal(clock.gridBeat(), clock.beats());

    // Beat 0 of the grid fell two seconds after the server started, at 2 b/s.
    await clock.joinTransport(transportServer({ originSample: 2 * 48000, tempo: 2.0 }));
    assert.equal(clock.joined, true);
    assert.equal(clock.tempo, 2.0, "the grid brings its tempo");
    assert.equal(clock.gridBeat(), 16, "(10s - 2s) * 2 b/s, whoever is asking");

    clock.leaveTransport();
    assert.equal(clock.joined, false);
    assert.equal(clock.gridBeat(), clock.beats(), "back on its own beats");
});

test("two clocks joined to one grid land on the same bar", async () => {
    const { makeClock, runTo, at } = sampleGrid();
    const grid = { originSample: 0, tempo: 1.0 };

    // Two independent clients, started three and a half seconds apart: they
    // agree on nothing except the grid they both read.
    at(0);
    const first = makeClock(1.0);
    await first.joinTransport(transportServer(grid));
    first.start();

    at(3.5);
    const second = makeClock(1.0);
    await second.joinTransport(transportServer(grid));
    second.start();

    const started: Record<string, { grid: number; own: number }> = {};
    const play = (clock: TempoClock, name: string) => {
        const routine = new Routine(function* () {
            started[name] = { grid: clock.gridBeat(), own: clock.beats() };
            yield 1;
        });
        clock.play(routine, 4);
    };
    play(first, "first");
    play(second, "second");

    runTo(10);
    // The next bar of the *shared* grid, for both — and they reach it from
    // different beats of their own, which is exactly what a grid is for.
    assert.equal(started.first?.grid, 4);
    assert.equal(started.second?.grid, 4);
    assert.equal(started.first?.own, 4);
    assert.equal(started.second?.own, 0.5);
});

test("a wall-clock clock maps the grid through the /clock_query anchor", async () => {
    const { clock } = harness(1.0);
    clock.start();

    // The server read sample 480000 at Unix second 1000; beat 0 of the grid is
    // ten seconds of samples earlier, so it fell at Unix 990.
    const server = transportServer(
        { originSample: 0, tempo: 2.0 },
        { sample: 480000, rate: 48000, unix: 1000 },
    );
    await clock.joinTransport(server);

    const expected = (Date.now() / 1000 - 990) * 2;
    assert.ok(
        Math.abs(clock.gridBeat() - expected) < 0.05,
        `the wall grid tracks Unix time (${clock.gridBeat()} vs ${expected})`,
    );
});

test("a server with no transport leaves the clock's own grid alone", async () => {
    const { clock, timebase } = harness(1.0);
    clock.start();
    timebase.advance(2.3);
    await clock.joinTransport(transportServer(null));
    assert.equal(clock.joined, false);
    assert.equal(clock.tempo, 1.0);
    assert.ok(Math.abs(clock.gridBeat() - 2.3) < 1e-9);
});
