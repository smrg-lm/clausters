// Copies a wasm-bindgen declaration file into its src/ mirror spot with the
// body of `InitOutput` reduced to the module's memory.
//
// That interface lists the module's raw exports, and their order and set are
// the linker's: the same Rust gives them in another order, or with a libc
// helper more or less (`memcmp`, `iswspace`), from one build to the next and
// between a debug and a release build. Nothing in the package reads them (a
// caller that needs an export types it itself), so left in, the versioned
// copies of these files would change on every build with no change to the
// API they declare. What remains is exactly what the Rust declares: the
// classes, the functions and their doc comments.
//
// Usage: node tools/stage-dts.mjs <from.d.ts> <to.d.ts>

import { readFileSync, writeFileSync } from "node:fs";

const [from, to] = process.argv.slice(2);
if (!from || !to) {
    console.error("usage: node tools/stage-dts.mjs <from.d.ts> <to.d.ts>");
    process.exit(2);
}

const lines = readFileSync(from, "utf8").split("\n");
const out = [];
let inside = false;
for (const line of lines) {
    if (!inside && line.startsWith("export interface InitOutput {")) {
        inside = true;
        out.push(line);
        continue;
    }
    if (inside) {
        if (line === "}") {
            inside = false;
            out.push(line);
        } else if (line.trim().startsWith("readonly memory:")) {
            out.push(line);
        }
        continue;
    }
    out.push(line);
}
if (inside) {
    console.error(`${from}: InitOutput is not closed`);
    process.exit(1);
}
writeFileSync(to, out.join("\n"));
