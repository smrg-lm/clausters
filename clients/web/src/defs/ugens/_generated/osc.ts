// The `osc` builders that mirror the wire, re-exported by `osc.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/osc.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * Sine by f64 phase accumulation, starting at phase 0. `phase` is a
 * running offset in cycles, so an audio-rate `phase` is phase modulation.
 */
export const sine = (freq: Channel = 440.0, phase: Channel = 0.0): Ugen =>
    new Ugen("Sine", [freq, phase]);

/**
 * A single-sample `1.0` every `freq` Hz, `0.0` between (`freq` 0 =
 * one impulse then silence). `phase` offsets the train in cycles: at 0 the
 * first sample is an impulse, and moving it moves the train, firing when
 * that carries it across a cycle. A negative `freq` runs it backwards.
 */
export const impulse = (freq: Channel = 1.0, phase: Channel = 0.0): Ugen =>
    new Ugen("Impulse", [freq, phase]);

/** Uniform white noise in +/-1. */
export const whiteNoise = (): Ugen => new Ugen("WhiteNoise", []);

/**
 * Equal energy per octave, -3 dB/octave (Voss-McCartney).
 *
 * A **quiet** signal by construction -- around 0.13 RMS against white noise's
 * 0.58, which is the level a def ported from sclang expects. Scale it up
 * rather than assuming something is wrong.
 */
export const pinkNoise = (): Ugen => new Ugen("PinkNoise", []);

/**
 * A random walk, -6 dB/octave. It **reflects** at +/-1 rather than clamping,
 * so it never rests against a rail.
 */
export const brownNoise = (): Ugen => new Ugen("BrownNoise", []);

/**
 * One randomly chosen bit of a 31-bit word flipped per sample.
 *
 * Not white noise with a twist: its spectrum leans low (about -2.9 dB/octave)
 * and its steps span every order of magnitude, which is what makes it sound
 * grainy rather than smooth.
 */
export const grayNoise = (): Ugen => new Ugen("GrayNoise", []);

/**
 * -1 or 1, nothing between -- a coin flip per sample. The loudest noise
 * available at a given peak, since every sample is at full scale.
 */
export const clipNoise = (): Ugen => new Ugen("ClipNoise", []);

/**
 * A new random value in +/-1 every `1/freq` seconds, **held** -- steps.
 * Not band limited: like the `lfSaw` family it is a modulation shape.
 */
export const lfNoise0 = (freq: Channel = 500.0): Ugen => new Ugen("LFNoise0", [freq]);

/**
 * `lfNoise0` with a linear ramp between values, so it has corners but no
 * jumps.
 */
export const lfNoise1 = (freq: Channel = 500.0): Ugen => new Ugen("LFNoise1", [freq]);

/**
 * `lfNoise0` with a quadratic between values, so the slope is continuous
 * too and there are no corners either.
 *
 * It aims at the midpoints between draws and carries its slope across them,
 * so it **overshoots**: the output reaches about +/-1.7, not +/-1. scsynth's does
 * the same.
 */
export const lfNoise2 = (freq: Channel = 500.0): Ugen => new Ugen("LFNoise2", [freq]);

/** `lfNoise0` restricted to +/-1 -- a random square. */
export const lfClipNoise = (freq: Channel = 500.0): Ugen =>
    new Ugen("LFClipNoise", [freq]);

/**
 * Random impulses in [0, 1) at a **mean** `density` per second.
 *
 * Every sample is an independent trial, so the gaps are exponential: this is
 * not a clock. Ten per second means ten on average, with clusters and silences
 * -- use `impulse` when you want them evenly spaced. The amplitudes are random
 * too, which matters if you feed it to something that cares.
 */
export const dust = (density: Channel = 1.0): Ugen => new Ugen("Dust", [density]);

/** `dust` firing both ways, in +/-1. */
export const dust2 = (density: Channel = 1.0): Ugen => new Ugen("Dust2", [density]);

/**
 * The chaotic map `y[n] = |chaos*y[n-1] - y[n-2] - 0.05|`.
 *
 * It has no RNG: the same `chaos` always gives the same signal, so it is
 * reproducible without a seed. The parameter changes the sound drastically and
 * **not** monotonically, so reach for it by ear. The output is one-sided and
 * carries DC -- put a `leakDc` after it before summing it into a bus.
 */
