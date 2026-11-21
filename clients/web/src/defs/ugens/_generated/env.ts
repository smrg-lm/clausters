// The `env` builders that mirror the wire, re-exported by `env.ts`.
//
// Generated from the server's UGen registry and
// `crates/clausters-server/src/dsp/registry/builders/env.toml` by
// `crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
// description and regenerate with
// `CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.

import { Ugen } from "../graph.ts";
import type { Channel } from "../graph.ts";

/**
 * Frees the enclosing synth while `signal` is greater than zero, passing
 * it through unchanged. The trigger-driven counterpart of a `DoneAction` --
 * use it when what ends the note is not the envelope.
 */
export const freeSelf = (signal: Channel): Ugen => new Ugen("FreeSelf", [signal]);

/**
 * Pauses the enclosing synth while `signal` is greater than zero, passing
 * it through. Resume with `Server.run`; it re-pauses only if the signal is
 * still up, so this is a gate rather than a one-way door.
 */
export const pauseSelf = (signal: Channel): Ugen => new Ugen("PauseSelf", [signal]);

/**
 * 1 once `source` has finished, 0 before -- a trigger the rest of the
 * graph can read.
 *
 * `source` must be a ugen that *can* finish (`envGen`, `line`, `xLine`);
 * the server rejects the def by name otherwise. What it reads is the done
 * flag, not the value on the wire: an envelope that has played out sits at
 * its final level, which tells you nothing.
 */
export const done = (source: Channel): Ugen => new Ugen("Done", [source]);

/**
 * Passes `source` through and frees the synth once it has finished. The
 * idiom for an envelope whose own `doneAction` is `DoneAction.NONE`
 * because something else in the graph still needs it.
 */
export const freeSelfWhenDone = (source: Channel): Ugen =>
    new Ugen("FreeSelfWhenDone", [source]);
