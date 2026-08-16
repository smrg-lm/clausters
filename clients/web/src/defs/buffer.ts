// Buffers, with client-side index allocation.
//
// The server's buffer pool is a finite boot-time resource (`--max-buffers`),
// indices allocated by the client (like scsynth). A `Buffer` holds an index
// and the server it lives on, and owns the `/buffer_*` commands addressed to it:
// `Buffer.alloc`, `Buffer.read` and `Buffer.load` create one, and `gen`,
// `zero`, `info`, `getSamples`, `setSamples` and `free` drive it.
//
// The handle keeps the `BufferInfo` the server last reported and reads its
// shape off it: a buffer only changes when a command of yours changes it, so
// unlike a node's record this one stays true (`clausters/defs/info.py`).
//
// The allocator is a registry (the core's occupancy map): a freed slot is
// always reusable, a double free is refused loudly, exhaustion throws instead
// of wrapping. The `Server` sizes it from its options (`maxBuffers`).

import { AllocationError, CommandError } from "../errors.ts";
import { Registry, shareOf } from "../base/core.ts";
import type { IdShare } from "../base/core.ts";
import { blobToSamples, samplesToBlob } from "../base/bulk.ts";
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
     * **In-page only**, now by cost rather than by impossibility: a socket
     * carrier would have to push every decoded sample over the wire in
     * `setSamples` chunks, where the server can read the file itself in one
     * command. Over a `--ws` server, load it server-side with `read`
     * (`/buffer_allocRead`).
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

    /**
     * Installs interleaved `samples` into a freshly allocated buffer — the
     * page's way back from a render to something that sounds.
     *
     * This is what closes the loop the `render` verb opens: the reference
     * client bounces a piece to a **file** and reads that file back with
     * `/buffer_allocRead`, and a page has neither a file to write nor a
     * filesystem the server could read one from. What it has instead is
     * memory shared with the engine, so the samples go in directly.
     *
     * **In-page only**, for the same reason `load` is: a socket carrier would
     * have to push every sample over the wire in `setSamples` chunks, which
     * is what to reach for when the samples exist only in the page and the
     * server is remote.
     */
    static async fromSamples(
        samples: Float32Array,
        channels = 1,
        sampleRate = 0.0,
        { timeout, server: on }: BufferOptions = {},
    ): Promise<Buffer> {
        const server = resolveServer(on);
        const bulkLoad = server.connection.bulkLoad;
        if (!bulkLoad) {
            throw new CommandError(
                "Buffer.fromSamples needs a carrier that shares memory with the "
                    + "server (the in-page engine); over a socket, allocate the "
                    + "buffer and write the samples with setSamples",
            );
        }
        const rate = sampleRate > 0
            ? sampleRate
            : (await server.queryInfo(timeout)).nominalSampleRate;
        const frames = Math.floor(samples.length / Math.max(1, channels));
        const buffer = await Buffer.alloc(frames, channels, { timeout, server });
        await bulkLoad.call(server.connection, buffer.bufnum, channels, rate, samples);
        return new Buffer(buffer.bufnum, frames, channels, rate, server);
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
     * The shared body of the destructive edits: fire, or await `/done`.
     *
     * They are async like every other write, and they **compose in flight** —
     * the server chains a batch of edits on one buffer, so several
     * `wait: false` edits in a row each build on the last rather than each on
     * the contents you started with.
     */
    private async edit(
        addr: string,
        rest: MsgArg[],
        wait: boolean,
        timeout?: number,
    ): Promise<void> {
        const args: MsgArg[] = [["i", this.bufnum], ...rest];
        if (!wait) {
            this.srv().sendMsg(addr, ...args);
            return;
        }
        await this.srv().command(addr, args, timeout);
    }

    /**
     * Scales a span of this buffer (`/buffer_gain`) — the destructive edit an
     * editor applies to a selection.
     *
     * `start` and `frames` are **frames**, not flat sample indices: a selection
     * is a stretch of time across every channel, and every channel of a frame
     * is scaled alike, so a fade can never tilt a stereo image. `frames` of -1
     * runs to the end.
     *
     * One value is a constant gain; give `to` for a fade, which sweeps
     * `factor` to `to` along `shape` — the same envelope shape numbers `Env`
     * and the breakpoint editor speak, `curve` read only by the
     * custom-curvature shape (5). So a fade in is `gain(0, { to: 1 })`, a fade
     * out `gain(1, { to: 0 })`, and silence is {@link silence}, which lands on
     * exact zeros where a fade only tends to one.
     */
    async gain(
        factor: number,
        {
            start = 0,
            frames = -1,
            to,
            shape = 1,
            curve = 0,
            wait = true,
            timeout,
        }: {
            start?: number;
            frames?: number;
            to?: number;
            shape?: number;
            curve?: number;
            wait?: boolean;
            timeout?: number;
        } = {},
    ): Promise<void> {
        await this.edit(
            "/buffer_gain",
            [
                ["i", start],
                ["i", frames],
                ["f", factor],
                ["f", to ?? factor],
                ["i", shape],
                ["f", curve],
            ],
            wait,
            timeout,
        );
    }

    /**
     * A fade in over a span, or out with `out: true` — {@link gain}'s two
     * common cases, spelled the way they are asked for.
     */
    async fade({
        start = 0,
        frames = -1,
        out = false,
        shape = 1,
        curve = 0,
        wait = true,
        timeout,
    }: {
        start?: number;
        frames?: number;
        out?: boolean;
        shape?: number;
        curve?: number;
        wait?: boolean;
        timeout?: number;
    } = {}): Promise<void> {
        await this.gain(out ? 1 : 0, {
            start,
            frames,
            to: out ? 0 : 1,
            shape,
            curve,
            wait,
            timeout,
        });
    }

    /**
     * Silences a span, on exact zeros (`/buffer_gain` with both ends at 0).
     * {@link zero} is the same thing over the whole buffer.
     */
    async silence({
        start = 0,
        frames = -1,
        wait = true,
        timeout,
    }: {
        start?: number;
        frames?: number;
        wait?: boolean;
        timeout?: number;
    } = {}): Promise<void> {
        await this.gain(0, { start, frames, to: 0, wait, timeout });
    }

    /**
     * Reverses a span of this buffer in place (`/buffer_reverse`).
     *
     * Frames are reversed, not samples: a stereo pair stays a stereo pair.
     * `start` and `frames` are frames, `frames: -1` to the end.
     */
    async reverse({
        start = 0,
        frames = -1,
        wait = true,
        timeout,
    }: {
        start?: number;
        frames?: number;
        wait?: boolean;
        timeout?: number;
    } = {}): Promise<void> {
        await this.edit(
            "/buffer_reverse",
            [
                ["i", start],
                ["i", frames],
            ],
            wait,
            timeout,
        );
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
     * `setSamples` is the way back, so an editor view can read, edit and write.
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
            // /buffer_getRange.reply: bufnum, start, blob -- the samples arrive
            // as bytes and are unpacked in one native call, never per sample.
            const run = blobToSamples(msg.args[2] as Uint8Array);
            if (run.length === 0) break; // past the end: the server has no more
            out.set(run.subarray(0, Math.min(run.length, total - got)), got);
            got += run.length;
        }
        return got === total ? out : out.subarray(0, got);
    }

    /**
     * Writes interleaved samples into this buffer (`/buffer_setRange`), in
     * chunks — the write half of `getSamples`, and the step that closes an
     * editor's read → edit → write cycle.
     *
     * `samples` is laid down from flat index `start`, so a stereo buffer is
     * written interleaved `L R L R …`, exactly as it reads back. The samples
     * cross as one little-endian `f32` blob per chunk rather than as float
     * arguments — the protocol's rule for bulk data, and what makes writing a
     * multi-megabyte edit a byte copy instead of a per-sample encode. The buffer
     * must already exist and keeps its shape: writing past its end rejects
     * rather than being clamped, since a short write would lose samples the
     * caller believes it stored.
     *
     * The shape comes from the server's mirror, so a write immediately after
     * `alloc` needs that alloc to have completed — which awaiting it already
     * guarantees. `chunk` sizes each round trip and defaults to the
     * transport's bound, exactly as in `getSamples`.
     */
    async setSamples(
        samples: ArrayLike<number>,
        {
            start = 0,
            chunk,
            wait = true,
            timeout,
        }: { start?: number; chunk?: number; wait?: boolean; timeout?: number } = {},
    ): Promise<void> {
        if (samples.length === 0) return;
        const server = this.srv();
        const step = chunk ?? (await server.bulkChunk(timeout));
        const floats = samples instanceof Float32Array
            ? samples
            : Float32Array.from(samples as ArrayLike<number>);
        for (let at = 0; at < floats.length; at += step) {
            // One native pack per chunk: the samples cross as bytes, so nothing
            // here or in the OSC encoder touches them one at a time.
            server.sendMsg(
                "/buffer_setRange",
                ["i", this.bufnum],
                ["i", start + at],
                samplesToBlob(floats.subarray(at, at + step)),
            );
        }
        // One barrier for the whole batch rather than a /done per chunk: the
        // queue completes them in order anyway, so awaiting per chunk would
        // cost a round trip per chunk -- time proportional to the edit's
        // *length* instead of its size.
        if (wait) await server.barrier(timeout);
    }

    /**
     * Writes one sample by flat index (`/buffer_set`) — the single-sample
     * counterpart of `setSamples`, for a touch-up that does not deserve a run.
     */
    async setSample(
        index: number,
        value: number,
        { wait = true, timeout }: { wait?: boolean; timeout?: number } = {},
    ): Promise<void> {
        const args: MsgArg[] = [["i", this.bufnum], ["i", index], ["f", value]];
        if (!wait) {
            this.srv().sendMsg("/buffer_set", ...args);
            return;
        }
        await this.srv().command("/buffer_set", args, timeout);
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

    constructor(size = NUM_BUFFERS, share?: IdShare) {
        this.size = size;
        this.registry = new Registry(...shareOf(0, size, share));
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
