// The `pan` builders that mirror the wire, re-exported by `pan.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/pan.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * Equal-power crossfade between two signals: -1 is all `a`, 1 is all
 * `b`, and the two gains hold unit power in between -- which keeps
 * *uncorrelated* signals at one loudness across the fade, and lifts
 * correlated ones by 3 dB in the middle. Use `linXfade2` when the two
 * sides are the same signal.
 */
export const xfade2 = (
    a: Channel,
    b: Channel,
    pan: Channel = 0.0,
    level: Channel = 1.0,
): Ugen => new Ugen("XFade2", [a, b, pan, level]);

/**
 * Crossfade with the constant-amplitude law -- a plain interpolation, half
 * of each at the centre. The right one for correlated sources.
 */
export const linXfade2 = (
    a: Channel,
    b: Channel,
    pan: Channel = 0.0,
    level: Channel = 1.0,
): Ugen => new Ugen("LinXFade2", [a, b, pan, level]);
