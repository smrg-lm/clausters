// The data paths end to end against a real `clausters --ws` server.
//
// `data.test.ts` asserts the decoding and the analysis with no server at all;
// what this suite adds is the half only a running server can answer for: that
// a subscription really arrives at ~the period asked for, that a tap carries
// the samples a synth is writing, and that a whole buffer reads back through
// the chunked bulk path.
//
// Needs the debug server built (`cargo build` at the workspace root) and the
// core wasm staged (`./build.sh`). Skips (does not fail) when the binary is
// missing, so `npm test` stays runnable from a source tree without a build.

import assert from "node:assert/strict";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { spawnChild } from "./child.ts";

import { WsConnection } from "../src/base/connection.ts";
import { loadCore } from "../src/base/core.ts";
import { Bus } from "../src/defs/bus.ts";
import { Buffer } from "../src/defs/buffer.ts";
import { Session } from "../src/multitrack.ts";
import { Synth } from "../src/defs/node.ts";
import { Server } from "../src/defs/server/index.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { control, out, outCtl, sine } from "../src/defs/ugens/index.ts";
import { BusStream, Peaks, TapStream } from "../src/data/index.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57991; // out of the default range, one per suite
// The server's own base OSC port (`--port`): UDP and TCP alike. Distinct
// per suite, so these servers are independent processes rather than one
// machine-wide singleton.
const udpPort = 57891;

const hasServer = await access(serverBin).then(() => true, () => false);

await loadCore();

async function withServer(body: (server: Server) => Promise<void>): Promise<void> {
    const child = spawnChild(serverBin, ["--port", String(udpPort), "--ws", String(wsPort),
        "--no-tcp", "--no-persist"]);
    let connection: WsConnection | null = null;
    let server: Server | null = null;
    try {
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "server WS endpoint never came up");
        server = await new Server({ connection }).attach();
        await body(server);
    } finally {
        server?.close();
        connection?.close();
        child.stop();
        await sleep(50);
    }
}

test("a control bus streams to the client at the period it asked for", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const bus = Bus.control(1, { server });
        // A steady value, written by the client rather than by a synth: what
        // is under test is the stream, not what feeds the bus.
        bus.set(0.75);
        await server.sync();

        const stream = await BusStream.open(server, [bus], { periodMs: 10 });
        assert.deepEqual(stream.buses, [bus.index]);
        await sleep(300);
        assert.ok(stream.snapshots > 5, `only ${stream.snapshots} snapshots in 300 ms`);
        assert.ok(
            Math.abs(stream.value(bus) - 0.75) < 1e-6,
            `streamed ${stream.value(bus)}`,
        );

        // The value the stream shows is the value `/bus_get` answers with.
        bus.set(-0.2);
        await sleep(150);
        assert.ok(Math.abs(stream.value(bus) - (await bus.get())) < 1e-6);

        const before = stream.snapshots;
        await stream.stop();
        await sleep(150);
        assert.equal(stream.snapshots, before, "a cancelled stream stops arriving");
        bus.free();
    });
});

test("a lfo on a bus reaches the client through the stream", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const bus = Bus.control(1, { server });
        await new SynthDef("ts_data_lfo", outCtl(control("bus", 0.0), sine(2.0))).send(server);
        new Synth("ts_data_lfo", { bus: bus.index }, { server });
        await server.sync();

        const stream = await BusStream.open(server, [bus], { periodMs: 10 });
        const seen: number[] = [];
        stream.onSnapshot((values) => seen.push(values[0]));
        await sleep(600); // more than one period of a 2 Hz oscillator
        await stream.stop();

        assert.ok(seen.length > 20, `only ${seen.length} snapshots`);
        const lo = Math.min(...seen);
        const hi = Math.max(...seen);
        assert.ok(hi > 0.5 && lo < -0.5, `the LFO swings: [${lo}, ${hi}]`);
    });
});

test("a tap carries the samples a synth is writing", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const info = await server.queryInfo();
        if (info.taps === 0) return; // a server built with no tap region

        const bus = Bus.audio(1, { server });
        await new SynthDef("ts_data_tone", out(control("bus", 0.0), sine(400.0).mul(0.5))).send(server);
        new Synth("ts_data_tone", { bus: bus.index }, { server });
        await server.sync();
        await sleep(200); // let the ring fill a window

        // The subscription is the watch: naming the bus is all it takes.
        const stream = await TapStream.open(server, [bus.index], {
            frames: 1024,
            periodMs: 20,
        });
        await sleep(300);
        const window = stream.window(bus.index);
        assert.ok(window, "no /bus_tapStream.reply arrived");
        assert.ok(window.samples.length > 0);
        assert.ok(window.endPosition > 0, "the window carries its stream position");

        const peak = Math.max(...window.samples.map(Math.abs));
        assert.ok(peak > 0.3 && peak <= 0.6, `the tone's amplitude: ${peak}`);

        // A second snapshot advances the tap's own sample axis.
        const first = window.endPosition;
        await sleep(150);
        assert.ok(stream.window(bus.index)!.endPosition > first, "the axis advances");

        await stream.stop();
        bus.free();
    });
});