export const crackle = (chaos: Channel = 1.5): Ugen => new Ugen("Crackle", [chaos]);

/**
 * Band-limited rising sawtooth in +/-1, starting at 0.
 *
 * Anti-aliased with PolyBLEP, which is very clean over the low and middle
 * range and progressively less so toward Nyquist (its residual grows about
 * as the square of the frequency). It carries no DC offset.
 */
export const saw = (freq: Channel = 440.0): Ugen => new Ugen("Saw", [freq]);

/**
 * Band-limited pulse in +/-1; `width` is the duty cycle (0.5 = square).
 *
 * Anti-aliased like `saw`. The width is clamped just inside `(0, 1)`,
 * where the two edges would coincide.
 */
export const pulse = (freq: Channel = 440.0, width: Channel = 0.5): Ugen =>
    new Ugen("Pulse", [freq, width]);

/**
 * Rising sawtooth in +/-1, **not** band-limited -- a modulation shape.
 *
 * `iphase` is the initial phase in **cycles**, `[0, 1)`, read once at the
 * first sample. (sclang measures the same argument in `[0, 2)`; every phase
 * in this client is in cycles.)
 */
export const lfSaw = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("LFSaw", [freq, iphase]);

/**
 * Square in `[0, 1]` -- a gate, not a bipolar waveform like `pulse` -- with
 * `width` as its duty cycle. Not band-limited. `iphase` as in `lfSaw`.
 */
export const lfPulse = (
    freq: Channel = 440.0,
    iphase: Channel = 0.0,
    width: Channel = 0.5,
): Ugen => new Ugen("LFPulse", [freq, iphase, width]);

/**
 * Triangle in +/-1, starting at 0 and rising. Not band-limited.
 * `iphase` as in `lfSaw`.
 */
export const lfTri = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("LFTri", [freq, iphase]);

/**
 * Triangle whose peak sits at `width` of the cycle, in +/-1: sweeps from a
 * falling ramp through a triangle to a rising one. Not band-limited.
 * `iphase` as in `lfSaw`.
 */
export const varSaw = (
    freq: Channel = 440.0,
    iphase: Channel = 0.0,
    width: Channel = 0.5,
): Ugen => new Ugen("VarSaw", [freq, iphase, width]);

/**
 * Ramp from `start` to `end` advancing by `rate` **per sample**,
 * wrapping at `end`; a rising `trig` jumps to `resetPos`.
 *
 * `rate` is in output units per sample, not Hz, which is what makes this the
 * index source for a buffer reader: a rate of 1 advances one frame per sample.
 */
export const phasor = (
    trig: Channel = 0.0,
    rate: Channel = 1.0,
    start: Channel = 0.0,
    end: Channel = 1.0,
    resetPos: Channel = 0.0,
): Ugen => new Ugen("Phasor", [trig, rate, start, end, resetPos]);

/**
 * The **transport's position**, in frames, minus `offset`.
 *
 * A buffer reader whose phase is this one follows the transport instead of
 * carrying a position of its own, so seeking
 * (`Server.transportLocateSample`), looping (`Server.transportLoop`) and
 * pausing (`Server.transportStop` over a governed group) belong to the
 * transport and not to the def. That is the shape a multitrack needs -- many
 * readers, one time -- and it is why a locate never has to reach into a node.
 *
 * It ramps one frame per sample while the transport rolls and holds while it
 * is stopped. `offset` is where this signal starts on that axis, so a clip
 * reads its own frame 0 when the transport reaches it; the subtraction happens
 * in double precision inside the UGen, which is what keeps the value exact
 * deep into a long take (a signal is 32-bit, and past about six minutes at
 * 48 kHz it can no longer count single frames -- subtracting afterwards with
 * `sub` has already lost that).
 *
 * ```ts
 * const take = bufRd(buf.bufnum, 0, transportPos());
 * ```
 */
export const transportPos = (offset: Channel = 0.0): Ugen =>
    new Ugen("TransportPos", [offset]);

