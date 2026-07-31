// The peak pyramid: a waveform that costs its window, not its buffer.
//
// A waveform view is never drawn sample by sample. The samples are reduced
// once into a min/max pyramid — level 0 summarizes every `baseBucket` samples,
// each level above halves the resolution — and a draw reads the level whose
// buckets match the current zoom, so each pixel column costs about one bucket
// and the work is proportional to the width of the view rather than to the
// length of the buffer.
//
// The reduction itself is `clausters_core::peaks`, reached through the core's
// wasm door: the same code the GUI host reduces with, and the same cache
// format it maps and the Python client writes. A page that builds a pyramid
// here and a host that maps one over there draw the identical columns.
//
// The samples come from wherever a buffer comes from — `Server.getSamples`
// over the wire, `fetchAudio` over HTTP — and after `build` they are not
// needed again.

import { Pyramid } from "../core/clausters_core_web.js";

/** A pixel row of a waveform: one `(min, max)` pair per column. */
export interface Columns {
    min: Float32Array;
    max: Float32Array;
}

/**
 * A built peak pyramid over one interleaved sample buffer.
 *
 * ```ts
 * const samples = await buffer.getSamples();
 * const peaks = Peaks.build(samples, { channels: buffer.channels });
 * const { min, max } = peaks.columns(0, { width: canvas.width });
 * ```
 *
 * Requires a prior `loadCore()`, like everything core-backed. The pyramid owns
 * memory on the wasm heap: `free()` releases it, and a long-lived page that
 * opens buffer after buffer should call it.
 */
export class Peaks {
    private inner: Pyramid;

    private constructor(inner: Pyramid) {
        this.inner = inner;
    }

    /**
     * Reduces `samples` (interleaved, `channels` of them). `baseBucket` is the
     * finest bucket: 256 — the default every client uses — costs ~0.8% of the
     * source in cache and resolves down to 256 samples per column, below which
     * a view should read the raw samples instead.
     */
    static build(
        samples: ArrayLike<number>,
        { channels = 1, baseBucket = 256 }: { channels?: number; baseBucket?: number } = {},
    ): Peaks {
        const flat =
            samples instanceof Float32Array ? samples : Float32Array.from(samples);
        return new Peaks(Pyramid.build(flat, channels, baseBucket));
    }

    /**
     * Reads back a serialized cache — one written here, or the file the GUI
     * host maps and the Python client writes. `undefined` when the bytes are
     * not a cache.
     */
    static fromBytes(bytes: Uint8Array): Peaks | undefined {
        const inner = Pyramid.fromBytes(bytes);
        return inner ? new Peaks(inner) : undefined;
    }

    /** This cache's bytes, in the format every client reads. */
    toBytes(): Uint8Array {
        return this.inner.toBytes();
    }

    /** Samples per channel — the span a view of this cache covers. */
    get frames(): number {
        return this.inner.frames;
    }

    get channels(): number {
        return this.inner.channels;
    }

    get baseBucket(): number {
        return this.inner.baseBucket;
    }

    get numLevels(): number {
        return this.inner.numLevels;
    }

    /** The source samples one entry of `level` summarizes. */
    levelBucket(level: number): number | undefined {
        return this.inner.levelBucket(level);
    }

    /** The level whose buckets match `samplesPerPx`. */
    levelFor(samplesPerPx: number): number {
        return this.inner.levelFor(samplesPerPx);
    }

    /**
     * One column: the `[min, max]` of `channel` over `[start, end)` at
     * `level`. `undefined` for an unknown channel or an empty level. A view
     * draws with `columns` instead — this is for reading a single figure.
     */
    column(
        channel: number,
        level: number,
        start: number,
        end: number,
    ): [number, number] | undefined {
        const pair = this.inner.column(channel, level, start, end);
        return pair ? [pair[0], pair[1]] : undefined;
    }

    /**
     * A whole pixel row in one crossing: `width` columns spanning
     * `[start, end)` of `channel`, at the level that span and width imply.
     *
     * This is what a view calls per frame — never a column at a time, and
     * never finer than the screen. `start`/`end` default to the whole buffer.
     */
    columns(
        channel: number,
        {
            width,
            start = 0,
            end = this.frames,
        }: { width: number; start?: number; end?: number },
    ): Columns {
        const flat = this.inner.columns(channel, start, end, Math.trunc(width));
        const n = flat.length / 2;
        const min = new Float32Array(n);
        const max = new Float32Array(n);
        for (let i = 0; i < n; i++) {
            min[i] = flat[i * 2];
            max[i] = flat[i * 2 + 1];
        }
        return { min, max };
    }

    /** Releases the wasm-side cache. The object is unusable afterwards. */
    free(): void {
        this.inner.free();
    }
}
