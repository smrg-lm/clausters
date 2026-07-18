"""Symbolic per-bin expressions for `pv_kernel` — the general per-frame
spectral mechanism.

The terms `mag`, `phase`, `bin_index`, `nbins`, `binfreq` and `param(i)` are
symbolic per-bin values; composing them with Python operators and math methods
(the same `AbstractObject` vocabulary UGen graphs and off-RT values use) builds
an expression tree that `pv_kernel` serializes to the postfix token list the
server's ``PV_Kernel`` validates and interprets — once per bin, on each fresh
spectral frame.

```python
from clausters.defs import fft, ifft, pv_kernel, SynthDef, control, out
from clausters.defs.pv_expr import mag, phase, bin_index, nbins, param

chain = fft(source)
# A spectral gate: zero the bins below a threshold parameter.
chain = pv_kernel(chain, mag=mag * (mag >= param(0)),
                  params=[control("thresh", 1.0)])
sig = ifft(chain)
```

**What an expression can be**: a pure map from one bin's values —
``(mag, phase, bin_index, nbins, binfreq, param(i)…)`` — to the bin's new
magnitude or phase. No state between bins or frames, no reading *other* bins:
cross-frame ops (freeze, smear) and bin remaps (shift) stay with the dedicated
``pv_*`` filters. Anything that *is* a per-bin map — gates, tilts, masks,
magnitude algebra — is an expression here, never a new server UGen.

The operator set is the shared table (`clausters.base.builtins` /
``clausters_core::builtins``): everything the value side and the UGen graphs
compute is available per bin, with the same formulas — a rendered kernel is
bit-identical between real-time and offline.
"""

from ..base.absobject import AbstractObject
from .ugens import _BINOP_OPS, _UNOP_OPS

__all__ = [
    "PvExpr", "mag", "phase", "bin_index", "nbins", "binfreq", "param",
    "pv_tokens",
]

#: `+ - * /` compose dedicated alias kinds in UGen graphs, but in a bin
#: expression every operator is a wire name; these four map straight through.
_ARITH = {"add", "sub", "mul", "div"}


class PvExpr(AbstractObject):
    """A node of a symbolic per-bin expression. Build these by composing the
    module's terms (`mag`, `phase`, …) with operators and math methods; pass
    the result to `pv_kernel`, which serializes it with `pv_tokens`."""

    def _compose_binop(self, selector, other):
        return _BinNode(_binop(selector), self, _operand(other))

    def _rcompose_binop(self, selector, other):
        return _BinNode(_binop(selector), _operand(other), self)

    def _compose_unop(self, selector):
        if selector not in _UNOP_OPS:
            raise TypeError(f"no per-bin operator {selector!r}")
        return _UnNode(selector, self)

    def _compose_narop(self, selector, *args):
        raise TypeError(f"no n-ary per-bin operator {selector!r}")


def _binop(selector):
    if selector not in _ARITH and selector not in _BINOP_OPS:
        raise TypeError(f"no per-bin operator {selector!r}")
    return selector


def _operand(x):
    if isinstance(x, PvExpr):
        return x
    if isinstance(x, bool) or not isinstance(x, (int, float)):
        raise TypeError(
            f"a per-bin expression operand must be a PvExpr term or a number, "
            f"got {x!r}"
        )
    return float(x)


class _Term(PvExpr):
    """A leaf term: one wire word (`"mag"`, `"bin"`, `"p0"`, …)."""

    def __init__(self, word):
        self.word = word

    def __repr__(self):
        return f"pv_expr.{self.word}"


class _UnNode(PvExpr):
    def __init__(self, op, a):
        self.op, self.a = op, a

    def __repr__(self):
        return f"{self.op}({self.a!r})"


class _BinNode(PvExpr):
    def __init__(self, op, a, b):
        self.op, self.a, self.b = op, a, b

    def __repr__(self):
        return f"{self.op}({self.a!r}, {self.b!r})"


#: The bin's magnitude.
mag = _Term("mag")
#: The bin's phase in radians.
phase = _Term("phase")
#: The bin index, ``0 .. nbins - 1`` (named to avoid shadowing ``bin``).
bin_index = _Term("bin")
#: The bin count (``fft_size / 2 + 1``).
nbins = _Term("nbins")
#: The bin's center frequency in Hz.
binfreq = _Term("binfreq")


def param(i) -> PvExpr:
    """Parameter ``i`` — `pv_kernel`'s ``params[i]`` signal input, sampled at
    the hop. Parameters are how an expression stays *controllable*: a
    threshold, a tilt amount, an LFO."""
    i = int(i)
    if i < 0:
        raise ValueError(f"parameter index must be >= 0, got {i}")
    return _Term(f"p{i}")


def pv_tokens(expr) -> list:
    """Serializes an expression tree (or a plain number) to the postfix token
    list the server's ``PV_Kernel`` consumes: numbers push constants, words are
    per-bin loads or operator names."""
    out = []

    def walk(node):
        if isinstance(node, float):
            out.append(node)
        elif isinstance(node, _Term):
            out.append(node.word)
        elif isinstance(node, _UnNode):
            walk(node.a)
            out.append(node.op)
        elif isinstance(node, _BinNode):
            walk(node.a)
            walk(node.b)
            out.append(node.op)
        else:
            raise TypeError(f"not a per-bin expression node: {node!r}")

    walk(_operand(expr))
    return out
