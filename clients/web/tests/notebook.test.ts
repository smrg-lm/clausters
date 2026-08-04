// The notebook widget's pure helpers: buffer handling, asset staging order and
// the blob-URL import rewrite. No DOM and no kernel — the rest of the module
// only runs in a cell, and this is the part that can be wrong in silence.

import assert from "node:assert/strict";
import { test } from "node:test";

import {
    asBytes, assetOrder, importsOf, resolvePath,
    rewriteImports,
} from "../src/notebook/widget.ts";

test("a DataView keeps its bytes", () => {
    // The regression this exists for: ipywidgets delivers comm buffers as
    // DataView, and `new Uint8Array(dataView)` takes the array-like path and
    // yields an EMPTY array — which reaches the host as "bad OSC packet:
    // Empty packet" rather than as a type error.
    const source = new Uint8Array([1, 2, 3, 4, 5]);
    const view = new DataView(source.buffer);
    assert.equal(new Uint8Array(view as never).length, 0, "the trap itself");
    assert.deepEqual([...asBytes(view)], [1, 2, 3, 4, 5]);
});

test("a view onto part of a buffer reads only its own window", () => {
    const source = new Uint8Array([9, 8, 7, 6, 5]);
    const view = new DataView(source.buffer, 1, 3);
    assert.deepEqual([...asBytes(view)], [8, 7, 6]);
});

test("a plain ArrayBuffer still works", () => {
    const source = new Uint8Array([42, 43]);
    assert.deepEqual([...asBytes(source.buffer)], [42, 43]);
});

test("a module is staged after everything it imports", () => {
    // The regression: worklet.js imports worklet-shim.js, and the ordering
    // this replaced ranked them equal by filename, so the importer went first
    // and its specifier was never rewritten -- "Failed to resolve module
    // specifier ./worklet-shim.js", on the audio thread, where it is hardest
    // to see.
    const names = [
        "engine/worklet.js",
        "engine/worklet-shim.js",
        "engine/clausters_web.js",
        "engine/clausters_web_bg.wasm",
    ];
    const sources = new Map([
        ["engine/worklet.js",
         'import "./worklet-shim.js";\nimport { x } from "./clausters_web.js";'],
        ["engine/worklet-shim.js", "export const shim = 1;"],
        ["engine/clausters_web.js", "export const x = 1;"],
    ]);
    const ordered = assetOrder(names, sources);
    const at = (n: string) => ordered.indexOf(n);
    assert.ok(at("engine/worklet-shim.js") < at("engine/worklet.js"));
    assert.ok(at("engine/clausters_web.js") < at("engine/worklet.js"));
    assert.equal(ordered.length, names.length);
});

test("the real asset list orders the engine correctly", () => {
    const ordered = assetOrder(
        ["engine/worklet.js", "engine/worklet-shim.js"],
        new Map([["engine/worklet.js", 'import "./worklet-shim.js";']]),
    );
    assert.deepEqual(ordered, ["engine/worklet-shim.js", "engine/worklet.js"]);
});

test("a cycle does not hang the staging", () => {
    const ordered = assetOrder(["a.js", "b.js"], new Map([
        ["a.js", 'import "./b.js";'],
        ["b.js", 'import "./a.js";'],
    ]));
    assert.equal(ordered.length, 2);
});

test("relative specifiers resolve against the asset's own directory", () => {
    assert.equal(resolvePath("gui-host", "../core/x.js"), "core/x.js");
    assert.equal(resolvePath("gui-host", "./y.js"), "gui-host/y.js");
    assert.equal(resolvePath("", "./z.js"), "z.js");
});

test("an import of a staged asset becomes its blob URL", () => {
    const urls = new Map([["core/clausters_core_web.js", "blob:fake-core"]]);
    const source = `import init from "../core/clausters_core_web.js";\n`
        + `const m = await import('../core/clausters_core_web.js');`;
    const out = rewriteImports(source, "gui-host/clausters_gui.js", urls);
    assert.ok(out.includes('from "blob:fake-core"'));
    assert.ok(out.includes("import('blob:fake-core')"));
    assert.ok(!out.includes("clausters_core_web.js"));
});

test("a side-effect import is rewritten too, and counted as a dependency", () => {
    // `import "./x.js"` has no `from`. The rewriter used to require one while
    // the dependency scanner did not, so the module was staged in the right
    // order and then shipped with its specifier untouched.
    const urls = new Map([["engine/worklet-shim.js", "blob:fake-shim"]]);
    const source = 'import "./worklet-shim.js";';
    assert.equal(rewriteImports(source, "engine/worklet.js", urls),
                 'import "blob:fake-shim";');
    assert.deepEqual(importsOf(source, "engine/worklet.js"),
                     ["engine/worklet-shim.js"]);
});

test("what the scanner finds is what the rewriter rewrites", () => {
    const source = [
        'import "./a.js";',
        'import x from "./b.js";',
        'const y = await import("./c.js");',
        'export { z } from "./d.js";',
    ].join("\n");
    const found = importsOf(source, "e/f.js");
    const urls = new Map(found.map((n, i) => [n, `blob:${i}`]));
    const out = rewriteImports(source, "e/f.js", urls);
    for (const name of found) {
        assert.ok(!out.includes(name.slice(name.lastIndexOf("/") + 1)),
                  `${name} was found but not rewritten`);
    }
});

test("an import of something not staged is left alone", () => {
    const source = `import x from "./not-sent.js";`;
    assert.equal(rewriteImports(source, "gui-host/g.js", new Map()), source);
});

test("the widget module has no value imports of its own", async () => {
    // anywidget serves this one module and nothing beside it, so any static
    // import of a sibling is a specifier the page cannot resolve -- it fails
    // as "Error resolving module specifier" and the view never renders.
    // Everything the module runs arrives over the comm and is imported from a
    // blob URL instead. Type-only imports are erased on emit, so they are fine.
    const { readFile } = await import("node:fs/promises");
    const source = await readFile(
        new URL("../src/notebook/widget.ts", import.meta.url), "utf8");
    const statics = [...source.matchAll(/^import\s+(.*?)\s*from\s*['"](.+?)['"]/gm)];
    const values = statics.filter(([, clause]) => !clause.startsWith("type"));
    assert.deepEqual(values.map(([, , spec]) => spec), [],
        "these must become dynamic imports of a staged asset");
});
