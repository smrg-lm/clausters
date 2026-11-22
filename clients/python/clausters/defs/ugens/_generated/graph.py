"""The `graph` builders that mirror the wire, re-exported by `graph`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/graph.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "madd",
    "sum3",
    "sum4",
    "lin_exp",
    "in_range",
    "in_rect",
    "mod_dif",
    "unwrap",
    "amp_comp",
    "amp_comp_a",
    "k2a",
    "a2k",
    "t2a",
    "t2k",
    "dc",
    "hasher",
    "mantissa_mask",
]


def madd(a, b, c) -> Ugen:
    """`a*b + c` in one UGen (the multiply-accumulate the server fuses). The
    plain expression `a * b + c` builds the same value with two op UGens; this
    is the fused equivalent.
    """
    return Ugen("MulAdd", [a, b, c])


def sum3(a, b, c) -> Ugen:
    """`a + b + c` in one UGen."""
    return Ugen("Sum3", [a, b, c])


def sum4(a, b, c, d) -> Ugen:
    """`a + b + c + d` in one UGen."""
    return Ugen("Sum4", [a, b, c, d])


def lin_exp(signal, srclo=0.0, srchi=1.0, dstlo=1.0, dsthi=2.0) -> Ugen:
    """`signal` mapped from `srclo..srchi` to `dstlo..dsthi` exponentially,
    unclipped: equal steps in, equal ratios out. The ends of the destination
    share a sign and are not zero.
    """
    return Ugen("LinExp", [signal, srclo, srchi, dstlo, dsthi])


def in_range(signal, lo=0.0, hi=1.0) -> Ugen:
    """1 where `lo <= signal <= hi`, else 0."""
    return Ugen("InRange", [signal, lo, hi])


def in_rect(x, y, left=0.0, top=0.0, right=1.0, bottom=1.0) -> Ugen:
    """1 where the point `(x, y)` lies in the rectangle from `left`, `top` to
    `right`, `bottom`, its edges included, else 0.
    """
    return Ugen("InRect", [x, y, left, top, right, bottom])


def mod_dif(x, y=0.0, mod=1.0) -> Ugen:
    """The distance from `x` to `y` on a circle of circumference `mod`, the
    shorter way round: in `[0, mod/2]`.
    """
    return Ugen("ModDif", [x, y, mod])


def unwrap(signal, lo=0.0, hi=1.0) -> Ugen:
    """Undoes a wrap into `lo..hi`: each jump of more than half the range adds
    or takes away a whole range, so a wrapped phase comes out as one that
    keeps going. The range is read at the start, and the output starts
    inside it.
    """
    return Ugen("Unwrap", [signal, lo, hi])


def amp_comp(freq=261.62558, root=261.62558, exp=0.3333) -> Ugen:
    """`(root / freq)^exp`: the gain that keeps a higher partial from sounding
    louder than a lower one of the same amplitude. An exponent of a third
    approximates equal loudness.
    """
    return Ugen("AmpComp", [freq, root, exp])


def amp_comp_a(freq=1000.0, root=0.0, min_amp=0.32, root_amp=1.0) -> Ugen:
    """A loudness compensation along the A-weighting curve: `root_amp` at
    `root`, `min_amp` where the weighting peaks (near 2.5 kHz), and more
    than `root_amp` below it.
    """
    return Ugen("AmpCompA", [freq, root, min_amp, root_amp])


def k2a(signal) -> Ugen:
    """A control-rate signal at audio rate, a straight line across each block.
    """
    return Ugen("K2A", [signal])


def a2k(signal) -> Ugen:
    """An audio-rate signal at control rate: each block's first sample."""
    return Ugen("A2K", [signal])


def t2a(signal, offset=0.0) -> Ugen:
    """A control-rate trigger at audio rate: one sample, `offset` samples into
    the block, takes the trigger's value at each rising edge; the rest are 0.
    """
    return Ugen("T2A", [signal, offset])


def t2k(signal) -> Ugen:
    """An audio-rate trigger at control rate: each block's largest sample, so a
    trigger anywhere in the block is kept.
    """
    return Ugen("T2K", [signal])


def dc(signal) -> Ugen:
    """The constant `signal`, taken at the synth's start and held."""
    return Ugen("DC", [signal])


def hasher(signal) -> Ugen:
    """`signal`'s bits through an integer hash, as a value in +/-1: noise that is
    a function of its input, the same for the same input every time.
    """
    return Ugen("Hasher", [signal])


def mantissa_mask(signal, bits=3.0) -> Ugen:
    """`signal` with only the top `bits` (0 to 23) of its mantissa kept: the same
    relative coarseness at every level.
    """
    return Ugen("MantissaMask", [signal, bits])