test("a generated buffer reads back in chunks, and reduces", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        // Filled by the server (`/buffer_gen`); the write direction is covered
        // by the read/edit/write test below.
        const frames = 5000;
        const buffer = await Buffer.alloc(frames, 1, { server });
        await buffer.gen("sine1", [["i", 7], 1.0]);

        const read = await buffer.getSamples();
        assert.equal(read.length, frames, "the whole buffer came back");
        const peak = Math.max(...read.map(Math.abs));
        assert.ok(peak > 0.9, `a normalized sine peaks at ~1, not ${peak}`);

        // A slice reads as a slice, and it is the same data as the whole.
        const slice = await buffer.getSamples({ start: 1000, count: 256 });
        assert.equal(slice.length, 256);
        for (let i = 0; i < 256; i += 37) {
            assert.ok(Math.abs(slice[i] - read[1000 + i]) < 1e-6, `sample ${i}`);
        }

        // A read past the end returns what the buffer holds, not a hang.
        const tail = await buffer.getSamples({ start: frames - 10, count: 100 });
        assert.equal(tail.length, 10);

        // And it reduces to the cache a waveform view is drawn from — the
        // whole of what a client does with a pyramid, since the picture over
        // it is the host's.
        const peaks = Peaks.build(read, { baseBucket: 256 });
        assert.equal(peaks.frames, frames);
        assert.ok(peaks.numLevels > 1, "5000 frames at 256 has levels above 0");
        assert.ok(peaks.toBytes().length > 0);
        peaks.free();

        buffer.free();
    });
});

test("a client writes samples and reads back exactly what it wrote", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const buffer = await Buffer.alloc(8, 1, { server });

        await buffer.setSamples([0.1, 0.2, 0.3, 0.4], { start: 2 });
        await buffer.setSample(0, -0.5);
        const read = Array.from(await buffer.getSamples());
        const expected = [-0.5, 0, 0.1, 0.2, 0.3, 0.4, 0, 0];
        for (let i = 0; i < expected.length; i++) {
            assert.ok(Math.abs(read[i]! - expected[i]!) < 1e-6, `sample ${i}: ${read[i]}`);
        }

        // Read, edit, write back -- the cycle an editor view makes.
        await buffer.setSamples(read.map((v) => v * 2));
        const again = await buffer.getSamples();
        for (let i = 0; i < expected.length; i++) {
            assert.ok(Math.abs(again[i]! - expected[i]! * 2) < 1e-6, `edited ${i}`);
        }

        // Chunking is transparent: several round trips, one result.
        await buffer.setSamples(new Float32Array(8).fill(1), { chunk: 3 });
        const filled = await buffer.getSamples();
        assert.ok(filled.every((v) => Math.abs(v - 1) < 1e-6), "chunked write landed whole");

        // A write past the end is refused, and refusing it changes nothing.
        await assert.rejects(
            buffer.setSamples([1, 1, 1], { start: 6 }),
            "a write past the end must not be clamped",
        );

        buffer.free();
    });
});

test("samples the client holds become a buffer over a socket too", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        // `fromSamples` is one call in both clients and works on every carrier.
        // Over the in-page engine it is a copy into shared memory; here there
        // is no shared memory, so it goes the way the Python client's
        // `from_samples` always goes -- `setSamples`' blob runs. Same call,
        // same result, which is the whole point of testing it *here*.
        const stereo = new Float32Array([0.1, -0.1, 0.2, -0.2, 0.3, -0.3]);
        const buffer = await Buffer.fromSamples(stereo, 2, 44100.0, { server });

        assert.equal(buffer.frames, 3, "interleaved: three frames of two channels");
        assert.equal(buffer.channels, 2);
        assert.equal(buffer.sampleRate, 44100.0);
        const read = await buffer.getSamples();
        for (let i = 0; i < stereo.length; i++) {
            assert.ok(Math.abs(read[i]! - stereo[i]!) < 1e-6, `sample ${i}: ${read[i]}`);
        }

        buffer.free();
    });
});

