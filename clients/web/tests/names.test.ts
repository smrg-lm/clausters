// What each module of the package exports, against the Python client's same
// module: `names-vectors.json` is each Python module's `__all__`, spelled for
// this client, with the differences `clients/docs/names.toml` decides
// (`gen-names-vectors.py` writes it). A name only one client has is a
// defect unless that file says why.
//
// Only values are compared, since a type has no name at run time; and a
// name that begins with an underscore is a module's own, shared with the
// package's other modules and not with a program, in both clients.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const root = new URL("..", import.meta.url);
const vectors = JSON.parse(
    readFileSync(new URL("tests/names-vectors.json", root), "utf8"),
) as Record<string, string[]>;
const exportsMap = JSON.parse(readFileSync(new URL("package.json", root), "utf8")).exports as Record<
    string,
    { default: string }
>;

/** The source module a subpath's built file comes from. */
function source(subpath: string): URL {
    const built = exportsMap[subpath]!.default;
    return new URL(built.replace("./dist/", "src/").replace(/\.js$/, ".ts"), root);
}

for (const [subpath, expected] of Object.entries(vectors)) {
    test(`clausters${subpath === "." ? "" : subpath.slice(1)} exports what the Python module does`, async () => {
        const module = (await import(source(subpath).href)) as Record<string, unknown>;
        const exported = Object.keys(module).filter((name) => !name.startsWith("_")).sort();
        const want = new Set(expected);
        const have = new Set(exported);
        assert.deepEqual(
            {
                onlyHere: exported.filter((name) => !want.has(name)),
                onlyInPython: expected.filter((name) => !have.has(name)),
            },
            { onlyHere: [], onlyInPython: [] },
        );
    });
}

test("every subpath is compared or is the platform's own", () => {
    const platform = ["./page", "./runtime"];
    for (const subpath of Object.keys(exportsMap)) {
        assert.ok(subpath in vectors || platform.includes(subpath), `${subpath} is compared with nothing`);
    }
});
