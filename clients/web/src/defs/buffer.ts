// Buffers, with client-side index allocation.
//
// The server's buffer pool is a finite boot-time resource (`--max-buffers`),
// indices allocated by the client (like scsynth). A `Buffer` holds an index
// and the server it lives on, and owns the `/buffer_*` commands addressed to it:
// `Buffer.alloc`, `Buffer.read` and `Buffer.load` create one, and `gen`,
// `zero`, `info`, `getSamples` and `free` drive it.
//
// The handle keeps the `BufferInfo` the server last reported and reads its
// shape off it: a buffer only changes when a command of yours changes it, so
// unlike a node's record this one stays true (`clausters/defs/info.py`).
//
// The allocator is a registry (the core's occupancy map): a freed slot is
// always reusable, a double free is refused loudly, exhaustion throws instead
// of wrapping. The `Server` sizes it from its options (`maxBuffers`).

import { AllocationError, CommandError } from "../errors.ts";
import { Registry } from "../base/core.ts";
import { fetchAudio, interleave } from "../data/samples.ts";
import type { MsgArg } from "../base/osc.ts";
import { parseBufferList } from "./info.ts";
import type { BufferInfo } from "./info.ts";
import type { Server } from "./server/index.ts";
import { resolveServer } from "./wire.ts";

export const NUM_BUFFERS = 4096;

/** What every `Buffer` constructor takes: which server, and how long to wait. */
export interface BufferOptions {
    /** How long to wait for the server's `/done`; the handle's by default. */
    timeout?: number;
    /** The server to allocate on; the ambient session's by default. */
    server?: Server;
}

export class Buffer {
    /**
     * What the server holds under this slot, as last read from it — a
     * buffer's shape only changes by a command of yours, so unlike a node's
     * record this one can be kept. `info` refreshes it; `frames`, `channels`
     * and `sampleRate` read it.
     */
    private record: BufferInfo;
    /**
     * The server this buffer lives on (set by `alloc`, `read` and `load`), so
     * its commands know where to go without being told.
     */
    readonly server?: Server;

    constructor(
        bufnum: number,
        frames = 0,
        channels = 1,
        sampleRate = 0.0,
        server?: Server,
    ) {
        this.record = { bufnum, frames, channels, sampleRate, exists: true };
        this.server = server;
    }

    /** The slot this buffer occupies in the server's pool. */
    get bufnum(): number {
        return this.record.bufnum;
    }

    /** Frames per channel, 0 while unknown (see `info`). */
    get frames(): number {
        return this.record.frames;
    }

    get channels(): number {
        return this.record.channels;
    }

    /** The server's rate for this buffer, 0 while unknown (see `info`). */
    get sampleRate(): number {
        return this.record.sampleRate;
    }

    // ---- constructors ----

    /** Allocates a zeroed buffer (`/buffer_alloc`). */
    static async alloc(
        frames: number,
        channels = 1,
        { wait = true, timeout, server: on }: BufferOptions & { wait?: boolean } = {},
    ): Promise<Buffer> {
        const server = resolveServer(on);
        const bufnum = server.buffers.alloc();
        const args: MsgArg[] = [
            ["i", bufnum],
            ["i", Math.trunc(frames)],
            ["i", Math.trunc(channels)],
        ];
        if (!wait) {
            server.sendMsg("/buffer_alloc", ...args);
            return new Buffer(bufnum, frames, channels, 0.0, server);
        }
        try {
            await server.command("/buffer_alloc", args, timeout);
        } catch (error) {
            server.buffers.free(bufnum);
            throw error;
        }
        return new Buffer(bufnum, frames, channels, 0.0, server);
    }

    /**
     * Loads a sound file into a freshly allocated buffer (`/buffer_allocRead`).
     * The path is the **server's**, so this reaches a native server over the
     * WebSocket carrier; the in-page engine has no filesystem (feed it
     * decoded samples with `load` instead).
     */
    static async read(
        path: string,
        {
            fileStart = 0,
            numFrames = 0,
            timeout,
            server: on,
        }: BufferOptions & { fileStart?: number; numFrames?: number } = {},
    ): Promise<Buffer> {
        const server = resolveServer(on);
        const bufnum = server.buffers.alloc();
        try {
            await server.command(
                "/buffer_allocRead",
                [["i", bufnum], path, ["i", fileStart], ["i", numFrames]],
                timeout,
            );
        } catch (error) {
            server.buffers.free(bufnum);
            throw error;
        }
        // The shape is the file's, so the client cannot know it in advance:
        // read it back, and the returned handle carries it.
        const buffer = new Buffer(bufnum, 0, 1, 0.0, server);
        await buffer.info(timeout);
        return buffer;
    }

