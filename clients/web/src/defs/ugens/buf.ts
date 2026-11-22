// Buffers: playing them, reading them as tables, asking about them (mirrors
// `clausters/defs/ugens/buf.py`).
//
// The players (`playBuf`, `bufRd`), the table oscillators and waveshaper that
// read a `/buffer_gen` table, and the init-rate queries that report a buffer's
// shape -- or the engine's own rate -- into the graph.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";
export * from "./_generated/buf.ts";

/**
 * **Records** `signal` into a buffer, one frame per sample -- the
 * self-advancing writer, as `playBuf` is the self-advancing reader.
 *
 * `recLevel` and `preLevel` are what make it a looper rather than a tape head:
 * each frame becomes `signal*recLevel + old*preLevel`, so `(1, 0)` overwrites,
 * `(1, 1)` overdubs onto what is there and `(1, 0.5)` overdubs with the older
 * layers fading.
 *
 * `run` at 0 holds the position and writes nothing, so a recording can be gated
 * without losing its place; a rising `trigger` re-cues to `offset`; without
 * `loop`, reaching the end stops the recording and fires `doneAction`. Passes
 * `signal` through.
 *
 * Recording into a buffer another node is playing is the ordinary case -- a
 * buffer's contents are mutable and only its shape is fixed.
 */
export const recordBuf = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    opts: {
        offset?: Channel;
        recLevel?: Channel;
        preLevel?: Channel;
        run?: Channel;
        loop?: Channel;
        trigger?: Channel;
        doneAction?: number;
    } = {},
): Ugen =>
    new Ugen("RecordBuf", [
        bufnum,
        chan,
        signal,
        opts.offset ?? 0.0,
        opts.recLevel ?? 1.0,
        opts.preLevel ?? 0.0,
        opts.run ?? 1.0,
        opts.loop ?? 0.0,
        opts.trigger ?? 0.0,
        opts.doneAction ?? 0,
    ]);

/**
 * At each rising edge of `trig`, an index drawn with the probabilities of
 * `weights` (summing to 1, or divided by their sum with `normalize` 1) -- the
 * weighted choice a `select` then reads a list by. A draw is made at the start
 * too. What unnormalized weights leave over goes to the last index.
 */
export const tWindex = (
    trig: Channel,
    weights: readonly Channel[],
    normalize: Channel = 0.0,
): Ugen => new Ugen("TWindex", [trig, normalize, ...weights]);
