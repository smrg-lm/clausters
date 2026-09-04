// `AppClock`: the application's seconds over the page's own loop.
//
// The twin of the reference client's `tests/test_event_loop.py` half that is
// about the *clock* rather than about the loop: there the clock sits on an
// `EventLoop` the client started, here on the loop the page already is. The
// class, the calls and what they mean are the same, so this checks the meaning
// -- a one-shot runs once, a number reschedules, a routine is driven by what it
// yields, `unsched` takes an item off, and `defer` lands after the current task
// rather than inside it.

import assert from "node:assert/strict";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { AppClock } from "../src/base/appclock.ts";
import { loadCore } from "../src/base/core.ts";
import { Routine } from "../src/base/stream.ts";

// A `Routine` seeds its own RNG from the core, so the wasm has to be in.
await loadCore();

test("AppClock: a one-shot runs once", async () => {
    const clock = new AppClock();
    let ran = 0;
    clock.sched(0.01, () => {
        ran += 1;
    });
    await sleep(60);
    assert.equal(ran, 1, "nothing returned, so nothing was rescheduled");
});

test("AppClock: a number reschedules, which is how a periodic task is written", async () => {
    const clock = new AppClock();
    let ran = 0;
    const tick = (): number | undefined => {
        ran += 1;
        return ran < 3 ? 0.01 : undefined;
    };
    clock.sched(0.01, tick);
    await sleep(120);
    assert.equal(ran, 3, "three passes, then it asked for no more");
});

test("AppClock: a routine is driven by what it yields", async () => {
    const clock = new AppClock();
    const seen: number[] = [];
    const routine = new Routine(function* (): Generator<number> {
        seen.push(1);
        yield 0.01;
        seen.push(2);
        yield 0.01;
        seen.push(3);
    });
    clock.play(routine);
    await sleep(120);
    assert.deepEqual(seen, [1, 2, 3]);
});

test("AppClock: unsched takes an item off and leaves the rest in order", async () => {
    const clock = new AppClock();
    const ran: string[] = [];
    // Neither returns its `push` -- a number is a *reschedule*, which is the
    // contract and is easy to hand over by accident.
    const dropped = (): void => {
        ran.push("dropped");
    };
    clock.sched(0.02, dropped);
    clock.sched(0.02, (): void => {
        ran.push("kept");
    });
    assert.equal(clock.unsched(dropped), true);
    assert.equal(clock.unsched(dropped), false, "nothing left to cancel");
    await sleep(80);
    assert.deepEqual(ran, ["kept"]);
});

test("AppClock: an item that throws loses its turn and nothing else", async () => {
    const clock = new AppClock();
    const errors: unknown[] = [];
    const original = console.error;
    console.error = (...args: unknown[]) => errors.push(args);
    try {
        clock.sched(0.01, () => {
            throw new Error("no");
        });
        let ran = false;
        clock.sched(0.02, () => {
            ran = true;
        });
        await sleep(80);
        assert.equal(ran, true, "the clock survived it");
        assert.equal(errors.length, 1, "and said so");
    } finally {
        console.error = original;
    }
});

test("AppClock: defer lands after the current task, not inside it", async () => {
    const clock = new AppClock();
    const order: string[] = [];
    clock.defer((): void => {
        order.push("deferred");
    });
    order.push("caller");
    // A microtask would already have run by here; a task has not.
    await Promise.resolve();
    assert.deepEqual(order, ["caller"], "handing work over is not calling it");
    await sleep(20);
    assert.deepEqual(order, ["caller", "deferred"]);
});

test("AppClock: elapsed counts from when the clock was made", async () => {
    const clock = new AppClock();
    assert.ok(clock.elapsed() < 0.05);
    await sleep(60);
    assert.ok(clock.elapsed() >= 0.04, "and it moves with the wall");
});
