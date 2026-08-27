#!/usr/bin/env node
// Is this working copy publishable as the `clausters` npm package?
//
// Two things a tarball cannot be trusted to carry on its own: the emitted
// modules AND the wasm `build.sh` stages beside them -- the three bundles it
// compiles (a `npm run build` alone leaves them stale or missing, and a
// package without them loads nothing) and the two vendored artifacts it copies
// from `vendor/`, whose absence is quieter and worse, since the package still
// loads and only fails at the def or the score -- and a version that agrees
// with the rest of the repository.
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
// The crates inherit `[workspace.package].version`, so that is where the one
// number is written; `scripts/set-version.sh` writes this file from it and
// `tests/versions.rs` contrasts every manifest that cannot inherit.
const cargo = readFileSync(join(here, "..", "..", "Cargo.toml"), "utf8");
const crateVersion =
    /^\[workspace\.package\][^[]*?^version = "([^"]+)"/ms.exec(cargo)?.[1];
if (!crateVersion) {
    problems.push("could not read [workspace.package].version from Cargo.toml");
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
    // The two vendored wasm artifacts, staged by build.sh from
    // clients/web/vendor. They are not compiled from our sources and they have
    // no usable published build, so they are built from pins by
    // third_party/build-{faust,verovio}-wasm.sh -- and because build.sh only
    // *notes* their absence, a package can be emitted without them and looks
    // complete: it loads, it plays, and then a Faust def will not compile and a
    // score will not engrave, on the user's machine and not here. That is what
    // this pair is for. In CI they come from .github/actions/wasm-vendor.
    "dist/vendor/faust/libfaust-wasm.js",
    "dist/vendor/faust/libfaust-wasm.wasm",
    "dist/vendor/faust/libfaust-wasm.data",
    "dist/vendor/verovio/verovio.js",
    "dist/vendor/verovio/verovio.wasm",
    // The clock's tick worker: loaded by URL into a scope of its own, so it is
    // never reached through the module graph a package check would follow.
    "dist/base/tick-worker.js",
    // The licence travels with the code.
    "COPYING",
    "README.md",
];
for (const path of required) {
    if (!existsSync(join(here, path))) {
        const how = path.startsWith("dist/vendor/")
            ? "run third_party/build-" +
              (path.includes("/faust/") ? "faust" : "verovio") +
              "-wasm.sh, then ./build.sh"
            : "run ./build.sh";
        problems.push(`missing from the package: ${path} (${how})`);
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