    /**
     * Loads an audio file at `url` into a freshly allocated buffer: the
     * browser's `/buffer_allocRead`, since a page has no filesystem and the
     * server's path means nothing to it.
     *
     * `fetch` + the page's own `decodeAudioData` produce the samples, which
     * the carrier installs directly — it shares memory with the engine. The
     * returned handle carries the decoded shape, so a view can lay out its
     * axis before reading a sample.
     *
     * **In-page only.** A socket carrier would have to write the samples
     * over the wire, and the server has no buffer-write command to write them
     * with; over a `--ws` server, load the file server-side with `read`
     * (`/buffer_allocRead`) instead.
     */
    static async load(
        url: string,
        { timeout, server: on }: BufferOptions = {},
    ): Promise<Buffer> {
        const server = resolveServer(on);
        const bulkLoad = server.connection.bulkLoad;
        if (!bulkLoad) {
            throw new CommandError(
                "Buffer.load needs a carrier that shares memory with the server " +
                    "(the in-page engine); over a socket use Buffer.read, which " +
                    "loads the file on the server",
            );
        }
        const rate = (await server.queryInfo(timeout)).nominalSampleRate;
        const decoded = await fetchAudio(url, { sampleRate: rate });
        const { numberOfChannels: channels, length: frames, sampleRate } = decoded;
        const samples = interleave(decoded);
        const buffer = await Buffer.alloc(frames, channels, { timeout, server });
        await bulkLoad.call(
            server.connection,
            buffer.bufnum,
            channels,
            sampleRate,
            samples,
        );
        return new Buffer(buffer.bufnum, frames, channels, sampleRate, server);
    }

    // ---- the commands addressed to this buffer ----

    /**
     * Fills this buffer through `/buffer_gen` (the wavetable/generator commands:
     * `"env"`, `"sine1"`/`"sine2"`/`"sine3"`, `"cheby"`, `"copy"`).
     *
     * `args` follow each command's own shape — the wavetable generators take
     * an integer flag word first, then their values. They are tagged by the
     * same rule as `sendMsg` (an integral number is an int32), so a flag
     * word arrives as the int the server requires.
     */
    async gen(
        cmd: string,
        args: MsgArg[] = [],
        { wait = true, timeout }: { wait?: boolean; timeout?: number } = {},
    ): Promise<void> {
        const payload: MsgArg[] = [["i", this.bufnum], cmd, ...args];
        if (!wait) {
            this.srv().sendMsg("/buffer_gen", ...payload);
            return;
        }
        await this.srv().command("/buffer_gen", payload, timeout);
    }

    /**
     * Reads a sound file into this buffer (`/buffer_read`), keeping its shape —
     * the in-place counterpart of `Buffer.read`, which allocates one to fit the
     * file.
     *
     * The path is the **server's**, as in `read`: this reaches a native server
     * over the WebSocket carrier, and means nothing to the in-page engine,
     * which has no filesystem.
     */
    async readInto(
        path: string,
        {
            fileStart = 0,
            numFrames = -1,
            bufStart = 0,
            wait = true,
            timeout,
        }: {
            fileStart?: number;
            numFrames?: number;
            bufStart?: number;
            wait?: boolean;
            timeout?: number;
        } = {},
    ): Promise<void> {
        const args: MsgArg[] = [
            ["i", this.bufnum],
            path,
            ["i", fileStart],
            ["i", numFrames],
            ["i", bufStart],
        ];
        if (!wait) {
            this.srv().sendMsg("/buffer_read", ...args);
            return;
        }
        await this.srv().command("/buffer_read", args, timeout);
    }

    /**
     * Writes this buffer to a WAV file on the **server's** filesystem
     * (`/buffer_write`); `sampleFormat` is `"int16"`, `"int24"` or `"float"`.
     *
     * Server-side, so it reaches a native server over the WebSocket carrier.
     * A page saving what it has read itself downloads a blob instead — the
     * samples are already there (`getSamples`).
     */
    async write(
        path: string,
        {
            sampleFormat = "int16",
            numFrames = -1,
            bufStart = 0,
            wait = true,
            timeout,
        }: {
            sampleFormat?: "int16" | "int24" | "float";
            numFrames?: number;
            bufStart?: number;
            wait?: boolean;
            timeout?: number;
        } = {},
    ): Promise<void> {
        const args: MsgArg[] = [
            ["i", this.bufnum],
            path,
            "wav",
            sampleFormat,
            ["i", numFrames],
            ["i", bufStart],
        ];
        if (!wait) {
            this.srv().sendMsg("/buffer_write", ...args);
            return;
        }
        await this.srv().command("/buffer_write", args, timeout);
    }

