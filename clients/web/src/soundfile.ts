/**
 * Read and write audio files from the web client.
 *
 * `SoundFile` stores a file's path and audio metadata: frame count, channel
 * count, sample rate, codec and sample format. Use {@link SoundFile.open} to
 * inspect a file, {@link SoundFile.read} to decode samples and
 * {@link SoundFile.write} to save samples as WAV. {@link SoundFile.empty}
 * describes silence without creating a file.
 *
 * Paths refer to OPFS in a browser and the local filesystem in Node.js.
 * Local file operations use the shared Clausters codecs and do not require
 * a running server. {@link SoundFile.toBuffer} loads samples into a server
 * buffer; the selected server must be able to read the file path.
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
 * An audio file path and its metadata.
 *
 * {@link SoundFile.open} reads metadata from an existing file.
 * {@link SoundFile.write} creates a WAV file and returns its metadata.
 * {@link SoundFile.empty} creates a descriptor for silence with `path=null`.
 * Samples are loaded when {@link SoundFile.read} or {@link SoundFile.toBuffer}
 * is called.
 */
export class SoundFile {
    /** File path, or `null` for a descriptor created by {@link SoundFile.empty}. */
    readonly path: string | null;
    /** Frames per channel. */
    readonly frames: number;
    /** Number of audio channels. */
    readonly channels: number;
    /** Sample rate in Hz. */
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
     * Read an audio file's metadata and return a `SoundFile`.
     *
     * Reads the frame count, channel count, sample rate, codec and sample format.
     * Supported formats include WAV, FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC and AIFF.
     * Rejects if the file cannot be opened or its format is unsupported.
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
     * Describe silence with the given channels, frame count and sample rate.
     *
     * Returns a `SoundFile` with `path=null`. This does not create a file or
     * allocate samples. {@link SoundFile.read} returns zeros and
     * {@link SoundFile.toBuffer} allocates a zeroed server buffer. An audio editor
     * asks for a destination on its first save.
     *
     * Throws `RangeError` if channels or sample rate are not positive, or frames
     * is negative.
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
     * Write interleaved samples to a WAV file and return its `SoundFile`.
     *
     * `samples` contains floating-point samples in frame-major order (for stereo:
     * left, right, left, right, ...). Each frame must contain `channels` samples.
     * `sampleRate` is in Hz. `sampleFormat` selects the WAV encoding: `"int16"`
     * (default), `"int24"` or `"float"`.
     *
     * Rejects if the samples, format or metadata are invalid, or the file cannot
     * be written.
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

    /** Duration in seconds, calculated as frames divided by sample rate. */
    get duration(): number {
        return this.sampleRate > 0 ? this.frames / this.sampleRate : 0;
    }

    /**
     * Return a document {@link Source} for this file.
     *
     * The source includes the channel count, frame count and sample rate. It
     * references the file path when available; otherwise it is a volatile source.
     */
    get source(): Source {
        const source = this.path !== null ? Source.file(this.path) : Source.volatile();
        return source.shaped(this.channels, this.frames, this.sampleRate);
    }

    /**
     * Decode a frame range into an interleaved `Float32Array` of samples.
     *
     * `start` is a zero-based frame offset. `frames=-1` reads to the end of the
     * file. Each frame contains one float32 sample per channel, in channel order.
     * Integer audio is scaled to `[-1, 1]`; floating-point audio can exceed that
     * range. The sample rate is unchanged. If `path` is `null`, returns zeros
     * for the requested range.
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
     * Load this file into a new server {@link Buffer}.
     *
     * Uses {@link Buffer.read} when the file has a path. The selected server must
     * be able to read that path. For `path=null`, uses {@link Buffer.alloc} to
     * create a zeroed buffer with the specified frame count, channel count and
     * sample rate. `options` is passed to the corresponding buffer method.
     *
     * Rejects if a file without a path has zero frames.
     */
    async toBuffer(options: BufferOptions = {}): Promise<Buffer> {
        if (this.path === null) {
            if (this.frames < 1) {
                throw new RangeError(
                    "a sound file of no frames has nothing to allocate: a buffer holds at least one",
                );
            }
            return Buffer.alloc(this.frames, this.channels, { ...options, sampleRate: this.sampleRate });
        }
        return Buffer.read(this, options);
    }
}
