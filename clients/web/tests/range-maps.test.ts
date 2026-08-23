// The warp family, against the Python client's own values.
//
// `linlin` and its seven siblings are SuperCollider's, and both clients bind
// the same `clausters_core::warp` — so the test that matters is not that the
// numbers are plausible but that they are *the same numbers*, computed in f32
// on both sides. The reference values here are sclang's, as they are in
// `clients/python/tests/test_range_maps.py`, and the two files assert the same
// facts in the same order.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import * as B from "../src/base/builtins.ts";

await loadCore(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", import.meta.url)),
);

const close = (got: number, want: number, tol = 1e-4): void =>
    assert.ok(Math.abs(got - want) <= Math.abs(want) * tol + 1e-6, `${got} != ${want}`);

test("the linear map is sclang's", () => {
    close(B.linlin(0.5, 0, 1, 20, 20000) as number, 10010);
    close(B.linlin(0, 0, 1, 20, 20000) as number, 20);
    close(B.linlin(1, 0, 1, 20, 20000) as number, 20000);
});

test("the exponential map is sclang's", () => {
    close(B.linexp(0.5, 0, 1, 20, 20000) as number, 632.4555);
    close(B.explin(632.4555, 20, 20000, 0, 1) as number, 0.5);
    close(B.expexp(632.4555, 20, 20000, 1, 100) as number, 10);
});

test("each map reads what its inverse writes", () => {
    for (const x of [0, 0.25, 0.5, 0.75, 1]) {
        close(B.explin(B.linexp(x, 0, 1, 20, 20000) as number, 20, 20000, 0, 1) as number, x, 1e-3);
        close(B.curvelin(B.lincurve(x, 0, 1, 0, 1) as number, 0, 1, 0, 1) as number, x, 1e-3);
    }
});

test("the bent map is sclang's and zero curvature is the linear one", () => {
    close(B.lincurve(0.5, 0, 1, 0, 1, -4) as number, 0.8807971);
    assert.equal(B.lincurve(0.3, 0, 1, 10, 20, 0), B.linlin(0.3, 0, 1, 10, 20));
});

test("an out-of-range input is trimmed by default", () => {
    close(B.linlin(2, 0, 1, 0, 10) as number, 10);
    close(B.linlin(-1, 0, 1, 0, 10) as number, 0);
});

test("clip none extrapolates instead", () => {
    close(B.linlin(2, 0, 1, 0, 10, "none") as number, 20);
    close(B.linlin(-1, 0, 1, 0, 10, "min") as number, 0);
    close(B.linlin(2, 0, 1, 0, 10, "min") as number, 20);
});

test("an unknown clip mode says so", () => {
    assert.throws(() => B.linlin(0.5, 0, 1, 0, 10, "sometimes" as never));
});

test("a bipolar value spans the range and is not trimmed", () => {
    close(B.range(-1, 100, 200) as number, 100);
    close(B.range(0, 100, 200) as number, 150);
    close(B.range(1, 100, 200) as number, 200);
    close(B.range(2, 100, 200) as number, 250);
    close(B.exprange(0, 1, 100) as number, 10);
});

test("an exponential end at zero is nudged rather than a NaN", () => {
    const y = B.linexp(0.5, 0, 1, 0, 1) as number;
    assert.ok(Number.isFinite(y) && y > 0 && y < 1, `${y}`);
});

test("an array maps elementwise", () => {
    assert.deepEqual(B.linlin([0, 1, 2], 0, 2, 60, 72), [60, 66, 72]);
    assert.deepEqual(B.linlin([0, 0.5, 1], 0, 1, 0, 10), [0, 5, 10]);
});

test("the whole family composes with the unary builtins", () => {
    const notes = B.linlin([0, 1, 2], 0, 2, 60, 72) as number[];
    const hz = B.midicps(notes) as number[];
    [261.62555, 369.99442, 523.2511].forEach((want, i) => close(hz[i]!, want));
});

test("one range serves the whole sequence", () => {
    assert.deepEqual(B.linexp([0, 1], 0, 1, 20, 20000), [20, 20000]);
});

test("an exponential input end at zero takes the same rule", () => {
    close(B.explin(0, 0, 1, 0, 1) as number, 0);
    assert.ok(Number.isFinite(B.expexp(0, 0, 1, 1, 100) as number));
});
