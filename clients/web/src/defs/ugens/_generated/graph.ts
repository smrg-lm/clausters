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
