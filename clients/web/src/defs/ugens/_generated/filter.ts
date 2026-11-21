// The `filter` builders that mirror the wire, re-exported by `filter.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/filter.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/** Second-order Butterworth lowpass: -3 dB at `freq`, -12 dB/octave. */
export const lpf = (signal: Channel, freq: Channel = 440.0): Ugen =>
    new Ugen("LPF", [signal, freq]);

/** Second-order Butterworth highpass: -3 dB at `freq`, -12 dB/octave. */
export const hpf = (signal: Channel, freq: Channel = 440.0): Ugen =>
    new Ugen("HPF", [signal, freq]);

/**
 * `y[n] = (1-|coef|)*x[n] + coef*y[n-1]` -- lowpass for a positive
 * coefficient, highpass for a negative one, unity in the passband.
 *
 * The parameter is the **pole**, not a cutoff, as in sclang. Use `lag` when
 * what you want is a time constant.
 */
export const onePole = (signal: Channel, coef: Channel = 0.5): Ugen =>
    new Ugen("OnePole", [signal, coef]);

/**
 * `y[n] = (1-|coef|)*x[n] + coef*x[n-1]` -- the zero-only sibling of
 * `onePole`.
 */
export const oneZero = (signal: Channel, coef: Channel = 0.5): Ugen =>
    new Ugen("OneZero", [signal, coef]);

/**
 * Removes DC: a zero exactly at 0 Hz with a pole just inside it. The
 * default corner is low enough to leave audio untouched.
 */
export const leakDc = (signal: Channel, coef: Channel = 0.995): Ugen =>
    new Ugen("LeakDC", [signal, coef]);

/**
 * Leaky accumulator, `y[n] = x[n] + coef*y[n-1]`. The coefficient is
 * clamped just inside 1 on the server, so it always forgets eventually
 * instead of running away on a DC input.
 */
export const integrator = (signal: Channel, coef: Channel = 0.999): Ugen =>
    new Ugen("Integrator", [signal, coef]);

/**
 * One-pole smoother: `signal` lagged over `time` seconds (symmetric);
 * `time` 0 passes through. The same UGen the server inserts for a lagged
 * control -- use it directly to smooth any signal.
 */
export const lag = (signal: Channel, time: Channel = 0.1): Ugen =>
    new Ugen("Lag", [signal, time]);

/** One-pole smoother with separate rise (`up`) and fall (`down`) times. */
export const varLag = (signal: Channel, up: Channel = 0.1, down: Channel = 0.1): Ugen =>
    new Ugen("VarLag", [signal, up, down]);

/** `delayN` over a channel of a pool buffer. */
export const bufDelayN = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
): Ugen => new Ugen("BufDelayN", [bufnum, chan, signal, delaytime]);

/** `delayL` over a channel of a pool buffer. */
export const bufDelayL = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
): Ugen => new Ugen("BufDelayL", [bufnum, chan, signal, delaytime]);

/** `delayC` over a channel of a pool buffer. */
export const bufDelayC = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
): Ugen => new Ugen("BufDelayC", [bufnum, chan, signal, delaytime]);

/** `combN` over a channel of a pool buffer. */
export const bufCombN = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("BufCombN", [bufnum, chan, signal, delaytime, decaytime]);

/** `combL` over a channel of a pool buffer. */
export const bufCombL = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("BufCombL", [bufnum, chan, signal, delaytime, decaytime]);

/** `combC` over a channel of a pool buffer. */
export const bufCombC = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("BufCombC", [bufnum, chan, signal, delaytime, decaytime]);

/** `allpassN` over a channel of a pool buffer. */
export const bufAllpassN = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("BufAllpassN", [bufnum, chan, signal, delaytime, decaytime]);

/** `allpassL` over a channel of a pool buffer. */
export const bufAllpassL = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("BufAllpassL", [bufnum, chan, signal, delaytime, decaytime]);

/** `allpassC` over a channel of a pool buffer. */
export const bufAllpassC = (
    bufnum: Channel,
    chan: Channel,
    signal: Channel,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("BufAllpassC", [bufnum, chan, signal, delaytime, decaytime]);
