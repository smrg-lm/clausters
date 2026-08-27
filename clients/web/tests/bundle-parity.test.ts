// The two writers of one format, and the mount that reads what they write.
//
// `gen-bundle-vectors.py` writes a reference bundle with the Python authoring
// API and freezes it — every file, byte for byte, plus the manifest and the
// GuiDef template — together with what the shared resolver makes of it for
// three mounts: the declared defaults, an attribute override, and a preset
// with an attribute over it. Two things are asserted from here:
//
// - the **writers agree**: the same authoring calls in TypeScript emit the
//   same bytes. That is a comparison on text rather than on shape because the
//   format is canonical JSON on both sides (`src/bundle-writer.ts` says what
//   canonical means and why the number rule is the one that costs something);
// - the **mount agrees**: the wasm door resolves those inputs into exactly the
//   allocation and tree the Python door did.
//
// What is being held is the cross-language contract of the *format*: a bundle
// authored in either language is one directory, and it mounts in a tab to what
// its author saw. Both sides call the one pass (`clausters_core::bundle`), so
// what can drift is a binding or a writer — which is what this catches.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Bundle } from "../src/bundle-writer.ts";
import type { Hole } from "../src/bundle-writer.ts";
import { bundle_requirements, bundle_resolve } from "../src/core/clausters_core_web.js";
import {
    DoneAction,
    Env,
    SynthDef,
    control,
    envGen,
    out,
    outCtl,
    sine,
} from "../src/defs/index.ts";
import { knob, meter, view } from "../src/gui/guidef.ts";

interface Vectors {
    manifest: unknown;
    template: unknown;
    files: Record<string, string>;
    requirements: { widgets: number; nodes: string[]; buses: unknown[]; buffers: string[] };
    cases: {
        name: string;
        attributes: Record<string, unknown>;
        preset: Record<string, unknown>;
        allocation: unknown;
        resolved: { def_id: number; tree: unknown; boot: unknown[][]; params: Record<string, unknown> };
    }[];
}

const here = new URL(".", import.meta.url);

// The resolver is a core export, so the wasm has to be in before it is called.
// Under node the bytes are passed explicitly — node's `fetch` cannot read a
// `file://` URL.
await loadCore();

const vectors = JSON.parse(
    await readFile(new URL("./bundle-vectors.json", here), "utf8"),
) as Vectors;

/**
 * The reference bundle, written the way `gen-bundle-vectors.py` writes it:
 * the same material, the same names, the same calls to the same API in the
 * same order. Where a spelling differs it is this language's spelling of the
 * one call — options before children in `view`, an options object where
 * Python takes keywords.
 */
function reference(): Bundle {
    // The bus reaches the def as a **control**, never baked in — the rule that
    // lets two instances share the one def that was sent.
    const voice = () => {
        const freq = control("freq", 220.0);
        const envBus = control("env_bus", 0.0);
        const env = envGen(Env.perc(), { doneAction: DoneAction.FREE_SELF });
        return new SynthDef("voice", out(0.0, sine(freq).mul(env)), outCtl(envBus, env));
    };

    const b = new Bundle("fm-voice");
    const freq = b.param("freq", "float", { default: 220.0, min: 60.0, max: 700.0 });
    const title = b.param("title", "string", { default: "FM voice" });
    const lfo = b.bus("lfo");
    const node = b.node("voice");
    b.synthdef(voice());
    b.gui(
        view(
            { title, layout: "col", w: 320, h: 200 },
            knob({
                label: "freq",
                value: freq,
                min: 60.0,
                max: 700.0,
                bind: ["/node_set", node, "freq"],
                id: 2,
            }),
            meter(lfo, { rate: "control", label: "env", id: 3 }),
        ),
    );
    b.boot(["/synth_new", "fm-voice.voice", node, 0, 0, "freq", freq, "env_bus", lfo]);
    b.preset("bright", { freq: 660.0, title: "bright voice" });
    return b;
}

test("the two writers emit one bundle, byte for byte", () => {
    const written = reference().files();
    assert.deepEqual(
        Object.keys(written).sort(),
        Object.keys(vectors.files).sort(),
        "the same files, by path",
    );
    for (const [path, text] of Object.entries(vectors.files)) {
        assert.equal(written[path], text, `${path} differs from what the Python writer emits`);
    }
});

test("what the TypeScript writer emits mounts to the frozen resolution", () => {
    const b = reference();
    const manifest = b.manifest();
    const template = b.record();
    for (const testCase of vectors.cases) {
        const got = JSON.parse(
            bundle_resolve(
                JSON.stringify({
                    manifest,
                    template,
                    allocation: testCase.allocation,
                    params: { attributes: testCase.attributes, preset: testCase.preset },
                }),
            ),
        );
        assert.deepEqual(got, testCase.resolved, testCase.name);
    }
});

test("write puts exactly those files on disk", async () => {
    const directory = await mkdtemp(join(tmpdir(), "clausters-bundle-"));
    try {
        await reference().write(directory);
        for (const [path, text] of Object.entries(vectors.files)) {
            assert.equal(await readFile(join(directory, ...path.split("/")), "utf8"), text, path);
        }
    } finally {
        await rm(directory, { recursive: true, force: true });
    }
});

test("an unmountable bundle is unwritable", () => {
    const b = new Bundle("bad-bundle");
    b.gui(view({}, meter("@nowhere" as Hole, { id: 2 })));
    // `@nowhere` is declared in no namespace, so the mount could not fill it.
    assert.throws(() => b.files(), /nowhere/);
});

test("what one instance needs matches the reference", () => {
    const got = JSON.parse(
        bundle_requirements(
            JSON.stringify({ manifest: vectors.manifest, template: vectors.template }),
        ),
    );
    assert.deepEqual(got, vectors.requirements);
});

for (const testCase of vectors.cases) {
    test(`a bundle written in Python resolves identically: ${testCase.name}`, () => {
        const got = JSON.parse(
            bundle_resolve(
                JSON.stringify({
                    manifest: vectors.manifest,
                    template: vectors.template,
                    allocation: testCase.allocation,
                    params: { attributes: testCase.attributes, preset: testCase.preset },
                }),
            ),
        );
        assert.deepEqual(got, testCase.resolved);
    });
}

test("the three mounts share no widget id, node or bus", () => {
    const ids = vectors.cases.map((c) => c.resolved.def_id);
    assert.equal(new Set(ids).size, ids.length, "each mount opened its own def id");
    // The knob's bind carries the node id, the meter's bus the bus — both
    // allocated per instance, which is what makes two of one bundle possible.
    const nodeOf = (c: (typeof vectors.cases)[number]) =>
        JSON.stringify((c.resolved.tree as any).children[0].bind);
    const busOf = (c: (typeof vectors.cases)[number]) =>
        (c.resolved.tree as any).children[1].bus;
    assert.equal(new Set(vectors.cases.map(nodeOf)).size, vectors.cases.length);
    assert.equal(new Set(vectors.cases.map(busOf)).size, vectors.cases.length);
});

test("the resolution order is attribute over preset over default", () => {
    const by = (name: string) => vectors.cases.find((c) => c.name === name)!;
    assert.equal(by("defaults").resolved.params.freq, 220.0);
    assert.equal(by("defaults").resolved.params.title, "FM voice");
    assert.equal(by("attribute").resolved.params.freq, 440.0);
    // The preset supplies the title, the attribute overrides the freq.
    assert.equal(by("preset_under_attribute").resolved.params.freq, 330.0);
    assert.equal(by("preset_under_attribute").resolved.params.title, "bright voice");
});
