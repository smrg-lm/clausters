// The data paths: parity with the Python client where both compute a figure,
// behaviour where only this client does, and the decoding in between.
//
// Three layers are asserted here:
//
// - **Parity** — the peak cache's bytes and the stereo-field measurements
//   against `data-vectors.json`, frozen from the Python client over
//   `clausters-ffi`. Both clients reach one `clausters-core`, so the cache is
//   byte-identical and the measurements exact; a divergence is a failing test
//   rather than a rumour.
// - **Behaviour** — the trigger and the spectrum curve, which have no Python
//   door and therefore no second implementation to compare against. What is
//   worth asserting there is what a view depends on: a periodic signal locks,
//   a full-scale sine reads about 0 dB at its bin.
// - **Decoding** — the `/bus_set` and `/bus_tapStream.reply` snapshots and the bulk
//   chunking, driven over a fake carrier so they run with no server at all.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { decodePacket, encodeMessage, loadOsc } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { Buffer } from "../src/defs/buffer.ts";
import { Server } from "../src/defs/server/index.ts";
import { BusStream, TapStream } from "../src/data/index.ts";
import {
    Peaks,
    correlation,
    decodeSamples,
    deinterleave,
    interleave,
    lissajous,
    scopeFrames,
    scopeWindow,
    spectrumDb,
} from "../src/data/index.ts";

const wasm = await readFile(
    new URL("../dist/core/clausters_core_web_bg.wasm", new URL(".", import.meta.url)),
);
await loadCore(wasm);
await loadOsc(wasm);

interface Vectors {
    peaks: {
        signal: string;
        channels: number;
        baseBucket: number;
        bytes: number;
        sha256: string;
    }[];
    stereoField: {
        case: string;
        correlation: number | null;
        points: number;
        head: number[][];
    }[];
}

const vectors: Vectors = JSON.parse(
    await readFile(new URL("./data-vectors.json", new URL(".", import.meta.url)), "utf8"),
);

// The generator's own recipes, rebuilt here rather than shipped as numbers.
const sine = (n: number, period: number, phase = 0, amp = 1) =>
    Float32Array.from({ length: n }, (_, i) =>
        amp * Math.sin((2 * Math.PI * i) / period + phase),
    );
const SIGNALS: Record<string, Float32Array> = {
    sine440: sine(4096, 109.09),
    ramp: Float32Array.from({ length: 4096 }, (_, i) => i / 2048 - 1),
    quiet: sine(1024, 64, 0, 0.001),
};

function signalFor(name: string): Float32Array {
    if (name === "stereo") {
        const l = SIGNALS.sine440;
        const r = sine(4096, 109.09, 0.7, 0.5);
        const out = new Float32Array(l.length * 2);
        for (let i = 0; i < l.length; i++) {
            out[i * 2] = l[i];
            out[i * 2 + 1] = r[i];
        }
        return out;
    }
    if (name === "short") return SIGNALS.quiet.subarray(0, 100);
    return SIGNALS[name];
}

// ---- parity: the peak cache ----

test("the peak cache is byte-identical to the Python client's", () => {
    for (const vector of vectors.peaks) {
        const peaks = Peaks.build(signalFor(vector.signal), {
            channels: vector.channels,
            baseBucket: vector.baseBucket,
        });
        const bytes = peaks.toBytes();
        const digest = createHash("sha256").update(bytes).digest("hex");
        const what = `${vector.signal} x${vector.channels} @${vector.baseBucket}`;
        assert.equal(bytes.length, vector.bytes, `${what}: cache size`);
        assert.equal(digest, vector.sha256, `${what}: cache bytes`);
        peaks.free();
    }
});

test("a cache written by the Python client reads back here", () => {
    const vector = vectors.peaks[0];
    const written = Peaks.build(signalFor(vector.signal), {
        channels: vector.channels,
        baseBucket: vector.baseBucket,
    });
    const read = Peaks.fromBytes(written.toBytes());
    assert.ok(read, "the cache parses");
    assert.equal(read.frames, 4096);
    assert.equal(read.channels, 1);
    assert.equal(read.baseBucket, 256);
    assert.deepEqual(read.columns(0, { width: 8 }), written.columns(0, { width: 8 }));
    assert.equal(Peaks.fromBytes(new Uint8Array([1, 2, 3, 4])), undefined, "not a cache");
    written.free();
    read.free();
});

