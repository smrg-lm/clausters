"""The `pan` builders that mirror the wire, re-exported by `pan`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/pan.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "xfade2",
    "lin_xfade2",
]


def xfade2(a, b, pan=0.0, level=1.0) -> Ugen:
    """Equal-power crossfade between two signals: -1 is all `a`, 1 is all
    `b`, and the two gains hold unit power in between -- which keeps
    *uncorrelated* signals at one loudness across the fade, and lifts
    correlated ones by 3 dB in the middle. Use `lin_xfade2` when the two
    sides are the same signal.
    """
    return Ugen("XFade2", [a, b, pan, level])


def lin_xfade2(a, b, pan=0.0, level=1.0) -> Ugen:
    """Crossfade with the constant-amplitude law -- a plain interpolation, half
    of each at the centre. The right one for correlated sources.
    """
    return Ugen("LinXFade2", [a, b, pan, level])
