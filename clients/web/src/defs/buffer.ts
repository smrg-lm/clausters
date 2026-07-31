// Buffers, with client-side index allocation.
//
// The server's buffer pool is a finite boot-time resource (`--max-buffers`),
// indices allocated by the client (like scsynth). A `Buffer` holds an index
// and the server it lives on, and owns the `/b_*` commands addressed to it:
// `Buffer.alloc`, `Buffer.read` and `Buffer.load` create one, and `gen`,
// `zero`, `query`, `getSamples` and `free` drive it.
//
// A handle is immutable, so `query` returns a **filled copy** where the Python
// client fills the handle in place: there is nothing to write to here.
//
// The allocator is a registry (the core's occupancy map): a freed slot is
// always reusable, a double free is refused loudly, exhaustion throws instead
// of wrapping. The `Server` sizes it from its options (`maxBuffers`).

import { AllocationError, CommandError } from "../errors.ts";
import { Registry } from "../base/core.ts";
import { fetchAudio, interleave } from "../data/samples.ts";
import type { MsgArg } from "../base/osc.ts";
import type { Server } from "./server.ts";

export const NUM_BUFFERS = 4096;

export class Buffer {
    readonly bufnum: number;
    readonly frames: number;
    readonly channels: number;
    readonly sampleRate: number;
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
        this.bufnum = bufnum;
        this.frames = frames;
        this.channels = channels;
        this.sampleRate = sampleRate;
        this.server = server;
    }

    // ---- constructors ----

    /** Allocates a zeroed buffer (`/b_alloc`). */
    static async alloc(
        server: Server,
        frames: number,
        channels = 1,
        { wait = true, timeout = 5.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<Buffer> {
        const bufnum = server.buffers.alloc();
        const args: MsgArg[] = [
            ["i", bufnum],
            ["i", Math.trunc(frames)],
            ["i", Math.trunc(channels)],
        ];
        if (!wait) {
            server.sendMsg("/b_alloc", ...args);
            return new Buffer(bufnum, frames, channels, 0.0, server);
        }
        try {
            await server.command("/b_alloc", args, timeout);
        } catch (error) {
            server.buffers.free(bufnum);
            throw error;
        }
        return new Buffer(bufnum, frames, channels, 0.0, server);
    }

    /**
     * Loads a sound file into a freshly allocated buffer (`/b_allocRead`).
     * The path is the **server's**, so this reaches a native server over the
     * WebSocket carrier; the in-page engine has no filesystem (feed it
     * decoded samples with `load` instead).
     */
    static async read(
        server: Server,
        path: string,
        {
            fileStart = 0,
            numFrames = 0,
            timeout = 10.0,
        }: { fileStart?: number; numFrames?: number; timeout?: number } = {},
    ): Promise<Buffer> {
        const bufnum = server.buffers.alloc();
        try {
            await server.command(
                "/b_allocRead",
                [["i", bufnum], path, ["i", fileStart], ["i", numFrames]],
                timeout,
            );
        } catch (error) {
            server.buffers.free(bufnum);
            throw error;
        }
        return new Buffer(bufnum, 0, 1, 0.0, server).query(timeout);
    }

    /**
     * Loads an audio file at `url` into a freshly allocated buffer: the
     * browser's `/b_allocRead`, since a page has no filesystem and the
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
     * (`/b_allocRead`) instead.
     */
    static async load(
        server: Server,
        url: string,
        { timeout = 30.0 }: { timeout?: number } = {},
    ): Promise<Buffer> {
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
        const buffer = await Buffer.alloc(server, frames, channels, { timeout });
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
     * Fills this buffer through `/b_gen` (the wavetable/generator commands:
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
        { wait = true, timeout = 5.0 }: { wait?: boolean; timeout?: number } = {},
    ): Promise<void> {
        const payload: MsgArg[] = [["i", this.bufnum], cmd, ...args];
        if (!wait) {
            this.srv().sendMsg("/b_gen", ...payload);
            return;
        }
        await this.srv().command("/b_gen", payload, timeout);
    }

    /** Zeroes this buffer (`/b_zero`). */
    async zero({
        wait = true,
        timeout = 5.0,
    }: { wait?: boolean; timeout?: number } = {}): Promise<void> {
        const args: MsgArg[] = [["i", this.bufnum]];
        if (!wait) {
            this.srv().sendMsg("/b_zero", ...args);
            return;
        }
        await this.srv().command("/b_zero", args, timeout);
    }

    /**
     * This buffer's shape as the server reports it (`/b_query`), as a filled
     * copy of the handle — the fields are readonly, so nothing is written in
     * place.
     */
    async query(timeout = 5.0): Promise<Buffer> {
        const server = this.srv();
        const msg = await server.request("/b_query", [["i", this.bufnum]], {
            expect: ["/b_info"],
            timeout,
        });
        const [, frames, channels, sampleRate] = msg.args;
        return new Buffer(
            this.bufnum,
            Number(frames),
            Number(channels),
            Number(sampleRate),
            server,
        );
    }

    /**
     * Reads interleaved samples out of this buffer (`/b_getn` → `/b_setn`), in
     * chunks, as one `Float32Array`. `count` -1 reads to the end (the shape is
     * queried first). Sample indices are flat across channels
     * (`frame * channels + channel`), so a stereo buffer reads `L R L R …`.
     *
     * `chunk` (samples per round trip) defaults to the transport's own bound —
     * the frame ceiling the server advertises, which is megabytes per reply on
     * a stream carrier. This is the path a waveform view is built from; feed
     * the result to `Peaks.build` and draw the columns.
     *
     * Reading is the only direction there is: the server has no `/b_setn`
     * **command** (`/b_setn` is `/b_getn`'s reply), so samples reach a buffer
     * by `gen`, by `read` on a native server, or by `load` in the page.
     */
    async getSamples({
        start = 0,
        count = -1,
        chunk,
        timeout = 10.0,
    }: { start?: number; count?: number; chunk?: number; timeout?: number } = {}):
        Promise<Float32Array> {
        const server = this.srv();
        const step = chunk ?? (await server.bulkChunk(timeout));
        let total = count;
        if (total < 0) {
            const shape = await this.query(timeout);
            total = Math.max(0, shape.frames * shape.channels - start);
        }
        const out = new Float32Array(total);
        let got = 0;
        while (got < total) {
            const n = Math.min(step, total - got);
            const msg = await server.request(
                "/b_getn",
                [["i", this.bufnum], ["i", start + got], ["i", n]],
                { expect: ["/b_setn"], timeout },
            );
            // /b_setn: bufnum, start, count, value...
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
        server.sendMsg("/b_free", ["i", this.bufnum]);
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