/**
 * The **transport's declick level**: 1 while it rolls, 0 while it is
 * stopped, and a straight ramp between the two across a stop and a play,
 * as long as `Server.transportFade` says (with none set, it is simply
 * whether the transport rolls).
 *
 * An output that must not click on a stop sits **outside** the governed
 * group, in one that follows the transport (`Server.transportFollow`), and
 * multiplies what the readers wrote by this: with a ramp the transport keeps
 * rolling while it falls, so the readers are still playing when it reaches
 * zero.
 *
 * ```ts
 * out(0, mul(in_(bus), transportFade()));
 * ```
 */
export const transportFade = (): Ugen => new Ugen("TransportFade", []);

/**
 * A sine-like shape in +/-1 made of two cubic arcs, its slope continuous,
 * **not** band-limited -- a modulation shape, within 0.02 of a sine.
 * Starts at 0 rising. `iphase` as in `lfSaw`.
 */
export const lfCub = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("LFCub", [freq, iphase]);

/**
 * A cosine-like shape in +/-1 made of two parabolas, **not** band-limited:
 * starts at its peak, within 0.056 of a cosine. `iphase` as in `lfSaw`.
 */
export const lfPar = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("LFPar", [freq, iphase]);

/**
 * A sawtooth at `sawFreq` restarted at every cycle of a master at
 * `syncFreq` (hard sync): sweeping `sawFreq` sweeps its harmonics.
 * Not band-limited.
 */
export const syncSaw = (syncFreq: Channel = 440.0, sawFreq: Channel = 440.0): Ugen =>
    new Ugen("SyncSaw", [syncFreq, sawFreq]);

/**
 * `freq` wobbled by `depth` (a fraction of it) at `rate` Hz, a parabolic
 * sine. Each cycle draws its own rate and the depth of each half, varied
 * by `ratevariation` and `depthvariation`, from the synth's random
 * generator. It starts after `delay` seconds and fades in over `onset`;
 * a rising `trig` starts it again from `iphase` (in cycles).
 */
export const vibrato = (
    freq: Channel = 440.0,
    rate: Channel = 6.0,
    depth: Channel = 0.02,
    delay: Channel = 0.0,
    onset: Channel = 0.0,
    ratevariation: Channel = 0.04,
    depthvariation: Channel = 0.1,
    iphase: Channel = 0.0,
    trig: Channel = 0.0,
): Ugen => new Ugen("Vibrato", [freq, rate, depth, delay, onset, ratevariation, depthvariation, iphase, trig]);

/**
 * A sine by the two-pole recurrence, the cheapest there is: for a fixed
 * tone. `iphase` is the initial phase in cycles. The frequency is read once
 * a block, and changing it moves the amplitude too: modulate `sine` instead.
 */
export const fsinOsc = (freq: Channel = 440.0, iphase: Channel = 0.0): Ugen =>
    new Ugen("FSinOsc", [freq, iphase]);

/**
 * A sine phase-modulated by its own last sample, `feedback` in radians:
 * a sine at 0, brighter as it rises, towards a sawtooth near 1.5 and noise
 * beyond.
 */
export const sinOscFb = (freq: Channel = 440.0, feedback: Channel = 0.0): Ugen =>
    new Ugen("SinOscFB", [freq, feedback]);

/**
 * One sine grain of `dur` seconds under a parabola peaking at `amp`, its
 * inputs read at the start. The synth is freed when the grain ends.
 */
export const psinGrain = (
    freq: Channel = 440.0,
    dur: Channel = 0.2,
    amp: Channel = 0.1,
): Ugen => new Ugen("PSinGrain", [freq, dur, amp]);

/**
 * A band-limited impulse train: `numharm` harmonics of `freq` of equal
 * amplitude, peak 1, kept below Nyquist. Changing `numharm` crossfades
 * across a block.
 */
export const blip = (freq: Channel = 440.0, numharm: Channel = 200.0): Ugen =>
    new Ugen("Blip", [freq, numharm]);

/**
 * A formant over a fundamental: at each cycle of `fundfreq`, a burst of a
 * sine at `formfreq` under a raised-cosine window `bwfreq` Hz wide, so the
 * spectrum peaks at `formfreq` whatever the fundamental.
 */
export const formant = (
    fundfreq: Channel = 440.0,
    formfreq: Channel = 1760.0,
    bwfreq: Channel = 880.0,
): Ugen => new Ugen("Formant", [fundfreq, formfreq, bwfreq]);

/**
 * A random value `freq` times a second, held: `lfNoise0` whose rate can
 * move, taking effect at once, at the cost of a phase per sample.
 */