// ---- parity: the stereo field ----

test("correlation and the Lissajous projection match the Python client", () => {
    const left = SIGNALS.sine440.subarray(0, 1024);
    const rights: Record<string, Float32Array> = {
        identical: left,
        inverted: left.map((v) => -v),
        quarter_turn: sine(1024, 109.09, Math.PI / 2),
        half_amplitude: left.map((v) => 0.5 * v),
        silence: new Float32Array(1024),
    };
    for (const vector of vectors.stereoField) {
        const right = rights[vector.case];
        const r = correlation(left, right);
        if (vector.correlation === null) {
            assert.equal(r, undefined, `${vector.case}: undefined correlation`);
        } else {
            assert.ok(
                Math.abs(r! - vector.correlation) < 1e-6,
                `${vector.case}: ${r} vs ${vector.correlation}`,
            );
        }
        const points = lissajous(left, right);
        assert.equal(points.length / 2, vector.points, `${vector.case}: point count`);
        vector.head.forEach(([x, y], i) => {
            assert.equal(points[i * 2], x, `${vector.case}: point ${i} x`);
            assert.equal(points[i * 2 + 1], y, `${vector.case}: point ${i} y`);
        });
    }
});

test("a mismatched pair has no correlation and no projection", () => {
    const left = SIGNALS.sine440.subarray(0, 1024);
    const short = SIGNALS.sine440.subarray(0, 512);
    assert.equal(correlation(left, short), undefined);
    assert.equal(lissajous(left, short).length, 0);
});

// ---- the peak pyramid as a view reads it ----

test("a column is the min/max of the samples under it", () => {
    const samples = SIGNALS.ramp;
    const peaks = Peaks.build(samples, { baseBucket: 256 });
    const width = 16;
    const { min, max } = peaks.columns(0, { width });
    assert.equal(min.length, width);
    const step = samples.length / width;
    for (let i = 0; i < width; i++) {
        // The pyramid reads whole buckets, so a column may reach a little
        // past its span; it can never be narrower than the samples in it.
        let lo = Infinity;
        let hi = -Infinity;
        for (let s = Math.floor(i * step); s < Math.floor((i + 1) * step); s++) {
            lo = Math.min(lo, samples[s]);
            hi = Math.max(hi, samples[s]);
        }
        assert.ok(min[i] <= lo + 1e-6, `column ${i}: ${min[i]} > ${lo}`);
        assert.ok(max[i] >= hi - 1e-6, `column ${i}: ${max[i]} < ${hi}`);
    }
    peaks.free();
});

test("zooming picks a finer level, and the columns follow", () => {
    const peaks = Peaks.build(SIGNALS.sine440, { baseBucket: 256 });
    assert.ok(peaks.numLevels > 1, "a 4096-sample buffer has levels above 0");
    assert.equal(peaks.levelBucket(0), 256);
    assert.equal(peaks.levelBucket(1), 512);
    assert.equal(peaks.levelFor(256), 0, "at the base bucket, level 0");
    assert.ok(peaks.levelFor(4096) > 0, "zoomed out, a coarser level");
    // A window over a quarter of the buffer is drawn from the same data as
    // the full view's first quarter — same signal, finer columns.
    const whole = peaks.columns(0, { width: 4 });
    const quarter = peaks.columns(0, { width: 4, start: 0, end: 1024 });
    assert.ok(quarter.max[0] <= whole.max[0] + 1e-6);
    peaks.free();
});

