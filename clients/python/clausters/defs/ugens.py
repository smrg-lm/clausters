"""UGen graph as composable, lowercase callables (port of the UGen side of
``sc3/synth``, adapted to Clausters' server format).

The UGen-graph counterpart of `clausters.defs.signals`: each function here
is a small **lowercase** callable that returns a `Ugen` node (one
output); composing nodes with Python operators or these functions builds the
graph a `SynthDef` serializes into the JSON
``SynthDefSpec`` the server's ``/d_recv`` consumes (``{"controls": […],
"ugens": […]}`` — see the server's ``synthdef`` module).

**Instance-based, no global build context.** Unlike sclang — where ``SynthDef``
build relies on a thread-global "current graph" that every ``UGen.new`` mutates
(``UGen.buildSynthDef``) — the graph here *is* the tree of composed objects: a
``Ugen``'s inputs hold its operands directly, and the `SynthDef` walks
that tree to emit the spec. Nothing is global, so several defs can be built
concurrently.

**The server's UGen set is small** (``SinOsc``, ``Impulse``, ``WhiteNoise``,
``In``/``InCtl``, ``Out``/``ReplaceOut``, ``PlayBuf``/``BufRd``,
``LocalIn``/``LocalOut`` and the four arithmetic ops). There are **no math
UGens**: only ``+ - * /`` map to UGens (``Add``/``Sub``/``Mul``/``Div``); any
other operator (``sin``, ``%``, ``min``, comparisons …) raises — reach for a
Faust def (`clausters.defs.signals`) when you need them.

Reserved controls ``in`` and ``out`` (the input/output buses, set with
``/s_new … "in" b "out" b``) are added by the server, not declared here.
"""

from ..base.absobject import AbstractObject

#: AbstractObject binary selector -> UGen kind. Only the four the server has.
_BINOP_UGEN = {"add": "Add", "sub": "Sub", "mul": "Mul", "div": "Div"}


class _Node(AbstractObject):
    """Shared operator dispatch for graph leaves (`Ugen`,
    `Control`): the four arithmetic operators compose UGen nodes, every
    other operator is rejected (the server has no UGen for it)."""

    def _compose_binop(self, selector, other):
        kind = _BINOP_UGEN.get(selector)
        if kind is None:
            raise TypeError(
                f"no UGen for operator {selector!r}: the server's UGen set has "
                f"only + - * / — use a Faust def (clausters.defs.signals) for {selector!r}"
            )
        return Ugen(kind, [self, other])

    def _rcompose_binop(self, selector, other):
        kind = _BINOP_UGEN.get(selector)
        if kind is None:
            raise TypeError(
                f"no UGen for operator {selector!r}: the server's UGen set has "
                f"only + - * / — use a Faust def (clausters.defs.signals) for {selector!r}"
            )
        return Ugen(kind, [other, self])

    def _compose_unop(self, selector):
        raise TypeError(
            f"no UGen for unary {selector!r}: UGen graphs have no math UGens — "
            f"use a Faust def (clausters.defs.signals)"
        )

    def _compose_narop(self, selector, *args):
        raise TypeError(f"no n-ary UGen for {selector!r}")


class Ugen(_Node):
    """One UGen node (one output). ``kind`` is a server UGen name; ``inputs``
    is a list of operands, each a `Ugen`, a `Control`, or a plain
    number (a constant). Build them with the lowercase callables below rather
    than directly."""

    def __init__(self, kind: str, inputs):
        self.kind = kind
        self.inputs = list(inputs)

    def __repr__(self):
        return f"Ugen({self.kind!r}, {self.inputs!r})"


class Control(_Node):
    """A named control with a default — a ``/s_new``/``/n_set`` parameter. Used
    as a UGen input it serializes to a ``{"control": index}`` reference; the
    `SynthDef` gathers the controls a graph references, in first-seen
    order."""

    def __init__(self, name: str, default: float = 0.0):
        self.name = str(name)
        self.default = float(default)

    def __repr__(self):
        return f"Control({self.name!r}, {self.default!r})"


def control(name: str, default: float = 0.0) -> Control:
    """A named control (``/s_new``/``/n_set`` parameter) with a default."""
    return Control(name, default)


# ---- lowercase UGen callables (the client's "instruction set") ----
# Input order matches the server's registry; see docs/schemas.md.


def sin_osc(freq=440.0) -> Ugen:
    """Sine by f64 phase accumulation, starting at phase 0."""
    return Ugen("SinOsc", [freq])


def impulse(freq=1.0) -> Ugen:
    """A single-sample ``1.0`` every ``freq`` Hz, ``0.0`` between (``freq`` 0 =
    one impulse then silence). The first sample is always an impulse."""
    return Ugen("Impulse", [freq])


def white_noise() -> Ugen:
    """Uniform white noise in ±1."""
    return Ugen("WhiteNoise", [])


def in_(bus=0.0) -> Ugen:
    """Reads an audio bus (sampled per block)."""
    return Ugen("In", [bus])


def in_ctl(bus=0.0) -> Ugen:
    """Reads a control-bus value, constant over the block."""
    return Ugen("InCtl", [bus])


def out(bus, signal) -> Ugen:
    """Sums ``signal`` into the audio ``bus`` (output happens only here)."""
    return Ugen("Out", [bus, signal])


def replace_out(bus, signal) -> Ugen:
    """Overwrites the audio ``bus`` with ``signal`` instead of summing."""
    return Ugen("ReplaceOut", [bus, signal])


def play_buf(bufnum, chan=0.0, rate=1.0, loop=0.0) -> Ugen:
    """Mono buffer player with linear interpolation; ``rate`` is frames per
    output sample (1.0 = server rate)."""
    return Ugen("PlayBuf", [bufnum, chan, rate, loop])


def buf_rd(bufnum, chan, phase, loop=0.0) -> Ugen:
    """Reads a buffer at a ``phase`` signal in frames (linear interpolation)."""
    return Ugen("BufRd", [bufnum, chan, phase, loop])


def local_in(channel=0.0) -> Ugen:
    """Reads synth-private feedback channel ``channel`` (a constant); pairs with
    `local_out` for one-block feedback. ``LocalIn`` must precede its
    ``LocalOut`` — the `SynthDef`'s topological order does that as long
    as the output graph reaches the ``local_in`` before the ``local_out``."""
    return Ugen("LocalIn", [channel])


def local_out(channel, signal) -> Ugen:
    """Writes ``signal`` into synth-private feedback channel ``channel`` (a
    constant); also passes ``signal`` through as its output (so it can be a
    SynthDef output to keep the write in the graph)."""
    return Ugen("LocalOut", [channel, signal])