export const lfdNoise0 = (freq: Channel = 500.0): Ugen => new Ugen("LFDNoise0", [freq]);

/** `lfdNoise0` joined by straight lines. */
export const lfdNoise1 = (freq: Channel = 500.0): Ugen => new Ugen("LFDNoise1", [freq]);

/** `lfdNoise0` joined by a cubic through four values, within +/-1. */
export const lfdNoise3 = (freq: Channel = 500.0): Ugen => new Ugen("LFDNoise3", [freq]);

/** `lfdNoise0` with only +/-1. */
export const lfdClipNoise = (freq: Channel = 500.0): Ugen =>
    new Ugen("LFDClipNoise", [freq]);

/**
 * The logistic map `y = r y (1 - y)`, `r` the `chaosParam`, iterated `freq`
 * times a second from `init` and held between: chaos from about 3.57 on,
 * cycles below.
 */
export const logistic = (
    chaosParam: Channel = 3.0,
    freq: Channel = 1000.0,
    init: Channel = 0.5,
): Ugen => new Ugen("Logistic", [chaosParam, freq, init]);

/**
 * The cusp map `x = a - b sqrt(|x|)`, stepped `freq` times a second and held between steps.
 */
export const cuspN = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = 1.9,
    xi: Channel = 0.0,
): Ugen => new Ugen("CuspN", [freq, a, b, xi]);

/**
 * The cusp map `x = a - b sqrt(|x|)`, stepped `freq` times a second and joined by lines.
 */
export const cuspL = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = 1.9,
    xi: Channel = 0.0,
): Ugen => new Ugen("CuspL", [freq, a, b, xi]);

/**
 * The gingerbread man map, `x = 1 - y + |x|` with `y` the last `x`, stepped `freq` times a second and held between steps.
 */
export const gbmanN = (
    freq: Channel = 22050.0,
    xi: Channel = 1.2,
    yi: Channel = 2.1,
): Ugen => new Ugen("GbmanN", [freq, xi, yi]);

/**
 * The gingerbread man map, `x = 1 - y + |x|` with `y` the last `x`, stepped `freq` times a second and joined by lines.
 */
export const gbmanL = (
    freq: Channel = 22050.0,
    xi: Channel = 1.2,
    yi: Channel = 2.1,
): Ugen => new Ugen("GbmanL", [freq, xi, yi]);

/**
 * The Henon map `x = 1 - a x^2 + b x'` from `x0` and `x1`, starting at `x1`, stepped `freq` times a second and held between steps.
 */
export const henonN = (
    freq: Channel = 22050.0,
    a: Channel = 1.4,
    b: Channel = 0.3,
    x0: Channel = 0.0,
    x1: Channel = 0.0,
): Ugen => new Ugen("HenonN", [freq, a, b, x0, x1]);

/**
 * The Henon map `x = 1 - a x^2 + b x'` from `x0` and `x1`, starting at `x1`, stepped `freq` times a second and joined by lines.
 */
export const henonL = (
    freq: Channel = 22050.0,
    a: Channel = 1.4,
    b: Channel = 0.3,
    x0: Channel = 0.0,
    x1: Channel = 0.0,
): Ugen => new Ugen("HenonL", [freq, a, b, x0, x1]);

/**
 * The Henon map `x = 1 - a x^2 + b x'` from `x0` and `x1`, starting at `x1`, stepped `freq` times a second and joined by a cubic.
 */
export const henonC = (
    freq: Channel = 22050.0,
    a: Channel = 1.4,
    b: Channel = 0.3,
    x0: Channel = 0.0,
    x1: Channel = 0.0,
): Ugen => new Ugen("HenonC", [freq, a, b, x0, x1]);

/**
 * Latoocarfian's map, `x = sin(b y) + c sin(b x)`, `y = sin(a x) + d sin(a y)`, stepped `freq` times a second and held between steps.
 */
export const latoocarfianN = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = 3.0,
    c: Channel = 0.5,
    d: Channel = 0.5,
    xi: Channel = 0.5,
    yi: Channel = 0.5,
): Ugen => new Ugen("LatoocarfianN", [freq, a, b, c, d, xi, yi]);

/**
 * Latoocarfian's map, `x = sin(b y) + c sin(b x)`, `y = sin(a x) + d sin(a y)`, stepped `freq` times a second and joined by lines.
 */
