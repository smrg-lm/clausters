// `SoundFile`: a file's header and samples through the server's codecs, its
// writer, and the empty file -- the same cases as the Python client's
// `tests/test_soundfile.py`.

import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { Buffer } from "../src/defs/buffer.ts";
import { SoundFile } from "../src/soundfile.ts";

async function inDir(body: (dir: string) => Promise<void>): Promise<void> {
    const dir = await mkdtemp(join(tmpdir(), "clausters-soundfile-"));
    try {
        await body(dir);
    } finally {
        await rm(dir, { recursive: true, force: true });
    }
}

test("a written file opens with its shape and reads back", () =>
    inDir(async (dir) => {
        const samples = Float32Array.from({ length: 600 }, (_, i) => ((i % 7) - 3) / 8);
        const path = join(dir, "take.wav");
        const written = await SoundFile.write(path, samples, 3, 44_100, { sampleFormat: "float" });
        const opened = await SoundFile.open(path);
        assert.deepEqual(opened, written);
        assert.deepEqual(
            [opened.frames, opened.channels, opened.sampleRate, opened.codec, opened.sampleFormat],
            [200, 3, 44_100, "wav", "float"],
        );
        assert.ok(Math.abs(opened.duration - 200 / 44_100) < 1e-12);
        assert.deepEqual([...await opened.read()], [...samples]);
        assert.deepEqual([...await opened.read(10, 5)], [...samples.slice(30, 45)]);
    }));

test("an integer file reads at its precision", () =>
    inDir(async (dir) => {
        const samples = [0.5, -0.25, 0.125, 0.0];
        for (const [format, step] of [["int16", 1 / 32767], ["int24", 1 / 8388607]] as const) {
            const file = await SoundFile.write(join(dir, `${format}.wav`), samples, 1, 48_000, {
                sampleFormat: format,
            });
            assert.equal((await SoundFile.open(file.path!)).sampleFormat, format);
            const back = await file.read();
            assert.ok(samples.every((v, i) => Math.abs(v - back[i]!) <= step));
        }
    }));

test("what cannot be a file is refused", () =>
    inDir(async (dir) => {
        await assert.rejects(SoundFile.open(join(dir, "missing.wav")));
        await assert.rejects(SoundFile.write(join(dir, "ragged.wav"), [0, 0, 0, 0, 0], 2, 48_000));
        assert.throws(() => SoundFile.empty(0, 10, 48_000), RangeError);
    }));

test("an empty file is a shape with no path", async () => {
    const empty = SoundFile.empty(2, 4, 48_000);
    assert.equal(empty.path, null);
    assert.equal(empty.codec, null);
    assert.deepEqual([...await empty.read()], new Array(8).fill(0));
    assert.deepEqual([...await empty.read(3)], [0, 0]);
    const source = empty.source;
    assert.deepEqual(source.location, { at: "volatile" });
    assert.deepEqual([source.channels, source.frames, source.sampleRate], [2, 4, 48_000]);
    await assert.rejects(Buffer.read(empty), RangeError);
});

test("a file is a file source", () =>
    inDir(async (dir) => {
        const file = await SoundFile.write(join(dir, "a.wav"), new Float32Array(6), 2, 48_000);
        assert.equal(file.source.path, file.path);
        assert.equal(file.source.frames, 3);
    }));
