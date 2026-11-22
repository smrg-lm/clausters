// The `buf` builders that mirror the wire, re-exported by `buf.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/buf.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * Mono buffer player with linear interpolation; `rate` is frames per
 * output sample (1.0 = server rate).
 *
 * Playing starts at `startPos` and a rising `trigger` re-cues there,
 * so one player is a re-usable voice rather than a one-use node. Without
 * `loop`, reaching the end stops it and fires `doneAction` (2 frees the
 * synth, so a one-shot leaves the tree by itself); a looping player never
 * finishes, so the action never fires.
 */
export const playBuf = (
    bufnum: Channel,
    chan: Channel = 0.0,
    rate: Channel = 1.0,
    loop: Channel = 0.0,
    trigger: Channel = 0.0,
    startPos: Channel = 0.0,
    doneAction: Channel = 0.0,
): Ugen => new Ugen("PlayBuf", [bufnum, chan, rate, loop, trigger, startPos, doneAction]);

/** Reads a buffer at a `phase` signal in frames (linear interpolation). */
export const bufRd = (
    bufnum: Channel,
    chan: Channel,
    phase: Channel,
    loop: Channel = 0.0,
    interpolation: Channel = 2.0,
): Ugen => new Ugen("BufRd", [bufnum, chan, phase, loop, interpolation]);

/**
 * Reads a buffer **at the transport's position**: `bufRd` at
 * `(position - offset) * rate + start` frames, the phase worked out inside
 * the UGen in double precision.
 *
 * `offset` is where frame `start` sits on the transport's axis (as
 * `transportPos` takes it), and `rate` the buffer's frames per frame of
 * the transport -- `bufRateScale` for a buffer at another rate, times a
 * playrate. Like `transportPos` it seeks, loops and holds with the
 * transport.
 *
 * It is one UGen rather than `bufRd` with `transportPos` as its phase
 * because a phase handed between UGens is a 32-bit signal, which past about
 * six minutes at 48 kHz repeats and drops frames: a long take read that way
 * distorts more the further it plays. Here nothing that changes per sample
 * is rounded to 32 bits.
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
 * Plays a **fed buffer at the transport's position**: the reader of a
 * take an application that holds its engine reads itself and hands over
 * through a ring. Over any other buffer it is silent; a buffer of the pool
 * is read at the transport's position by `transportBufRd`.
 *
 * It follows the transport as `transportBufRd` does -- `offset` is where
 * frame `start` sits on the transport's axis. A locate is a short silence
 * and never a stale frame; an owner that cannot keep up plays silence too,
 * counted in `ServerStatus.starvedSamples`. Silent outside the stream and
 * while the transport is stopped.
 *
 * Mono, like every buffer reader: `chan` picks one channel of the stream.
 */
export const transportDiskIn = (
    bufnum: Channel,
    chan: Channel,
    offset: Channel = 0.0,
    start: Channel = 0.0,
    loop: Channel = 0.0,
): Ugen => new Ugen("TransportDiskIn", [bufnum, chan, offset, start, loop]);

/**
 * **Writes** `signal` into a buffer at `phase` (frames) -- `bufRd`'s
 * write-side twin, and stateless in the same way.
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
 * Interpolating wavetable oscillator. `bufnum` must hold a
 * **wavetable-format** buffer (fill it with `buf.gen(...)` and a
 * `/bufferGen` command whose wavetable flag is set); `phase` is an offset in
 * radians.
 */
export const osc = (bufnum: Channel, freq: Channel = 440.0, phase: Channel = 0.0): Ugen =>
    new Ugen("Osc", [bufnum, freq, phase]);

/**
 * Non-interpolating oscillator over a **plain** (non-wavetable) buffer;
 * rawer and cheaper than `osc`.
 */
export const oscN = (
    bufnum: Channel,
    freq: Channel = 440.0,
    phase: Channel = 0.0,
): Ugen => new Ugen("OscN", [bufnum, freq, phase]);

/**
 * Like `osc` but the buffer number is a signal: reads wavetables
 * `bufpos` and `bufpos + 1` and crossfades by the fractional part, so
 * sweeping `bufpos` morphs a bank of adjacent tables (allocate them
 * contiguously, all the same size).
 */
export const vosc = (
    bufpos: Channel,
    freq: Channel = 440.0,
    phase: Channel = 0.0,
): Ugen => new Ugen("VOsc", [bufpos, freq, phase]);

/**
 * Waveshaper: maps `signal` (in +-1, clamped) through a transfer table
 * in wavetable format (typically a `cheby` `/bufferGen`); the table's first
 * point is `signal = -1`, its last `signal = +1`.
 */
export const shaper = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("Shaper", [bufnum, signal]);