export const latoocarfianL = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = 3.0,
    c: Channel = 0.5,
    d: Channel = 0.5,
    xi: Channel = 0.5,
    yi: Channel = 0.5,
): Ugen => new Ugen("LatoocarfianL", [freq, a, b, c, d, xi, yi]);

/**
 * Latoocarfian's map, `x = sin(b y) + c sin(b x)`, `y = sin(a x) + d sin(a y)`, stepped `freq` times a second and joined by a cubic.
 */
export const latoocarfianC = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = 3.0,
    c: Channel = 0.5,
    d: Channel = 0.5,
    xi: Channel = 0.5,
    yi: Channel = 0.5,
): Ugen => new Ugen("LatoocarfianC", [freq, a, b, c, d, xi, yi]);

/**
 * The linear congruential generator `x = (a x + c) mod m`, scaled to +/-1, stepped `freq` times a second and held between steps.
 */
export const linCongN = (
    freq: Channel = 22050.0,
    a: Channel = 1.1,
    c: Channel = 0.13,
    m: Channel = 1.0,
    xi: Channel = 0.0,
): Ugen => new Ugen("LinCongN", [freq, a, c, m, xi]);

/**
 * The linear congruential generator `x = (a x + c) mod m`, scaled to +/-1, stepped `freq` times a second and joined by lines.
 */
export const linCongL = (
    freq: Channel = 22050.0,
    a: Channel = 1.1,
    c: Channel = 0.13,
    m: Channel = 1.0,
    xi: Channel = 0.0,
): Ugen => new Ugen("LinCongL", [freq, a, c, m, xi]);

/**
 * The linear congruential generator `x = (a x + c) mod m`, scaled to +/-1, stepped `freq` times a second and joined by a cubic.
 */
export const linCongC = (
    freq: Channel = 22050.0,
    a: Channel = 1.1,
    c: Channel = 0.13,
    m: Channel = 1.0,
    xi: Channel = 0.0,
): Ugen => new Ugen("LinCongC", [freq, a, c, m, xi]);

/**
 * The Lorenz system, integrated with step `h` (Runge-Kutta), its `x` scaled by 0.04, stepped `freq` times a second and joined by lines.
 */
export const lorenzL = (
    freq: Channel = 22050.0,
    s: Channel = 10.0,
    r: Channel = 28.0,
    b: Channel = 2.667,
    h: Channel = 0.05,
    xi: Channel = 0.1,
    yi: Channel = 0.0,
    zi: Channel = 0.0,
): Ugen => new Ugen("LorenzL", [freq, s, r, b, h, xi, yi, zi]);

/**
 * The quadratic map `x = a x^2 + b x + c`, stepped `freq` times a second and held between steps.
 */
export const quadN = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = -1.0,
    c: Channel = -0.75,
    xi: Channel = 0.0,
): Ugen => new Ugen("QuadN", [freq, a, b, c, xi]);

/**
 * The quadratic map `x = a x^2 + b x + c`, stepped `freq` times a second and joined by lines.
 */
export const quadL = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = -1.0,
    c: Channel = -0.75,
    xi: Channel = 0.0,
): Ugen => new Ugen("QuadL", [freq, a, b, c, xi]);

/**
 * The quadratic map `x = a x^2 + b x + c`, stepped `freq` times a second and joined by a cubic.
 */
export const quadC = (
    freq: Channel = 22050.0,
    a: Channel = 1.0,
    b: Channel = -1.0,
    c: Channel = -0.75,
    xi: Channel = 0.0,
): Ugen => new Ugen("QuadC", [freq, a, b, c, xi]);

/**
 * Chirikov's standard map, its `x` scaled to +/-1, stepped `freq` times a second and held between steps.
 */
export const standardN = (
    freq: Channel = 22050.0,
    k: Channel = 1.0,
    xi: Channel = 0.5,
    yi: Channel = 0.0,
): Ugen => new Ugen("StandardN", [freq, k, xi, yi]);

/**
 * Chirikov's standard map, its `x` scaled to +/-1, stepped `freq` times a second and joined by lines.
 */
export const standardL = (
    freq: Channel = 22050.0,
    k: Channel = 1.0,
    xi: Channel = 0.5,
    yi: Channel = 0.0,
): Ugen => new Ugen("StandardL", [freq, k, xi, yi]);

