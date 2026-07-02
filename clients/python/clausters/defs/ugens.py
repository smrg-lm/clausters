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
``LocalIn``/``LocalOut``, ``EnvGen`` and the four arithmetic ops). There are
**no math UGens**: only ``+ - * /`` map to UGens (``Add``/``Sub``/``Mul``/``Div``);
any other operator (``sin``, ``%``, ``min``, comparisons …) raises — reach for a
Faust def (`clausters.defs.signals`) when you need them.

Envelopes are the `Env` breakpoint builder plus the `env_gen` callable, which
serialize to the ``EnvGen`` UGen's flat input list.

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


# ---- envelopes (EnvGen) ----


class DoneAction:
    """The action `env_gen` takes when its envelope finishes, mirroring the
    subset of scsynth done actions the server implements. Pass one as
    ``done_action``."""

    #: Do nothing; the envelope just holds its final level.
    NONE = 0
    #: Pause the synth (stops processing; it stays in the tree).
    PAUSE_SELF = 1
    #: Free the synth — the usual choice for a one-shot or a released note.
    FREE_SELF = 2
    #: Free the synth's whole enclosing group.
    FREE_GROUP = 14


#: Envelope shape name -> the server's shape number. A numeric curve value maps
#: to the custom-curvature shape (5) instead.
_SHAPE_NUMBERS = {
    "step": 0,
    "lin": 1,
    "linear": 1,
    "exp": 2,
    "exponential": 2,
    "sin": 3,
    "sine": 3,
    "wel": 4,
    "welch": 4,
    "sqr": 6,
    "squared": 6,
    "cub": 7,
    "cubed": 7,
    "hold": 8,
}


def _resolve_curve(spec):
    """A shape name (``'lin'``, ``'exp'``, ``'sin'`` …) or a numeric curvature
    -> ``(shape_number, curve_value)``. A number selects the custom-curvature
    shape, where 0 is linear, positive starts slow, negative starts fast."""
    if isinstance(spec, str):
        try:
            return (_SHAPE_NUMBERS[spec], 0.0)
        except KeyError:
            raise ValueError(
                f"unknown envelope shape {spec!r}; use one of "
                f"{sorted(set(_SHAPE_NUMBERS))} or a numeric curvature"
            ) from None
    return (5, float(spec))


class Env:
    """A breakpoint envelope: `levels` (one more than `times`), the segment
    `times` in seconds, and a `curve` per segment (a shape name, a numeric
    curvature, or a list of either, one per segment).

    `release_node` is the index into `levels` where the envelope sustains while
    the gate is held (``None`` = no sustain, plays straight through). Feed it to
    `env_gen`. Modelled on SuperCollider's ``Env``; the shapes match the
    server's `EnvGen`."""

    def __init__(self, levels, times, curve="lin", release_node=None, loop_node=None):
        self.levels = [float(x) for x in levels]
        self.times = [float(x) for x in times]
        if len(self.levels) != len(self.times) + 1:
            raise ValueError(
                f"levels ({len(self.levels)}) must be one longer than "
                f"times ({len(self.times)})"
            )
        if isinstance(curve, (list, tuple)):
            if len(curve) != len(self.times):
                raise ValueError(
                    f"curve list ({len(curve)}) must match the number of "
                    f"segments ({len(self.times)})"
                )
            self.curves = list(curve)
        else:
            self.curves = [curve] * len(self.times)
        self.release_node = release_node
        self.loop_node = loop_node

    @classmethod
    def perc(cls, attack=0.01, release=1.0, level=1.0, curve=-4.0):
        """A fixed-duration percussive hit: 0 -> `level` -> 0. No sustain, so a
        rising gate triggers the whole thing."""
        return cls([0.0, level, 0.0], [attack, release], curve)

    @classmethod
    def adsr(
        cls,
        attack=0.01,
        decay=0.3,
        sustain=0.5,
        release=1.0,
        peak=1.0,
        curve=-4.0,
    ):
        """The classic attack/decay/sustain/release. Sustains at ``peak *
        sustain`` (the release node) until the gate falls."""
        return cls(
            [0.0, peak, peak * sustain, 0.0],
            [attack, decay, release],
            curve,
            release_node=2,
        )

    @classmethod
    def asr(cls, attack=0.01, sustain=1.0, release=1.0, curve=-4.0):
        """Attack to `sustain`, hold there until release, then fall to 0."""
        return cls([0.0, sustain, 0.0], [attack, release], curve, release_node=1)

    def to_inputs(self):
        """The envelope as the flat number list `env_gen` appends after its
        fixed inputs: ``initLevel, numSegments, releaseNode, loopNode`` then
        ``target, duration, shape, curve`` per segment."""
        n = len(self.times)
        rel = -1.0 if self.release_node is None else float(self.release_node)
        loop = -1.0 if self.loop_node is None else float(self.loop_node)
        out = [self.levels[0], float(n), rel, loop]
        for i in range(n):
            shape, cval = _resolve_curve(self.curves[i])
            out += [self.levels[i + 1], self.times[i], float(shape), cval]
        return out


def env_gen(
    env: Env,
    gate=1.0,
    level_scale=1.0,
    level_bias=0.0,
    time_scale=1.0,
    done_action=DoneAction.NONE,
) -> Ugen:
    """Plays an `Env`. A rising `gate` (re)triggers from the start; while the
    gate is held the envelope sustains at the env's release node; when the gate
    falls it plays the release segments. `level_scale`/`level_bias` affine the
    output, `time_scale` stretches every segment. `done_action` is taken when
    the envelope finishes (see `DoneAction`)."""
    fixed = [gate, level_scale, level_bias, time_scale, float(done_action)]
    return Ugen("EnvGen", fixed + env.to_inputs())
