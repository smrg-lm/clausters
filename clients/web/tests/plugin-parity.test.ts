// The plugin catalog against the Python client's.
//
// `plugin-vectors.json` is what the Python client's `PluginCatalog` read from
// a set of scan rows, what its searches found and what references its
// `PluginDef` built (`gen-plugin-vectors.py`). The web client reads the same
// rows through the same core, so every answer must be the same.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { PluginCatalog, PluginDef } from "../src/defs/plugindef.ts";
import type { PluginQuery, PluginRoute } from "../src/defs/plugindef.ts";

interface Answer { value?: unknown; error?: string }
const vectors = JSON.parse(await readFile(new URL("./plugin-vectors.json", import.meta.url), "utf8")) as {
    rows: Record<string, unknown>[];
    catalog: { plugins: Record<string, unknown>[]; errors: unknown[] };
    finds: { name: string | null; query: PluginQuery; result: Answer }[];
    filters: { name: string | null; query: PluginQuery; result: number[] }[];
    references: { plugin: string; options: { inputs?: PluginRoute; outputs?: PluginRoute; state?: number[] }; result: Answer }[];
};
await loadCore();

function answer(call: () => unknown): Answer {
    try { return { value: call() }; } catch (error) { return { error: (error as Error).message }; }
}

test("a catalog reads the same rows as the reference client", () => {
    const catalog = PluginCatalog.fromRows(vectors.rows);
    assert.deepEqual(catalog.plugins.map((p) => p.data()), vectors.catalog.plugins);
    assert.deepEqual(catalog.errors, vectors.catalog.errors);
});

test("searches find and filter as the reference client's", () => {
    const catalog = PluginCatalog.fromRows(vectors.rows);
    for (const { name, query, result } of vectors.finds) {
        assert.deepEqual(answer(() => catalog.plugins.indexOf(catalog.find(name, query))), result, `${name} ${JSON.stringify(query)}`);
    }
    for (const { name, query, result } of vectors.filters) {
        assert.deepEqual(catalog.filter(name, query).map((p) => catalog.plugins.indexOf(p)), result);
    }
});

test("a def's reference is the reference client's", () => {
    const catalog = PluginCatalog.fromRows(vectors.rows);
    for (const { plugin, options, result } of vectors.references) {
        const got = answer(() => new PluginDef("def", catalog.find(plugin, plugin === "Plate" ? { format: "clap" } : {}), {
            inputs: options.inputs ?? null,
            outputs: "outputs" in options ? options.outputs : 0,
            state: options.state ?? null,
        }).reference);
        assert.deepEqual(got, result, `${plugin} ${JSON.stringify(options)}`);
    }
});
