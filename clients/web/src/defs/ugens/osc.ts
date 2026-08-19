// Oscillators and noise: what a graph starts from (mirrors
// `clausters/defs/ugens/osc.py`).
//
// The band-limited pair (`saw`, `pulse`) beside the naive `lf*` forms, the
// noise family by spectrum, and `phasor` as the ramp the table readers run on.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/** Sine by f64 phase accumulation, starting at phase 0. */
export const sine = (freq: Channel = 440.0): Ugen => new Ugen("Sine", [freq]);

/**
 * A single-sample `1.0` every `freq` Hz, `0.0` between (`freq` 0 = one
 * impulse then silence). The first sample is always an impulse.
 */
export const impulse = (freq: Channel = 1.0): Ugen => new Ugen("Impulse", [freq]);

/** Uniform white noise in ±1. */
export const whiteNoise = (): Ugen => new Ugen("WhiteNoise", []);

/** Noise with equal power per octave (−3 dB/octave). */
export const pinkNoise = (): Ugen => new Ugen("PinkNoise", []);

/** Brownian noise (−6 dB/octave): a bounded random walk. */
export const brownNoise = (): Ugen => new Ugen("BrownNoise", []);

/** Noise whose spectrum is flat to the *ear* rather than to a meter. */
export const grayNoise = (): Ugen => new Ugen("GrayNoise", []);

/** Noise that is only ever −1 or +1: white noise hard-clipped. */
export const clipNoise = (): Ugen => new Ugen("ClipNoise", []);

/** Steps to a new random value `freq` times a second, holding it between. */
export const lfNoise0 = (freq: Channel = 500.0): Ugen => new Ugen("LFNoise0", [freq]);

/** Ramps linearly between random values at `freq` per second. */
export const lfNoise1 = (freq: Channel = 500.0): Ugen => new Ugen("LFNoise1", [freq]);

/** Quadratically interpolated random values at `freq` per second. */
export const lfNoise2 = (freq: Channel = 500.0): Ugen => new Ugen("LFNoise2", [freq]);

/** `lfNoise0`, clipped: steps between −1 and +1 only. */
export const lfClipNoise = (freq: Channel = 500.0): Ugen =>
    new Ugen("LFClipNoise", [freq]);

/** Random impulses in 0..1 at an average `density` per second. */
export const dust = (density: Channel = 1.0): Ugen => new Ugen("Dust", [density]);

/** `dust` with bipolar impulses (−1..1). */
export const dust2 = (density: Channel = 1.0): Ugen => new Ugen("Dust2", [density]);

/** A chaotic noise source (the logistic map); `chaos` in 0..2. */
export const crackle = (chaos: Channel = 1.5): Ugen => new Ugen("Crackle", [chaos]);

/** Band-limited sawtooth (PolyBLEP), falling from +1 to −1. */
export const saw = (freq: Channel = 440.0): Ugen => new Ugen("Saw", [freq]);

/** Band-limited pulse (PolyBLEP); `width` is the duty cycle in 0..1. */
export const pulse = (freq: Channel = 440.0, width: Channel = 0.5): Ugen =>
    new Ugen("Pulse", [freq, width]);

/** Naive (aliasing) sawtooth — cheap, meant for control rate. */
export const lfSaw = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("LFSaw", [freq, iphase]);

/** Naive (aliasing) pulse — cheap, meant for control rate. */
export const lfPulse = (
    freq: Channel = 440.0,
    iphase: Channel = 0.0,
    width: Channel = 0.5,
): Ugen => new Ugen("LFPulse", [freq, iphase, width]);

/** Naive (aliasing) triangle — cheap, meant for control rate. */
export const lfTri = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("LFTri", [freq, iphase]);

/**
 * A sawtooth whose peak position is `width`: from a ramp up through a
 * triangle to a ramp down.
 */
export const varSaw = (
    freq: Channel = 440.0,
    iphase: Channel = 0.0,
    width: Channel = 0.5,
): Ugen => new Ugen("VarSaw", [freq, iphase, width]);

/**
 * A ramp from `start` to `end` advancing by `rate` per sample, wrapping and
 * restarting at `resetPos` on each trigger — the phase source `bufRd` reads.
 */
export const phasor = (
    trig: Channel = 0.0,
    rate: Channel = 1.0,
    start: Channel = 0.0,
    end: Channel = 1.0,
    resetPos: Channel = 0.0,
): Ugen => new Ugen("Phasor", [trig, rate, start, end, resetPos]);

/**
 * The **transport's position in the piece**, in frames, minus `offset`.
 *
 * A buffer reader whose phase is this one follows the transport instead of
 * carrying a position of its own, so seeking (`Server.transportLocateSample`),
 * looping (`Server.transportLoop`) and pausing (`Server.transportStop` over a
 * governed group) belong to the transport and not to the def. That is the
 * shape a multitrack needs — many readers, one time — and it is why a locate
 * never has to reach into a node.
 *
 * It ramps one frame per sample while the transport rolls and holds while it
 * is stopped. `offset` is where this signal starts in the piece, so a clip
 * reads its own frame 0 when the transport reaches it; the subtraction happens
 * in double precision inside the UGen, which is what keeps the value exact
 * deep into a long piece (a signal is 32-bit, and past about six minutes at
 * 48 kHz it can no longer count single frames — subtracting afterwards with
 * `sub` has already lost that).
 *
 * ```ts
 * const take = bufRd(buf.bufnum, 0, transportPos());
 * ```
 */
export const transportPos = (offset: Channel = 0.0): Ugen =>
    new Ugen("TransportPos", [offset]);
