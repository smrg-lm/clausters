// The `analysis` builders that mirror the wire, re-exported by `analysis.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/analysis.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * The spectrum's centre of mass in hertz: every bin's frequency weighted
 * by its magnitude, DC included. Updated at each frame and held between
 * them; silence reads 0. It reads the chain and leaves it as it was.
 */
export const specCentroid = (chain: Channel): Ugen => new Ugen("SpecCentroid", [chain]);

/**
 * The geometric mean of the frame's magnitudes over their arithmetic mean:
 * 1 for white noise, towards 0 for a few partials (a bin of magnitude 0 is
 * left out of the geometric mean). Silence reads 0.8, very quiet white
 * noise.
 */
export const specFlatness = (chain: Channel): Ugen => new Ugen("SpecFlatness", [chain]);

/**
 * The frequency below which `fraction` of the spectrum's magnitude lies,
 * in hertz (0.5 the median, 0.95 a rolloff), or as a bin number when
 * `binout > 0`. `interpolate > 0` places it between the two bins it falls
 * between.
 */
export const specPcile = (
    chain: Channel,
    fraction: Channel = 0.5,
    interpolate: Channel = 0.0,
    binout: Channel = 0.0,
): Ugen => new Ugen("SpecPcile", [chain, fraction, interpolate, binout]);

/**
 * Perceived loudness in sones: the power in 42 bands of about an ERB each
 * (each bin masked by the one below at `smask` of its power), through the
 * equal-loudness contours to phons, each band falling by at most `tmask`
 * phons a frame, summed. The same through a chain of any size and window;
 * silence reads 0.
 */
export const loudness = (
    chain: Channel,
    smask: Channel = 0.25,
    tmask: Channel = 1.0,
): Ugen => new Ugen("Loudness", [chain, smask, tmask]);

/**
 * The key the music is in: 0 to 11 the major keys from C, 12 to 23 the
 * minor ones. The power at six partials of each note from A1 to G#6 makes
 * a chroma (keeping `chromaleak` of the last frame's), matched against a
 * major and a minor profile in every key; the matches' memory falls to 1%
 * over `keydecay` seconds. A 4096-point chain resolves the low notes.
 */
export const keyTrack = (
    chain: Channel,
    keydecay: Channel = 2.0,
    chromaleak: Channel = 0.5,
): Ugen => new Ugen("KeyTrack", [chain, keydecay, chromaleak]);

/**
 * 1 on the frames an onset starts in, held to the next frame. Each bin is
 * divided by its recent peak (`whtype` 1; the peak falls by 60 dB in
 * `relaxtime` seconds, and is at least `floor`), an onset detection
 * function of the frame is taken (`odftype`: 0 power, 1 sum of magnitudes,
 * 2 complex domain, 3 complex domain over the rising bins, 4 phase
 * deviation, 5 phase deviation weighted by magnitude, 6 modified
 * Kullback-Leibler), the median of its last `medianspan` values is taken
 * off, and an onset is where that rises past `threshold`, at least
 * `mingap` frames after the last. `rawodf > 0` outputs the detection
 * function instead. `odftype` and `medianspan` are constants.
 */
export const onsets = (
    chain: Channel,
    threshold: Channel = 0.5,
    odftype: Channel = 3.0,
    relaxtime: Channel = 1.0,
    floor: Channel = 0.1,
    mingap: Channel = 10.0,
    medianspan: Channel = 11.0,
    whtype: Channel = 1.0,
    rawodf: Channel = 0.0,
): Ugen => new Ugen("Onsets", [chain, threshold, odftype, relaxtime, floor, mingap, medianspan, whtype, rawodf]);

/**
 * An audio-rate 1 over the slice of a frame where `proph` times
 * Hainsworth's measure (the mean rise in `log2` magnitude between 30 Hz
 * and 5 kHz) plus `propf` times Foote's (one minus the frame's cosine
 * similarity with the last) passes `threshold`, then nothing for
 * `waittime` seconds.
 */
export const pvHainsworthFoote = (
    chain: Channel,
    proph: Channel = 0.0,
    propf: Channel = 0.0,
    threshold: Channel = 1.0,
    waittime: Channel = 0.04,
): Ugen => new Ugen("PV_HainsworthFoote", [chain, proph, propf, threshold, waittime]);

/**
 * An audio-rate 1 over the slice of a frame where the weighted sum of the
 * frame-to-frame changes in four features -- the spectral centroid
 * (`propsc`), the energy above 4 kHz (`prophfe`), the high-frequency
 * content (`prophfc`) and the spectral flux (`propsf`) -- passes
 * `threshold`, then nothing for `waittime` seconds.
 */
export const pvJensenAndersen = (
    chain: Channel,
    propsc: Channel = 0.25,
    prophfe: Channel = 0.25,
    prophfc: Channel = 0.25,
    propsf: Channel = 0.25,
    threshold: Channel = 1.0,
    waittime: Channel = 0.04,
): Ugen => new Ugen("PV_JensenAndersen", [chain, propsc, prophfe, prophfc, propsf, threshold, waittime]);