/**
 * A feedback sine map, `x = sin(im y + fb x)`, `y = (a y + c) mod 2 pi`, stepped `freq` times a second and held between steps.
 */
export const fbSineN = (
    freq: Channel = 22050.0,
    im: Channel = 1.0,
    fb: Channel = 0.1,
    a: Channel = 1.1,
    c: Channel = 0.5,
    xi: Channel = 0.1,
    yi: Channel = 0.1,
): Ugen => new Ugen("FBSineN", [freq, im, fb, a, c, xi, yi]);

/**
 * A feedback sine map, `x = sin(im y + fb x)`, `y = (a y + c) mod 2 pi`, stepped `freq` times a second and joined by lines.
 */
export const fbSineL = (
    freq: Channel = 22050.0,
    im: Channel = 1.0,
    fb: Channel = 0.1,
    a: Channel = 1.1,
    c: Channel = 0.5,
    xi: Channel = 0.1,
    yi: Channel = 0.1,
): Ugen => new Ugen("FBSineL", [freq, im, fb, a, c, xi, yi]);

/**
 * A feedback sine map, `x = sin(im y + fb x)`, `y = (a y + c) mod 2 pi`, stepped `freq` times a second and joined by a cubic.
 */
export const fbSineC = (
    freq: Channel = 22050.0,
    im: Channel = 1.0,
    fb: Channel = 0.1,
    a: Channel = 1.1,
    c: Channel = 0.5,
    xi: Channel = 0.1,
    yi: Channel = 0.1,
): Ugen => new Ugen("FBSineC", [freq, im, fb, a, c, xi, yi]);

/**
 * Xenakis's dynamic stochastic synthesis: a cycle of `knum` control points
 * (of `initCps` in memory), each level and duration taking a random walk
 * each cycle, by distribution `ampdist`/`durdist` (0 linear, 1 Cauchy,
 * 2 logistic, 3 hyperbolic cosine, 4 arcsine, 5 exponential, 6 the
 * parameter itself) scaled by `ampscale`/`durscale`; the durations map to a
 * frequency between `minfreq` and `maxfreq`.
 */
export const gendy1 = (
    ampdist: Channel = 1.0,
    durdist: Channel = 1.0,
    adparam: Channel = 1.0,
    ddparam: Channel = 1.0,
    minfreq: Channel = 440.0,
    maxfreq: Channel = 660.0,
    ampscale: Channel = 0.5,
    durscale: Channel = 0.5,
    initCps: Channel = 12.0,
    knum: Channel = 12.0,
): Ugen => new Ugen("Gendy1", [ampdist, durdist, adparam, ddparam, minfreq, maxfreq, ampscale, durscale, initCps, knum]);

/**
 * `gendy1` with a second-order walk: the steps walk too, the levels' driven
 * by a Lehmer congruence of `a` and `c`.
 */
export const gendy2 = (
    ampdist: Channel = 1.0,
    durdist: Channel = 1.0,
    adparam: Channel = 1.0,
    ddparam: Channel = 1.0,
    minfreq: Channel = 440.0,
    maxfreq: Channel = 660.0,
    ampscale: Channel = 0.5,
    durscale: Channel = 0.5,
    initCps: Channel = 12.0,
    knum: Channel = 12.0,
    a: Channel = 1.17,
    c: Channel = 0.31,
): Ugen => new Ugen("Gendy2", [ampdist, durdist, adparam, ddparam, minfreq, maxfreq, ampscale, durscale, initCps, knum, a, c]);

/**
 * `gendy1`'s walks at a fixed pitch `freq`: the durations are proportions of
 * the cycle, drawn together each cycle, and every cycle starts at 0.
 */
export const gendy3 = (
    ampdist: Channel = 1.0,
    durdist: Channel = 1.0,
    adparam: Channel = 1.0,
    ddparam: Channel = 1.0,
    freq: Channel = 440.0,
    ampscale: Channel = 0.5,
    durscale: Channel = 0.5,
    initCps: Channel = 12.0,
    knum: Channel = 12.0,
): Ugen => new Ugen("Gendy3", [ampdist, durdist, adparam, ddparam, freq, ampscale, durscale, initCps, knum]);
