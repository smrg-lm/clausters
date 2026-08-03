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
    conv,
    dbrown,
    dbufrd,
    demand,
    dgeom,
    dibrown,
    diskIn,
    diskOut,
    diwhite,
    drand,
    dseq,
    dseries,
    dshuf,
    dstutter,
    dswitch1,
    dup,
    duty,
    dwhite,
    dxrand,
    envGen,
    fft,
    ifft,
    impulse,
    lpf,
    madd,
    midSide,
    mix,
    out,
    pan2,
    panAz,
    partconvFrames,
    pvAdd,
    pvBinShift,
    pvBrickWall,
    pvCopyPhase,
    pvKernel,
    pvMagAbove,
    pvMagBelow,
    pvMagClip,
    pvMagFreeze,
    pvMagMul,
    pvMagShift,
    pvMagSmear,
    pvMax,
    pvMin,
    pvMul,
    rotate2,
    saw,
    sendTrig,
    sine,
    stereoWidth,
    svf,
    svfMorph,
    tduty,
    whiteNoise,
} from "../src/defs/ugens/index.ts";
import type { Channel } from "../src/defs/ugens/index.ts";
import {
    binIndex,
    binfreq,
    mag,
    nbins,
    param,
    phase,
    pvOp,
} from "../src/defs/pv_expr.ts";

const here = new URL(".", import.meta.url);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

interface Vectors {
    synthdefs: { name: string; spec: unknown }[];
    faustdefs: { name: string; payload: unknown }[];
    graphdefs: { name: string; spec: unknown }[];
    scalars: { name: string; args: number[]; value: number }[];
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

    // ---- the full catalogue: one case per family the port filled out ----

    // Every demand *source*, nested the way the family is meant to be: a
    // sequence whose items are themselves streams. The `dr` rate rides along.
    demand_sources: () => {
        const steps = dseq([
            dseries(3, 60.0, 2.0),
            dgeom(2, 220.0, 1.5),
            dwhite(1, 100.0, 200.0),
            diwhite(1, 48.0, 72.0),
            dbrown(1, 0.0, 1.0, 0.1),
            dibrown(1, 0, 12, 1),
        ], 2.0);
        return new SynthDef(
            "sources",
            out(0.0, sine(demand(impulse(8.0), 0.0, steps))),
        );
    },

    // The pickers, the stutter, the buffer read and both drivers.
    demand_drivers: () => {
        const picked = dswitch1(
            dxrand([0.0, 1.0, 2.0], 0.0),
            dshuf([110.0, 220.0, 330.0], 1.0),
            dstutter(2.0, drand([440.0, 550.0])),
            dbufrd(control("buf", 0.0, { rate: "ir" }), dseries(0, 0.0, 1.0)),
        );
        return new SynthDef(
            "drivers",
            out(
                0.0,
                sine(duty(dseq([0.25, 0.5], 0.0), 0.0, picked, DoneAction.NONE))
                    .mul(tduty(0.5, 0.0, 0.2, DoneAction.NONE, 1.0)),
            ),
        );
    },

    // The frequency-domain chain: `fft` carries the static fields, every
    // `pv*` transforms in place, `ifft` closes it. Two chains, so the
    // combiners have a B side.
    spectral_chain: () => {
        const opts = { fftSize: 512, hop: 0.25, wintype: 1 };
        let a = fft(whiteNoise(), 1.0, opts);
        let b = fft(saw(110.0), 1.0, opts);
        a = pvMagAbove(a, 3.0);
        a = pvMagBelow(a, 200.0);
        a = pvMagClip(a, 50.0);
        a = pvBrickWall(a, 0.4);
        a = pvMagSmear(a, 2.0);
        a = pvMagFreeze(a, control("freeze", 0.0));
        a = pvBinShift(a, 1.5, 2.0);
        a = pvMagShift(a, 0.5, -1.0);
        b = pvMul(b, pvAdd(pvMin(a, b), pvMax(a, b)));
        b = pvCopyPhase(pvMagMul(a, b), b);
        return new SynthDef("spectral", out(0.0, ifft(b)));
    },

    // A per-bin program: every term the expression language has, a unary, a
    // comparison and both expressions, so the token lists are compared whole.
    pv_kernel_expr: () => {
        const tilt = param(0).mul(
            pvOp("add", 1.0, pvOp("mul", 4.0, binIndex).div(nbins)),
        );
        return new SynthDef(
            "kernel",
            out(0.0, ifft(pvKernel(fft(whiteNoise()), {
                mag: mag.mul(mag.ge(tilt)).mul(binfreq.div(1000.0).sqrt()),
                phase: phase.add(param(1)),
                params: [control("thresh", 2.0), control("spin", 0.0)],
            }))),
        );
    },

    // Partitioned convolution, whose two static fields size the instance.
    convolution: () =>
        new SynthDef(
            "conv",
            out(0.0, conv(saw(110.0), control("kernel", 0.0, { rate: "ir" }), {
                fftSize: 512,
                partitions: 8,
            })),
        ),

    // The stereo field: the three matrices, each building one UGen per
    // channel with the index as its last input.
    stereo_field: () => {
        const ms = midSide(sine(220.0), saw(110.0));
        const turned = rotate2(ms.at(0), ms.at(1), 0.25);
        return new SynthDef(
            "field",
            out(0.0, stereoWidth(turned.at(0), turned.at(1), 1.5)),
        );
    },

    ring_pan: () =>
        new SynthDef(
            "ring",
            out(0.0, panAz(4, whiteNoise(), 0.3, 0.5, 3.0, 0.0)),
        ),

    // The state-variable filter, once with the tap gains given directly and
    // once swept by `svfMorph` — with a signal position, whose clamps are
    // graph nodes, and with a constant one, whose clamps fold to numbers.
    svf_taps: () =>
        new SynthDef(
            "taps",
            out(0.0, svf(saw(110.0), 800.0, { rq: 0.3 }, 1.0, -0.5, 1.0)),
        ),

    svf_sweep: () => {
        const pos = control("morph", 0.0);
        return new SynthDef(
            "sweep",
            out(
                0.0,
                svf(saw(110.0), 800.0, { rq: 0.3 }, ...svfMorph(pos))
                    .add(svf(saw(55.0), 400.0, { rq: 0.3 }, ...svfMorph(0.5))),
            ),
        );
    },

    // Streaming disk I/O: two static fields each, and a def whose root is the
    // recorder's pass-through.
    disk_io: () =>
        new SynthDef(
            "disk",
            out(0.0, diskOut(
                "/tmp/take.wav",
                diskIn("/tmp/loop.wav", 0.0, true).mul(0.5),
                "float",
            )),
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

// The catalogue's one plain-number helper: it sizes a buffer rather than
// building a graph, so it is frozen as values.
test("scalar parity: partconvFrames", () => {
    for (const row of vectors.scalars.filter((r) => r.name === "partconv_frames")) {
        const [irFrames, fftSize] = row.args as [number, number];
        assert.equal(partconvFrames(irFrames, fftSize), row.value,
            `partconvFrames(${String(irFrames)}, ${String(fftSize)})`);
    }
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
    const { delayL } = await import("../src/defs/ugens/index.ts");
    assert.throws(() => delayL(sine(440.0), sine(1.0)), TypeError);
    // With the size stated it builds, and the size rides as a static field.
    const spec = new SynthDef(
        "echo",
        out(0.0, delayL(sine(440.0), sine(1.0).mul(0.1), 0.5)),
    ).spec();
    const line = spec.ugens.find((u) => u.kind === "DelayL");
    assert.equal(line?.max_delay, 0.5);
});
