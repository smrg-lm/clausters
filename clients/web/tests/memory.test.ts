// The engine's linear memory is reserved, not grown into.
//
// `WebAssembly.Memory.grow` detaches the `ArrayBuffer` and every JS view over
// it, and in a page it happens on the audio thread: the OSC pump runs inside
// the worklet's `process`, so a command that allocates allocates there. The
// engine therefore reserves its memory at link time
// (`crates/clausters-web/build.rs`) and the numbers are asserted here rather
// than trusted, because nothing else would notice them going away — the flags
// pass through `wasm-bindgen`, which rewrites the module.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import * as engine from "../dist/engine/clausters_web.js";

const PAGE = 65536;
const RESERVED = 16 * 1024 * 1024;
const CEILING = 256 * 1024 * 1024;

const bytes = readFileSync(
    new URL("../dist/engine/clausters_web_bg.wasm", new URL(".", import.meta.url)),
);
const wasm = engine.initSync({ module: bytes });

/**
 * The declared limits of the module's one memory, read out of the binary's
 * own section — the instance can only report what it currently has, so a
 * missing ceiling is invisible from the JS side.
 */
function limits(module: Uint8Array): { min: number; max: number | null } {
    let at = 8; // the magic and the version
    const leb = () => {
        let value = 0;
        let shift = 0;
        for (;;) {
            const byte = module[at++];
            value |= (byte & 0x7f) << shift;
            shift += 7;
            if (!(byte & 0x80)) return value;
        }
    };
    while (at < module.length) {
        const id = module[at++];
        const size = leb();
        const end = at + size;
        if (id === 5) {
            assert.equal(leb(), 1, "the engine declares exactly one memory");
            const flags = module[at++];
            const min = leb();
            return { min, max: flags & 1 ? leb() : null };
        }
        at = end;
    }
    throw new Error("the engine's wasm has no memory section");
}

test("the engine reserves its memory and declares a ceiling", () => {
    const { min, max } = limits(bytes);
    assert.equal(min * PAGE, RESERVED);
    assert.equal(max === null ? null : max * PAGE, CEILING);
});

test("booting and running the engine does not grow it", () => {
    assert.equal(wasm.memory.buffer.byteLength, RESERVED);
    const server = new engine.WebServer(48000, 2, 0);
    assert.equal(wasm.memory.buffer.byteLength, RESERVED, "the boot fits the reservation");
    const out = new Float32Array(server.block_frames() * 2);
    for (let i = 0; i < 1000; i++) {
        server.process(out);
        server.poll();
    }
    assert.equal(wasm.memory.buffer.byteLength, RESERVED, "an idle engine allocates nothing");
});
