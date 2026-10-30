// An offline render written to its file a run at a time, reading the files
// its score reads the moment it needs them (`src/engine/file-render.ts`).
//
// Under node the files are the disk, read and written through `fs`; in a page
// they are the page's storage, through sync access handles in a Worker of the
// render's own (`tests/disk.html`). The render is the engine's either way, so
// what is checked here is what both places get:
//
//  - a render to a file is the render in memory, sample for sample;
//  - a file streamed by `DiskIn` in a render is read from its first frame --
//    a stream reads inside the block that needs it, as a native render's
//    waits for its thread, so nothing is played as silence.

import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { OscNrtInterface } from "../src/base/connection.ts";
import type { OscArg } from "../src/base/osc.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { diskIn, diskOut, out, sine } from "../src/defs/ugens/index.ts";
import { decodeSoundfile } from "../src/engine/codec.ts";
import { loadRenderer } from "../src/engine/render.ts";
import { renderScore } from "../src/render.ts";
import { runSteps } from "../src/steps.ts";
import { StepRunner } from "../src/core/clausters_core_web.js";
import type { Server } from "../src/defs/server/index.ts";

const here = new URL(".", import.meta.url);
await loadCore();
await loadRenderer(await readFile(new URL("../dist/engine/clausters_web_bg.wasm", here)));

const RATE = 48_000;

/** A mono float WAV of `frames` samples, every one `value`. */
function constant(value: number, frames: number): Uint8Array {
    const bytes = new Uint8Array(44 + frames * 4);
    const view = new DataView(bytes.buffer);
    const ascii = (at: number, text: string) =>
        [...text].forEach((c, i) => (bytes[at + i] = c.charCodeAt(0)));
    ascii(0, "RIFF"); view.setUint32(4, 36 + frames * 4, true); ascii(8, "WAVE");
    ascii(12, "fmt "); view.setUint32(16, 16, true); view.setUint16(20, 3, true);
    view.setUint16(22, 1, true); view.setUint32(24, RATE, true);
    view.setUint32(28, RATE * 4, true); view.setUint16(32, 4, true);
    view.setUint16(34, 32, true); ascii(36, "data"); view.setUint32(40, frames * 4, true);
    for (let i = 0; i < frames; i++) view.setFloat32(44 + i * 4, value, true);
    return bytes;
}

const s = (v: string): OscArg => ["s", v];
const i = (v: number): OscArg => ["i", v];

test("a render to a file is the render in memory, sample for sample", async () => {
    const dir = await mkdtemp(join(tmpdir(), "clausters-file-render-"));
    try {
        const score = () => {
            const nrt = new OscNrtInterface();
            const def = new SynthDef("tone", out(0, sine(660).mul(0.5)));
            nrt.addBundle(0, [
                { addr: "/def_send", args: [s("synth"), s(def.dumpDef())] },
                { addr: "/synth_new", args: [s("tone"), i(1000), i(0), i(1)] },
            ]);
            nrt.addBundle(0.75, [{ addr: "/node_free", args: [i(1000)] }]);
            return nrt.score.bytes();
        };
        const path = join(dir, "tone.w64");
        const inMemory = await renderScore(score(), { sampleRate: RATE, channels: 1, seed: 7 });
        const written = await renderScore(score(), { sampleRate: RATE, channels: 1, seed: 7, path });
        assert.equal(written.samples, null);
        assert.equal(written.frames, inMemory.frames);
        const back = await decodeSoundfile(await readFile(path), path, 0, -1);
        assert.equal(back.frames, inMemory.frames);
        assert.deepEqual([...back.samples], [...inMemory.samples!]);
        assert.deepEqual(written.peak, inMemory.peak);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a file a render streams is read from its first frame", async () => {
    const dir = await mkdtemp(join(tmpdir(), "clausters-file-render-"));
    try {
        const a = join(dir, "a.wav");
        await writeFile(a, constant(0.25, 48_000));
        const def = new SynthDef("stream", out(0, diskIn(1, { path: a })));
        const nrt = new OscNrtInterface();
        nrt.addBundle(0, [
            { addr: "/def_send", args: [s("synth"), s(def.dumpDef())] },
            { addr: "/synth_new", args: [s("stream"), i(1000), i(0), i(0)] },
        ]);
        nrt.addBundle(0.5, [{ addr: "/node_free", args: [i(1000)] }]);
        const path = join(dir, "out.wav");
        const stats = await renderScore(nrt.score.bytes(), { sampleRate: RATE, channels: 1, path });
        assert.equal(stats.frames, 24_000);
        const back = await decodeSoundfile(await readFile(path), path, 0, -1);
        const samples = [...back.samples];
        assert.equal(samples[0], 0.25, "the first frame is the file's, not silence");
        assert.ok(samples.every((v) => v === 0.25), "the file throughout");
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a render step writes its file and releases what follows it", async () => {
    const dir = await mkdtemp(join(tmpdir(), "clausters-file-render-"));
    try {
        const nrt = new OscNrtInterface();
        const def = new SynthDef("tone", out(0, sine(660).mul(0.5)));
        nrt.addBundle(0, [
            { addr: "/def_send", args: [s("synth"), s(def.dumpDef())] },
            { addr: "/synth_new", args: [s("tone"), i(1000), i(0), i(0)] },
        ]);
        nrt.addBundle(0.5, [{ addr: "/node_free", args: [i(1000)] }]);
        const score = [...nrt.score.bytes()].map((b) => b.toString(16).padStart(2, "0")).join("");
        const path = join(dir, "tone.w64");
        const sent: string[] = [];
        const server = { sendMsg: (addr: string) => sent.push(addr) } as unknown as Server;
        await runSteps(server, new StepRunner(), [
            { send: { addr: "/before", args: [] } },
            { render: { score, path, rate: RATE, channels: 1, format: "float" } },
            { send: { addr: "/after", args: [] } },
        ]);
        assert.deepEqual(sent, ["/before", "/after"]);
        const back = await decodeSoundfile(await readFile(path), path, 0, -1);
        assert.equal(back.frames, 24_000);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});

test("a render with no path keeps no output and does its work", async () => {
    const dir = await mkdtemp(join(tmpdir(), "clausters-file-render-"));
    try {
        const written = join(dir, "tone.wav");
        const nrt = new OscNrtInterface();
        const def = new SynthDef("recorded", diskOut(sine(660), { path: written, format: "float" }));
        nrt.addBundle(0, [
            { addr: "/def_send", args: [s("synth"), s(def.dumpDef())] },
            { addr: "/synth_new", args: [s("recorded"), i(1000), i(0), i(0)] },
        ]);
        nrt.addBundle(0.5, [{ addr: "/node_free", args: [i(1000)] }]);
        const score = [...nrt.score.bytes()].map((b) => b.toString(16).padStart(2, "0")).join("");
        const { renderStep } = await import("../src/steps.ts");
        await renderStep({ score, path: "", rate: RATE, channels: 1, format: "float" });
        const head = new TextDecoder().decode((await readFile(written)).subarray(0, 4));
        assert.equal(head, "RIFF");
        // what the DiskOut wrote, and no rendered file
        const { readdir } = await import("node:fs/promises");
        assert.deepEqual(await readdir(dir), ["tone.wav"]);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
});
