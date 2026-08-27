// The npm package, checked before anyone publishes one.
//
// Publishing is a step nobody rehearses: the mistakes it makes -- a tarball
// with the modules but not the wasm bundles `build.sh` stages, a version that
// drifted from the crate's, an `exports` entry pointing at a file the `files`
// list leaves out -- are all invisible until an install fails somewhere else.
// So the checker `prepublishOnly` runs (`tools/check-package.mjs`) runs here
// too, and `npm pack --dry-run` is read for what the tarball would actually
// contain.
//
// Skips when dist/ has not been built, like the rest of the suite.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { promisify } from "node:util";

const run = promisify(execFile);
const root = new URL("..", import.meta.url).pathname;

const built = await access(`${root}dist/index.js`).then(() => true, () => false);
const skip = built ? false : "dist/ is not built (run ./build.sh)";

test("the working copy is publishable", { skip }, async () => {
    // Exit 0 or the message says what is missing.
    const { stdout } = await run("node", ["tools/check-package.mjs"], { cwd: root });
    assert.match(stdout, /publishable/);
});

test("the tarball carries the runtime and the wasm bundles", { skip }, async () => {
    const { stdout } = await run("npm", ["pack", "--dry-run", "--json"], { cwd: root });
    const files: string[] = JSON.parse(stdout)[0].files.map(
        (f: { path: string }) => f.path,
    );

    for (const path of [
        "dist/index.js",
        "dist/runtime.js",
        "dist/engine/clausters_web_bg.wasm",
        "dist/engine/worklet.js",
        "dist/gui-host/clausters_gui_bg.wasm",
        "dist/core/clausters_core_web_bg.wasm",
        // The vendored pair: the Faust compiler and the engraver. Asserted
        // here and not only in the checker because their absence is the one
        // that does not show -- the package installs and runs without them.
        "dist/vendor/faust/libfaust-wasm.wasm",
        "dist/vendor/verovio/verovio.wasm",
        "COPYING",
    ]) {
        assert.ok(files.includes(path), `${path} is missing from the tarball`);
    }

    // The suites, the pages and the book are not part of the package.
    for (const path of files) {
        assert.ok(
            !path.startsWith("tests/") && !path.startsWith("docs/") &&
                !path.startsWith("examples/"),
            `${path} should not be published`,
        );
    }
});

test("every exports entry points at a file that ships", { skip }, async () => {
    const pkg = JSON.parse(await readFile(`${root}package.json`, "utf8"));
    for (const entry of Object.values(pkg.exports) as Record<string, string>[]) {
        for (const target of Object.values(entry)) {
            await access(`${root}${target.replace(/^\.\//, "")}`);
        }
    }
});
