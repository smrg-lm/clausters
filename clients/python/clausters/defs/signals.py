"""Faust Signal API as composable, lowercase callables.

The user-facing way to build a FaustDef. Each function here is a small
**lowercase** callable (a design choice that keeps graph-building fluent in
Python) that returns a :class:`Signal`; composing signals with Python operators
or these functions builds the JSON **signal tree** the server's `/d_faust`
consumes (``{"signals": [ <node>, … ]}``, one node per output — see the
server's ``faust::signals``). The same lowercase pattern will return UGen-graph
nodes for SynthDefs later.

A :class:`Signal` is an :class:`~clausters.base.absobject.AbstractObject`, so
``hslider("freq", …).sin() * 0.2`` and ``sin(x) * 0.2`` both compose the graph.
Plain numbers are constants (Faust ``int``/``real``); explicit feedback uses
``recursion``/``self_`` (one sample of delay), and ``input(n)`` reads audio
input ``n``.

Reserved controls ``in`` and ``out`` (set with ``/s_new … "in" b "out" b``)
choose the input/output buses; they are added by the server, not declared here.
"""

from ..base.absobject import AbstractObject

# AbstractObject selector -> Faust Signal API op name.
_BINARY = {
    "add": "add", "sub": "sub", "mul": "mul", "div": "div", "mod": "rem",
    "pow": "pow", "min": "min", "max": "max", "atan2": "atan2",
    "gt": "gt", "lt": "lt", "ge": "ge", "le": "le", "eq": "eq", "ne": "ne",
    "bitand": "and", "bitor": "or", "bitxor": "xor", "lshift": "lsh", "rshift": "rsh",
}
_UNARY = {
    "abs": "abs", "floor": "floor", "ceil": "ceil", "sin": "sin", "cos": "cos",
    "tan": "tan", "asin": "asin", "acos": "acos", "atan": "atan", "exp": "exp",
    "log": "log", "log10": "log10", "sqrt": "sqrt", "as_int": "intcast",
    "as_float": "floatcast",
}


class Signal(AbstractObject):
    """One node of a Faust signal graph (one output). Wrap a number to make a
    constant; compose with operators or the module functions."""

    def __init__(self, node):
        # `node` is a JSON-able dict, or a bare number (a constant).
        self.node = node

    @staticmethod
    def _node_of(x):
        return x.node if isinstance(x, Signal) else x

    def to_json(self):
        return self.node

    # --- AbstractObject hooks: build graph nodes ---

    def _compose_unop(self, selector):
        if selector == "neg":  # Faust has no unary neg; 0 - x
            return Signal({"op": "sub", "in": [0.0, self.node]})
        op = _UNARY.get(selector)
        if op is None:
            raise ValueError(f"no Faust signal op for unary {selector!r}")
        return Signal({"op": op, "in": [self.node]})

    def _compose_binop(self, selector, other):
        op = _BINARY[selector]
        return Signal({"op": op, "in": [self.node, self._node_of(other)]})

    def _rcompose_binop(self, selector, other):
        op = _BINARY[selector]
        return Signal({"op": op, "in": [self._node_of(other), self.node]})

    def __repr__(self):
        return f"Signal({self.node!r})"


def signal(x) -> Signal:
    """Coerces a number or :class:`Signal` into a :class:`Signal`."""
    return x if isinstance(x, Signal) else Signal(x)


def _n(x):
    return Signal._node_of(x)


# ---- sources / structure ----

def input(index: int = 0) -> Signal:
    """Audio input ``index`` (Faust ``CsigInput``)."""
    return Signal({"op": "input", "index": int(index)})


def self_() -> Signal:
    """The one-sample-delayed output of the enclosing :func:`recursion`."""
    return Signal({"op": "self"})


def recursion(body) -> Signal:
    """Single feedback: ``body`` is a signal that may reference :func:`self_`."""
    return Signal({"op": "recursion", "in": [_n(body)]})


def rec(fn) -> Signal:
    """Pythonic feedback: ``fn(s)`` builds the body from its own delayed
    output ``s`` (sugar over :func:`recursion`/:func:`self_`)."""
    return recursion(fn(self_()))


def delay(x, n) -> Signal:
    """``x`` delayed by ``n`` samples (Faust ``CsigDelay``)."""
    return Signal({"op": "delay", "in": [_n(x), _n(n)]})


def delay1(x) -> Signal:
    return Signal({"op": "delay1", "in": [_n(x)]})


def select2(sel, a, b) -> Signal:
    return Signal({"op": "select2", "in": [_n(sel), _n(a), _n(b)]})


def select3(sel, a, b, c) -> Signal:
    return Signal({"op": "select3", "in": [_n(sel), _n(a), _n(b), _n(c)]})


# ---- unary functions (also available as methods) ----

def _unary(op):
    return lambda x: Signal({"op": op, "in": [_n(x)]})


sin = _unary("sin")
cos = _unary("cos")
tan = _unary("tan")
asin = _unary("asin")
acos = _unary("acos")
atan = _unary("atan")
exp = _unary("exp")
exp10 = _unary("exp10")
log = _unary("log")
log10 = _unary("log10")
sqrt = _unary("sqrt")
abs = _unary("abs")  # noqa: A001 — Signal API name, by design
floor = _unary("floor")
ceil = _unary("ceil")
rint = _unary("rint")


# ---- binary functions ----

def _binary(op):
    return lambda a, b: Signal({"op": op, "in": [_n(a), _n(b)]})


min = _binary("min")  # noqa: A001
max = _binary("max")  # noqa: A001
pow = _binary("pow")  # noqa: A001
atan2 = _binary("atan2")
fmod = _binary("fmod")
rem = _binary("rem")


# ---- controls (labels become control names) ----

def hslider(label, init, lo, hi, step) -> Signal:
    return Signal({"op": "hslider", "label": label, "init": init,
                   "min": lo, "max": hi, "step": step})


def vslider(label, init, lo, hi, step) -> Signal:
    return Signal({"op": "vslider", "label": label, "init": init,
                   "min": lo, "max": hi, "step": step})


def nentry(label, init, lo, hi, step) -> Signal:
    return Signal({"op": "nentry", "label": label, "init": init,
                   "min": lo, "max": hi, "step": step})


def button(label) -> Signal:
    return Signal({"op": "button", "label": label})


def checkbox(label) -> Signal:
    return Signal({"op": "checkbox", "label": label})


# ---- tables ----

def waveform(values) -> Signal:
    return Signal({"op": "waveform", "values": [float(v) for v in values]})


def rdtable(size, init, ridx) -> Signal:
    return Signal({"op": "rdtable", "in": [_n(size), _n(init), _n(ridx)]})


def rwtable(size, init, widx, wsig, ridx) -> Signal:
    return Signal({"op": "rwtable",
                   "in": [_n(size), _n(init), _n(widx), _n(wsig), _n(ridx)]})
