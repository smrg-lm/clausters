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
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadCore } from "../src/base/core.ts";
import { loadOsc } from "../src/base/osc.ts";
import { Bus } from "../src/defs/bus.ts";
import { Buffer } from "../src/defs/buffer.ts";
import { Synth } from "../src/defs/node.ts";
import { Server } from "../src/defs/server.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { control, out, outCtl, sine } from "../src/defs/ugens.ts";
import { BusStream, Peaks, TapStream, scopeFrames, scopeWindow } from "../src/data/index.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57991; // its own port: the suites run one server at a time

const hasServer = await access(serverBin).then(() => true, () => false);

const wasm = await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here));
await loadOsc(wasm);
await loadCore(wasm);

async function withServer(body: (server: Server) => Promise<void>): Promise<void> {
    const process = spawn(
        serverBin,
        ["--ws", String(wsPort), "--no-tcp", "--no-persist"],
        { stdio: "ignore" },
    );
    let connection: WsConnection | null = null;
    let server: Server | null = null;
    try {
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "server WS endpoint never came up");
        server = await Server.open(connection);
        await body(server);
    } finally {
        server?.close();
        connection?.close();
        process.kill();
        await sleep(50);
    }
}

test("a control bus streams to the client at the period it asked for", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const bus = Bus.control(server);
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

        // The value the stream shows is the value `/c_get` answers with.
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
        const bus = Bus.control(server);
        await new SynthDef("ts_data_lfo", outCtl(control("bus", 0.0), sine(2.0))).send(server);
        Synth.new(server, "ts_data_lfo", { bus: bus.index });
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

        const bus = Bus.audio(server);
        await new SynthDef("ts_data_tone", out(control("bus", 0.0), sine(400.0).mul(0.5))).send(server);
        Synth.new(server, "ts_data_tone", { bus: bus.index });
        await server.sync();
        await sleep(200); // let the ring fill a window

        const frames = scopeFrames(10.0, info.nominalSampleRate);
        // The subscription is the watch: naming the bus is all it takes.
        const stream = await TapStream.open(server, [bus.index], { frames, periodMs: 20 });
        await sleep(300);
        const window = stream.window(bus.index);
        assert.ok(window, "no /tap_data arrived");
        assert.ok(window.samples.length > 0);
        assert.ok(window.endPosition > 0, "the window carries its stream position");

        const peak = Math.max(...window.samples.map(Math.abs));
        assert.ok(peak > 0.3 && peak <= 0.6, `the tone's amplitude: ${peak}`);

        // The trace locks, which is what makes a drawn scope stand still.
        const trace = scopeWindow(window.samples, {
            windowMs: 10.0,
            sampleRate: info.nominalSampleRate,
        });
        assert.equal(trace.locked, true, "a steady tone locks the trigger");

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
        // The buffer is filled by the server (`/b_gen`), since a client has no
        // way to write one: the read direction is the whole of the bulk path.
        const frames = 5000;
        const buffer = await Buffer.alloc(server, frames, 1);
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

        // And what a waveform view would draw from it spans the tone.
        const peaks = Peaks.build(read, { baseBucket: 256 });
        assert.equal(peaks.frames, frames);
        const { min, max } = peaks.columns(0, { width: 10 });
        assert.ok(Math.max(...max) > 0.9 && Math.min(...min) < -0.9);
        peaks.free();

        buffer.free();
    });
});