/** The engine sample rate in Hz, computed once at init (`ir`). */
export const sampleRate = (): Ugen => new Ugen("SampleRate", [], { rate: "ir" });

/** The number of frames in a buffer, block-constant (`kr`). */
export const bufFrames = (bufnum: Channel): Ugen =>
    new Ugen("BufFrames", [bufnum], { rate: "kr" });

/** The buffer's own sample rate (Hz), block-constant (`kr`). */
export const bufSampleRate = (bufnum: Channel): Ugen =>
    new Ugen("BufSampleRate", [bufnum], { rate: "kr" });

/**
 * `fileSr / serverSr`, block-constant (`kr`); feed `playBuf`'s
 * `rate` (`bufRateScale(buf) * pitch`) to play at the file's true pitch
 * without the client knowing either rate.
 */
export const bufRateScale = (bufnum: Channel): Ugen =>
    new Ugen("BufRateScale", [bufnum], { rate: "kr" });

/** The buffer's channel count, block-constant (`kr`). */
export const bufChannels = (bufnum: Channel): Ugen =>
    new Ugen("BufChannels", [bufnum], { rate: "kr" });

/**
 * The buffer's duration in seconds (`frames / fileSr`), block-constant
 * (`kr`).
 */
export const bufDur = (bufnum: Channel): Ugen =>
    new Ugen("BufDur", [bufnum], { rate: "kr" });

/**
 * One uniform random value in `[lo, hi)`, drawn once at synth init and
 * held for the node's life (`ir`); `lo`/`hi` must be constants or `ir`.
 */
export const rand = (lo: Channel = 0.0, hi: Channel = 1.0): Ugen =>
    new Ugen("Rand", [lo, hi], { rate: "ir" });

/**
 * One random integer in `[lo, hi]`, both bounds included (and truncated),
 * drawn once at synth init and held.
 */
export const irand = (lo: Channel = 0.0, hi: Channel = 127.0): Ugen =>
    new Ugen("IRand", [lo, hi], { rate: "ir" });

/**
 * One value between `lo` and `hi` with equal chances per ratio (log-uniform,
 * so an octave is as likely as any other), drawn once at synth init and held.
 * `lo` and `hi` must have one sign and not be zero.
 */
export const expRand = (lo: Channel = 0.01, hi: Channel = 1.0): Ugen =>
    new Ugen("ExpRand", [lo, hi], { rate: "ir" });

/**
 * One value in `[lo, hi)` leaning towards `lo` (the lower of two uniform
 * draws), or towards `hi` when `minmax` is positive, drawn once at synth init
 * and held.
 */
export const linRand = (lo: Channel, hi: Channel, minmax: Channel = 0.0): Ugen =>
    new Ugen("LinRand", [lo, hi, minmax], { rate: "ir" });

/**
 * One value in `[lo, hi)`, the mean of `n` uniform draws: flat for 1, a
 * triangle for 2, nearer a bell the more there are. Drawn once at synth init
 * and held.
 */
export const nRand = (lo: Channel = 0.0, hi: Channel = 1.0, n: Channel = 0.0): Ugen =>
    new Ugen("NRand", [lo, hi, n], { rate: "ir" });

/**
 * A uniform random value in `[lo, hi)`, drawn again at each rising edge of
 * `trig` and held between; the first is drawn at the synth's start.
 */
export const tRand = (lo: Channel = 0.0, hi: Channel = 1.0, trig: Channel = 0.0): Ugen =>
    new Ugen("TRand", [lo, hi, trig]);

/**
 * A random integer in `[lo, hi]`, drawn again at each rising edge of `trig`
 * and held between; the first is drawn at the synth's start.
 */
export const tiRand = (
    lo: Channel = 0.0,
    hi: Channel = 127.0,
    trig: Channel = 0.0,
): Ugen => new Ugen("TIRand", [lo, hi, trig]);

/**
 * A log-uniform random value between `lo` and `hi`, drawn again at each
 * rising edge of `trig` and held between; the first is drawn at the synth's
 * start.
 */
export const tExpRand = (
    lo: Channel = 0.01,
    hi: Channel = 1.0,
    trig: Channel = 0.0,
): Ugen => new Ugen("TExpRand", [lo, hi, trig]);

/**
 * Lets each rising edge of `trig` through with probability `prob` (the
 * trigger's own value, for that sample), and outputs 0 otherwise.
 */
export const coinGate = (prob: Channel = 0.5, trig: Channel = 0.0): Ugen =>
    new Ugen("CoinGate", [prob, trig]);

/**
 * Chooses which of the server's 64 random generators this synth's random
 * UGens draw from, `id` 0 to 63 (0 until it runs). Synths drawing from
 * different generators are independent streams: a reseed of one leaves the
 * others alone, and they can run side by side in a parallel group. Outputs 0.
 */
