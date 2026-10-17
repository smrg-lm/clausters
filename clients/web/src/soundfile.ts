/**
 * **A sound file on the client's side**: its path and its shape, read from the
 * header, and the one place a client reads or writes audio files.
 *
 * A {@link Buffer} is memory on the **server**; a `SoundFile` is a file, with a
 * path or -- made with {@link SoundFile.empty} -- not written anywhere yet. The
 * two meet through reads and writes: {@link SoundFile.toBuffer} (or
 * {@link Buffer.read}) loads one into a buffer, and {@link Buffer.write}
 * answers with the `SoundFile` it wrote.
 *
 * Every byte goes through the server's own codecs, bound in the module the NRT
 * worker decodes with, so the page needs no audio library of its own: the
 * header and the samples come from the decoder `/buffer_allocRead` uses, and
 * {@link SoundFile.write} writes the bytes the Python client writes for the
 * same samples. A path is the page's own storage (`opfs`) in a tab and the
 * disk under node.
 *
 * @module
 */

import { readFileAt, writeFileAt } from "./base/files.ts";
import { Buffer } from "./defs/buffer.ts";
import type { BufferOptions } from "./defs/buffer.ts";
import { decodeSoundfile, encodeWav, probeSoundfile } from "./engine/codec.ts";
import { Source } from "./multitrack.ts";

/** A WAV sample format {@link SoundFile.write} can write. */
export type SoundFileFormat = "int16" | "int24" | "float";

/**
 * A sound file: where it is and what shape it has.
 *
 * Made by {@link SoundFile.open} (a file, its header read), by
 * {@link SoundFile.empty} (a shape with no file yet) or by
 * {@link SoundFile.write} and {@link Buffer.write} (the file just written).
 * Nothing here holds samples: {@link SoundFile.read} decodes them when they
 * are asked for.
 */
export class SoundFile {
    /** Where the file is, or `null` for one not written yet. */
    readonly path: string | null;
    /** Frames per channel. */
    readonly frames: number;
    readonly channels: number;
    /** The file's own rate; nothing resamples. */
    readonly sampleRate: number;
    /**
     * The container's short name (`"wav"`, `"flac"`, `"mp3"`, ...); `null` for
     * a file not written yet.
     */
    readonly codec: string | null;
    /**
     * A WAV's sample format (`"int16"`, `"int24"`, `"int32"`, `"float"`);
     * `null` for other formats and for a file not written yet.
     */
    readonly sampleFormat: string | null;

    constructor(
        path: string | null,
        frames: number,
        channels: number,
        sampleRate: number,
        codec: string | null = null,
        sampleFormat: string | null = null,
    ) {
        this.path = path;
        this.frames = frames;
        this.channels = channels;
        this.sampleRate = sampleRate;
        this.codec = codec;
        this.sampleFormat = sampleFormat;
    }

    /**
     * The file at `path`, its shape read from the header -- no sample is kept.
     * WAV, FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and the rest: whatever
     * the server's decoder reads. Rejects when the file is missing or not
     * audio.
     */
    static async open(path: string): Promise<SoundFile> {
        const info = await probeSoundfile(await readFileAt(path), path);
        return new SoundFile(
            path,
            info.frames,
            info.channels,
            info.sampleRate,
            info.codec,
            info.sampleFormat || null,
        );
    }

    /**
     * A file of this shape that is not written anywhere yet: silence to open
     * in an editor, which asks for a path when it is saved.
     */
    static empty(channels: number, frames: number, sampleRate: number): SoundFile {
        if (!(channels >= 1) || !(frames >= 0) || !(sampleRate > 0)) {
            throw new RangeError(
                `an empty sound file needs channels >= 1, frames >= 0 and a positive rate, `
                    + `got ${channels}, ${frames}, ${sampleRate}`,
            );
        }
        return new SoundFile(null, Math.trunc(frames), Math.trunc(channels), sampleRate);
    }

    /**
     * Writes interleaved `samples` to a WAV file at `path`, and resolves with
     * the file. What a render returns is saved this way:
     *
     * ```ts
     * const stats = await render(...);
     * await SoundFile.write("take.wav", stats.samples!, stats.channels, stats.sampleRate);
     * ```
     */
    static async write(
        path: string,
        samples: ArrayLike<number>,
        channels: number,
        sampleRate: number,
        { sampleFormat = "int16" }: { sampleFormat?: SoundFileFormat } = {},
    ): Promise<SoundFile> {
        if (!(channels >= 1) || samples.length % channels !== 0) {
            throw new RangeError(
                `${path}: ${samples.length} samples are not whole frames of ${channels} channels`,
            );
        }
        if (!(sampleRate > 0)) {
            throw new RangeError(`${path}: sample rate must be positive, got ${sampleRate}`);
        }
        const data = samples instanceof Float32Array ? samples : Float32Array.from(samples);
        await writeFileAt(path, await encodeWav(data, channels, sampleRate, sampleFormat));
        return new SoundFile(path, data.length / channels, channels, sampleRate, "wav", sampleFormat);
    }

    /** Length in seconds. */
    get duration(): number {
        return this.sampleRate > 0 ? this.frames / this.sampleRate : 0;
    }

    /**
     * The document {@link Source} this file is: a file source, or a volatile
     * one for a file not written yet, with its shape.
     */
    get source(): Source {
        const source = this.path !== null ? Source.file(this.path) : Source.volatile();
        return source.shaped(this.channels, this.frames, this.sampleRate);
    }

    /**
     * Decodes `frames` frames from `start` (`-1`, the default, to the end)
     * into interleaved samples in `[-1, 1]`, at the file's own rate. A file
     * not written yet reads as the silence it holds.
     */
    async read(start = 0, frames = -1): Promise<Float32Array> {
        if (this.path === null) {
            const from = Math.min(Math.max(0, start), this.frames);
            const count = frames < 0 ? this.frames - from : Math.min(frames, this.frames - from);
            return new Float32Array(count * this.channels);
        }
        const decoded = await decodeSoundfile(await readFileAt(this.path), this.path, start, frames);
        return decoded.samples;
    }

    /**
     * Loads the file into a new server buffer ({@link Buffer.read}); a file
     * not written yet becomes a zeroed buffer of its shape
     * ({@link Buffer.alloc}).
     */
    async toBuffer(options: BufferOptions = {}): Promise<Buffer> {
        if (this.path === null) {
            return Buffer.alloc(this.frames, this.channels, { ...options, sampleRate: this.sampleRate });
        }
        return Buffer.read(this, options);
    }
}
