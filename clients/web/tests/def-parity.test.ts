// The def model against the Python client's, on the shared vectors.
//
// `gen-def-vectors.py` freezes the spec JSON the Python builders emit for a
// set of graphs; each case here rebuilds the same graph with the TS builders
// and asserts the emitted spec is identical. The two surfaces are written
// independently (TypeScript composes by method where Python composes by
// operator) — what has to match is only the wire, which is the whole point of
// the shared def format.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadOsc } from "../src/base/osc.ts";
import { FaustDef } from "../src/defs/faustdef.ts";
import { GraphDef } from "../src/defs/graphdef.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import * as sig from "../src/defs/signals.ts";
import {
    DoneAction,
    Env,
    chans,
    control,
    dup,
    envGen,
    lpf,
    madd,
    mix,
    out,
    pan2,
    saw,
    sendTrig,
    sine,
    whiteNoise,
} from "../src/defs/ugens.ts";
import type { Channel } from "../src/defs/ugens.ts";

const here = new URL(".", import.meta.url);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

interface Vectors {
    synthdefs: { name: string; spec: unknown }[];
    faustdefs: { name: string; payload: unknown }[];
    graphdefs: { name: string; spec: unknown }[];
}

const vectors: Vectors = JSON.parse(
    await readFile(new URL("def-vectors.json", here), "utf8"),
) as Vectors;

const find = <T extends { name: string }>(rows: T[], name: string): T => {
    const row = rows.find((r) => r.name === name);
    assert.ok(row, `no vector named '${name}' — regenerate def-vectors.json`);
    return row;
};

/** The TS side of each named vector, built independently of the Python one. */
const synthdefs: Record<string, () => SynthDef> = {
    beep: () => new SynthDef("beep", out(0.0, sine(440.0))),

    controls_stereo: () => {
        const freq = control("freq", 440.0);
        const amp = control("amp", 0.2);
        return new SynthDef("controls_stereo", out(0.0, dup(sine(freq).mul(amp))));
    },

    // A control reused in two places serializes once and is referenced twice.
    shared_control: () => {
        const shared = control("freq", 200.0);
        return new SynthDef(
            "shared",
            out(0.0, sine(shared).mul(sine(shared.mul(2.0)))),
        );
    },

    typed_controls_env: () => {
        const gate = control("gate", 1.0, { rate: "tr" });
        const cutoff = control("cutoff", 800.0, { lag: 0.1, lagDown: 0.5 });
        const env = Env.adsr(0.01, 0.2, 0.6, 0.4);
        return new SynthDef(
            "voice",
            out(
                0.0,
                lpf(saw(110.0), cutoff).mul(
                    envGen(env, { gate, doneAction: DoneAction.FREE_SELF }),
                ),
            ),
        );
    },

    generic_ops: () =>
        new SynthDef("ops", out(0.0, sine(440.0).distort().max(0.1))),

    mix_fold: () => {
        const voices: Channel[] = Array.from({ length: 7 }, (_u, n) =>
            sine(110.0 * (n + 1)));
        return new SynthDef("fold", out(0.0, madd(mix(voices), 0.1, 0.0)));
    },

    pan: () => new SynthDef("pan", out(0.0, pan2(whiteNoise(), 0.3))),

    side_effect_only: () =>
        new SynthDef("watch", sendTrig(sine(1.0), 7, 0.5)),

    rates_and_chans: () =>
        new SynthDef(
            "rates",
            out(0.0, chans(sine(5.0).atRate("kr"), sine(7.0).atRate("kr"))),
        ),
};

for (const [name, build] of Object.entries(synthdefs)) {
    test(`SynthDef parity: ${name}`, () => {
        const expected = find(vectors.synthdefs, name).spec;
        assert.deepEqual(build().spec(), expected);
    });
}

// The Faust signal tree: one tone def, and the same tone as two outputs.
function faustTone(): sig.Signal {
    const freq = sig.hslider("freq", 440.0, 20.0, 2000.0, 0.01);
    const amp = sig.hslider("amp", 0.2, 0.0, 1.0, 0.001);
    // A phasor by explicit feedback: the running sum of freq/sr, wrapped.
    const step = () => freq.div(sig.sr());
    const phasor = sig.rec((s) => {
        const next = s.add(step());
        return next.sub(next.floor());
    });
    return sig.sin(phasor.mul(2.0 * sig.PI)).mul(amp);
}

test("FaustDef parity: a signal-tree tone", () => {
    const expected = find(vectors.faustdefs, "faust_tone").payload;
    const def = FaustDef.fromSignals("tone", faustTone());
    assert.deepEqual(JSON.parse(def.dumpDef()), expected);
    assert.deepEqual(def.controlNames(), ["freq", "amp"]);
});

test("FaustDef parity: two outputs", () => {
    const expected = find(vectors.faustdefs, "faust_stereo").payload;
    const tone = faustTone();
    const def = FaustDef.fromSignals("stereo", tone, tone.mul(0.5));
    assert.deepEqual(JSON.parse(def.dumpDef()), expected);
});

test("GraphDef parity: a wired chain with a scaled port", () => {
    const expected = find(vectors.graphdefs, "graph_chain").spec;
    const g = new GraphDef("chain");
    const bus = g.bus("mix");
    const src = g.add("gsrc", { out: bus, level: 1.0 });
    g.add("gsink", { in: bus, out: "OUT" });
    g.add("gvoice", { out: bus }, { voice: true });
    g.port("gain", [src.control("level").scaled(2.0, 0.1)], 0.5);
    assert.deepEqual(g.spec(), expected);
});

// ---- the rules the spec walk enforces, which no vector can show ----

test("a def needs at least one root", () => {
    assert.throws(() => new SynthDef("empty"), TypeError);
});

test("a channel list cannot feed a single-channel input", () => {
    const stereo = dup(sine(440.0));
    assert.throws(
        () => new SynthDef("bad", out(0.0, lpf(stereo as never, 800.0))).spec(),
        TypeError,
    );
});

test("one control name cannot carry two definitions", () => {
    const a = control("freq", 440.0);
    const b = control("freq", 220.0);
    assert.throws(
        () => new SynthDef("clash", out(0.0, sine(a).mul(sine(b)))).spec(),
        TypeError,
    );
});

test("a modulated delaytime must state how long the line is", async () => {
    const { delayL } = await import("../src/defs/ugens.ts");
    assert.throws(() => delayL(sine(440.0), sine(1.0)), TypeError);
    // With the size stated it builds, and the size rides as a static field.
    const spec = new SynthDef(
        "echo",
        out(0.0, delayL(sine(440.0), sine(1.0).mul(0.1), 0.5)),
    ).spec();
    const line = spec.ugens.find((u) => u.kind === "DelayL");
    assert.equal(line?.max_delay, 0.5);
});
