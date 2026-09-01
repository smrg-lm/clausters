// The two perceptual frequency scales, against the Python client's own values.
//
// `cpsmel`/`cpsbark` and their inverses bind the same `clausters_core::scale`
// the GUI host's ruler and the spectrogram shader read, so the facts asserted
// here are the ones `clients/python/tests/test_base.py` asserts, in the same
// order. Unlike every other builtin these are **f64**: they are not entries in
// the server's unary-op table, so nothing has to round to f32 to match a UGen.

import assert from "node:assert/strict";
import test from "node:test";

import * as B from "../src/base/builtins.ts";
import { loadCore } from "../src/base/core.ts";

await loadCore();

const close = (got: number, want: number, tol: number): void =>
    assert.ok(Math.abs(got - want) <= tol, `${got} != ${want}`);

test("a kilohertz sits where each scale says it does", () => {
    close(B.cpsmel(1000) as number, 1000, 0.1);
    close(B.cpsbark(1000) as number, 8.53, 0.05);
});

test("each scale reads what its inverse writes", () => {
    close(B.melcps(B.cpsmel(440) as number) as number, 440, 1e-6);
    close(B.barkcps(B.cpsbark(440) as number) as number, 440, 1e-6);
});

test("zero hertz is -0.53 bark, the formula and not an error", () => {
    close(B.cpsbark(0) as number, -0.53, 1e-2);
});

test("a sequence in, a sequence out, like every other builtin", () => {
    const got = B.cpsmel([100, 1000]) as number[];
    close(got[0]!, B.cpsmel(100) as number, 1e-12);
    close(got[1]!, 1000, 0.1);
});
