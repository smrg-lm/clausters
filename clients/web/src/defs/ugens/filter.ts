// Filters, delays and smoothers: what a signal is put through (mirrors
// `clausters/defs/ugens/filter.py`).
//
// One state-variable implementation stands behind every two-pole name, one
// delay line behind the nine `delay`/`comb`/`allpass` forms (chosen by
// interpolation), and the one-pole smoothers that make a control move instead
// of jump.

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/** Resolves the mutually exclusive `rq`/`q` pair into a wire `rq`. */
function resonance(rq?: Channel, q?: Channel): Channel {
    if (q === undefined) return rq ?? 1.0;
    if (rq !== undefined) throw new TypeError("give either rq or q, not both");
    if (typeof q === "number") {
        if (q === 0) {
            throw new TypeError("q must be non-zero; use rq=0 for infinite Q");
        }
        return 1.0 / q;
    }
    return q.recip();
}

/** The resonance of the two-pole filters: `rq` (1/Q, 0 = infinite) or `q`. */
export interface Resonance {
    rq?: Channel;
    q?: Channel;
}

/** Second-order Butterworth lowpass: −3 dB at `freq`, −12 dB/octave. */
export const lpf = (signal: Channel, freq: Channel = 440.0): Ugen =>
    new Ugen("LPF", [signal, freq]);

/** Second-order Butterworth highpass: −3 dB at `freq`, −12 dB/octave. */
export const hpf = (signal: Channel, freq: Channel = 440.0): Ugen =>
    new Ugen("HPF", [signal, freq]);

/** Resonant lowpass; unity gain at DC. */
export const rlpf = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("RLPF", [signal, freq, resonance(res.rq, res.q)]);

/** Resonant highpass; unity gain at Nyquist. */
export const rhpf = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("RHPF", [signal, freq, resonance(res.rq, res.q)]);

/** Bandpass with **unity gain at the centre**; `rq` is its bandwidth ratio. */
export const bpf = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("BPF", [signal, freq, resonance(res.rq, res.q)]);

/** Band reject (notch); unity gain in both passbands, a true null at `freq`. */
export const brf = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("BRF", [signal, freq, resonance(res.rq, res.q)]);

/** Resonator with unity gain at the peak — the same structure as `bpf`. */
export const resonz = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("Resonz", [signal, freq, resonance(res.rq, res.q)]);

/** One-pole filter: `coef` positive lowpasses, negative highpasses. */
export const onePole = (signal: Channel, coef: Channel = 0.5): Ugen =>
    new Ugen("OnePole", [signal, coef]);

/** One-zero filter: `coef` positive lowpasses, negative highpasses. */
export const oneZero = (signal: Channel, coef: Channel = 0.5): Ugen =>
    new Ugen("OneZero", [signal, coef]);

/**
 * Removes the DC offset with a very low corner — what a feedback loop or an
 * asymmetric waveshaper leaves behind.
 */
export const leakDc = (signal: Channel, coef: Channel = 0.995): Ugen =>
    new Ugen("LeakDC", [signal, coef]);

/** A leaky integrator: `y[n] = x[n] + coef·y[n-1]`. */
export const integrator = (signal: Channel, coef: Channel = 0.999): Ugen =>
    new Ugen("Integrator", [signal, coef]);

// --- delay lines ---
//
// `maxDelay` is **static**: it sizes the line the server allocates when the
// synth is built, so it cannot grow later and a `delaytime` past it is
// clamped. Left unset it follows a constant `delaytime`, which is what a
// fixed delay wants; a *modulated* delaytime has to state its longest reach.

function lineSize(
    kind: string,
    delaytime: Channel,
    maxDelay?: number,
): Record<string, unknown> {
    if (maxDelay === undefined) {
        if (typeof delaytime !== "number") {
            throw new TypeError(
                `${kind}: a modulated delaytime needs an explicit maxDelay ` +
                    "(it sizes the line, and the line is allocated once)",
            );
        }
        maxDelay = delaytime;
    }
    return { max_delay: Number(maxDelay) };
}

/** Pure delay, no interpolation: the delay is rounded to whole samples. */
export const delayN = (
    signal: Channel,
    delaytime: Channel = 0.2,
    maxDelay?: number,
): Ugen =>
    new Ugen("DelayN", [signal, delaytime], {
        static: lineSize("DelayN", delaytime, maxDelay),
    });

/** Delay with linear interpolation — the one a modulated delaytime wants. */
export const delayL = (
    signal: Channel,
    delaytime: Channel = 0.2,
    maxDelay?: number,
): Ugen =>
    new Ugen("DelayL", [signal, delaytime], {
        static: lineSize("DelayL", delaytime, maxDelay),
    });

/** Delay with cubic interpolation: smoother under modulation than `delayL`. */
export const delayC = (
    signal: Channel,
    delaytime: Channel = 0.2,
    maxDelay?: number,
): Ugen =>
    new Ugen("DelayC", [signal, delaytime], {
        static: lineSize("DelayC", delaytime, maxDelay),
    });

/**
 * Comb filter (feedback delay), no interpolation. `decaytime` is the time to
 * fall 60 dB; negative inverts the feedback.
 */
export const combN = (
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("CombN", [signal, delaytime, decaytime], {
        static: lineSize("CombN", delaytime, maxDelay),
    });

/** Comb filter with linear interpolation. */
export const combL = (
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("CombL", [signal, delaytime, decaytime], {
        static: lineSize("CombL", delaytime, maxDelay),
    });

/** Comb filter with cubic interpolation. */
export const combC = (
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("CombC", [signal, delaytime, decaytime], {
        static: lineSize("CombC", delaytime, maxDelay),
    });

/** Schroeder allpass (the reverb building block), no interpolation. */
export const allpassN = (
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("AllpassN", [signal, delaytime, decaytime], {
        static: lineSize("AllpassN", delaytime, maxDelay),
    });

/** Schroeder allpass with linear interpolation. */
export const allpassL = (
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("AllpassL", [signal, delaytime, decaytime], {
        static: lineSize("AllpassL", delaytime, maxDelay),
    });

/** Schroeder allpass with cubic interpolation. */
export const allpassC = (
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    maxDelay?: number,
): Ugen =>
    new Ugen("AllpassC", [signal, delaytime, decaytime], {
        static: lineSize("AllpassC", delaytime, maxDelay),
    });

// --- one-pole smoothers ---

// --- one-pole smoothers ---

/**
 * One-pole smoother: `signal` lagged over `time` seconds (symmetric); `time`
 * 0 passes through. The same UGen the server inserts for a lagged control.
 */
export const lag = (signal: Channel, time: Channel = 0.1): Ugen =>
    new Ugen("Lag", [signal, time]);

/** One-pole smoother with separate rise (`up`) and fall (`down`) times. */
export const varLag = (
    signal: Channel,
    up: Channel = 0.1,
    down: Channel = 0.1,
): Ugen => new Ugen("VarLag", [signal, up, down]);
