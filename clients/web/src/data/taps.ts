// Audio buses, streamed to the script.
//
// A control bus carries one value per block; an oscilloscope, a phasescope and
// a spectrum need the samples themselves. An audio bus does not sit in shared
// memory the way a control bus does, so the server records the ones it is
// asked for and a page reads them as `/bus_tapStream.reply` — the newest window of each
// subscribed bus, every period. **A script names the bus**: the subscription
// is itself the request to record it, and the ring behind it is the server's
// bookkeeping.
//
// What this module keeps that a raw subscription does not is **the sample
// axis**: every window arrives with its `endPosition`, the total samples ever
// recorded at the window's end, so consecutive snapshots can be placed on the
// bus's own timeline — they overlap or gap by exactly the position delta,
// never by a guess about the period.
//
// **What is not here: the trace.** Framing a display window and aligning it on
// a trigger so a periodic signal stands still is what an oscilloscope *draws*,
// and the drawing is the GUI host's — `scope(bus)`, or a `scope` widget in a
// GuiDef, which asks the server for the same tap and stands the trace still.
// What a script does with a window here is measure it.

import type { Server } from "../defs/server/index.ts";
import { OscFunc } from "../responders.ts";
import type { ResponderMessage } from "../responders.ts";
import { STREAM_PERIOD_MS } from "./buses.ts";

/** One bus's newest window, on that bus's own sample axis. */
export interface TapWindow {
    /** The samples, oldest first. */
    samples: Float32Array;
    /** Total samples ever written to the ring at this window's end. */
    endPosition: number;
}

/**
 * A live view of a set of audio buses.
 *
 * ```ts
 * const taps = await TapStream.open(server, [left], { frames: 2048 });
 * taps.onData(() => {
 *     const [l, r] = deinterleave(taps.interleaved(left, 2), 2);
 *     report(correlation(l, r));
 * });
 * // ... later
 * await taps.stop();   // and the server stops recording
 * ```
 *
 * Opening the stream is what starts the recording and stopping it is what ends
 * it — there is no separate routing step, and no ring index anywhere.
 *
 * At most 8 buses per subscription, and one subscription per client — a second
 * `TapStream` on the same `Server` replaces the first (the server's rule), and
 * over the in-page carrier that client includes the GUI host, so a host
 * oscilloscope and this displace each other (see `BusStream`).
 * `frames` is clamped by the server to the transport's bound and to half the
 * ring, so a window may come back shorter than asked.
 */
export class TapStream {
    readonly server: Server;
    /** The audio buses watched. */
    readonly buses: readonly number[];
    /** Frames per window this stream asked for. */
    readonly frames: number;

    private windows = new Map<number, TapWindow>();
    private listeners = new Set<(bus: number, window: TapWindow) => void>();
    /** The responder decoding this stream's windows, while subscribed. */
    private responder: OscFunc | null = null;

    private constructor(server: Server, buses: readonly number[], frames: number) {
        this.server = server;
        this.buses = buses;
        this.frames = frames;
    }

    /** Subscribes to `buses`, resolving on the server's ack. */
    static async open(
        server: Server,
        buses: readonly number[],
        {
            frames = 2048,
            periodMs = STREAM_PERIOD_MS,
            timeout = 5.0,
        }: { frames?: number; periodMs?: number; timeout?: number } = {},
    ): Promise<TapStream> {
        const stream = new TapStream(server, [...buses], frames);
        stream.responder = new OscFunc(
            (msg) => stream.take(msg),
            "/bus_tapStream.reply",
            { recv: server.receiver },
        );
        try {
            await server.streamTaps(periodMs, frames, buses, timeout);
        } catch (error) {
            stream.detach();
            throw error;
        }
        return stream;
    }

    /**
     * One bus's newest window, or `undefined` before its first snapshot — a
     * bus whose recording has not filled a window yet sends nothing at all.
     */
    window(bus: number): TapWindow | undefined {
        return this.windows.get(bus);
    }

    /**
     * The newest windows of `count` adjacent buses from `first`, interleaved
     * frame-major (`L R L R …`) over the frames they share — the layout a
     * stereo view reads, and the one `lissajous` and `correlation` take.
     * Empty until every one of those buses has a window.
     */
    interleaved(first: number, count: number): Float32Array {
        const windows: TapWindow[] = [];
        for (let i = 0; i < count; i++) {
            const window = this.windows.get(first + i);
            if (!window) return new Float32Array(0);
            windows.push(window);
        }
        const frames = Math.min(...windows.map((w) => w.samples.length));
        const out = new Float32Array(frames * count);
        for (let f = 0; f < frames; f++) {
            for (let ch = 0; ch < count; ch++) {
                // Align on the newest sample of each: the windows may differ
                // in length, and what a stereo view pairs is the freshest.
                const samples = windows[ch].samples;
                out[f * count + ch] = samples[samples.length - frames + f];
            }
        }
        return out;
    }

    /**
     * Calls `handler` with each window as it lands (one call per tap per
     * period); returns the unsubscribe.
     */
    onData(handler: (bus: number, window: TapWindow) => void): () => void {
        this.listeners.add(handler);
        return () => this.listeners.delete(handler);
    }

    /**
     * Cancels the subscription, which is also what stops the recording: the
     * server drops the watch this stream held on each of its buses.
     */
    async stop(timeout = 5.0): Promise<void> {
        this.detach();
        await this.server.streamTaps(0, 0, [], timeout);
    }

    private detach(): void {
        this.responder?.free();
        this.responder = null;
        this.listeners.clear();
    }

    /** One `/bus_tapStream.reply bus endPosition blob` snapshot. */
    private take(msg: ResponderMessage): void {
        const bus = Number(msg[1]);
        if (!this.buses.includes(bus)) return;
        const blob = msg[3];
        if (!(blob instanceof Uint8Array)) return;
        const window: TapWindow = {
            samples: decodeSamples(blob),
            endPosition: Number(msg[2]),
        };
        this.windows.set(bus, window);
        for (const handler of [...this.listeners]) handler(bus, window);
    }
}

/**
 * A `/bus_tapStream.reply` blob — raw little-endian `f32` — as samples. Read through a
 * `DataView`, so the endianness is the wire's and not the machine's, and an
 * unaligned blob offset is a non-issue.
 */
export function decodeSamples(blob: Uint8Array): Float32Array {
    const view = new DataView(blob.buffer, blob.byteOffset, blob.byteLength);
    const out = new Float32Array(Math.floor(blob.byteLength / 4));
    for (let i = 0; i < out.length; i++) out[i] = view.getFloat32(i * 4, true);
    return out;
}
