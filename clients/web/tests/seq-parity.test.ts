// The automation lane against the Python client's, on the shared vectors.
//
// `gen-seq-vectors.py` freezes what the reference lane emits — the internal
// control def's spec, the flat `/buffer_gen "env"` argument list a curve
// discretizes into, and the break-point round trip. Each case here rebuilds
// the same curve with the TS surface and asserts the values are identical:
// what has to match is the wire, never the source.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Env } from "../src/defs/ugens/index.ts";
import { Automation, autoLaneDef, envGenArgs, LANE_DEF } from "../src/seq/automation.ts";

const here = new URL(".", import.meta.url);

await loadCore();

interface AutomationVector {
    name: string;
    points: number[] | null;
    env_args: number[];
    to_points: number[];
    duration: number;
}

const vectors = JSON.parse(
    await readFile(new URL("./seq-vectors.json", here), "utf8"),
) as {
    lane_def: { name: string; spec: unknown };
    automations: AutomationVector[];
};

/** The reference curves, rebuilt independently through the TS surface. */
const built: Record<string, Automation> = {
    drawn_curve: Automation.fromPoints(
        [[0.0, 200.0, 1, 0.0], [2.0, 4000.0, 2, 0.0], [3.0, 800.0, 5, -4.0]],
        null,
        { name: "cutoff" },
    ),
    leading_delay: Automation.fromPoints(
        [1.0, 0.0, 1, 0.0, 3.0, 1.0, 1, 0.0],
        null,
    ),
    adsr_env: new Automation(Env.adsr(0.01, 0.2, 0.6, 0.4), null, { name: "amp" }),
};

test("the automation lane def emits the reference spec", () => {
    assert.equal(LANE_DEF, vectors.lane_def.name);
    assert.deepEqual(autoLaneDef().spec(), vectors.lane_def.spec);
});

for (const vector of vectors.automations) {
    test(`automation '${vector.name}' matches the reference curve`, () => {
        const auto = built[vector.name];
        assert.ok(auto, `no TS case built for vector '${vector.name}'`);

        // The `/buffer_gen "env"` payload: the numbers, and the tags that
        // keep the bytes the reference client's (a shape is an int).
        const args = envGenArgs(auto.env);
        assert.deepEqual(
            args.map((a) => (Array.isArray(a) ? a[1] : a)),
            vector.env_args,
        );
        assert.deepEqual(
            args.map((a) => (Array.isArray(a) ? a[0] : "?")),
            vector.env_args.map((_, i) => (i > 0 && i % 4 === 3 ? "i" : "f")),
        );

        // The break-point round trip the `bpf` editor rides on, and the
        // length the timeline places the curve by.
        assert.deepEqual(auto.toPoints(), vector.to_points);
        assert.ok(Math.abs(auto.duration() - vector.duration) < 1e-9);
    });
}

test("a single target and a list of them normalize the same way", () => {
    const one = new Automation(Env.adsr(), [1001, "cutoff"]);
    assert.deepEqual(one.targets, [[1001, "cutoff"]]);
    assert.equal(one.name, "cutoff");

    const many = new Automation(Env.adsr(), [[1001, "cutoff"], [1002, "amp"]]);
    assert.deepEqual(many.targets, [[1001, "cutoff"], [1002, "amp"]]);
    assert.equal(many.name, "cutoff");

    // No target at all is legal (a curve on a bus somebody else reads).
    const none = new Automation(Env.adsr(), null);
    assert.deepEqual(none.targets, []);
    assert.equal(none.name, "automation");
});

test("play refuses an unprepared curve by naming prepare", () => {
    const auto = new Automation(Env.adsr(), null);
    assert.throws(() => auto.play(), /prepare/);
});
