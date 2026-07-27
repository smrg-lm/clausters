// The bundle the Python writer emits, resolved through the browser's door.
//
// `gen-bundle-vectors.py` writes a reference bundle with the Python authoring
// API and freezes it — the manifest, the GuiDef template — together with what
// the shared resolver makes of it for three mounts: the declared defaults, an
// attribute override, and a preset with an attribute over it. This asserts the
// wasm door produces exactly the same from exactly those inputs.
//
// What is being held is the cross-language contract of the *format*: a bundle
// authored in Python mounts in a tab to what the author saw. Both sides call
// the one pass (`clausters_core::bundle`), so the only thing that can drift is
// a binding — which is what this catches.
//
// (TypeScript gets its own writer after Python, the repo's standing rule; when
// it does, it asserts against this same vector from the other side.)
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { bundle_requirements, bundle_resolve } from "../src/core/clausters_core_web.js";

interface Vectors {
    manifest: unknown;
    template: unknown;
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
await loadCore(await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)));

const vectors = JSON.parse(
    await readFile(new URL("./bundle-vectors.json", here), "utf8"),
) as Vectors;

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
