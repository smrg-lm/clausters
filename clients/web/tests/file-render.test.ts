// An offline render written to its file a run at a time, reading the files
// its score reads the moment it needs them (`src/engine/file-render.ts`).
//
// Under node the files are the disk, read and written through `fs`; in a page
// they are the page's storage, through sync access handles in a Worker of the
// render's own (`tests/disk.html`). The render is the engine's either way, so
// what is checked here is what both places get:
//
//  - a render to a file is the render in memory, sample for sample;
//  - a list of two files played by `TransportDiskIn` in a render is read from
//    its first frame -- a stream reads inside the block that needs it, as a
//    native render's waits for its thread, so nothing is played as silence --
//    and the `PeaksOut` beside it writes its file.

import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { OscNrtInterface } from "../src/base/connection.ts";
import type { OscArg } from "../src/base/osc.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { out, peaksOut, sine, transportDiskIn } from "../src/defs/ugens/index.ts";
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
const f = (v: number): OscArg => ["f", v];

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

test("a list of files is read from its first frame, and its peaks are written", async () => {
    const dir = await mkdtemp(join(tmpdir(), "clausters-file-render-"));
    try {
        const a = join(dir, "a.wav");
        const b = join(dir, "b.wav");
        await writeFile(a, constant(0.25, 12_000));
        await writeFile(b, constant(-0.5, 12_000));
        const peaks = join(dir, "list.peaks");
        const reader = transportDiskIn(1, 0, 0, 0, 0);
        const def = new SynthDef("list", out(0, reader), peaksOut(reader, { path: peaks }));
        const part = (path: string): OscArg[] =>
            [s(path), i(0), i(12_000), i(0), i(0), i(0), f(0), i(0), f(0), i(0)];
        const nrt = new OscNrtInterface();
        nrt.addBundle(0, [
            { addr: "/buffer_stitch", args: [i(1), i(1), f(RATE), ...part(a), ...part(b)] },
            { addr: "/def_send", args: [s("synth"), s(def.dumpDef())] },
            { addr: "/group_new", args: [i(100), i(1), i(0)] },
            { addr: "/transport_group", args: [i(0), i(100)] },
            { addr: "/transport_play", args: [i(0)] },
            { addr: "/synth_new", args: [s("list"), i(1000), i(0), i(100)] },
        ]);
        nrt.addBundle(0.5, [{ addr: "/node_free", args: [i(1000)] }]);
        const path = join(dir, "out.wav");
        const stats = await renderScore(nrt.score.bytes(), { sampleRate: RATE, channels: 1, path });
        assert.equal(stats.frames, 24_000);
        const back = await decodeSoundfile(await readFile(path), path, 0, -1);
        const samples = [...back.samples];
        assert.equal(samples[0], 0.25, "the first frame is the list's, not silence");
        assert.ok(samples.slice(0, 12_000).every((v) => v === 0.25), "the first file throughout");
        assert.ok(samples.slice(12_000).every((v) => v === -0.5), "then the second");
        const head = new TextDecoder().decode((await readFile(peaks)).subarray(0, 4));
        assert.equal(head, "CLPK");
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
