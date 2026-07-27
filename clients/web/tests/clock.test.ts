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
import { ManualTimebase } from "../src/base/timebase.ts";
import { Routine } from "../src/base/stream.ts";

await loadCore(
    await readFile(
        new URL("../dist/core/clausters_core_web_bg.wasm", new URL(".", import.meta.url)),
    ),
);

/// A clock on the manual seams, plus a `run(seconds)` that advances time and
/// fires whatever the clock asked for along the way.
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