    /** Zeroes this buffer (`/buffer_zero`). */
    async zero({
        wait = true,
        timeout,
    }: { wait?: boolean; timeout?: number } = {}): Promise<void> {
        const args: MsgArg[] = [["i", this.bufnum]];
        if (!wait) {
            this.srv().sendMsg("/buffer_zero", ...args);
            return;
        }
        await this.srv().command("/buffer_zero", args, timeout);
    }

    /**
     * Asks the running server what it holds in this slot (`/buffer_query` →
     * `/buffer_query.reply bufnum frames channels sampleRate`), keeps the record on the
     * handle and returns it.
     *
     * Unlike a node's, a buffer's record is worth keeping: its shape changes
     * only by a command of yours, so what this reads stays true until you
     * change it. A slot with nothing in it (never allocated, or freed) comes
     * back with `exists` false rather than throwing.
     */
    async info(timeout?: number): Promise<BufferInfo> {
        const msg = await this.srv().request("/buffer_query", [["i", this.bufnum]], {
            expect: ["/buffer_query.reply"],
            timeout,
        });
        this.record = parseBufferList(msg.args)[0]!;
        return this.record;
    }

    /**
     * Reads interleaved samples out of this buffer (`/buffer_getRange` → `/buffer_getRange.reply`), in
     * chunks, as one `Float32Array`. `count` -1 reads to the end (the shape is
     * queried first). Sample indices are flat across channels
     * (`frame * channels + channel`), so a stereo buffer reads `L R L R …`.
     *
     * `chunk` (samples per round trip) defaults to the transport's own bound —
     * the frame ceiling the server advertises, which is megabytes per reply on
     * a stream carrier. This is the path a waveform view is built from; feed
     * the result to `Peaks.build` and draw the columns.
     *
     * Reading is the only direction there is: the server has no buffer-write
     * command at all, so samples reach a buffer by `gen`, by `read` on a native
     * server, or by `load` in the page.
     */
    async getSamples({
        start = 0,
        count = -1,
        chunk,
        timeout,
    }: { start?: number; count?: number; chunk?: number; timeout?: number } = {}):
        Promise<Float32Array> {
        const server = this.srv();
        const step = chunk ?? (await server.bulkChunk(timeout));
        let total = count;
        if (total < 0) {
            const shape = await this.info(timeout);
            total = Math.max(0, shape.frames * shape.channels - start);
        }
        const out = new Float32Array(total);
        let got = 0;
        while (got < total) {
            const n = Math.min(step, total - got);
            const msg = await server.request(
                "/buffer_getRange",
                [["i", this.bufnum], ["i", start + got], ["i", n]],
                { expect: ["/buffer_getRange.reply"], timeout },
            );
            // /buffer_getRange.reply: bufnum, start, count, value...
            const returned = Number(msg.args[2]);
            if (returned <= 0) break; // past the end: the server has no more
            for (let i = 0; i < returned; i++) {
                out[got + i] = Number(msg.args[3 + i]);
            }
            got += returned;
        }
        return got === total ? out : out.subarray(0, got);
    }

    /** Frees this buffer on the server and returns its index to the pool. */
    free(): void {
        const server = this.srv();
        server.sendMsg("/buffer_free", ["i", this.bufnum]);
        server.buffers.free(this.bufnum);
    }

    /** This buffer's server, or a clear failure when the handle carries none. */
    private srv(): Server {
        if (!this.server) {
            throw new Error(
                `buffer ${this.bufnum} has no server: build the handle with one, ` +
                    `e.g. new Buffer(${this.bufnum}, 0, 1, 0, server)`,
            );
        }
        return this.server;
    }
}

/** Anything a command can address by buffer number: a handle or the number. */
export type BufferLike = Buffer | number;

/** The index behind a buffer handle or a bare number. */
export function bufferNumber(buf: BufferLike): number {
    return typeof buf === "number" ? buf : buf.bufnum;
}

export class BufferAllocator {
    readonly size: number;
    private registry: Registry;

    constructor(size = NUM_BUFFERS) {
        this.size = size;
        this.registry = new Registry(0, size);
    }

    /** A free buffer index; throws when the pool is exhausted. */
    alloc(): number {
        const bufnum = this.registry.alloc(1);
        if (bufnum === undefined) {
            throw new AllocationError("out of buffer slots");
        }
        return bufnum;
    }

    /**
     * Returns `bufnum` to the pool. A double free (or an index this
     * allocator never handed out) throws — a lost buffer slot is a client
     * bug, never absorbed silently.
     */
    free(bufnum: number): void {
        if (!this.registry.release(bufnum, 1)) {
            throw new AllocationError(
                `double free of buffer ${bufnum}: not currently allocated`,
            );
        }
    }

    /** How many buffer slots are currently allocated. */
    get inUse(): number {
        return this.registry.inUse;
    }
}
