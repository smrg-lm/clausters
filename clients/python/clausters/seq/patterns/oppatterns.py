"""What an operator composes: a pattern of a pattern's values, operated on.

``pseq([1, 2, 3]) + 60`` is a `pbinop`, ``pwhite().sqrt()`` a `punop`,
``p.linlin(0, 1, 200, 800)`` a `pnarop`; they are what the operators build and
are rarely written by name. Each step takes one value from every operand (a
bare operand is the same value every time) and applies the
`clausters.base.builtins` function of the selector; the pattern ends when any
operand ends. They are value patterns: an event pattern refuses an operator.
"""

from ...base.builtins import value_op
from ...base.stream import StopStream
from ..pattern import as_stream, pattern


def _apply(inval, selector, operands, static):
    fn = value_op(selector)
    streams = [as_stream(x) for x in operands]
    while True:
        try:
            values = [s.next(inval) for s in streams]
        except StopStream:
            return inval
        inval = yield fn(*values, **static)


@pattern
def punop(inval, selector, a):
    """The unary operator ``selector`` on each value of ``a``."""
    return (yield from _apply(inval, selector, (a,), {}))


@pattern
def pbinop(inval, selector, a, b):
    """The binary operator ``selector`` on each pair of values of ``a`` and
    ``b``."""
    return (yield from _apply(inval, selector, (a, b), {}))


@pattern
def pnarop(inval, selector, a, *args, **static):
    """The n-ary operator ``selector`` (a range map, ``clip``) on each value of
    ``a`` and of each of ``args``; ``static`` are its keyword arguments, the
    same for every value (``clip="minmax"``)."""
    return (yield from _apply(inval, selector, (a, *args), static))
