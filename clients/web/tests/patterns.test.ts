// Patterns as functions: the form (`pattern`, the incoming value, iteration),
// the two kinds, and the operators a pattern and a stream compose with
// (mirrors `clients/python/tests/test_patterns.py`).

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import * as b from "../src/base/builtins.ts";
import { signal } from "../src/defs/signals.ts";
import { Ugen, sine } from "../src/defs/ugens/index.ts";
import { seed } from "../src/base/rand.ts";
import { Routine, StopStream } from "../src/base/stream.ts";
import {
    pfunc,
    pfuncn,
    pif,
    plazy,
    pproduct,
    prout,
    pclump,
    pcollect,
    pdelta,
    pdiff,
    pdrop,
    pdur,
    pflatten,
    pgate,
    platch,
    plen,
    pprorate,
    preject,
    pselect,
    pstutter,
    pwrap,
    INF,
    pdfsm,
    place,
    placep,
    pshuffle,
    pslide,
    pswitch,
    pswitch1,
    ptuple,
    pwalk,
    pxrand,
    pbeta,
    pbrown,
    pcauchy,
    pexprand,
    pgauss,
    pgeom,
    phprand,
    plprand,
    pmeanrand,
    ppoisson,
    pprob,
    pvalue,
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

// ---- the whole AbstractObject surface, and the builtins over a pattern ----

test("a pattern has the n-ary operators a signal has", () => {
    assert.deepEqual([...pseq([0, 0.5, 1]).linlin(0, 1, 200, 800)], [200, 500, 800]);
    assert.deepEqual([...pseq([-2, 0.5, 3]).clip(0, 1)], [0, 0.5, 1]);
    assert.deepEqual([...pseq([0, 10]).blend(20, 0.5)], [10, 15]);
    assert.deepEqual([...pseq([0, 180]).raddeg().degrad()], [0, 180]);
    assert.ok(Math.abs([...pseq([0.9]).moddif(0.1)][0]! - 0.2) < 1e-12);
    assert.deepEqual([...pseq([0.25]).bilin(0.5, 0, 1, 10, 0, 100)], [5]);
});

test("the operators sc3 had and this one did not", () => {
    assert.deepEqual([...pseq([0]).bitnot()], [-1]);
    assert.deepEqual([...pseq([7]).idiv(2)], [3]);
    assert.deepEqual([...pseq([0, 1]).not()], [1, 0]);
    assert.deepEqual([...pseq([3, 4]).eq(3)], [1, 0]);
    assert.deepEqual([...pseq([3]).ne(3)], [0]);
    assert.deepEqual([...pseq([6]).bitand(3)], [2]);
    assert.deepEqual([...pseq([1]).leftshift(3)], [8]);
    assert.deepEqual([...pseq([2.5]).rint()], [2]);
});

test("the random operators draw per value under the seed", () => {
    seed(3);
    const a = [...pseq([10, 10]).rand(), ...pseq([1]).rrand(2)];
    seed(3);
    assert.deepEqual([...pseq([10, 10]).rand(), ...pseq([1]).rrand(2)], a);
    assert.ok(a[0]! >= 0 && a[0]! < 10 && a[2]! >= 1 && a[2]! <= 2);
});

test("a builtin given a pattern answers what its method does", () => {
    assert.deepEqual([...b.midicps(pseq([69, 81]))], [...pseq([69, 81]).midicps()]);
    assert.deepEqual([...b.add(2, pseq([1, 2]))], [3, 4]);
    assert.deepEqual([...b.clip(pseq([-1, 2]), 0, 1)], [0, 1]);
    assert.deepEqual([...b.cbrt(pseq([8, 27]))], [2, 3]);
    assert.deepEqual([...b.degkey(pseq([0, 1, 2]), [0, 2, 4, 5, 7, 9, 11])], [0, 2, 4]);
    assert.ok(b.midicps(pseq([60])) instanceof Pattern);
    assert.throws(() => b.clip(0.5, pseq([0]) as never, 1), /first argument/);
});

test("a list function refuses a pattern by name", () => {
    for (const fn of [b.normalize, b.integrate, b.normalizesum]) {
        assert.throws(() => (fn as (x: unknown) => unknown)(pseq([1, 2, 3])), /whole list/);
    }
});

test("a graph composes the value functions it can and refuses the rest", () => {
    assert.deepEqual(JSON.stringify(sine(1).degrad()), JSON.stringify(sine(1).mul(Math.PI / 180)));
    assert.ok(sine(1).blend(sine(2), 0.25) instanceof Ugen);
    assert.ok(b.midicps(sine(1)) instanceof Ugen);
    for (const bad of [() => sine(1).moddif(0), () => b.cbrt(sine(1))]) {
        assert.throws(bad, /no UGen computes/);
    }
    assert.throws(() => signal(1).linlin(0, 1, 0, 2), /has no 'linlin'/);
});

test("a list function reshapes any items and refuses a pattern", () => {
    assert.deepEqual(b.mirror(["a", "b", "c"]), ["a", "b", "c", "b", "a"]);
    assert.deepEqual(b.pyramidg([1, 2, 3], 5), [[1], [1, 2], [1, 2, 3], [1, 2], [1]]);
    assert.deepEqual([...pseq(b.pyramid([60, 62, 64]))], [60, 60, 62, 60, 62, 64]);
    assert.throws(() => b.pyramid(pseq([1]) as never), /whole list/);
});

// ---- value and random patterns ----

const mean = (xs: Iterable<number>): number => {
    const a = [...xs];
    return a.reduce((s, x) => s + x, 0) / a.length;
};
const median = (xs: Iterable<number>): number => {
    const a = [...xs].sort((x, y) => x - y);
    return a[a.length >> 1]!;
};

test("an argument may be a pattern read per step", () => {
    assert.deepEqual([...pseries(0, pseq([1, 2, 3]))], [0, 1, 3]);
    assert.deepEqual([...pgeom(1, pseq([2, 3]), 3)], [1, 2]);
    assert.deepEqual([...pseq<number>([pvalue(pseq([1, 2])), pvalue(3)])], [1, 2, 3]);
    const lows = [...pwhite(pseq([0, 10, 20]), pseq([1, 11, 21]))];
    assert.ok(lows.length === 3 && lows.every((x, i) => x >= i * 10 && x < i * 10 + 1));
});

test("the distributions have their shapes", () => {
    const n = 4000;
    assert.ok(Math.abs(mean(plprand(0, 1, n)) - 1 / 3) < 0.03);
    assert.ok(Math.abs(mean(phprand(0, 1, n)) - 2 / 3) < 0.03);
    assert.ok(Math.abs(mean(pmeanrand(0, 1, n)) - 0.5) < 0.03);
    assert.ok(Math.abs(mean(pbeta(0, 1, 2, 6, n)) - 0.25) < 0.03);
    assert.ok(Math.abs(mean(pgauss(60, 3, n)) - 60) < 0.3);
    assert.ok(Math.abs(mean(ppoisson(3, n)) - 3) < 0.15);
    assert.ok([...ppoisson(3, 50)].every(Number.isInteger));
    assert.ok(Math.abs(median(pcauchy(5, 0.5, n)) - 5) < 0.1);
    const xs = [...pexprand(100, 1000, n)];
    assert.ok(xs.every((x) => x >= 100 && x <= 1000));
    assert.ok(Math.abs(median(xs) - 316.2) < 25);
    assert.ok(Math.abs(mean(pprob([0, 1, 0], 0, 10, null, n)) - 5) < 0.2);
});

test("a random walk stays inside its bounds", () => {
    const walk = [...pbrown(0, 1, 0.1, 2000)];
    assert.ok(walk.every((x) => x >= 0 && x <= 1));
    assert.ok(walk.slice(1).every((x, i) => Math.abs(x - walk[i]!) <= 0.2));
});

// ---- list patterns ----

test("the list patterns read their list", () => {
    assert.deepEqual([...pseq([1, 2, 3], 2, 1)], [2, 3, 1, 2, 3, 1]);
    assert.deepEqual([...pslide([1, 2, 3, 4, 5], 3, 3, 1)], [1, 2, 3, 2, 3, 4, 3, 4, 5]);
    assert.deepEqual([...place([1, [2, 3]], 4)], [1, 2, 1, 3, 1, 2, 1, 3]);
    assert.deepEqual([...placep<number>([pseq([1, 2]), pseq([10, 20, 30])], 5)], [1, 10, 2, 20, 30]);
    assert.deepEqual([...ptuple<number>([pseq([1, 2]), pseq([10, 20, 30])])], [[1, 10], [2, 20]]);
    assert.deepEqual([...pswitch<number>([pseq([1, 2]), 9], pseq([0, 1, 0]))], [1, 2, 9, 1, 2]);
    assert.deepEqual([...pswitch1<number>([pseq([1, 2, 3]), pseq([10, 20])], pseq([0, 1, 0]))], [1, 10, 2]);
    assert.deepEqual([...pwalk([1, 2, 3, 4], pseq([1], 8), pseq([1, -1], INF))], [1, 2, 3, 4, 3, 2, 1, 2]);
    const xs = [...pxrand([1, 2, 3], 200)];
    assert.ok(xs.slice(1).every((x, i) => x !== xs[i]));
    const order = [...pshuffle([1, 2, 3, 4], 3)];
    assert.deepEqual([...order.slice(0, 4)].sort(), [1, 2, 3, 4]);
    assert.deepEqual(order.slice(4, 8), order.slice(0, 4));
    const machine = pdfsm([
        pseq([0, 1, 1, 0]),
        { 0: [0, "x"], 1: [1, "y"], default: [0, "z"] },
        { 0: [0, "p"], default: [1, "q"] },
    ]);
    assert.deepEqual([...machine], ["x", "y", "q", "p"]);
});

test("a list of event patterns plays", () => {
    const phrase = pbind({ degree: pseq([0, 1]) });
    for (const p of [pxrand([phrase, phrase]), pshuffle([phrase]), pslide([phrase]), pwalk([phrase])]) {
        assert.ok(p instanceof EventPattern);
    }
});

// ---- filter patterns ----

test("the filters change what comes out", () => {
    assert.deepEqual([...pcollect((x: number) => x * x, pseq([1, 2, 3]))], [1, 4, 9]);
    assert.deepEqual([...pselect((x: number) => x % 2, pseq([1, 2, 3]))], [1, 3]);
    assert.deepEqual([...preject((x: number) => x % 2, pseq([1, 2, 3]))], [2]);
    assert.deepEqual([...pcollect((_x: number, e?: unknown) => e, pseq([1]))], [undefined]);
    assert.deepEqual([...pdrop(pseq([1, 2, 3]), 1)], [2, 3]);
    assert.deepEqual([...plen(pseries(0, 1), 3)], [0, 1, 2]);
    assert.deepEqual([...pstutter(pseq([1, 2]), pseq([2, 3]))], [1, 1, 2, 2, 2]);
    assert.deepEqual([...platch(pseries(0, 1), pseq([true, false, true]))], [0, 0, 1]);
    assert.deepEqual([...pwrap(pseq([-1, 5]), 0, 4)], [3, 1]);
    assert.deepEqual([...pclump(pseq([1, 2, 3]), 2)], [[1, 2], [3]]);
    assert.deepEqual([...pflatten(pseq<unknown>([[1, [2, 3]], 4]), 2)], [1, 2, 3, 4]);
    assert.deepEqual([...pdiff(pseq([1, 4, 9]))], [3, 5]);
    assert.deepEqual([...pprorate(pseq([2]), pseq([0.25]))], [0.5, 1.5]);
});

test("a gate takes a value when a repeat starts", () => {
    const phrase = pbind({ n: pn(pseq([0, 1]), INF, "g"), x: pgate(pseries(0, 1), INF, "g") });
    assert.deepEqual([...plen(phrase, 6)].map((e) => e.get("x")), [0, 0, 1, 1, 2, 2]);
});

test("the time filters cut and delay events", () => {
    const cut = [...pdur(2.5, pbind({ dur: 1, degree: pseq([0, 1, 2, 3]) }))];
    assert.deepEqual(cut.map((e) => e.delta()), [1, 1, 0.5]);
    const late = [...pdelta(1.5, pbind({ dur: 0.5, degree: pseq([0, 1]) }))];
    assert.deepEqual(late.map((e) => e.get("type")), ["rest", "note", "note"]);
    assert.ok(pdur(1, pbind({ degree: 0 })) instanceof EventPattern);
});

// ---- function patterns ----

test("a function computes values when asked", () => {
    const body = function* (inval?: unknown) {
        for (let i = 0; i < 3; i++) inval = yield i * 10;
    };
    assert.deepEqual([...prout(body)], [0, 10, 20]);
    assert.deepEqual([...prout(body)], [0, 10, 20]);
    assert.deepEqual([...pfuncn(() => pseq([1, 2]), 2)], [1, 2, 1, 2]);
    const built: number[] = [];
    const lazy = plazy(() => {
        built.push(1);
        return pseq([5, 6]);
    });
    assert.deepEqual(built, []);
    assert.deepEqual([...lazy], [5, 6]);
    assert.deepEqual(built, [1]);
    assert.deepEqual([...pif(pseq([true, false, true]), pseries(0, 1), pseries(100, 1))], [0, 100, 1]);
    assert.deepEqual([...pproduct(null, [pseq([1, 2]), pseq([10, 20])])], [[1, 10], [1, 20], [2, 10], [2, 20]]);
    const phrase = pbind({ degree: pseq([1, 2]), octave: pfunc((e?: unknown) => (e as { degree: number }).degree + 4) });
    assert.deepEqual([...phrase].map((e) => e.get("octave")), [5, 6]);
});
