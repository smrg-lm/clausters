// Audio taps, streamed to the script.
//
// A control bus carries one value per block; an oscilloscope, a phasescope and
// a spectrum need the samples themselves. That is what an audio tap is: a
// pre-allocated ring on the server that `/tap` routes an audio bus into, read
// natively out of shared memory and, in a page, streamed as `/tap_data` — the
// newest window of each subscribed tap, every period.
//
// Two things this module keeps that a raw subscription does not:
//
// - **The sample axis.** Every window arrives with the tap's `endPosition`,
//   the total samples ever written at the window's end, so consecutive
//   snapshots can be placed on the tap's own timeline: they overlap or gap by
//   exactly the position delta, never by a guess about the period.
// - **The trace.** A free-running window makes a periodic signal crawl across
//   the view; `scopeWindow` aligns it on a rising crossing with the core's own
//   trigger — the one the GUI host draws with, so the two traces agree.

import type { Server } from "../defs/server.ts";
import type { OscMessage } from "../base/osc.ts";
import {
    oscil_align,
    oscil_display_frames,
    oscil_raw_frames,
} from "../core/clausters_core_web.js";
import { STREAM_PERIOD_MS } from "./buses.ts";

/** One tap's newest window, on the tap's own sample axis. */
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
 * A live view of a set of audio taps.
 *
 * ```ts
 * const tap = server.taps.alloc();          // from the registry, never by hand
 * server.tap(tap, bus);                     // route the bus into the ring
 * const taps = await TapStream.open(server, [tap], { frames: 2048 });
 * taps.onData((index, window) => draw(scopeWindow(window.samples)));
 * // ... later
 * await taps.stop();
 * server.tap(tap, -1);
 * server.taps.free(tap);
 * ```
 *
 * At most 8 taps per subscription, and one subscription per client — a second
 * `TapStream` on the same `Server` replaces the first (the server's rule).
 * `frames` is clamped by the server to the transport's bound and to half the
 * ring, so a window may come back shorter than asked.
 */
export class TapStream {
    readonly server: Server;
    /** The tap indices watched. */
    readonly taps: readonly number[];
    /** Frames per window this stream asked for. */
    readonly frames: number;

    private windows = new Map<number, TapWindow>();
    private listeners = new Set<(tap: number, window: TapWindow) => void>();
    private unsubscribe: (() => void) | null = null;

    private constructor(server: Server, taps: readonly number[], frames: number) {
        this.server = server;
        this.taps = taps;
        this.frames = frames;
    }

    /** Subscribes to `taps`, resolving on the server's ack. */
    static async open(
        server: Server,
        taps: readonly number[],
        {
            frames = 2048,
            periodMs = STREAM_PERIOD_MS,
            timeout = 5.0,
        }: { frames?: number; periodMs?: number; timeout?: number } = {},
    ): Promise<TapStream> {
        const stream = new TapStream(server, [...taps], frames);
        stream.unsubscribe = server.onReply((msg) => stream.take(msg));
        try {
            await server.streamTaps(periodMs, frames, taps, timeout);
        } catch (error) {
            stream.detach();
            throw error;
        }
        return stream;
    }

    /**
     * One tap's newest window, or `undefined` before its first snapshot — a
     * tap whose ring has not filled a window yet sends nothing at all.
     */
    window(tap: number): TapWindow | undefined {
        return this.windows.get(tap);
    }

    /**
     * The newest windows of `count` adjacent taps from `first`, interleaved
     * frame-major (`L R L R …`) over the frames they share — the layout a
     * stereo view reads, and the one `lissajous` and `correlation` take.
     * Empty until every one of those taps has a window.
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
    onData(handler: (tap: number, window: TapWindow) => void): () => void {
        this.listeners.add(handler);
        return () => this.listeners.delete(handler);
    }

    /** Cancels the subscription. The taps keep running; `/tap … -1` stops one. */
    async stop(timeout = 5.0): Promise<void> {
        this.detach();
        await this.server.streamTaps(0, 0, [], timeout);
    }

    private detach(): void {
        this.unsubscribe?.();
        this.unsubscribe = null;
        this.listeners.clear();
    }

    /** One `/tap_data tap endPosition blob` snapshot. */
    private take(msg: OscMessage): void {
        if (msg.addr !== "/tap_data") return;
        const tap = Number(msg.args[0]);
        if (!this.taps.includes(tap)) return;
        const blob = msg.args[2];
        if (!(blob instanceof Uint8Array)) return;
        const window: TapWindow = {
            samples: decodeSamples(blob),
            endPosition: Number(msg.args[1]),
        };
        this.windows.set(tap, window);
        for (const handler of [...this.listeners]) handler(tap, window);
    }
}

/**
 * A `/tap_data` blob — raw little-endian `f32` — as samples. Read through a
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
 * The triggered trace inside a raw tap window: the display window starting at
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