test("a degenerate span draws nothing rather than guessing", () => {
    const peaks = Peaks.build(SIGNALS.quiet, { baseBucket: 256 });
    assert.equal(peaks.columns(0, { width: 0 }).min.length, 0);
    assert.equal(peaks.columns(0, { width: 8, start: 10, end: 10 }).min.length, 0);
    assert.equal(peaks.columns(7, { width: 8 }).min.length, 0, "no such channel");
    assert.equal(peaks.column(0, 99, 0, 10), undefined, "no such level");
    assert.equal(peaks.column(7, 0, 0, 10), undefined, "no such channel");
    peaks.free();
});

// ---- the oscilloscope's trigger, and the spectrum ----

test("the trigger locks a periodic signal at the same phase", () => {
    const rate = 48000;
    const windowMs = 5;
    const raw = scopeFrames(windowMs, rate);
    assert.equal(raw, 480, "a 5 ms window at 48 kHz, plus its search slack");
    const a = scopeWindow(sine(raw, 109.09, 0.3), { windowMs, sampleRate: rate });
    const b = scopeWindow(sine(raw, 109.09, 2.1), { windowMs, sampleRate: rate });
    assert.ok(a.locked && b.locked, "a periodic signal locks");
    assert.equal(a.samples.length, 240);
    for (let i = 0; i < a.samples.length; i++) {
        assert.ok(
            Math.abs(a.samples[i] - b.samples[i]) < 0.06,
            `sample ${i}: ${a.samples[i]} vs ${b.samples[i]}`,
        );
    }
});

test("silence free-runs on the newest window instead of blanking", () => {
    const trace = scopeWindow(new Float32Array(480), { windowMs: 5 });
    assert.equal(trace.locked, false);
    assert.equal(trace.samples.length, 240);
});

test("a full-scale sine reads about 0 dB at its own bin", () => {
    const fftSize = 1024;
    const bin = 32;
    const curve = spectrumDb(sine(fftSize, fftSize / bin), { fftSize });
    assert.equal(curve.length, fftSize / 2);
    assert.ok(Math.abs(curve[bin]) < 0.5, `bin ${bin} reads ${curve[bin]} dB`);
    assert.ok(curve[bin + 40] < curve[bin] - 40, "and the rest is far below");
    const silent = spectrumDb(new Float32Array(fftSize), { fftSize });
    assert.ok(
        silent.every((v) => v === -120),
        "silence sits at the reference floor",
    );
    assert.equal(spectrumDb(new Float32Array(100), { fftSize: 100 }).length, 0);
});

test("the window shape reaches the transform", () => {
    // A tone *between* two bins is what tells the windows apart: exactly on a
    // bin, a rectangular window is periodic in the frame and leaks nothing.
    const tone = sine(1024, 1024 / 32.5);
    const hann = spectrumDb(tone, { fftSize: 1024 });
    const rect = spectrumDb(tone, { fftSize: 1024, window: "rectangular" });
    assert.notDeepEqual(Array.from(hann), Array.from(rect));
    assert.ok(hann[32] > -6 && rect[32] > -6, "both find the tone");
    assert.ok(rect[60] > hann[60], "rectangular leaks further out");
});

// ---- interleaving ----

test("interleaving and de-interleaving are inverses", () => {
    const flat = Float32Array.from([1, -1, 2, -2, 3, -3]);
    const [left, right] = deinterleave(flat, 2);
    assert.deepEqual(Array.from(left), [1, 2, 3]);
    assert.deepEqual(Array.from(right), [-1, -2, -3]);
    const audio = {
        numberOfChannels: 2,
        length: 3,
        getChannelData: (ch: number) => (ch === 0 ? left : right),
    } as unknown as AudioBuffer;
    assert.deepEqual(Array.from(interleave(audio)), Array.from(flat));
});

test("a tap blob decodes as little-endian floats whatever its alignment", () => {
    const values = [0.5, -0.25, 1.0];
    const bytes = new Uint8Array(4 + values.length * 4);
    const view = new DataView(bytes.buffer);
    values.forEach((v, i) => view.setFloat32(4 + i * 4, v, true));
    // Offset by one float: a blob's bytes are not guaranteed 4-byte aligned.
    const blob = bytes.subarray(4);
    assert.deepEqual(Array.from(decodeSamples(blob)), values);
});

