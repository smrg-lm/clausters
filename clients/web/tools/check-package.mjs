#!/usr/bin/env node
// Is this working copy publishable as the `clausters` npm package?
//
// Two things a tarball cannot be trusted to carry on its own: the emitted
// modules AND the three wasm bundles `build.sh` stages beside them (a `npm
// run build` alone leaves them stale or missing, and a package without them
// loads nothing), and a version that agrees with the rest of the repository.
// `prepublishOnly` runs this, so a publish that would ship either mistake
// stops here; `tests/package.test.ts` runs it too, so the check is exercised
// long before anyone publishes.
//
// Usage:  node tools/check-package.mjs        (from clients/web/)

import { readFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(dirname(fileURLToPath(import.meta.url)));
const problems = [];

const pkg = JSON.parse(readFileSync(join(here, "package.json"), "utf8"));

// --- the version, against the workspace's ---
//
// The package, the crate and the Python wheel are one release: the repo's
// SemVer. (The binary ABIs are counted separately -- see the root CLAUDE.md.)
const cargo = readFileSync(join(here, "..", "..", "Cargo.toml"), "utf8");
const crateVersion = /^\[package\][^[]*?^version = "([^"]+)"/ms.exec(cargo)?.[1];
if (!crateVersion) {
    problems.push("could not read the workspace crate version from Cargo.toml");
} else if (crateVersion !== pkg.version) {
    problems.push(
        `package.json is ${pkg.version} but the crate is ${crateVersion}: ` +
            "one release, one version",
    );
}

// --- what the tarball has to contain ---
const required = [
    // The two entry points and their declarations.
    "dist/index.js",
    "dist/index.d.ts",
    "dist/runtime.js",
    "dist/runtime.d.ts",
    // The wasm bundles: the engine, the GUI host and the shared core.
    "dist/engine/clausters_web.js",
    "dist/engine/clausters_web_bg.wasm",
    "dist/engine/worklet.js",
    "dist/gui-host/clausters_gui.js",
    "dist/gui-host/clausters_gui_bg.wasm",
    "dist/core/clausters_core_web.js",
    "dist/core/clausters_core_web_bg.wasm",
    // The licence travels with the code.
    "COPYING",
    "README.md",
];
for (const path of required) {
    if (!existsSync(join(here, path))) {
        problems.push(`missing from the package: ${path} (run ./build.sh)`);
    }
}

// --- the `files` list has to cover them ---
const covered = (path) =>
    (pkg.files ?? []).some((entry) =>
        entry.endsWith("/") ? path.startsWith(entry) : entry === path
    );
for (const path of required) {
    if (!covered(path)) {
        problems.push(`package.json "files" does not include ${path}`);
    }
}

if (problems.length > 0) {
    console.error("the package is not publishable:");
    for (const problem of problems) console.error(`  - ${problem}`);
    process.exit(1);
}
console.log(`clausters ${pkg.version}: publishable (dist/ complete)`);
