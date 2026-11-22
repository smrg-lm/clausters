// Filters, delays and smoothers: what a signal is put through (mirrors
// `clausters/defs/ugens/filter.py`).
//
// One state-variable implementation stands behind every two-pole name, one
// delay line behind the nine `delay`/`comb`/`allpass` forms (chosen by
// interpolation), and the one-pole smoothers that make a control move instead
// of jump.
//
// The nine delay forms also run over a **pool buffer** (`bufDelayN` and the
// rest): same interpolation, same feedback, same arithmetic; what differs is
// where the line lives. A pool buffer's contents are addressable, so the delay
// can be inspected, resampled, saved or played by another node while it runs
// -- the shared case, and the whole reason these exist beside the private
// family. No `maxDelay`: the buffer's own frame count is the bound. The buffer
// is **not** zeroed by the UGen (allocating and clearing it are yours), a
// missing buffer or a channel it does not have plays silence, and
// `bufnum`/`chan` being inputs means a line can be moved between buffers with
// a `node.set`.

import { ChannelList, Ugen, channelBinop, channelUnop, chans } from "./graph.ts";
import type { Channel } from "./graph.ts";
import { partials } from "./osc.ts";
export * from "./_generated/filter.ts";

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
    return q.reciprocal();
}

/** The resonance of the two-pole filters: `rq` (1/Q, 0 = infinite) or `q`. */
export interface Resonance {
    rq?: Channel;
    q?: Channel;
}

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

/** Resonator with unity gain at the peak -- the same structure as `bpf`. */
export const resonz = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
): Ugen => new Ugen("Resonz", [signal, freq, resonance(res.rq, res.q)]);

/**
 * The state-variable filter with its three tap gains as **signal inputs**, so
 * the response itself can be modulated.
 *
 * Every classic response is a mix of the three taps, and each of these is a
 * valid argument triple:
 *
 * | response | `low` | `band` | `high` |
 * |---|---|---|---|
 * | lowpass | 1 | 0 | 0 |
 * | bandpass (peak gain Q) | 0 | 1 | 0 |
 * | bandpass (unity peak) | 0 | `rq` | 0 |
 * | highpass | 0 | 0 | 1 |
 * | notch | 1 | 0 | 1 |
 * | peak | -1 | 0 | 1 |
 * | allpass | 1 | `-rq` | 1 |
 *
 * Sweeping between them costs the mix and nothing else: the three taps come
 * out of the same pair of integrator updates. See `svfMorph` for the one-knob
 * version.
 */
export const svf = (
    signal: Channel,
    freq: Channel = 440.0,
    res: Resonance = {},
    low: Channel = 1.0,
    band: Channel = 0.0,
    high: Channel = 0.0,
): Ugen =>
    new Ugen("Svf", [signal, freq, resonance(res.rq, res.q), low, band, high]);

/**
 * The `(low, band, high)` gains for a continuous lowpass -> bandpass ->
 * highpass sweep, to spread into `svf`: `svf(sig, freq, res, ...svfMorph(p))`.
 *
 * `pos` runs 0 -> 1 -> 2 and may be a signal, so the response becomes an
 * automation lane like any other. The ordering lives here rather than on the
 * wire, where committing to one arbitrary sequence of responses would exclude
 * every other (notch, peak, allpass are all reachable through `svf` itself).
 */
export function svfMorph(pos: Channel): [Channel, Channel, Channel] {
    const clamp01 = (x: Channel): Channel =>
        channelBinop(channelBinop(x, "max", 0.0), "min", 1.0);

    const low = clamp01(channelBinop(1.0, "sub", pos));
    const high = clamp01(channelBinop(pos, "sub", 1.0));
    // A triangle peaking at pos == 1: 1 - |pos - 1|.
    const band = clamp01(
        channelBinop(1.0, "sub", channelUnop(channelBinop(pos, "sub", 1.0), "abs")),
    );
    return [low, band, high];
}

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

/** Delay with linear interpolation -- the one a modulated delaytime wants. */
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

// --- smoothers ---

/**
 * A straight line to each new value of `signal`, taking `time` seconds.
 *
 * Where `lag` approaches a new value and never quite reaches it, this
 * arrives, in `time` and exactly; changing `time` partway rescales what is
 * left of the line. `start` is where the output begins, and is the signal
 * itself unless given -- a glide in from `start` when it is.
 */
export const varLag = (signal: Channel, time: Channel = 0.1, start?: Channel): Ugen =>
    new Ugen("VarLag", [signal, time, start ?? signal]);

/**
 * A bank of fixed resonators `signal` excites: one per entry of `freqs`, at
 * `amps` (1 unless given), each ringing `times` seconds (1 unless given) to
 * -60 dB; the frequencies times `freqscale` plus `freqoffset`, the times
 * times `decayscale`. Read at the synth's start.
 */
export const klank = (
    signal: Channel,
    freqs: readonly Channel[],
    amps: readonly Channel[] | null = null,
    times: readonly Channel[] | null = null,
    freqscale: Channel = 1.0,
    freqoffset: Channel = 0.0,
    decayscale: Channel = 1.0,
): Ugen =>
    new Ugen("Klank", [signal, freqscale, freqoffset, decayscale, ...partials(freqs, amps, times, 1.0)]);

/**
 * `signal` as its analytic pair, two channels 90 degrees apart across the
 * audio band: the real part and the imaginary. Their magnitude
 * `sqrt(re^2 + im^2)` is the signal's envelope.
 */
export const hilbert = (signal: Channel): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("Hilbert", [signal, c])));

/**
 * `freeVerb` of a pair: both channels' reverberators fed their sum, the
 * right's lines a little longer. Returns the pair.
 */
export const freeVerb2 = (
    signal: Channel,
    signal2: Channel,
    mix: Channel = 0.33,
    room: Channel = 0.5,
    damp: Channel = 0.5,
): ChannelList =>
    chans([0.0, 1.0].map((c) => new Ugen("FreeVerb2", [signal, signal2, mix, room, damp, c])));

/**
 * A room `roomsize` metres across whose tail falls 60 dB in `revtime` seconds:
 * a feedback delay network with early reflections. `damping` lowpasses the
 * tail and `inputbw` the input; `spread` sets the two outputs apart.
 * `maxroomsize` is a constant that sizes the lines. Returns the pair.
 */
export const gverb = (
    signal: Channel,
    roomsize: Channel = 10.0,
    revtime: Channel = 3.0,
    damping: Channel = 0.5,
    inputbw: Channel = 0.5,
    spread: Channel = 15.0,
    drylevel: Channel = 1.0,
    earlyreflevel: Channel = 0.7,
    taillevel: Channel = 0.5,
    maxroomsize: Channel = 300.0,
): ChannelList =>
    chans(
        [0.0, 1.0].map(
            (c) =>
                new Ugen("GVerb", [
                    signal, roomsize, revtime, damping, inputbw, spread, drylevel,
                    earlyreflevel, taillevel, maxroomsize, c,
                ]),
        ),
    );