export const randId = (id: Channel = 0.0): Ugen => new Ugen("RandID", [id]);

/**
 * At each rising edge of `trig`, reseeds the generator this synth draws from
 * with `seed`, so every synth drawing from it starts the same sequence again
 * -- how a group's randomness is restarted on purpose. At the initial rate it
 * reseeds once, when the synth starts. A def of its own does nothing else:
 * it is a root, and is not wrapped in an `out`. Outputs 0.
 */
export const randSeed = (trig: Channel = 0.0, seed: Channel = 56789.0): Ugen =>
    new Ugen("RandSeed", [trig, seed]);

/**
 * `vosc` read at three frequencies and summed: three detuned voices of one
 * morphing timbre.
 */
export const vosc3 = (
    bufpos: Channel,
    freq1: Channel = 110.0,
    freq2: Channel = 220.0,
    freq3: Channel = 440.0,
): Ugen => new Ugen("VOsc3", [bufpos, freq1, freq2, freq3]);

/**
 * `osc` twice, at `freq + beats/2` and `freq - beats/2`, summed: two voices
 * beating `beats` times a second, a chorus from one wavetable. The peak is
 * twice the table's.
 */
export const cosc = (
    bufnum: Channel,
    freq: Channel = 440.0,
    beats: Channel = 0.5,
): Ugen => new Ugen("COsc", [bufnum, freq, beats]);

/**
 * The buffer as a table: its value at `signal` truncated, the index clipped
 * to the table.
 */
export const index = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("Index", [bufnum, signal]);

/**
 * The buffer as a table read at a fractional index, linearly between the two
 * values either side, the index clipped to the table.
 */
export const indexL = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("IndexL", [bufnum, signal]);

/** `index` with the index wrapped around the table. */
export const wrapIndex = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("WrapIndex", [bufnum, signal]);

/** `index` with the index reflected at the table's ends. */
export const foldIndex = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("FoldIndex", [bufnum, signal]);

/**
 * The inverse of `indexL` over an ascending table: the fractional index at
 * which the table reaches `signal`.
 */
export const indexInBetween = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("IndexInBetween", [bufnum, signal]);

/** The index of the first table value equal to `signal`, or -1. */
export const detectIndex = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("DetectIndex", [bufnum, signal]);

/**
 * A scale degree to a key: the buffer holds one octave of the scale, and
 * degree `signal` (floored) is its step plus `octave` per whole octave up or
 * down. Degree -1 is the last step an octave down.
 */
export const degreeToKey = (
    bufnum: Channel,
    signal: Channel,
    octave: Channel = 12.0,
): Ugen => new Ugen("DegreeToKey", [bufnum, signal, octave]);

/** The samples in a control period. */
export const blockSize = (): Ugen => new Ugen("BlockSize", [], { rate: "ir" });

/** Control periods a second. */
export const controlRate = (): Ugen => new Ugen("ControlRate", [], { rate: "ir" });

/** Seconds in a control period. */
export const controlDur = (): Ugen => new Ugen("ControlDur", [], { rate: "ir" });

/** Seconds in a sample. */
export const sampleDur = (): Ugen => new Ugen("SampleDur", [], { rate: "ir" });

/** `2 pi` over the sample rate. */
export const radiansPerSample = (): Ugen =>
    new Ugen("RadiansPerSample", [], { rate: "ir" });

/**
 * Where within its first sample the timed bundle that started the synth fell, in `[0, 1)`: what places a grain between samples.
 */
export const subsampleOffset = (): Ugen =>
    new Ugen("SubsampleOffset", [], { rate: "ir" });

/** The server's audio buses. */
export const numAudioBuses = (): Ugen => new Ugen("NumAudioBuses", [], { rate: "ir" });

/** The server's control buses. */
export const numControlBuses = (): Ugen =>
    new Ugen("NumControlBuses", [], { rate: "ir" });

/** The hardware inputs: the audio buses after the outputs. */
export const numInputBuses = (): Ugen => new Ugen("NumInputBuses", [], { rate: "ir" });

/** The hardware outputs: audio buses from 0. */
export const numOutputBuses = (): Ugen => new Ugen("NumOutputBuses", [], { rate: "ir" });

/** The slots of the server's buffer pool. */
export const numBuffers = (): Ugen => new Ugen("NumBuffers", [], { rate: "ir" });

/** The id of the node this synth is. */
export const nodeId = (): Ugen => new Ugen("NodeID", [], { rate: "ir" });

/** The synths in the server's tree when the synth starts. */
export const numRunningSynths = (): Ugen =>
    new Ugen("NumRunningSynths", [], { rate: "ir" });

/** The number of samples in a buffer, every channel's: frames times channels. */
export const bufSamples = (bufnum: Channel): Ugen =>
    new Ugen("BufSamples", [bufnum], { rate: "kr" });
