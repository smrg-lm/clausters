// Every name the book imports from the package exists.
//
// Nothing else reads the code in a book: tsc reads `src/`, and the doc build
// reads links, so a snippet whose import no longer resolves compiles, tests and
// builds clean -- which is how the document chapter came to import an
// `Arrangement` a rename had retired and to edit with a `doc.ARRANGEMENT` that
// was never there. This does not run the snippets (most need an engine, a host
// or a page); it resolves every `import { ... } from "clausters..."` in a
// javascript or typescript block of the book and of the README against the
// built package, through its `exports` map -- what a reader's import goes
// through -- and, where an imported name is itself a namespace, every
// `alias.Name` the same block reads off it.
//
// The Python book has the same check in
// `clients/python/tests/test_book_imports.py`.
//
// Skips when dist/ has not been built, like the rest of the suite.

import assert from "node:assert/strict";
import { access, readdir, readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("..", import.meta.url).pathname;

const built = await access(`${root}dist/index.js`).then(() => true, () => false);
const skip = built ? false : "dist/ is not built (run ./build.sh)";

// The generated API reference is TypeDoc's reading of the source, not prose.
async function pages(dir: string): Promise<string[]> {
    const out: string[] = [];
    for (const entry of await readdir(dir, { withFileTypes: true })) {
        const path = `${dir}/${entry.name}`;
        if (entry.isDirectory()) {
            if (entry.name !== "api") out.push(...await pages(path));
        } else if (entry.name.endsWith(".md")) {
            out.push(path);
        }
    }
    return out;
}

interface Import { page: string; line: number; specifier: string; name: string; member?: string }

async function imports(): Promise<Import[]> {
    const out: Import[] = [];
    const fence = /^```(?:js|javascript|ts|typescript)[^\n]*\n([\s\S]*?)^```/gm;
    const named = /import\s*\{([^}]*)\}\s*from\s*"(clausters(?:\/[\w-]+)?)"/g;
    for (const page of [...await pages(`${root}docs/src`), `${root}README.md`]) {
        const text = await readFile(page, "utf8");
        for (const block of text.matchAll(fence)) {
            const body = block[1];
            const line0 = text.slice(0, block.index! + block[0].indexOf(body)).split("\n").length;
            for (const m of body.matchAll(named)) {
                const line = line0 + body.slice(0, m.index).split("\n").length - 1;
                for (const part of m[1].split(",")) {
                    const spec = part.trim();
                    if (!spec || spec.startsWith("type ")) continue;
                    const [name, alias = name] = spec.split(/\s+as\s+/);
                    const where = { page: page.slice(root.length), line, specifier: m[2], name };
                    out.push(where);
                    for (const use of body.matchAll(new RegExp(`\\b${alias}\\.([A-Za-z_]\\w*)`, "g"))) {
                        out.push({ ...where, member: use[1] });
                    }
                }
            }
        }
    }
    return out;
}

// The page modules define custom elements, which node has no registry for.
function stubTheDom() {
    const g = globalThis as Record<string, unknown>;
    g.HTMLElement ??= class {};
    g.customElements ??= { define() {}, get() {} };
}

test("every name the book imports from the package exists", { skip }, async () => {
    stubTheDom();
    const exportsMap = JSON.parse(await readFile(`${root}package.json`, "utf8")).exports;
    const found = await imports();
    // A pattern that stopped matching would pass every page silently.
    assert.ok(found.length > 50, `only ${found.length} imports read`);
    const missing: string[] = [];
    for (const { page, line, specifier, name, member } of found) {
        const subpath = specifier === "clausters" ? "." : `./${specifier.slice("clausters/".length)}`;
        const target = exportsMap[subpath];
        if (!target) {
            missing.push(`${page}:${line} "${specifier}" is not a subpath the package exports`);
            continue;
        }
        const module = await import(`${root}${target.default}`);
        const value = module[name];
        if (member === undefined) {
            if (!(name in module)) missing.push(`${page}:${line} ${specifier} has no ${name}`);
        } else if (value && typeof value === "object" && value[Symbol.toStringTag] === "Module"
                   && !(member in value)) {
            missing.push(`${page}:${line} ${name}.${member} does not exist`);
        }
    }
    assert.deepEqual(missing, []);
});
