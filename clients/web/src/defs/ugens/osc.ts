// Oscillators and noise: what a graph starts from (mirrors
// `clausters/defs/ugens/osc.py`).
//
// The band-limited pair (`saw`, `pulse`) beside the naive `lf*` forms, the
// noise family by spectrum, and `phasor` as the ramp the table readers run on.

export * from "./_generated/osc.ts";

import { Ugen } from "./graph.ts";
import type { Channel } from "./graph.ts";

/**
 * `freqs`, `amps` (ones unless given) and a third list (`fill` unless given),
 * as the flat triples a bank takes.
 */
export function partials(
    freqs: readonly Channel[],
    amps: readonly Channel[] | null,
    third: readonly Channel[] | null,
    fill: number,
): Channel[] {
    const a = amps ?? freqs.map(() => 1.0);
    const t = third ?? freqs.map(() => fill);
    if (a.length !== freqs.length || t.length !== freqs.length) {
        throw new RangeError("a bank's lists must have one entry per partial");
    }
    return freqs.flatMap((f, i) => [f, a[i]!, t[i]!]);
}

/**
 * A bank of fixed sines: one per entry of `freqs`, at `amps` (1 unless given)
 * and starting at `phases` (cycles, 0 unless given), the frequencies times
 * `freqscale` plus `freqoffset`. Read at the synth's start: a fixed spectrum,
 * as cheap as sines get.
 */
export const klang = (
    freqs: readonly Channel[],
    amps: readonly Channel[] | null = null,
    phases: readonly Channel[] | null = null,
    freqscale: Channel = 1.0,
    freqoffset: Channel = 0.0,
): Ugen => new Ugen("Klang", [freqscale, freqoffset, ...partials(freqs, amps, phases, 0.0)]);
