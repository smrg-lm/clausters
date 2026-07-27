// The slim run-time entry's one invariant, asserted rather than hoped for.
//
// `dist/runtime.js` is what a page that *mounts* a bundle loads: the engine,
// the host, the OSC codec and the mount. A mounted bundle is data — the
// builders ran in the authoring script — so the def builders (`defs/`), the
// GuiDef builders (`gui/guidef.js`) and the sequencing layer (`seq/`) have no
// run-time use, and shipping them to every reader of a page that embeds an
// instrument is exactly the weight this entry exists to avoid.
//
// The check walks the **emitted** module graph (dist/, after `npm run build`),
// not the sources: an import added anywhere along the chain — the entry, the
// element, the mount, the page host — shows up here. It skips when dist/ has
// not been built, so `npm test` stays runnable from a fresh checkout.

import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import test from "node:test";

const dist = new URL("../dist/", import.meta.url).pathname;
const entry = `${dist}runtime.js`;

/// The module specifiers `runtime.js` must never reach, transitively.
const FORBIDDEN = [
    { what: "the def builders", match: (p: string) => p.startsWith("defs/") },
    { what: "the GuiDef builders", match: (p: string) => p === "gui/guidef.js" },
    { what: "the sequencing layer", match: (p: string) => p.startsWith("seq/") },
];

/// Every specifier `source` imports — static and dynamic alike (the host's
/// wasm glue is loaded with a dynamic `import()`, and that counts).
function importsOf(source: string): string[] {
    const out: string[] = [];
    const patterns = [
        /(?:^|\n)\s*(?:import|export)[\s\S]*?from\s*["']([^"']+)["']/g,
        /(?:^|\n)\s*import\s*["']([^"']+)["']/g,
        /\bimport\s*\(\s*["']([^"']+)["']\s*\)/g,
    ];
    for (const pattern of patterns) {
        for (const [, spec] of source.matchAll(pattern)) out.push(spec);
    }
    return out;
}

/// The transitive closure of `entry`, as paths relative to dist/. Only
/// relative specifiers are followed; a bare one would be a package import,
/// which this package does not have.
async function moduleGraph(entry: string): Promise<Set<string>> {
    const seen = new Set<string>();
    const queue = [entry];
    while (queue.length > 0) {
        const file = queue.pop()!;
        if (seen.has(file)) continue;
        seen.add(file);
        let source: string;
        try {
            source = await readFile(file, "utf8");
        } catch {
            continue; // a .wasm or an absent artifact: nothing to walk
        }
        for (const spec of importsOf(source)) {
            if (!spec.startsWith(".")) continue;
            queue.push(resolve(dirname(file), spec));
        }
    }
    return new Set([...seen].map((f) => relative(dist, f)));
}

test("the run-time entry never reaches the authoring layers", async (t) => {
    try {
        await access(entry);
    } catch {
        return t.skip("dist/runtime.js is not built (run ./build.sh)");
    }
    const graph = await moduleGraph(entry);
    // Sanity: the walk actually followed something, so an empty graph cannot
    // pass the exclusions by accident.
    assert.ok(graph.has("elements.js"), "the entry reaches the element");
    assert.ok(graph.has("bundle.js"), "the entry reaches the mount");
    assert.ok(graph.has("engine/server.js"), "the entry reaches the engine");

    for (const { what, match } of FORBIDDEN) {
        const reached = [...graph].filter(match);
        assert.deepEqual(
            reached,
            [],
            `the run-time entry reaches ${what}: ${reached.join(", ")}`,
        );
    }
});

test("the full facade does reach them — that is the difference", async (t) => {
    const facade = `${dist}index.js`;
    try {
        await access(facade);
    } catch {
        return t.skip("dist/index.js is not built (run ./build.sh)");
    }
    const graph = await moduleGraph(facade);
    assert.ok(
        [...graph].some((p) => p.startsWith("defs/")),
        "the package facade carries the def builders",
    );
    assert.ok(graph.has("gui/guidef.js"), "the package facade carries the GuiDef builders");
});
