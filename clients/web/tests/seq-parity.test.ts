// A break-point curve against the Python client's, on the shared vectors.
//
// `gen-seq-vectors.py` freezes what the reference client emits for a curve --
// the flat `/buffer_gen "env"` argument list an envelope fills a buffer with,
// the break-point round trip, and the span the curve covers. Each case here
// rebuilds the same curve with the TS surface and asserts the values are
// identical: what has to match is the wire, never the source.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Bpf, Env, envGenArgs } from "../src/defs/ugens/index.ts";

const here = new URL(".", import.meta.url);

await loadCore();

interface CurveVector {
    name: string;
    points: number[] | null;
    env_args: number[];
    to_points: number[];
    duration: number;
    release_node?: number | null;
}

const vectors = JSON.parse(
    await readFile(new URL("./seq-vectors.json", here), "utf8"),
) as { curves: CurveVector[] };

/** The reference curves, rebuilt independently through the TS surface. */
const built: Record<string, Bpf | Env> = {
    drawn_curve: new Bpf([[0.0, 200.0, 1, 0.0], [2.0, 4000.0, 2, 0.0], [3.0, 800.0, 5, -4.0]]),
    // The same points written with the shape *names* an `Env` takes.
    named_shapes: new Bpf([[0.0, 200.0, "lin"], [2.0, 4000.0, "exp"], [3.0, 800.0, -4.0]]),
    leading_delay: new Bpf([1.0, 0.0, 1, 0.0, 3.0, 1.0, 1, 0.0]),
    adsr_env: Env.adsr(0.01, 0.2, 0.6, 0.4),
};

for (const vector of vectors.curves) {
    test(`curve '${vector.name}' matches the reference`, () => {
        const source = built[vector.name];
        assert.ok(source, `no TS case built for vector '${vector.name}'`);
        const curve = source instanceof Bpf ? source : Bpf.fromEnv(source);

        // The `/buffer_gen "env"` payload: the numbers, and the tags that keep
        // the bytes the reference client's (a shape is an int).
        const args = envGenArgs(source);
        assert.deepEqual(
            args.map((a) => (Array.isArray(a) ? a[1] : a)),
            vector.env_args,
        );
        assert.deepEqual(
            args.map((a) => (Array.isArray(a) ? a[0] : "?")),
            vector.env_args.map((_, i) => (i > 0 && i % 4 === 3 ? "i" : "f")),
        );

        // The break-point round trip the `bpf` editor rides on, and the span
        // the curve covers.
        assert.deepEqual(curve.toPoints(), vector.to_points);
        assert.ok(Math.abs(curve.duration() - vector.duration) < 1e-9);

        if (vector.release_node !== undefined && vector.release_node !== null) {
            assert.equal(curve.releaseNode, vector.release_node);
        }
    });
}

test("the two bases round-trip through each other", () => {
    // The shape belongs to the segment that *leaves* a point, so an Env's
    // per-segment curves land on all but the last point and come back whole.
    const env = new Env([0.2, 0.8, 0.5], [1.0, 2.0], ["lin", "exp"]);
    const back = Bpf.fromEnv(env).toEnv();
    assert.deepEqual(back.levels, env.levels);
    assert.deepEqual(back.times, env.times);
    assert.deepEqual(back.toInputs(), env.toInputs());
});

test("a drawn delay moves the sustain with it", () => {
    // A first point later than the axis start becomes a leading `hold`
    // segment, so every index after it moves by one.
    const delayed = new Bpf([[1.0, 0.0], [2.0, 1.0], [3.0, 0.0]], { releaseNode: 1 });
    const made = delayed.toEnv();
    assert.deepEqual(made.times, [1.0, 1.0, 1.0]);
    assert.equal(made.releaseNode, 2);
    assert.equal(delayed.duration(), 2.0);
});

test("a curve needs two break points", () => {
    assert.throws(() => new Bpf([[0.0, 1.0]]), /two break points/);
});
