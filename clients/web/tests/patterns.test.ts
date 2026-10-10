// Patterns as functions: the form (`pattern`, the incoming value, iteration),
// the two kinds, and the operators a pattern and a stream compose with
// (mirrors `clients/python/tests/test_patterns.py`).

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { seed } from "../src/base/rand.ts";
import { Routine, StopStream } from "../src/base/stream.ts";
import {
    EventPattern,
    Pattern,
    embed,
    pattern,
    pbind,
    pbinop,
    pn,
    pnarop,
    pseq,
    pseries,
    punop,
    pwhite,
} from "../src/seq/index.ts";

await loadCore();

const take = <T>(iterable: Iterable<T>, n: number): T[] => {
    const out: T[] = [];
    for (const x of iterable) {
        if (out.length === n) break;
        out.push(x);
    }
    return out;
};

// ---- the form ----

test("a pattern is a function returning a reusable description", () => {
    const p = pseq([1, 2, 3]);
    assert.ok(p instanceof Pattern && !(p instanceof EventPattern));
    assert.deepEqual([...p], [1, 2, 3]);
    const again: number[] = [];
    for (const x of p) again.push(x);
    assert.deepEqual(again, [1, 2, 3]);
    assert.equal(String(p), "pseq([1, 2, 3])");
});

test("a pattern keeps a copy of its list", () => {
    const items = [1, 2];
    const p = pseq(items);
    items.push(3);
    assert.deepEqual([...p], [1, 2]);
});

test("an endless pattern is bounded by the loop", () => {
    assert.deepEqual(take(pseries(0, 1), 4), [0, 1, 2, 3]);
});

test("a generator function becomes a pattern", () => {
    const pcount = pattern(function* pcount(inval: unknown, n: number) {
        for (let i = 0; i < n; i++) inval = yield i;
        return inval;
    });
    assert.equal(pcount.name, "pcount");
    assert.deepEqual([...pcount(3)], [0, 1, 2]);
    assert.deepEqual([...pseq<number>([pcount(2), 9])], [0, 1, 9]);
});

test("the incoming value reaches the generator and its embeds", () => {
    const pecho = pattern(function* pecho(inval: unknown, n: number) {
        for (let i = 0; i < n; i++) inval = yield inval;
        return inval;
    });
    const pwrap = pattern(function* pwrap(inval: unknown, source: Pattern) {
        return yield* embed(source, inval);
    });
    const s = pwrap(pecho(3)).stream();
    assert.deepEqual([s.next("a"), s.next("b"), s.next("c")], ["a", "b", "c"]);
    assert.throws(() => s.next("d"), StopStream);
});

test("a pbind hands its keys the event being built", () => {
    const pdoubleDegree = pattern(function* pdoubleDegree(inval: unknown) {
        for (;;) inval = yield (inval as { degree: number }).degree * 2;
    });
    const events = [...pbind({ degree: pseq([1, 2]), octave: pdoubleDegree() })];
    assert.deepEqual(events.map((e) => e.get("octave")), [2, 4]);
});

test("the kind is decided when the pattern is built", () => {
    const phrase = pbind({ degree: pseq([0, 1]) });
    assert.ok(phrase instanceof EventPattern);
    assert.ok(pseq([phrase, phrase]) instanceof EventPattern);
    assert.ok(pn(phrase, 2) instanceof EventPattern);
    assert.ok(!(pseq<unknown>([phrase, 1]) instanceof EventPattern));
    assert.ok(!("play" in pseq([1, 2])));
});

// ---- the operators ----

test("an operator composes a pattern of values", () => {
    assert.deepEqual([...pseq([74, 60, 46]).add(2)], [76, 62, 48]);
    assert.deepEqual([...pseq([1, 2, 3]).mul(pseq([10, 20]))], [10, 40]);
    assert.deepEqual([...pseq([1, -2]).neg()], [-1, 2]);
    assert.deepEqual([...pseq([69, 81]).midicps()], [440, 880]);
    assert.deepEqual([...pseq([1, 5, 9]).min(4)], [1, 4, 4]);
    assert.deepEqual([...pseq([13, 14]).mod(12)], [1, 2]);
});

test("a composed pattern is a pattern", () => {
    const p = pseq([1, 2]).add(60);
    assert.ok(p instanceof Pattern);
    assert.equal(String(p), 'pbinop("add", pseq([1, 2]), 60)');
    assert.deepEqual([...pn(p, 2)], [61, 62, 61, 62]);
});

test("the operator patterns by name", () => {
    assert.deepEqual([...punop("neg", pseq([1]))], [-1]);
    assert.deepEqual([...pbinop("max", pseq([1, 7]), 3)], [3, 7]);
    assert.deepEqual([...pnarop("linlin", pseq([0, 0.5, 1]), 0, 1, 0, 10)], [0, 5, 10]);
    assert.deepEqual([...pnarop("clip", pseq([-1, 2]), 0, 1)], [0, 1]);
});

test("an event pattern refuses an operator", () => {
    assert.throws(() => pbind({ degree: 0 }).add(2), /event pattern/);
    assert.throws(() => pseq([1]).add(pbind({ degree: 0 })), /event pattern/);
});

test("random values compose under the seed", () => {
    seed(5);
    const a = [...pwhite(0, 1, 4).mul(100).floor()];
    seed(5);
    assert.deepEqual([...pwhite(0, 1, 4).mul(100).floor()], a);
    assert.ok(a.every((x) => x >= 0 && x < 100));
});

test("a stream composes by operators", () => {
    const s = new Routine(function* () {
        for (let i = 0; i < 3; i++) yield i;
    }).mul(10).add(1);
    assert.deepEqual([s.next(), s.next(), s.next()], [1, 11, 21]);
    assert.throws(() => s.next(), StopStream);
    assert.deepEqual([...pseq([1, 2]).stream().add(1)], [2, 3]);
});
