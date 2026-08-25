// The patcher's two levels against the Python client's, on the shared vectors.
//
// `gen-patch-vectors.py` decodes a handful of defs with the Python surface and
// freezes what leaves the model: the `{boxes, cords}` a def reads as, and the
// widget schema the host is handed. Each case here decodes the same def through
// the TypeScript surface and asserts the same two results — a Def view is a
// *reading* of a def, so a difference is one client seeing a graph the other
// does not have.
//
// One label is expected to differ and is declared below: where the two clients
// deliberately spell a builder's parameter differently, the inlet a Def view
// captions with it differs too, because the caption is the name a caller of
// *that* client types. Every other name must agree.
//
// Run with `npm test`; this suite needs nothing staged.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
    DefPatch,
    FaustDef,
    GraphPatch,
    SynthDef,
    control,
    in_,
    out,
    sine,
} from "../src/defs/index.ts";
import { hslider, sin } from "../src/defs/signals.ts";

interface Port {
    name: string;
    dir: "in" | "out";
    rate: string;
}
interface FrozenBox {
    def: string;
    kind: string | null;
    role: string;
    ports: Port[];
}
interface Case {
    boxes: FrozenBox[];
    cords: { from_box: number; from_port: number; to_box: number; to_port: number }[];
    widget: { boxes: Record<string, unknown>[]; cords: number[] };
    spec?: unknown;
}

const vectors: { cases: Record<string, Case> } = JSON.parse(
    await readFile(new URL("./patch-vectors.json", import.meta.url), "utf8"),
);

/**
 * Inlet labels the two clients spell differently on purpose — the resonant
 * filters take the wire's `rq` as `res` here and as `rq` there, which
 * `ugen-catalog.test.ts` already declares against the server's own catalog.
 * Nothing in the vectors uses one yet; the table is here so that when one does,
 * the difference is read as declared rather than as drift.
 */
const ALIASES: Record<string, Record<string, string>> = {
    RLPF: { rq: "res" },
    RHPF: { rq: "res" },
    BPF: { rq: "res" },
    BRF: { rq: "res" },
    Resonz: { rq: "res" },
    Svf: { rq: "res" },
};

/** The frozen form of one model, in the shape `gen-patch-vectors.py` writes. */
function frozen(model: DefPatch | GraphPatch): Case {
    return {
        boxes: model.boxes.map((b) => ({
            def: b.def,
            kind: b.kind ?? null,
            role: b.role ?? "object",
            ports: b.ports.map((p) => ({ name: p.name, dir: p.dir, rate: p.rate })),
        })),
        cords: model.cords,
        widget: model.toWidget() as Case["widget"],
    };
}

/** The def every cord weight is drawn from — `examples/editors/patch2.html`'s own. */
function tremoloSine(): SynthDef {
    const freq = control("freq", 220.0);
    const amp = control("amp", 0.2);
    const detune = control("detune", 1.5, { rate: "ir" });
    const tremolo = sine(control("lfo", 5.0)).atRate("kr").mul(0.5).add(0.5);
    const carrier = sine(freq.mul(detune));
    const sig = carrier.mul(amp).mul(tremolo);
    return new SynthDef("tremolo_sine", out(0.0, sig), out(1.0, sig));
}

function sharedInput(): SynthDef {
    const osc = sine(control("freq", 110.0));
    return new SynthDef("shared_input", out(0.0, osc.mul(osc)));
}

function fmTone(): FaustDef {
    const freq = hslider("freq", 220.0, 20.0, 2000.0, 0.1);
    const gain = hslider("gain", 0.2, 0.0, 1.0, 0.01);
    const modulator = sin(freq.mul(3.0)).mul(40.0);
    return FaustDef.fromSignals("fm_tone", sin(freq.add(modulator)).mul(gain));
}

function toneAndDac(): GraphPatch {
    const tone = new SynthDef("tone", out(control("out", 0.0), sine(control("freq", 220.0))));
    const dac = new SynthDef("dac", out(0.0, in_(control("in", 0.0))));
    const patch = new GraphPatch();
    const a = patch.add(tone);
    const b = patch.add(dac);
    patch.connect(a, "out", b, "in");
    return patch;
}

/** Assert one case against its frozen twin, alias by alias. */
function assertCase(name: string, model: DefPatch | GraphPatch): Case {
    const expected = vectors.cases[name];
    assert.ok(expected, `${name} is not in the vectors`);
    const got = frozen(model);
    assert.equal(got.boxes.length, expected.boxes.length, `${name}: box count`);
    got.boxes.forEach((box, i) => {
        const want = expected.boxes[i]!;
        const alias = ALIASES[want.def] ?? {};
        const renamed = {
            ...want,
            ports: want.ports.map((p) => ({ ...p, name: alias[p.name] ?? p.name })),
        };
        assert.deepEqual(box, renamed, `${name}: box ${i} (${want.def})`);
    });
    assert.deepEqual(got.cords, expected.cords, `${name}: cords`);
    assert.deepEqual(got.widget.cords, expected.widget.cords, `${name}: widget cords`);
    assert.equal(got.widget.boxes.length, expected.widget.boxes.length);
    return expected;
}

test("a SynthDef decodes into the same boxes and cords as the Python client's", () => {
    const sdef = tremoloSine();
    const expected = assertCase("tremolo_sine", DefPatch.fromSynthdef(sdef));
    // The picture is a reading of *this* def, and the vectors froze the Python
    // client's spec for it: the two defs are the same def, or the parity above
    // is a comparison of two different graphs that happen to agree.
    assert.deepEqual(sdef.spec(), expected.spec);
});

test("a UGen feeding two inputs is one box with two cords", () => {
    assertCase("shared_input", DefPatch.fromSynthdef(sharedInput()));
});

test("a Faust signal tree decodes node for node, and an opaque def is one box", () => {
    assertCase("fm_tone", DefPatch.fromFaustdef(fmTone()));
    assertCase("opaque", DefPatch.fromFaustdef(FaustDef.fromSource("opaque", "process = os.osc(440);")));
});

test("level 1 renders through the same widget schema", () => {
    assertCase("graph_level1", toneAndDac());
});

test("the decode is faithful: the round trip reproduces the def", () => {
    const sdef = tremoloSine();
    const rebuilt = DefPatch.fromSynthdef(sdef).toSynthdef(sdef.name);
    assert.deepEqual(rebuilt.spec(), sdef.spec());
});

test("a Faust patch has no SynthDef to rebuild, and says so", () => {
    const patch = DefPatch.fromFaustdef(fmTone());
    assert.throws(() => patch.toSynthdef("fm_tone"), /only rebuilds a UGen-graph patch/);
});