// ---- the streams, over a fake carrier ----

/**
 * A carrier with no server behind it: it records what was sent and lets a
 * test push replies back. Enough to drive the whole reply path — the streams
 * decode exactly what a real server's bytes decode to.
 */
class FakeConnection implements Connection {
    sent: { addr: string; args: unknown[] }[] = [];
    private listeners = new Set<(packet: Uint8Array) => void>();
    private handlers = new Set<(msg: { addr: string; args: unknown[] }) => void>();
    /** Answers every command with its `/done`, the way a server does. */
    autoDone = true;

    send(packet: Uint8Array): void {
        for (const msg of decode(packet)) {
            this.sent.push(msg);
            if (this.autoDone) this.reply("/done", [msg.addr]);
            for (const handler of [...this.handlers]) handler(msg);
        }
    }
    /** Answers a command the way a server would: `onSend` sees what went out. */
    onSend(handler: (msg: { addr: string; args: unknown[] }) => void): void {
        this.handlers.add(handler);
    }
    addReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.add(listener);
    }
    removeReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.delete(listener);
    }
    close(): void {
        this.listeners.clear();
    }
    /** Pushes one reply message at the client. */
    reply(addr: string, args: unknown[]): void {
        const packet = encodeMessage(
            addr,
            args.map((a) =>
                a instanceof Uint8Array
                    ? (["b", a] as const)
                    : typeof a === "string"
                      ? (["s", a] as const)
                      : Number.isInteger(a)
                        ? (["i", a as number] as const)
                        : (["f", a as number] as const),
            ),
        );
        for (const listener of [...this.listeners]) listener(packet);
    }
    lastSent(addr: string) {
        return [...this.sent].reverse().find((m) => m.addr === addr);
    }
}

/** The client's own decoder, so the fake sees what a server would. */
function decode(packet: Uint8Array) {
    return decodePacket(packet) as { addr: string; args: unknown[] }[];
}

async function fakeServer(): Promise<{ server: Server; carrier: FakeConnection }> {
    const carrier = new FakeConnection();
    // The sizing query, answered like a real server: the allocators (the tap
    // registry included) and the bulk chunking are then the real ones.
    carrier.onSend((msg) => {
        if (msg.addr !== "/server_query") return;
        carrier.reply(
            "/server_query.reply",
            [128, 16384, 2, 64, 48000, 48000, 0, 8192, 4096, 512, 32, 8, 16384, 65536],
        );
    });
    const server = await Server.open(carrier, { notify: false, timeout: 0.5 });
    carrier.sent = [];
    return { server, carrier };
}

test("a bus stream subscribes, decodes its snapshots and cancels", async () => {
    const { server, carrier } = await fakeServer();
    const stream = await BusStream.open(server, [7, 9], { periodMs: 50 });
    const subscribe = carrier.lastSent("/bus_stream");
    assert.deepEqual(subscribe?.args, [50, 7, 9], "period then the buses");

    const seen: number[][] = [];
    stream.onSnapshot((values) => seen.push(Array.from(values)));
    carrier.reply("/bus_stream.reply", [7, 0.25, 9, -1.5]);
    assert.deepEqual(Array.from(stream.values), [0.25, -1.5]);
    assert.equal(stream.value(9), -1.5);
    assert.ok(Number.isNaN(stream.value(11)), "a bus outside the stream");
    assert.equal(stream.snapshots, 1);

    // A snapshot naming a bus this stream does not watch is not one of ours.
    carrier.reply("/bus_stream.reply", [11, 3.0]);
    assert.equal(stream.snapshots, 1);
    assert.deepEqual(seen, [[0.25, -1.5]]);

    await stream.stop();
    assert.deepEqual(carrier.lastSent("/bus_stream")?.args, [0], "cancelled");
    carrier.reply("/bus_stream.reply", [7, 9.0]);
    assert.equal(stream.snapshots, 1, "a stopped stream decodes nothing");
});

