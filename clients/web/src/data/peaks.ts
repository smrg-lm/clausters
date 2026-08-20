// The peak pyramid: a summary that costs a window, not a buffer.
//
// A waveform view is never drawn sample by sample. The samples are reduced
// once into a min/max pyramid — level 0 summarizes every `baseBucket` samples,
// each level above halves the resolution — and a drawing reads the level whose
// buckets match its zoom, so a picture costs about a bucket per pixel column
// and the work is proportional to the width of the view rather than to the
// length of the buffer.
//
// The reduction itself is `clausters_core::peaks`, reached through the core's
// wasm door: the same code the GUI host reduces with, and the same cache
// format it maps and the Python client writes. A cache built here and a cache
// mapped over there are the same bytes.
//
// **The cache is data; the picture is the host's.** The reading a drawing
// needs — which level a magnification calls for, a column per pixel, the join
// that inks one column out to the next — happens where the drawing does, and
// the drawing is never here (a `waveform` widget over a `buffer`, a `cache`
// file or a `path`). What a page reads off a pyramid is the cache in the
// cache's own units: how long it is, how many levels it has, and one cell's
// `[min, max]` over a span of samples.
//
// The samples come from wherever a buffer comes from — `Server.getSamples`
// over the wire, `fetchAudio` over HTTP — and after `build` they are not
// needed again.

import { Pyramid } from "../core/clausters_core_web.js";

/**
 * A built peak pyramid over one interleaved sample buffer.
 *
 * ```ts
 * const samples = await buffer.getSamples();
 * const peaks = Peaks.build(samples, { channels: buffer.channels });
 * const bytes = peaks.toBytes();        // the cache every client reads
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
     * An **empty** pyramid over `frames` frames — the picture of a take that
     * has been allocated and not yet recorded into, ready to be filled by
     * `writeBuckets` as the reports arrive.
     *
     * It is not `build` over a buffer of zeros: that would allocate the take
     * (230 MB for ten minutes of stereo) to summarize samples nobody wrote.
     */
    static empty(
        frames: number,
        { channels = 1, baseBucket = 256 }: { channels?: number; baseBucket?: number } = {},
    ): Peaks {
        return new Peaks(Pyramid.empty(Math.trunc(frames), channels, baseBucket));
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

    /**
     * Folds a `/buffer_stream.reply` report into this pyramid — how a page
     * follows a recording it cannot map.
     *
     * The server sends the *overview* of what was written rather than the
     * samples (about 2 kB/s a channel where the audio is 190), and the
     * measuring already happened at the writer's end: this puts the buckets
     * where they belong and rebuilds the levels above them, so the picture
     * grows into the one the samples would have built.
     *
     * `stats` is the reply's blob — pass it as it arrived, or as `f32`s if you
     * already read them. Either way it is **bucket-major and channel-minor**:
     * for each bucket of `bucket` frames in order, for each channel, `min`,
     * `max` and mean square. `startFrame` is where the report begins on the
     * buffer's own sample axis.
     *
     * Returns `false`, changing nothing, when the report is on another grid
     * than this cache: another bucket size, a start off a bucket boundary, or
     * a run that does not fit. Subscribe with this cache's `baseBucket` and
     * they agree by construction.
     *
     * ```ts
     * const peaks = Peaks.build(new Float32Array(frames * channels), { channels });
     * new OscFunc(([, bufnum, startFrame, bucket, blob]) => {
     *     if (bufnum === take.bufnum) {
     *         peaks.writeBuckets(startFrame as number, bucket as number, blob as Uint8Array);
     *     }
     * }, "/buffer_stream.reply", { recv: server.receiver });
     * await server.streamBuffers(50, [take], peaks.baseBucket);
     * ```
     */
    writeBuckets(
        startFrame: number,
        bucket: number,
        stats: ArrayLike<number> | Uint8Array,
    ): boolean {
        let flat: Float32Array;
        if (stats instanceof Uint8Array) {
            // The blob as the wire carries it: little-endian `f32`s, at
            // whatever offset the datagram left them — which is why this is a
            // `DataView` read and not a `Float32Array` over the same buffer.
            const view = new DataView(stats.buffer, stats.byteOffset, stats.byteLength);
            flat = new Float32Array(Math.floor(stats.byteLength / 4));
            for (let i = 0; i < flat.length; i++) flat[i] = view.getFloat32(i * 4, true);
        } else {
            flat = stats instanceof Float32Array ? stats : Float32Array.from(stats);
        }
        return this.inner.writeBuckets(Math.trunc(startFrame), Math.trunc(bucket), flat);
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

    /**
     * One cell: the `[min, max]` of `channel` over `[start, end)` at `level`.
     * `undefined` for an unknown channel or an empty level.
     *
     * A read of the summary in its own units, for checking what it says about
     * a span of samples. Drawing a *view* of it is the GUI host's: a
     * `waveform` widget over the buffer, the cache file or the samples.
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

    /** Releases the wasm-side cache. The object is unusable afterwards. */
    free(): void {
        this.inner.free();
    }
}
