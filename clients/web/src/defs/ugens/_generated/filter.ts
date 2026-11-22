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

/**
 * One-pole smoother with separate rise (`up`) and fall (`down`) times: the
 * UGen a control with a separate downward lag inserts.
 */
export const lagUd = (signal: Channel, up: Channel = 0.1, down: Channel = 0.1): Ugen =>
    new Ugen("LagUD", [signal, up, down]);

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

/** The input one sample late; `x1` is the sample it starts with. */
export const delay1 = (signal: Channel, x1: Channel = 0.0): Ugen =>
    new Ugen("Delay1", [signal, x1]);

/** The input two samples late; `x1` and `x2` are the samples it starts with. */
export const delay2 = (signal: Channel, x1: Channel = 0.0, x2: Channel = 0.0): Ugen =>
    new Ugen("Delay2", [signal, x1, x2]);

/** Two-point average, `0.5 (x0 + x1)`: a zero at Nyquist. */
export const lpz1 = (signal: Channel): Ugen => new Ugen("LPZ1", [signal]);

/** Two-point difference, `0.5 (x0 - x1)`: a zero at DC. */
export const hpz1 = (signal: Channel): Ugen => new Ugen("HPZ1", [signal]);

/** Two-zero lowpass, `0.25 (x0 + 2 x1 + x2)`. */
export const lpz2 = (signal: Channel): Ugen => new Ugen("LPZ2", [signal]);

/** Two-zero highpass, `0.25 (x0 - 2 x1 + x2)`. */
export const hpz2 = (signal: Channel): Ugen => new Ugen("HPZ2", [signal]);

/** Two-zero bandpass, `0.5 (x0 - x2)`: zeros at DC and Nyquist. */
export const bpz2 = (signal: Channel): Ugen => new Ugen("BPZ2", [signal]);

/** Two-zero band reject, `0.5 (x0 + x2)`: zeros at a quarter of the rate. */
export const brz2 = (signal: Channel): Ugen => new Ugen("BRZ2", [signal]);

/** The signal's rate of change per second, `rate (x0 - x1)`. */
export const slope = (signal: Channel): Ugen => new Ugen("Slope", [signal]);

/** Every other sample negated: the spectrum mirrored about a quarter of the rate. */
export const flip = (signal: Channel): Ugen => new Ugen("Flip", [signal]);

/** Two zeros at `freq` with `radius`. */
export const twoZero = (
    signal: Channel,
    freq: Channel = 440.0,
    radius: Channel = 0.8,
): Ugen => new Ugen("TwoZero", [signal, freq, radius]);

/**
 * First-order section with its coefficients as inputs: `w = x + b1 w1`, `y = a0 w + a1 w1`.
 */
export const fos = (
    signal: Channel,
    a0: Channel = 0.0,
    a1: Channel = 0.0,
    b1: Channel = 0.0,
): Ugen => new Ugen("FOS", [signal, a0, a1, b1]);

/**
 * Second-order section with its coefficients as inputs: `w = x + b1 w1 + b2 w2`, `y = a0 w + a1 w1 + a2 w2`.
 */
export const sos = (
    signal: Channel,
    a0: Channel = 0.0,
    a1: Channel = 0.0,
    a2: Channel = 0.0,
    b1: Channel = 0.0,
    b2: Channel = 0.0,
): Ugen => new Ugen("SOS", [signal, a0, a1, a2, b1, b2]);

/** The input sampled every `time` seconds and joined by straight lines. */
export const ramp = (signal: Channel, time: Channel = 0.1): Ugen =>
    new Ugen("Ramp", [signal, time]);

/** The input followed at no more than `up` units per second rising and `down` falling. */
export const slew = (signal: Channel, up: Channel = 1.0, down: Channel = 1.0): Ugen =>
    new Ugen("Slew", [signal, up, down]);

/** Two poles at `freq` with `radius`: a resonance. */
export const twoPole = (
    signal: Channel,
    freq: Channel = 440.0,
    radius: Channel = 0.8,
): Ugen => new Ugen("TwoPole", [signal, freq, radius]);

/** The two-pole allpass at `freq` with `radius`: phase only. */
export const apf = (
    signal: Channel,
    freq: Channel = 440.0,
    radius: Channel = 0.8,
): Ugen => new Ugen("APF", [signal, freq, radius]);

