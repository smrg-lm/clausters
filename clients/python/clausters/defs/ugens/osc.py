"""Oscillators and noise: what a graph starts from.

The band-limited pair (`saw`, `pulse`) beside the naive `lf_*` forms, the noise
family by spectrum, and `phasor` as the ramp the table readers run on.
"""

from .graph import Ugen
from ._generated.osc import *  # noqa: F403 -- the builders that mirror the wire



def _partials(freqs, amps, third, fill):
    """`freqs`, `amps` (ones unless given) and a third list (`fill` unless
    given), as the flat triples a bank takes."""
    freqs = list(freqs)
    amps = [1.0] * len(freqs) if amps is None else list(amps)
    third = [fill] * len(freqs) if third is None else list(third)
    if not (len(freqs) == len(amps) == len(third)):
        raise ValueError("a bank's lists must have one entry per partial")
    out = []
    for f, a, t in zip(freqs, amps, third):
        out += [f, a, t]
    return out


def klang(freqs, amps=None, phases=None, freqscale=1.0, freqoffset=0.0) -> Ugen:
    """A bank of fixed sines: one per entry of `freqs`, at `amps` (1 unless
    given) and starting at `phases` (cycles, 0 unless given), the frequencies
    times `freqscale` plus `freqoffset`. Read at the synth's start: a fixed
    spectrum, as cheap as sines get."""
    return Ugen("Klang", [freqscale, freqoffset, *_partials(freqs, amps, phases, 0.0)])