test("a join plays as one buffer and says what it is made of", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        // The same test the Python client runs, through the same two calls: a
        // cut assembled from two takes is one buffer, and asking what it is
        // made of is how a view learns it may not offer an editable waveform
        // over it.
        const one = await Buffer.fromSamples(
            new Float32Array([1.0, 2.0, 3.0, 4.0]), 1, 0, { server });
        const two = await Buffer.fromSamples(
            new Float32Array([5.0, 6.0, 7.0, 8.0]), 1, 0, { server });

        // The second take's tail, then the first take's head: one take out of
        // order is the same mechanism as two takes.
        const join = await Buffer.stitch(
            [{ source: two, start: 2, frames: 2 }, { source: one, start: 0, frames: 2 }],
            { server },
        );
        assert.equal(join.frames, 4);
        assert.equal(join.channels, 1);
        const read = await join.getSamples({ start: 0, count: 4 });
        assert.deepEqual(Array.from(read), [7.0, 8.0, 1.0, 2.0]);

        const parts = await join.parts();
        assert.deepEqual(
            parts.map((p) => [p.source, p.start, p.frames]),
            [[two.bufnum, 2, 2], [one.bufnum, 0, 2]],
        );
        assert.deepEqual(parts[0]!.channels, [0], "every part spells its whole map");

        // A buffer that owns its samples answers with no parts, which is the
        // answer to "is this a join" rather than a refusal.
        assert.deepEqual(await one.parts(), []);

        // And a join is read, never written: it is replaced instead.
        await assert.rejects(() => join.setSamples(new Float32Array([0.0])));

        join.free();
        one.free();
        two.free();
    });
});

test("a saved session loads its takes and then its join", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        // The same test the Python client runs: reopening a session is reading
        // it and loading it, and the join plays the spans the document states.
        const folder = await mkdtemp(join(tmpdir(), "clausters-load-"));
        try {
            for (const [name, samples] of [
                ["one.wav", [1.0, 2.0, 3.0, 4.0]],
                ["two.wav", [5.0, 6.0, 7.0, 8.0]],
            ] as const) {
                const written = await Buffer.fromSamples(new Float32Array(samples), 1, 0, { server });
                await written.write(join(folder, name), { sampleFormat: "float" });
                written.free();
            }

            const window = (region: number, source: number) => ({
                id: region, position: 0.0, length: 1.0,
                content: { fill: "window", window: {
                    source: { source, lifetime: "session", generation: 0 },
                    start: 0.0, duration: 1.0 } },
            });
            const span = (source: number, start: number, end: number) => ({
                source: { source, lifetime: "session", generation: 0, range: { start, end } },
            });
            const take = (name: string) => ({
                location: { at: "file", path: name }, lifetime: "session", channels: 1, frames: 4,
            });
            const saved = Session.read({
                format: 2,
                multitrack: { tracks: [{ id: 10, name: "t", lanes: [
                    { id: 11, regions: [window(20, 3), window(21, 4)] }] }] },
                sources: {
                    "1": take("one.wav"),
                    "2": take("two.wav"),
                    "3": { location: { at: "segments", parts: [span(2, 2, 4), span(1, 0, 2)] },
                           lifetime: "session", channels: 1, frames: 4 },
                    "4": { location: { at: "volatile" }, lifetime: "temporary" },
                },
            });

            const before = server.buffers.inUse;
            const loaded = await saved.load(server, { beside: folder });
            assert.deepEqual([...loaded.keys()].sort(), [1, 2, 3], "the takes only the join reads load too");
            assert.equal(server.buffers.inUse, before + 3, "what the load did not take is given back");
            assert.equal(loaded.get(1)!.frames, 4);
            assert.equal(loaded.get(1)!.path, join(folder, "one.wav"));

            const joined = loaded.get(3)!;
            assert.deepEqual(Array.from(await joined.getSamples({ start: 0, count: 4 })), [7.0, 8.0, 1.0, 2.0]);
            assert.deepEqual(
                (await joined.parts()).map((p) => [p.source, p.start, p.frames]),
                [[loaded.get(2)!.bufnum, 2, 2], [loaded.get(1)!.bufnum, 0, 2]],
            );
            for (const buffer of loaded.values()) buffer.free();

            // A file that is not there is the server's refusal of its read, and
            // the load leaves nothing of itself behind.
            await rm(join(folder, "two.wav"));
            const inUse = server.buffers.inUse;
            await assert.rejects(() => saved.load(server, { beside: folder }));
            assert.equal(server.buffers.inUse, inUse);
        } finally {
            await rm(folder, { recursive: true, force: true });
        }
    });
});
