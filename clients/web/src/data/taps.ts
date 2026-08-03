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
// Two things this module keeps that a raw subscription does not:
//
// - **The sample axis.** Every window arrives with its `endPosition`, the
//   total samples ever recorded at the window's end, so consecutive snapshots
//   can be placed on the bus's own timeline: they overlap or gap by exactly
//   the position delta, never by a guess about the period.
// - **The trace.** A free-running window makes a periodic signal crawl across
//   the view; `scopeWindow` aligns it on a rising crossing with the core's own
//   trigger — the one the GUI host draws with, so the two traces agree.

import type { Server } from "../defs/server/index.ts";
import { OscFunc } from "../responders.ts";
import type { ResponderMessage } from "../responders.ts";
import {
    oscil_align,
    oscil_display_frames,
    oscil_raw_frames,
} from "../core/clausters_core_web.js";
import { STREAM_PERIOD_MS } from "./buses.ts";

/** One bus's newest window, on that bus's own sample axis. */
export interface TapWindow {
    /** The samples, oldest first. */
    samples: Float32Array;
    /** Total samples ever written to the ring at this window's end. */
    endPosition: number;
}

/** A triggered oscilloscope trace: the display window and whether it locked. */
export interface ScopeTrace {
    samples: Float32Array;
    /** `true` = the trigger fired; `false` = free-running on the newest data. */
    locked: boolean;
}

/**
 * A live view of a set of audio buses.
 *
 * ```ts
 * const taps = await TapStream.open(server, [bus], { frames: 2048 });
 * taps.onData((bus, window) => draw(scopeWindow(window.samples)));
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

/**
 * How many raw samples a `windowMs` trace needs — the display window plus the
 * trigger's search slack. What to ask `TapStream.open` for.
 */
export function scopeFrames(windowMs: number, sampleRate = 48000): number {
    return oscil_raw_frames(oscil_display_frames(windowMs, sampleRate));
}

/**
 * The triggered trace inside a raw window: the display window starting at
 * the latest rising crossing of `trigger` (so a periodic signal stands still),
 * falling back to the newest samples when there is no crossing — silence, DC,
 * or no rising edge — with `locked` saying which happened.
 *
 * The alignment is the core's, the same one the GUI host's `scope` widget
 * draws with, so a trace drawn here and one drawn by the host from the same
 * tap are the same trace.
 */
export function scopeWindow(
    raw: Float32Array,
    {
        windowMs = 20.0,
        sampleRate = 48000,
        trigger = 0.0,
    }: { windowMs?: number; sampleRate?: number; trigger?: number } = {},
): ScopeTrace {
    const display = oscil_display_frames(windowMs, sampleRate);
    const [start, locked] = oscil_align(raw, display, trigger);
    return {
        samples: raw.subarray(start, start + display),
        locked: locked === 1,
    };
}
