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
