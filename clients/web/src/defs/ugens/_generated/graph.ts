// The `graph` builders that mirror the wire, re-exported by `graph.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/graph.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * `a*b + c` in one UGen (the multiply-accumulate the server fuses). The
 * plain expression `a * b + c` builds the same value with two op UGens; this
 * is the fused equivalent.
 */
export const madd = (a: Channel, b: Channel, c: Channel): Ugen =>
    new Ugen("MulAdd", [a, b, c]);

/** `a + b + c` in one UGen. */
export const sum3 = (a: Channel, b: Channel, c: Channel): Ugen =>
    new Ugen("Sum3", [a, b, c]);

/** `a + b + c + d` in one UGen. */
export const sum4 = (a: Channel, b: Channel, c: Channel, d: Channel): Ugen =>
    new Ugen("Sum4", [a, b, c, d]);

/**
 * `signal` mapped from `srclo..srchi` to `dstlo..dsthi` exponentially,
 * unclipped: equal steps in, equal ratios out. The ends of the destination
 * share a sign and are not zero.
 */
export const linExp = (
    signal: Channel,
    srclo: Channel = 0.0,
    srchi: Channel = 1.0,
    dstlo: Channel = 1.0,
    dsthi: Channel = 2.0,
): Ugen => new Ugen("LinExp", [signal, srclo, srchi, dstlo, dsthi]);

/** 1 where `lo <= signal <= hi`, else 0. */
export const inRange = (signal: Channel, lo: Channel = 0.0, hi: Channel = 1.0): Ugen =>
    new Ugen("InRange", [signal, lo, hi]);

/**
 * 1 where the point `(x, y)` lies in the rectangle from `left`, `top` to
 * `right`, `bottom`, its edges included, else 0.
 */
export const inRect = (
    x: Channel,
    y: Channel,
    left: Channel = 0.0,
    top: Channel = 0.0,
    right: Channel = 1.0,
    bottom: Channel = 1.0,
): Ugen => new Ugen("InRect", [x, y, left, top, right, bottom]);

/**
 * The distance from `x` to `y` on a circle of circumference `mod`, the
 * shorter way round: in `[0, mod/2]`.
 */
export const modDif = (x: Channel, y: Channel = 0.0, mod: Channel = 1.0): Ugen =>
    new Ugen("ModDif", [x, y, mod]);

/**
 * Undoes a wrap into `lo..hi`: each jump of more than half the range adds
 * or takes away a whole range, so a wrapped phase comes out as one that
 * keeps going. The range is read at the start, and the output starts
 * inside it.
 */
export const unwrap = (signal: Channel, lo: Channel = 0.0, hi: Channel = 1.0): Ugen =>
    new Ugen("Unwrap", [signal, lo, hi]);

/**
 * `(root / freq)^exp`: the gain that keeps a higher partial from sounding
 * louder than a lower one of the same amplitude. An exponent of a third
 * approximates equal loudness.
 */
export const ampComp = (
    freq: Channel = 261.62558,
    root: Channel = 261.62558,
    exp: Channel = 0.3333,
): Ugen => new Ugen("AmpComp", [freq, root, exp]);

/**
 * A loudness compensation along the A-weighting curve: `rootAmp` at
 * `root`, `minAmp` where the weighting peaks (near 2.5 kHz), and more
 * than `rootAmp` below it.
 */
export const ampCompA = (
    freq: Channel = 1000.0,
    root: Channel = 0.0,
    minAmp: Channel = 0.32,
    rootAmp: Channel = 1.0,
): Ugen => new Ugen("AmpCompA", [freq, root, minAmp, rootAmp]);

/** A control-rate signal at audio rate, a straight line across each block. */
export const k2a = (signal: Channel): Ugen => new Ugen("K2A", [signal]);

/** An audio-rate signal at control rate: each block's first sample. */
export const a2k = (signal: Channel): Ugen => new Ugen("A2K", [signal]);

/**
 * A control-rate trigger at audio rate: one sample, `offset` samples into
 * the block, takes the trigger's value at each rising edge; the rest are 0.
 */
export const t2a = (signal: Channel, offset: Channel = 0.0): Ugen =>
    new Ugen("T2A", [signal, offset]);

/**
 * An audio-rate trigger at control rate: each block's largest sample, so a
 * trigger anywhere in the block is kept.
 */
export const t2k = (signal: Channel): Ugen => new Ugen("T2K", [signal]);

/** The constant `signal`, taken at the synth's start and held. */
export const dc = (signal: Channel): Ugen => new Ugen("DC", [signal]);

/**
 * `signal`'s bits through an integer hash, as a value in +/-1: noise that is
 * a function of its input, the same for the same input every time.
 */
export const hasher = (signal: Channel): Ugen => new Ugen("Hasher", [signal]);

/**
 * `signal` with only the top `bits` (0 to 23) of its mantissa kept: the same
 * relative coarseness at every level.
 */
export const mantissaMask = (signal: Channel, bits: Channel = 3.0): Ugen =>
    new Ugen("MantissaMask", [signal, bits]);
