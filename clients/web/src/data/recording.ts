// A recording, followed from the page: the overview arrives, the samples never do.
//
// Every other way samples reaches a picture announces itself — a client sends
// samples, a peer edits a span and says so. A recording does not: a `RecordBuf`
// fills a buffer block by block from the audio thread, which is the one place
// that must never send a message. What the writer publishes instead is how far
// it has got, into the server's shared memory — and a page maps nothing, so
// that number is exactly what it cannot read.
//
// `/buffer_stream` is that reading, for whoever cannot map: the server sends
// the *overview* of the frames that appeared (min, max and mean square per
// bucket — the peak pyramid's own three statistics), at about a hundredth of
// the audio's bandwidth. This class is the receiving end: one `Peaks` per
// buffer, growing as the reports land, drawn like any other pyramid.

import type { Server } from "../defs/server/index.ts";
import type { Buffer } from "../defs/buffer.ts";
import { OscFunc, type ResponderMessage } from "../responders.ts";
import { Peaks } from "./peaks.ts";

/** The default report cadence: 20 a second, finer than a take grows. */
export const RECORDING_PERIOD_MS = 50;

/** A take being recorded, as this stream needs to know it. */
export interface TakeShape {
    bufnum: number;
    /** Frames per channel — the buffer's full length, not what is written. */
    frames: number;
    channels: number;
}

/** A `Buffer` handle or the shape spelled out. */
export type TakeLike = Buffer | TakeShape;

function shapeOf(take: TakeLike): TakeShape {
    return { bufnum: take.bufnum, frames: take.frames, channels: take.channels };
}

/**
 * Follows takes as they record, over `/buffer_stream`.
 *
 * ```ts
 * const take = await Buffer.alloc(10 * 48000, 1, { server });
 * const stream = await data.RecordingStream.open(server, [take]);
 * stream.onReport((bufnum) => draw(stream.peaks(bufnum), stream.written(bufnum)));
 * new Synth("record_something", { buf: take.bufnum }, { server });
 * ```
 *
 * Each take gets a pyramid **allocated at its full length** and empty: a take's
 * picture is the whole of the box it will fill, so the axis does not move while
 * it fills. Reports write the buckets that were measured and nothing else, so
 * what has not been recorded reads as the silence the buffer is — draw only up
 * to `written` to tell the two apart, which is what the GUI host's `fills` prop
 * does for a host-drawn view.
 *
 * **Only the overview arrives.** Zoomed in past the base bucket a page has its
 * own copy of the samples and the wire carried none, so the fine regime is
 * silent: to edit or play what was recorded, read it back with
 * `Server.getSamples` once the take is finished.
 *
 * One subscription per client, and the server **replaces** it on every call —
 * so a page whose GUI host is also following a recording (a `waveform` with
 * `fills`) must not open one of these beside it: the two would cancel each
 * other. Opening this stream is the choice to draw the take yourself.
 */
export class RecordingStream {
    readonly server: Server;
    /** The buckets each report is measured over, and the pyramids' own. */
    readonly bucket: number;
    /** Reports applied so far — a view can tell a repaint from a stall. */
    reports = 0;

    private takes = new Map<number, { peaks: Peaks; written: number }>();
    private listeners = new Set<(bufnum: number, stream: RecordingStream) => void>();
    private responder: OscFunc | null = null;

    private constructor(server: Server, bucket: number) {
        this.server = server;
        this.bucket = bucket;
    }

    /**
     * Subscribes to `takes` and resolves once the server has acked. A
     * subscription watches what happens **next**: samples already recorded is
     * a read (`Server.getSamples`), not a stream.
     */
    static async open(
        server: Server,
        takes: readonly TakeLike[],
        { periodMs = RECORDING_PERIOD_MS, baseBucket = 256, timeout = 5.0 } = {},
    ): Promise<RecordingStream> {
        const stream = new RecordingStream(server, baseBucket);
        for (const take of takes) {
            const { bufnum, frames, channels } = shapeOf(take);
            // Empty rather than built over silence: a ten-minute stereo take
            // would be 230 MB of zeros to summarize what nobody wrote.
            const peaks = Peaks.empty(frames, {
                channels: Math.max(1, channels),
                baseBucket,
            });
            stream.takes.set(bufnum, { peaks, written: 0 });
        }
        stream.responder = new OscFunc(
            (msg) => stream.take(msg),
            "/buffer_stream.reply",
            { recv: server.receiver },
        );
        try {
            await server.streamBuffers(
                periodMs,
                [...stream.takes.keys()],
                baseBucket,
                timeout,
            );
        } catch (error) {
            stream.free();
            throw error;
        }
        return stream;
    }

    /** The pyramid of one take, or `undefined` when it is not in this stream. */
    peaks(take: TakeLike | number): Peaks | undefined {
        return this.takes.get(typeof take === "number" ? take : take.bufnum)?.peaks;
    }

    /**
     * How far one take has been reported, in frames — the end of the last
     * whole bucket the writer had filled. Past it the pyramid is the silence
     * the buffer was allocated as, so this is where a trace should stop.
     */
    written(take: TakeLike | number): number {
        return this.takes.get(typeof take === "number" ? take : take.bufnum)?.written ?? 0;
    }

    /**
     * Calls `handler` with each take that grew, as its report lands; returns
     * the unsubscribe. The handler runs from the reply dispatch, so keep it to
     * storing and drawing — never a round trip.
     */
    onReport(handler: (bufnum: number, stream: RecordingStream) => void): () => void {
        this.listeners.add(handler);
        return () => this.listeners.delete(handler);
    }

    /**
     * Cancels the subscription on the server and stops decoding. The pyramids
     * stay readable — a finished take is still a picture — until `free`.
     */
    async stop(timeout = 5.0): Promise<void> {
        this.responder?.free();
        this.responder = null;
        this.listeners.clear();
        await this.server.streamBuffers(0, [], this.bucket, timeout);
    }

    /** Releases the wasm-side pyramids. The stream is unusable afterwards. */
    free(): void {
        this.responder?.free();
        this.responder = null;
        this.listeners.clear();
        for (const { peaks } of this.takes.values()) peaks.free();
        this.takes.clear();
    }

    /** One `/buffer_stream.reply bufnum startFrame bucket blob` into a pyramid. */
    private take(msg: ResponderMessage): void {
        // The address is `msg[0]`, so the first argument is `msg[1]` — the
        // reference client's shape, and what every responder here reads.
        const [, bufnum, startFrame, bucket, blob] = msg;
        const entry = this.takes.get(Number(bufnum));
        if (!entry || !(blob instanceof Uint8Array)) return;
        if (!entry.peaks.writeBuckets(Number(startFrame), Number(bucket), blob)) return;
        const channels = Math.max(1, entry.peaks.channels);
        const buckets = Math.floor(blob.byteLength / 4 / (channels * 3));
        entry.written = Number(startFrame) + buckets * Number(bucket);
        this.reports++;
        for (const handler of [...this.listeners]) handler(Number(bufnum), this);
    }
}