/** A resonator at `freq` ringing `decaytime` seconds to -60 dB. */
export const ringz = (
    signal: Channel,
    freq: Channel = 440.0,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("Ringz", [signal, freq, decaytime]);

/** A resonance at `freq` that rises in `attacktime` and falls in `decaytime`. */
export const formlet = (
    signal: Channel,
    freq: Channel = 440.0,
    attacktime: Channel = 1.0,
    decaytime: Channel = 1.0,
): Ugen => new Ugen("Formlet", [signal, freq, attacktime, decaytime]);

/** A boost or cut of `db` around `freq`, `rq` its bandwidth. */
export const midEq = (
    signal: Channel,
    freq: Channel = 440.0,
    rq: Channel = 1.0,
    db: Channel = 0.0,
): Ugen => new Ugen("MidEQ", [signal, freq, rq, db]);

/** Cookbook lowpass biquad, `rq` the reciprocal of Q. */
export const bLowPass = (
    signal: Channel,
    freq: Channel = 1200.0,
    rq: Channel = 1.0,
): Ugen => new Ugen("BLowPass", [signal, freq, rq]);

/** Cookbook highpass biquad, `rq` the reciprocal of Q. */
export const bHiPass = (
    signal: Channel,
    freq: Channel = 1200.0,
    rq: Channel = 1.0,
): Ugen => new Ugen("BHiPass", [signal, freq, rq]);

/** Cookbook allpass biquad, `rq` the reciprocal of Q. */
export const bAllPass = (
    signal: Channel,
    freq: Channel = 1200.0,
    rq: Channel = 1.0,
): Ugen => new Ugen("BAllPass", [signal, freq, rq]);

/** Cookbook bandpass biquad, `bw` its bandwidth in octaves. */
export const bBandPass = (
    signal: Channel,
    freq: Channel = 1200.0,
    bw: Channel = 1.0,
): Ugen => new Ugen("BBandPass", [signal, freq, bw]);

/** Cookbook band-reject biquad, `bw` its bandwidth in octaves. */
export const bBandStop = (
    signal: Channel,
    freq: Channel = 1200.0,
    bw: Channel = 1.0,
): Ugen => new Ugen("BBandStop", [signal, freq, bw]);

/** Cookbook peaking EQ: `db` of boost or cut at `freq`, `rq` its width. */
export const bPeakEq = (
    signal: Channel,
    freq: Channel = 1200.0,
    rq: Channel = 1.0,
    db: Channel = 0.0,
): Ugen => new Ugen("BPeakEQ", [signal, freq, rq, db]);

/** Cookbook low shelf: `db` below `freq`, `rs` the shelf's slope (1 the steepest). */
export const bLowShelf = (
    signal: Channel,
    freq: Channel = 1200.0,
    rs: Channel = 1.0,
    db: Channel = 0.0,
): Ugen => new Ugen("BLowShelf", [signal, freq, rs, db]);

/** Cookbook high shelf: `db` above `freq`, `rs` the shelf's slope (1 the steepest). */
export const bHiShelf = (
    signal: Channel,
    freq: Channel = 1200.0,
    rs: Channel = 1.0,
    db: Channel = 0.0,
): Ugen => new Ugen("BHiShelf", [signal, freq, rs, db]);

/** Two `lag`s in a row: a smoother start than one. */
export const lag2 = (signal: Channel, time: Channel = 0.1): Ugen =>
    new Ugen("Lag2", [signal, time]);

/** Three `lag`s in a row. */
export const lag3 = (signal: Channel, time: Channel = 0.1): Ugen =>
    new Ugen("Lag3", [signal, time]);

/** Two `lagUd`s in a row, each with separate rise and fall times. */
export const lag2Ud = (signal: Channel, up: Channel = 0.1, down: Channel = 0.1): Ugen =>
    new Ugen("Lag2UD", [signal, up, down]);

/** Three `lagUd`s in a row. */
export const lag3Ud = (signal: Channel, up: Channel = 0.1, down: Channel = 0.1): Ugen =>
    new Ugen("Lag3UD", [signal, up, down]);

/**
 * The median of the last `length` samples (1 to 31, odd for a true median):
 * removes spikes shorter than half the window and keeps edges.
 */
export const median = (length: Channel, signal: Channel): Ugen =>
    new Ugen("Median", [length, signal]);

/**
 * The Moog ladder (zero-delay feedback): a four-pole lowpass at `freq`, its
 * resonance `gain` from 0 to 4 (self-oscillating at 4); a positive `reset`
 * clears it.
 */
export const moogFf = (
    signal: Channel,
    freq: Channel = 100.0,
    gain: Channel = 2.0,
    reset: Channel = 0.0,
): Ugen => new Ugen("MoogFF", [signal, freq, gain, reset]);

/**
 * An envelope follower: `|signal|` coming within a tenth of a rise in
 * `attacktime` and of a fall in `releasetime`.
 */
export const amplitude = (
    signal: Channel,
    attacktime: Channel = 0.01,
    releasetime: Channel = 0.01,
): Ugen => new Ugen("Amplitude", [signal, attacktime, releasetime]);

/**
 * A compressor, expander or gate: `control`'s envelope sets the gain,
 * `(env / thresh)^(slope - 1)` with `slopebelow` under `thresh` and
 * `slopeabove` over it, following in `clamptime` and `relaxtime`.
 */
export const compander = (
    signal: Channel,
    control: Channel = 0.0,
    thresh: Channel = 0.5,
    slopebelow: Channel = 1.0,
    slopeabove: Channel = 1.0,
    clamptime: Channel = 0.01,
    relaxtime: Channel = 0.1,
): Ugen => new Ugen("Compander", [signal, control, thresh, slopebelow, slopeabove, clamptime, relaxtime]);

/**
 * `signal` brought to peak at `level`, by a gain that looks `dur` ahead:
 * the output is `2 dur` late.
 */
export const normalizer = (
    signal: Channel,
    level: Channel = 1.0,
    dur: Channel = 0.01,
): Ugen => new Ugen("Normalizer", [signal, level, dur]);

/**
 * `signal` kept below `level` by a gain that looks `dur` ahead and moves in
 * lines: the output is `2 dur` late and never passes `level`.
 */
export const limiter = (
    signal: Channel,
    level: Channel = 1.0,
    dur: Channel = 0.01,
): Ugen => new Ugen("Limiter", [signal, level, dur]);

/**
 * Every frequency of `signal` moved by `freq` hertz (down for a negative
 * one): single-sideband modulation. `phase` offsets the oscillator in
 * cycles.
 */
export const freqShift = (
    signal: Channel,
    freq: Channel = 0.0,
    phase: Channel = 0.0,
): Ugen => new Ugen("FreqShift", [signal, freq, phase]);

/**
 * A plucked string (Karplus-Strong): a rising `trig` lets one period of
 * `signal` into a line `delaytime` seconds long that feeds back through a
 * one-pole lowpass of coefficient `coef`, decaying by 60 dB in
 * `decaytime`. `maxdelaytime` is a constant and sizes the line.
 */
export const pluck = (
    signal: Channel,
    trig: Channel = 1.0,
    maxdelaytime: Channel = 0.2,
    delaytime: Channel = 0.2,
    decaytime: Channel = 1.0,
    coef: Channel = 0.5,
): Ugen => new Ugen("Pluck", [signal, trig, maxdelaytime, delaytime, decaytime, coef]);

/**
 * `signal` at `pitchRatio` times its pitch (0 to 4), by four overlapping
 * grains of `windowSize` seconds read at that speed. `pitchDispersion`
 * moves each grain's ratio and `timeDispersion` its start, at random, by
 * up to that much. `windowSize` is read at the start.
 */
export const pitchShift = (
    signal: Channel,
    windowSize: Channel = 0.2,
    pitchRatio: Channel = 1.0,
    pitchDispersion: Channel = 0.0,
    timeDispersion: Channel = 0.0,
): Ugen => new Ugen("PitchShift", [signal, windowSize, pitchRatio, pitchDispersion, timeDispersion]);

/**
 * Writes `signal` round a mono buffer, cleared at its start, and outputs
 * the frame each sample went to: the phase `delTapRd` reads behind.
 */
export const delTapWr = (bufnum: Channel, signal: Channel): Ugen =>
    new Ugen("DelTapWr", [bufnum, signal]);

/**
 * Reads the buffer a `delTapWr` writes, `deltime` seconds behind the
 * `phase` it outputs, with no interpolation (`interp` 1), linear (2) or
 * cubic (4). Any number of taps read one writer.
 */
export const delTapRd = (
    bufnum: Channel,
    phase: Channel,
    deltime: Channel,
    interp: Channel = 1.0,
): Ugen => new Ugen("DelTapRd", [bufnum, phase, deltime, interp]);