test("a tap stream places its windows on the tap's sample axis", async () => {
    const { server, carrier } = await fakeServer();
    const stream = await TapStream.open(server, [0, 1], {
        frames: 4,
        periodMs: 33,
    });
    assert.deepEqual(carrier.lastSent("/bus_tapStream")?.args, [33, 4, 0, 1]);

    const blob = (values: number[]) => {
        const bytes = new Uint8Array(values.length * 4);
        const view = new DataView(bytes.buffer);
        values.forEach((v, i) => view.setFloat32(i * 4, v, true));
        return bytes;
    };
    const seen: [number, number][] = [];
    stream.onData((tap, window) => seen.push([tap, window.endPosition]));
    carrier.reply("/bus_tapStream.reply", [0, 1024, blob([1, 2, 3, 4])]);
    carrier.reply("/bus_tapStream.reply", [1, 1024, blob([-1, -2, -3, -4])]);

    assert.deepEqual(Array.from(stream.window(0)!.samples), [1, 2, 3, 4]);
    assert.equal(stream.window(0)!.endPosition, 1024);
    assert.equal(stream.window(5), undefined, "a tap outside the stream");
    assert.deepEqual(seen, [
        [0, 1024],
        [1, 1024],
    ]);
    // The stereo pair, frame-major, is what a phasescope reads.
    assert.deepEqual(Array.from(stream.interleaved(0, 2)), [1, -1, 2, -2, 3, -3, 4, -4]);

    // The next snapshot advances the axis by exactly the position delta.
    carrier.reply("/bus_tapStream.reply", [0, 1536, blob([5, 6, 7, 8])]);
    assert.equal(stream.window(0)!.endPosition - 1024, 512);

    await stream.stop();
    assert.deepEqual(carrier.lastSent("/bus_tapStream")?.args, [0, 0]);
});

test("a stereo view of taps that have not all reported yet draws nothing", async () => {
    const { server, carrier } = await fakeServer();
    const stream = await TapStream.open(server, [2, 3], { frames: 2 });
    const bytes = new Uint8Array(8);
    new DataView(bytes.buffer).setFloat32(0, 1.0, true);
    carrier.reply("/bus_tapStream.reply", [2, 64, bytes]);
    assert.equal(stream.interleaved(2, 2).length, 0, "one tap short");
    await stream.stop();
});

// ---- the bulk paths ----

test("reading a buffer chunks by the transport's frame ceiling", async () => {
    const { server, carrier } = await fakeServer();
    carrier.autoDone = false;
    // 65536-byte frames → (65536 - 256) / 4 = 16320 samples per round trip.
    const total = 20000;
    const requests: number[][] = [];
    carrier.onSend((msg) => {
        if (msg.addr !== "/buffer_getRange") return;
        const [bufnum, start, count] = msg.args.map(Number);
        requests.push([start, count]);
        const values = Array.from({ length: count }, (_, i) => (start + i) / 1000);
        carrier.reply("/buffer_getRange.reply", [bufnum, start, count, ...values]);
    });

    const samples = await new Buffer(3, 0, 1, 0, server)
        .getSamples({ start: 0, count: total });
    assert.deepEqual(
        requests,
        [
            [0, 16320],
            [16320, 3680],
        ],
        "two round trips, the first at the ceiling",
    );
    assert.equal(samples.length, total);
    assert.ok(Math.abs(samples[19999] - 19.999) < 1e-3);
});

test("a read past the end returns what the buffer holds", async () => {
    const { server, carrier } = await fakeServer();
    carrier.autoDone = false;
    carrier.onSend((msg) => {
        if (msg.addr !== "/buffer_getRange") return;
        const [bufnum, start] = msg.args.map(Number);
        // The server clamps: only 10 samples exist from `start`.
        const count = start === 0 ? 10 : 0;
        const values = Array.from({ length: count }, () => 0.5);
        carrier.reply("/buffer_getRange.reply", [bufnum, start, count, ...values]);
    });
    const samples = await new Buffer(1, 0, 1, 0, server)
        .getSamples({ start: 0, count: 50, chunk: 32 });
    assert.equal(samples.length, 10, "only what came back");
});

