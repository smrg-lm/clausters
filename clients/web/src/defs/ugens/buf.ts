// Buffers: playing them, reading them as tables, asking about them (mirrors
// `clausters/defs/ugens/buf.py`).
//
// The players (`playBuf`, `bufRd`), the table oscillators and waveshaper that
// read a `/buffer_gen` table, and the init-rate queries that report a buffer's
// shape — or the engine's own rate — into the graph.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * Mono buffer player with linear interpolation; `rate` is frames per output
 * sample (1.0 = server rate).
 */
export const playBuf = (
    bufnum: Channel,
    chan: Channel = 0.0,
    rate: Channel = 1.0,
    loop: Channel = 0.0,
): Ugen => new Ugen("PlayBuf", [bufnum, chan, rate, loop]);

/** Reads a buffer at a `phase` signal in frames (linear interpolation). */
export const bufRd = (
    bufnum: Channel,
    chan: Channel,
    phase: Channel,
    loop: Channel = 0.0,
): Ugen => new Ugen("BufRd", [bufnum, chan, phase, loop]);

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
 * Waveshaper: maps `signal` (in ±1, clamped) through a transfer table in
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
