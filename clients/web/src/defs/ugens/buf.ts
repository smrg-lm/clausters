// Buffers: playing them, reading them as tables, asking about them (mirrors
// `clausters/defs/ugens/buf.py`).
//
// The players (`playBuf`, `bufRd`), the table oscillators and waveshaper that
// read a `/buffer_gen` table, and the init-rate queries that report a buffer's
// shape -- or the engine's own rate -- into the graph.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * Mono buffer player with linear interpolation; `rate` is frames per output
 * sample (1.0 = server rate).
 *
 * Playing starts at `startPos` and a rising `trigger` re-cues there, so one
 * player is a re-usable voice rather than a one-use node. Without `loop`,
 * reaching the end stops it and fires `doneAction` (2 frees the synth, so a
 * one-shot leaves the tree by itself); a looping player never finishes, so the
 * action never fires.
 */
export const playBuf = (
    bufnum: Channel,
    chan: Channel = 0.0,
    rate: Channel = 1.0,
    loop: Channel = 0.0,
    trigger: Channel = 0.0,
    startPos: Channel = 0.0,
    doneAction: Channel = 0,
): Ugen =>
    new Ugen("PlayBuf", [bufnum, chan, rate, loop, trigger, startPos, doneAction]);

/** Reads a buffer at a `phase` signal in frames (linear interpolation). */
export const bufRd = (
    bufnum: Channel,
    chan: Channel,
    phase: Channel,
    loop: Channel = 0.0,
): Ugen => new Ugen("BufRd", [bufnum, chan, phase, loop]);

/**
 * Reads a buffer **at the transport's position**: `bufRd` at
 * `(position - offset) * rate + start` frames, the phase worked out inside
 * the UGen in double precision.
 *
 * `offset` is where frame `start` sits on the transport's axis (as
 * `transportPos` takes it), and `rate` the buffer's frames per frame of the
 * transport -- `bufRateScale` for a buffer at another rate, times a playrate.
 * Like `transportPos` it seeks, loops and holds with the transport.
 *
 * It is one UGen rather than `bufRd(..., transportPos())` because a phase
 * handed between UGens is a 32-bit signal, which past about six minutes at
 * 48 kHz repeats and drops frames: a long take read that way distorts more the
 * further it plays. Here nothing that changes per sample is rounded to 32 bits.
 *
 * ```ts
 * const take = transportBufRd(buf.bufnum, 0, 0, bufRateScale(buf.bufnum));
 * ```
 */
export const transportBufRd = (
    bufnum: Channel,
    chan: Channel,
    offset: Channel = 0.0,
    rate: Channel = 1.0,
    start: Channel = 0.0,
    loop: Channel = 0.0,
): Ugen => new Ugen("TransportBufRd", [bufnum, chan, offset, rate, start, loop]);

/**
 * Plays a buffer **from disk, at the transport's position**: the reader of a
 * join with parts over files (`Buffer.stitch` given a path), which no
 * random-access reader plays.
 *
 * It follows the transport as `transportBufRd` does -- `offset` is where frame
 * `start` sits on the transport's axis -- and reads ahead of it on a thread of
 * its own, so a seam between two files is a read made before the playhead gets
 * there. A locate is a short silence and never a stale frame; a disk that
 * cannot keep up plays silence too, counted in `ServerStatus.starvedSamples`.
 * Each part carries its own rate, so there is no `rate` here. Silent outside
 * the join and while the transport is stopped.
 *
 * Mono, like every buffer reader: `chan` picks one channel of the join.
 *
 * ```ts
 * const take = transportDiskIn(join.bufnum, 0, clipStart);
 * ```
 */
export const transportDiskIn = (
    bufnum: Channel,
    chan: Channel,
    offset: Channel = 0.0,
    start: Channel = 0.0,
    loop: Channel = 0.0,
): Ugen => new Ugen("TransportDiskIn", [bufnum, chan, offset, start, loop]);

/**
 * **Writes** `signal` into a buffer at `phase` (frames) -- `bufRd`'s write-side
 * twin, and stateless in the same way.
 *
 * No interpolation: the write lands on the frame the phase names, truncated.
 * Spreading one sample over two frames would store a value the signal never
 * had, and consecutive writes would fight over the same cells.
 *
 * Passes `signal` through as its output, so a chain can go on using what it
 * just recorded without a second wire. Out-of-range phases wrap with `loop`
 * and write nothing otherwise.
 */
export const bufWr = (
    bufnum: Channel,
    chan: Channel,
    phase: Channel,
    loop: Channel,
    signal: Channel,
): Ugen => new Ugen("BufWr", [bufnum, chan, phase, loop, signal]);

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
 * Interpolating wavetable oscillator; `bufnum` must hold a
 * **wavetable-format** buffer.
 */
export const osc = (
    bufnum: Channel,
    freq: Channel = 440.0,
    phase: Channel = 0.0,
): Ugen => new Ugen("Osc", [bufnum, freq, phase]);

/** Non-interpolating oscillator over a **plain** (non-wavetable) buffer. */
export const oscN = (
    bufnum: Channel,
    freq: Channel = 440.0,
    phase: Channel = 0.0,
): Ugen => new Ugen("OscN", [bufnum, freq, phase]);

/**
 * Like `osc` but the buffer number is a signal: reads wavetables `bufpos`
 * and `bufpos + 1` and crossfades by the fractional part.
 */
export const vosc = (
    bufpos: Channel,
    freq: Channel = 440.0,
    phase: Channel = 0.0,
): Ugen => new Ugen("VOsc", [bufpos, freq, phase]);

/**
 * Waveshaper: maps `signal` (in +/-1, clamped) through a transfer table in
 * wavetable format (typically a `cheby` `/buffer_gen`).
 */
export const shaper = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("Shaper", [bufnum, signal]);

/** The number of frames in a buffer, block-constant (`kr`). */
export const bufFrames = (bufnum: Channel): Ugen =>
    new Ugen("BufFrames", [bufnum], { rate: "kr" });

/** The buffer's own sample rate (Hz), block-constant (`kr`). */
export const bufSampleRate = (bufnum: Channel): Ugen =>
    new Ugen("BufSampleRate", [bufnum], { rate: "kr" });

/**
 * `fileSr / serverSr`, block-constant (`kr`); feed `playBuf`'s `rate` to
 * play at the file's true pitch without the client knowing either rate.
 */
export const bufRateScale = (bufnum: Channel): Ugen =>
    new Ugen("BufRateScale", [bufnum], { rate: "kr" });

/** The buffer's channel count, block-constant (`kr`). */
export const bufChannels = (bufnum: Channel): Ugen =>
    new Ugen("BufChannels", [bufnum], { rate: "kr" });

/** The buffer's duration in seconds, block-constant (`kr`). */
export const bufDur = (bufnum: Channel): Ugen =>
    new Ugen("BufDur", [bufnum], { rate: "kr" });

// --- scalar / init-rate (ir) ---

// --- scalar / init-rate (ir) ---

/** The engine sample rate in Hz, computed once at init (`ir`). */
export const sampleRate = (): Ugen => new Ugen("SampleRate", [], { rate: "ir" });

/**
 * One uniform random value in `[lo, hi)`, drawn once at synth init and held
 * for the node's life (`ir`); `lo`/`hi` must be constants or `ir`.
 */
export const rand = (lo: Channel = 0.0, hi: Channel = 1.0): Ugen =>
    new Ugen("Rand", [lo, hi], { rate: "ir" });
