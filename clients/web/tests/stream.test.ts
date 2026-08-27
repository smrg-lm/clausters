// The coroutine layer: a routine yields delays, ends by `StopStream`, and
// carries its own random stream.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { FunctionStream, Routine, StopStream } from "../src/base/stream.ts";
import { Rng, seed, spawnRng } from "../src/base/rand.ts";

await loadCore();

test("a routine yields its delays in order and then ends", () => {
    const seen: string[] = [];
    const routine = new Routine(function* () {
        seen.push("a");
        yield 0.25;
        seen.push("b");
        yield 1;
        seen.push("c");
    });

    assert.equal(routine.state, "init");
    assert.equal(routine.next(), 0.25);
    assert.equal(routine.state, "running");
    assert.equal(routine.next(), 1);
    assert.throws(() => routine.next(), StopStream);
    assert.equal(routine.state, "done");
    assert.deepEqual(seen, ["a", "b", "c"]);
    // Once done it stays done, however often the clock asks again.
    assert.throws(() => routine.next(), StopStream);
});

test("a routine receives the value fed into each resumption", () => {
    const fed: unknown[] = [];
    const routine = new Routine(function* (inval) {
        fed.push(inval);
        fed.push(yield 1);
        fed.push(yield 1);
    });
    routine.next("first");
    routine.next("second");
    // The last resumption runs the body past its final yield: the value still
    // arrives, and the routine ends in the same call.
    assert.throws(() => routine.next("third"), StopStream);
    assert.deepEqual(fed, ["first", "second", "third"]);
});

test("reset restarts the generator function", () => {
    let runs = 0;
    const routine = new Routine(function* () {
        runs += 1;
        yield 1;
    });
    routine.next();
    assert.throws(() => routine.next(), StopStream);
    routine.reset();
    assert.equal(routine.state, "init");
    assert.equal(routine.next(), 1);
    assert.equal(runs, 2);
});

test("a function stream calls its callable, and resets through its hook", () => {
    let n = 0;
    const stream = new FunctionStream(
        () => (n += 1),
        () => {
            n = 0;
        },
    );
    assert.equal(stream.next(), 1);
    assert.equal(stream.next(), 2);
    stream.reset();
    assert.equal(stream.next(), 1);
});

test("iterating a stream runs it to its end", () => {
    let left = 3;
    const stream = new FunctionStream(() => {
        if (left-- === 0) throw new StopStream();
        return left;
    });
    assert.deepEqual([...stream], [2, 1, 0]);
});

test("each routine draws from its own stream, derived at creation", () => {
    seed(42);
    const first = new Routine(function* () {
        yield 1;
    });
    const second = new Routine(function* () {
        yield 1;
    });
    // Two routines created in order under one seed get different streams...
    assert.notEqual(first.rng!.nextF64(), second.rng!.nextF64());

    // ...and the same seed replays exactly the same pair.
    seed(42);
    const again = new Routine(function* () {
        yield 1;
    });
    seed(42);
    const expected = new Rng(42).spawn();
    assert.equal(again.rng!.nextF64(), expected.nextF64());
});

test("spawning outside a routine derives from the root stream", () => {
    seed(7);
    const a = spawnRng().nextF64();
    seed(7);
    const b = spawnRng().nextF64();
    assert.equal(a, b);
});
