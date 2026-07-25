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

**The server's UGen set is deliberately focused**: oscillators/sources
(``sine``, ``impulse``, ``white_noise``, `rand`), the table oscillators and
waveshaper (``osc``/``oscn``/``vosc``, ``shaper``), bus and buffer I/O
(``in_``/``in_ctl``, ``out``/``replace_out``, ``play_buf``/``buf_rd``, the
``buf_*`` info queries), streaming disk I/O (``disk_in``/``disk_out``),
feedback (``local_in``/``local_out``), the ``env_gen`` envelope, the ``lag``/
``var_lag`` smoothers, the demand pair (``dseq``/``demand``), and the fused
``madd``/``sum3``/``sum4``. **Maths works**: ``+ - * /`` map to the
``Add``/``Sub``/``Mul``/``Div`` kinds and every other operator or method
(``%``, ``min``/``max``, comparisons, ``.sin()``, ``.midicps()``,
``.distort()`` …) composes a generic ``BinaryOpUGen``/``UnaryOpUGen`` carrying
the operator name — the same op the value side computes, so the two agree
bit-for-bit. Reach for a Faust def (`clausters.defs.signals`) only for genuinely
custom per-sample DSP (recursion, tables, sample-accurate feedback).

**Multichannel is an explicit container**, not implicit expansion: `dup` fans
a signal out into a `ChannelList` (by reference for a node, by evaluation for
a callable), operators broadcast/zip over it (wrapping the shorter side
modulo, the value side's rule), ``out(bus, chans)`` lays the channels on
consecutive buses, and `mix` folds a list back to one channel through the
fused sums. sclang-style per-argument expansion (``sine([440, 443])``) is
deliberately **not** implemented — a channel list reaching a single-channel
input is a `TypeError` at serialization.

Each UGen output carries a **rate** (``ir``/``kr``/``ar``/``dr``); it defaults
per kind and can be set with `Ugen.at_rate`. Controls carry a **type** and an
optional **lag** — see `control`/`Control`.

Envelopes are the `Env` breakpoint builder plus the `env_gen` callable, which
serialize to the ``EnvGen`` UGen's flat input list.

Reserved controls ``in`` and ``out`` (the input/output buses, set with
``/s_new … "in" b "out" b``) are added by the server, not declared here.
"""

from ..base import builtins as _builtins
from ..base.absobject import AbstractObject

#: The four arithmetic selectors keep their dedicated alias kinds, so existing
#: defs and their serialized graphs are unchanged.
_BINOP_UGEN = {"add": "Add", "sub": "Sub", "mul": "Mul", "div": "Div"}

#: Every other operator/method selector composes a generic ``BinaryOpUGen``/
#: ``UnaryOpUGen`` whose ``op`` is the operator **name** (S3) — the same name
#: the server's `clausters_core::builtins` table resolves, and the same op the
#: value side (`clausters.base.builtins`) computes, so a graph op and an off-RT
#: value agree. The selector *is* the wire name (no numeric index crosses the
#: wire); these sets say which selectors have a server op.
_BINOP_OPS = frozenset({
    "mod", "pow", "min", "max", "atan2", "gt", "lt", "ge", "le", "eq", "ne",
    "bitand", "bitor", "bitxor", "lshift", "rshift", "hypot", "ring1", "ring2",
    "ring3", "ring4", "sumsqr", "difsqr", "sqrsum", "sqrdif", "absdif",
    "thresh", "clip2", "excess", "round", "trunc", "fold2", "wrap2", "gcd",
    "lcm", "hypot_apx",
})
_UNOP_OPS = frozenset({
    "neg", "abs", "sin", "cos", "tan", "asin", "acos", "atan", "exp", "log",
    "log10", "log2", "sqrt", "floor", "ceil", "rint", "as_int", "as_float",
    "squared", "cubed", "recip", "frac", "sign", "sinh", "cosh", "tanh",
    "distort", "softclip", "midicps", "cpsmidi", "midiratio", "ratiomidi",
    "dbamp", "ampdb", "octcps", "cpsoct",
})


class _Node(AbstractObject):
    """Shared operator dispatch for graph leaves (`Ugen`, `Control`): `+ - * /`
    compose the dedicated alias kinds; every other operator and math method
    (`%`, `min`, comparisons, `.sin()`, `.midicps()`, …) composes a generic
    `BinaryOpUGen`/`UnaryOpUGen` carrying the operator name."""

    def _compose_binop(self, selector, other):
        if isinstance(other, (ChannelList, list, tuple)):
            return ChannelList(other)._rcompose_binop(selector, self)
        kind = _BINOP_UGEN.get(selector)
        if kind is not None:
            return Ugen(kind, [self, other])
        if selector not in _BINOP_OPS:
            raise TypeError(f"no binary UGen for operator {selector!r}")
        return Ugen("BinaryOpUGen", [self, other], op=selector)

    def _rcompose_binop(self, selector, other):
        if isinstance(other, (ChannelList, list, tuple)):
            return ChannelList(other)._compose_binop(selector, self)
        kind = _BINOP_UGEN.get(selector)
        if kind is not None:
            return Ugen(kind, [other, self])
        if selector not in _BINOP_OPS:
            raise TypeError(f"no binary UGen for operator {selector!r}")
        return Ugen("BinaryOpUGen", [other, self], op=selector)

    def _compose_unop(self, selector):
        if selector not in _UNOP_OPS:
            raise TypeError(f"no unary UGen for operator {selector!r}")
        return Ugen("UnaryOpUGen", [self], op=selector)

    def _compose_narop(self, selector, *args):
        raise TypeError(f"no n-ary UGen for {selector!r}")

    def dup(self, n=2) -> "ChannelList":
        """This node repeated (by reference) as ``n`` channels — see `dup`."""
        return ChannelList([self] * n)


class Ugen(_Node):
    """One UGen node (one output). ``kind`` is a server UGen name; ``inputs``
    is a list of operands, each a `Ugen`, a `Control`, or a plain
    number (a constant). Build them with the lowercase callables below rather
    than directly.

    ``rate`` is the optional output calculation rate (``"ir"``/``"kr"``/
    ``"ar"``/``"dr"``); ``None`` lets the server pick the kind's default (``ar``
    for signal UGens). Set it fluently with `at_rate`. ``op`` is the operator
    **name** carried by the generic ``BinaryOpUGen``/``UnaryOpUGen`` (S3), e.g.
    ``"mul"`` / ``"midicps"``; ``None`` for every other kind. ``label`` is the
    string tag the side-effect UGens carry — ``send_reply``'s command name and
    ``poll``'s label; ``None`` for every other kind. ``static`` is a dict of any
    other non-signal fields (the spectral UGens' ``fft_size``/``hop``/
    ``wintype``); it merges verbatim into the serialized UGen spec."""

    def __init__(self, kind: str, inputs, rate=None, op=None, label=None, static=None):
        self.kind = kind
        self.inputs = list(inputs)
        self.rate = None if rate is None else str(rate)
        self.op = None if op is None else str(op)
        self.label = None if label is None else str(label)
        self.static = dict(static) if static else None

    def at_rate(self, rate: str) -> "Ugen":
        """Set this UGen's output rate (``"ir"``/``"kr"``/``"ar"``/``"dr"``) and
        return it, e.g. ``sine(5.0).at_rate("kr")`` for a control-rate LFO."""
        self.rate = str(rate)
        return self

    def __repr__(self):
        return f"Ugen({self.kind!r}, {self.inputs!r})"


#: Control types the server accepts (with their spellings). ``None`` = default
#: (``kr``). See ``docs/schemas.md`` "Control types".
_CONTROL_RATES = {"kr", "control", "tr", "trigger", "ir", "scalar"}


class Control(_Node):
    """A named control (a ``/s_new``/``/n_set`` parameter) with a default and an
    optional **type** and **lag** (S2), mirroring the server's control types:

    - ``rate="tr"`` — a **trigger**: a ``/n_set`` holds for one block, then the
      server resets it to 0 (drives an `env_gen` gate, a sample-and-hold).
    - ``rate="ir"`` — a **scalar**: read once at init and frozen; a later
      ``/n_set`` is ignored. As ``ir`` it may feed an ``ir`` input (`rand`,
      buffer-info UGens).
    - ``lag`` (seconds) — smooth a ``kr`` control's changes with an implicit
      one-pole (a `lag`/`var_lag` UGen the server inserts); ``lag_down`` gives a
      separate downward time.

    Used as a UGen input it serializes to a ``{"control": index}`` reference;
    the `SynthDef` gathers the controls a graph references, in first-seen
    order."""

    def __init__(self, name, default=0.0, rate=None, lag=None, lag_down=None):
        self.name = str(name)
        self.default = float(default)
        self.rate = None if rate is None else str(rate)
        self.lag = None if lag is None else float(lag)
        self.lag_down = None if lag_down is None else float(lag_down)
        if self.rate is not None and self.rate not in _CONTROL_RATES:
            raise ValueError(
                f"unknown control type {self.rate!r}; use one of "
                f"{sorted(_CONTROL_RATES)}"
            )
        if self.lag_down is not None and self.lag is None:
            raise ValueError("lag_down requires lag (the up time)")

    def _signature(self):
        """The full identity used to detect conflicting reuses of a name."""
        return (self.default, self.rate, self.lag, self.lag_down)

    def __repr__(self):
        return f"Control({self.name!r}, {self.default!r})"


def control(name, default=0.0, rate=None, lag=None, lag_down=None) -> Control:
    """A named control (``/s_new``/``/n_set`` parameter). ``rate`` is its type
    (``"tr"`` trigger, ``"ir"`` scalar, or the default ``kr``); ``lag`` (with an
    optional ``lag_down``) smooths a ``kr`` control. See `Control`."""
    return Control(name, default, rate=rate, lag=lag, lag_down=lag_down)


def _channel(m):
    """Validates one channel-list member: a graph leaf or a plain number."""
    if isinstance(m, ChannelList):
        raise TypeError(
            "nested channel lists are not supported: mix() the inner one down "
            "or build a flat list"
        )
    if isinstance(m, bool) or not isinstance(m, (_Node, int, float)):
        raise TypeError(f"not a UGen graph node: {m!r}")
    return m


def _channel_unop(m, selector):
    if isinstance(m, _Node):
        return m._compose_unop(selector)
    return _builtins.UNARY[selector](m)


def _channel_binop(a, selector, b):
    if isinstance(a, _Node):
        return a._compose_binop(selector, b)
    if isinstance(b, _Node):
        return b._rcompose_binop(selector, a)
    return _builtins.BINARY[selector](a, b)


class ChannelList(AbstractObject):
    """An ordered list of channels — the client's multichannel container.

    Members are graph leaves (`Ugen`/`Control`) or plain numbers. Operators
    and math methods map over the members and return a new `ChannelList`: a
    scalar operand **broadcasts** to every channel, a list operand **zips**
    channel-wise, and unequal lengths wrap the shorter one modulo — the same
    rule the value side applies to plain lists (`clausters.base.builtins`).
    Plain Python lists/tuples are accepted anywhere a `ChannelList` is (they
    coerce), but the class is the one with graph operators — ``[a, b] * 2``
    is Python list repetition, ``chans(a, b) * 2`` is a graph.

    The container never crosses the wire: `out` and friends unroll it onto
    consecutive buses, and the `SynthDef` serialization flattens it — the
    server only ever sees single-channel UGens. Feeding one to a
    single-channel input (``env_gen(gate=chans(...))``) is an error: index it
    or `mix` it down. Build one with `dup`, a literal list at an accepting
    argument, or `chans`."""

    def __init__(self, items):
        if isinstance(items, ChannelList):
            items = items.items
        members = [_channel(m) for m in items]
        if not members:
            raise ValueError("a channel list needs at least one channel")
        self.items = members

    def __iter__(self):
        return iter(self.items)

    def __len__(self):
        return len(self.items)

    def __getitem__(self, i):
        got = self.items[i]
        return ChannelList(got) if isinstance(i, slice) else got

    def __repr__(self):
        return f"ChannelList({self.items!r})"

    def _pairs(self, other):
        """Channel pairs for a binary op: broadcast a scalar, zip a list
        (wrapping the shorter side modulo)."""
        if isinstance(other, (ChannelList, list, tuple)):
            o = ChannelList(other).items
            n = max(len(self.items), len(o))
            return [(self.items[i % len(self.items)], o[i % len(o)]) for i in range(n)]
        return [(m, other) for m in self.items]

    def _compose_unop(self, selector):
        return ChannelList([_channel_unop(m, selector) for m in self.items])

    def _compose_binop(self, selector, other):
        return ChannelList(
            [_channel_binop(a, selector, b) for a, b in self._pairs(other)]
        )

    def _rcompose_binop(self, selector, other):
        return ChannelList(
            [_channel_binop(b, selector, a) for a, b in self._pairs(other)]
        )

    def _compose_narop(self, selector, *args):
        raise TypeError(f"no n-ary UGen for {selector!r}")

    def at_rate(self, rate: str) -> "ChannelList":
        """Sets every member's output rate (see `Ugen.at_rate`)."""
        for m in self.items:
            if isinstance(m, Ugen):
                m.at_rate(rate)
        return self

    def mix(self):
        """This list folded to one channel — see `mix`."""
        return mix(self)


def chans(*items) -> ChannelList:
    """A `ChannelList` from the arguments (``chans(a, b)``) or from a single
    iterable (``chans([a, b])``)."""
    if len(items) == 1 and isinstance(items[0], (ChannelList, list, tuple)):
        return ChannelList(items[0])
    return ChannelList(items)


def dup(x, n=2) -> ChannelList:
    """``x`` as ``n`` channels.

    A graph node (or a number) is repeated **by reference** — the graph
    serializes it once, fanned out to every channel, so ``dup(sine(440))`` is
    a cheap mono→stereo: identical channels. A **callable** is evaluated ``n``
    times — ``dup(white_noise, 8)`` (or ``dup(lambda: sine(rand(438, 442)),
    8)``) builds ``n`` *distinct* UGens, which is what a decorrelated or
    detuned bank needs; duplicating a `white_noise` by reference would give
    ``n`` copies of the *same* noise. This mirrors sclang's ``ugen.dup`` vs
    ``{ }.dup``. Also available as a method: ``sine(440).dup(8)`` (always by
    reference)."""
    if isinstance(n, bool) or not isinstance(n, int) or n < 1:
        raise ValueError(f"dup needs a positive channel count, got {n!r}")
    if isinstance(x, (int, float)) or isinstance(x, AbstractObject):
        return ChannelList([x] * n)
    if callable(x):
        return ChannelList([x() for _ in range(n)])
    raise TypeError(f"cannot dup {x!r}: expected a graph node, a number or a callable")


def mix(x):
    """``x`` folded to one channel by summing.

    The inverse gesture of `dup`: a `ChannelList` (or plain list) becomes one
    signal, folded with the fused sum kinds — `sum4`/`sum3` chunks instead of
    an `Add` chain, so an 8-channel mix costs 2 UGens + 1, not 7. A scalar or
    single node passes through; a list of plain numbers folds to a number."""
    if not isinstance(x, (ChannelList, list, tuple)):
        return x
    items = ChannelList(x).items
    if all(not isinstance(m, _Node) for m in items):
        total = items[0]
        for m in items[1:]:
            total = _builtins.BINARY["add"](total, m)
        return total
    while len(items) > 1:
        folded = []
        for k in range(0, len(items), 4):
            chunk = items[k:k + 4]
            if len(chunk) == 4:
                folded.append(sum4(*chunk))
            elif len(chunk) == 3:
                folded.append(sum3(*chunk))
            elif len(chunk) == 2:
                folded.append(_channel_binop(chunk[0], "add", chunk[1]))
            else:
                folded.append(chunk[0])
        items = folded
    return items[0]


# ---- lowercase UGen callables (the client's "instruction set") ----
# Input order matches the server's registry; see docs/schemas.md.


def sine(freq=440.0) -> Ugen:
    """Sine by f64 phase accumulation, starting at phase 0."""
    return Ugen("Sine", [freq])


def impulse(freq=1.0) -> Ugen:
    """A single-sample ``1.0`` every ``freq`` Hz, ``0.0`` between (``freq`` 0 =
    one impulse then silence). The first sample is always an impulse."""
    return Ugen("Impulse", [freq])


def white_noise() -> Ugen:
    """Uniform white noise in ±1."""
    return Ugen("WhiteNoise", [])


def saw(freq=440.0) -> Ugen:
    """Band-limited rising sawtooth in ±1, starting at 0.

    Anti-aliased with PolyBLEP, which is very clean over the low and middle
    range and progressively less so toward Nyquist (its residual grows about
    as the square of the frequency). It carries no DC offset.
    """
    return Ugen("Saw", [freq])


def pulse(freq=440.0, width=0.5) -> Ugen:
    """Band-limited pulse in ±1; ``width`` is the duty cycle (0.5 = square).

    Anti-aliased like `saw`. The width is clamped just inside ``(0, 1)``,
    where the two edges would coincide.
    """
    return Ugen("Pulse", [freq, width])


def lf_saw(freq=440.0, iphase=0.0) -> Ugen:
    """Rising sawtooth in ±1, **not** band-limited — a modulation shape.

    ``iphase`` is the initial phase in **cycles**, ``[0, 1)``, read once at the
    first sample. (sclang measures the same argument in ``[0, 2)``; every phase
    in this client is in cycles.)
    """
    return Ugen("LFSaw", [freq, iphase])


def lf_pulse(freq=440.0, iphase=0.0, width=0.5) -> Ugen:
    """Square in ``[0, 1]`` — a gate, not a bipolar waveform like `pulse` — with
    ``width`` as its duty cycle. Not band-limited. ``iphase`` as in `lf_saw`."""
    return Ugen("LFPulse", [freq, iphase, width])


def lf_tri(freq=440.0, iphase=0.0) -> Ugen:
    """Triangle in ±1, starting at 0 and rising. Not band-limited.
    ``iphase`` as in `lf_saw`."""
    return Ugen("LFTri", [freq, iphase])


def var_saw(freq=440.0, iphase=0.0, width=0.5) -> Ugen:
    """Triangle whose peak sits at ``width`` of the cycle, in ±1: sweeps from a
    falling ramp through a triangle to a rising one. Not band-limited.
    ``iphase`` as in `lf_saw`."""
    return Ugen("VarSaw", [freq, iphase, width])


def phasor(trig=0.0, rate=1.0, start=0.0, end=1.0, reset_pos=0.0) -> Ugen:
    """Ramp from ``start`` to ``end`` advancing by ``rate`` **per sample**,
    wrapping at ``end``; a rising ``trig`` jumps to ``reset_pos``.

    ``rate`` is in output units per sample, not Hz, which is what makes this the
    index source for a buffer reader: a rate of 1 advances one frame per sample.
    """
    return Ugen("Phasor", [trig, rate, start, end, reset_pos])


# ---- filters ----------------------------------------------------------------
#
# One state-variable implementation stands behind every two-pole name; the row
# chooses the tap mix. Resonance travels on the wire as ``rq`` (the reciprocal
# of Q), which is scsynth's contract and the parameter with the clean domain:
# ``rq = 0`` is infinite Q, representable exactly, where ``Q = 0`` would divide
# by zero. Because ``rq`` is awkward to *think* in, each resonant builder also
# accepts ``q=``; a constant folds here at graph-build time and a signal
# composes one reciprocal, which is nothing next to the filter it feeds.


def _resonance(rq, q):
    """Resolves the mutually exclusive ``rq`` / ``q`` pair into a wire ``rq``."""
    if q is None:
        return 1.0 if rq is None else rq
    if rq is not None:
        raise TypeError("give either rq or q, not both")
    if isinstance(q, (int, float)):
        if q == 0:
            raise ValueError("q must be non-zero; use rq=0 for infinite Q")
        return 1.0 / q
    return _channel_unop(q, "recip")


def lpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth lowpass: -3 dB at ``freq``, -12 dB/octave."""
    return Ugen("LPF", [signal, freq])


def hpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth highpass: -3 dB at ``freq``, -12 dB/octave."""
    return Ugen("HPF", [signal, freq])


def rlpf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Resonant lowpass. Give the resonance as ``rq`` (1/Q, 0 = infinite) or
    as ``q``; unity gain at DC."""
    return Ugen("RLPF", [signal, freq, _resonance(rq, q)])


def rhpf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Resonant highpass; unity gain at Nyquist. Resonance as in `rlpf`."""
    return Ugen("RHPF", [signal, freq, _resonance(rq, q)])


def bpf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Bandpass with **unity gain at the centre**; ``rq`` is its bandwidth
    ratio. Resonance as in `rlpf`."""
    return Ugen("BPF", [signal, freq, _resonance(rq, q)])


def brf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Band reject (notch); unity gain in both passbands, a true null at
    ``freq``. Resonance as in `rlpf`."""
    return Ugen("BRF", [signal, freq, _resonance(rq, q)])


def resonz(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Resonator with unity gain at the peak.

    The same structure and parameterization as `bpf` — sclang ships two
    historically distinct two-pole resonators that promise the same thing, and
    here one implementation carries both names.
    """
    return Ugen("Resonz", [signal, freq, _resonance(rq, q)])


def svf(signal, freq=440.0, rq=None, low=0.0, band=0.0, high=0.0, *,
        q=None) -> Ugen:
    """The state-variable filter with its three tap gains as **signal inputs**,
    so the response itself can be modulated.

    Every classic response is a mix of the three taps, and each of these is a
    valid argument triple:

    | response | ``low`` | ``band`` | ``high`` |
    |---|---|---|---|
    | lowpass | 1 | 0 | 0 |
    | bandpass (peak gain Q) | 0 | 1 | 0 |
    | bandpass (unity peak) | 0 | ``rq`` | 0 |
    | highpass | 0 | 0 | 1 |
    | notch | 1 | 0 | 1 |
    | peak | -1 | 0 | 1 |
    | allpass | 1 | ``-rq`` | 1 |

    Sweeping between them costs the mix and nothing else: the three taps come
    out of the same pair of integrator updates. See `svf_morph` for the
    one-knob version.
    """
    return Ugen("Svf", [signal, freq, _resonance(rq, q), low, band, high])


def svf_morph(pos):
    """The ``(low, band, high)`` gains for a continuous lowpass → bandpass →
    highpass sweep, to splat into `svf`: ``svf(sig, freq, rq, *svf_morph(p))``.

    ``pos`` runs 0 → 1 → 2 and may be a signal, so the response becomes an
    automation lane like any other. The ordering lives here rather than on the
    wire, where committing to one arbitrary sequence of responses would exclude
    every other (notch, peak, allpass are all reachable through `svf` itself).
    """
    def clamp01(x):
        return _channel_binop(_channel_binop(x, "max", 0.0), "min", 1.0)

    low = clamp01(_channel_binop(1.0, "sub", pos))
    high = clamp01(_channel_binop(pos, "sub", 1.0))
    # A triangle peaking at pos == 1: 1 - |pos - 1|.
    band = clamp01(
        _channel_binop(1.0, "sub", _channel_unop(_channel_binop(pos, "sub", 1.0), "abs"))
    )
    return low, band, high


def one_pole(signal, coef=0.5) -> Ugen:
    """``y[n] = (1-|coef|)·x[n] + coef·y[n-1]`` — lowpass for a positive
    coefficient, highpass for a negative one, unity in the passband.

    The parameter is the **pole**, not a cutoff, as in sclang. Use `lag` when
    what you want is a time constant.
    """
    return Ugen("OnePole", [signal, coef])


def one_zero(signal, coef=0.5) -> Ugen:
    """``y[n] = (1-|coef|)·x[n] + coef·x[n-1]`` — the zero-only sibling of
    `one_pole`."""
    return Ugen("OneZero", [signal, coef])


def leak_dc(signal, coef=0.995) -> Ugen:
    """Removes DC: a zero exactly at 0 Hz with a pole just inside it. The
    default corner is low enough to leave audio untouched."""
    return Ugen("LeakDC", [signal, coef])


def integrator(signal, coef=0.999) -> Ugen:
    """Leaky accumulator, ``y[n] = x[n] + coef·y[n-1]``. The coefficient is
    clamped just inside 1 on the server, so it always forgets eventually
    instead of running away on a DC input."""
    return Ugen("Integrator", [signal, coef])


# ---- delays -----------------------------------------------------------------
#
# One line implementation behind nine names, chosen by interpolation (``N``
# none, ``L`` linear, ``C`` cubic) and by feedback (none, comb, allpass).
#
# ``max_delay`` is **static**: it sizes the line the server allocates when the
# synth is built, so it cannot grow later and a `delaytime` past it is clamped.
# Left unset it follows a constant ``delaytime``, which is what you want for a
# fixed delay; a *modulated* delaytime has to state the longest it will reach.


def _line(kind, delaytime, max_delay):
    """The static ``max_delay`` field, defaulted from a constant delay time."""
    if max_delay is None:
        if not isinstance(delaytime, (int, float)):
            raise TypeError(
                f"{kind}: a modulated delaytime needs an explicit max_delay "
                "(it sizes the line, and the line is allocated once)"
            )
        max_delay = delaytime
    return {"max_delay": float(max_delay)}


def delay_n(signal, delaytime=0.2, *, max_delay=None) -> Ugen:
    """Pure delay, no interpolation: the delay is rounded to whole samples."""
    return Ugen("DelayN", [signal, delaytime],
                static=_line("DelayN", delaytime, max_delay))


def delay_l(signal, delaytime=0.2, *, max_delay=None) -> Ugen:
    """Pure delay with linear interpolation — a fractional delay, at the cost of
    a gentle lowpass that deepens toward Nyquist (about -1.6 dB at 9 kHz on a
    half-sample delay)."""
    return Ugen("DelayL", [signal, delaytime],
                static=_line("DelayL", delaytime, max_delay))


def delay_c(signal, delaytime=0.2, *, max_delay=None) -> Ugen:
    """Pure delay with four-point cubic interpolation — about -0.36 dB at 9 kHz
    where `delay_l` loses 1.6 dB. The one to modulate."""
    return Ugen("DelayC", [signal, delaytime],
                static=_line("DelayC", delaytime, max_delay))


def comb_n(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Feedback comb, no interpolation. ``decaytime`` is the time for the echo
    train to fall 60 dB **counting from the first echo**, which is the direct
    path and always comes back at full level. A negative decay time inverts
    alternate echoes; zero leaves a single echo."""
    return Ugen("CombN", [signal, delaytime, decaytime],
                static=_line("CombN", delaytime, max_delay))


def comb_l(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Feedback comb with linear interpolation. Decay as in `comb_n`."""
    return Ugen("CombL", [signal, delaytime, decaytime],
                static=_line("CombL", delaytime, max_delay))


def comb_c(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Feedback comb with cubic interpolation. Decay as in `comb_n`."""
    return Ugen("CombC", [signal, delaytime, decaytime],
                static=_line("CombC", delaytime, max_delay))


def allpass_n(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Schroeder allpass, no interpolation: the magnitude response is exactly
    flat and only the phase is shaped, which is what makes it the diffusion
    stage of a reverb. Decay as in `comb_n`."""
    return Ugen("AllpassN", [signal, delaytime, decaytime],
                static=_line("AllpassN", delaytime, max_delay))


def allpass_l(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Schroeder allpass with linear interpolation."""
    return Ugen("AllpassL", [signal, delaytime, decaytime],
                static=_line("AllpassL", delaytime, max_delay))


def allpass_c(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Schroeder allpass with cubic interpolation — the one to modulate."""
    return Ugen("AllpassC", [signal, delaytime, decaytime],
                static=_line("AllpassC", delaytime, max_delay))


def in_(bus=0.0) -> Ugen:
    """Reads an audio bus (sampled per block)."""
    return Ugen("In", [bus])


def in_ctl(bus=0.0) -> Ugen:
    """Reads a control-bus value, constant over the block."""
    return Ugen("InCtl", [bus])


def _out_channels(kind, bus, signal):
    """One writer per channel on consecutive buses (``bus``, ``bus+1``, …) —
    the point where a channel list becomes buses. The base ``bus`` must be a
    number: a signal bus cannot be offset per channel client-side."""
    if isinstance(bus, bool) or not isinstance(bus, (int, float)):
        raise TypeError(
            f"a multichannel {kind} needs a constant bus to lay channels on "
            f"consecutive buses, got {bus!r}"
        )
    sig = ChannelList(signal)
    return ChannelList(
        [Ugen(kind, [float(bus) + i, s]) for i, s in enumerate(sig.items)]
    )


def out_ctl(bus, signal) -> "Ugen | ChannelList":
    """Writes ``signal``'s latest per-block value to a **control** ``bus`` — the
    write side of `in_ctl`, so a node reading that bus (via ``/n_map`` or
    `in_ctl`) tracks it. Passes ``signal`` through as its output. A channel
    list writes its channels to consecutive buses."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("OutCtl", bus, signal)
    return Ugen("OutCtl", [bus, signal])


def out(bus, signal) -> "Ugen | ChannelList":
    """Sums ``signal`` into the audio ``bus`` (output happens only here). A
    channel list writes its channels to consecutive buses: ``out(0,
    dup(sig))`` is a stereo output."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("Out", bus, signal)
    return Ugen("Out", [bus, signal])


def replace_out(bus, signal) -> "Ugen | ChannelList":
    """Overwrites the audio ``bus`` with ``signal`` instead of summing. A
    channel list overwrites consecutive buses."""
    if isinstance(signal, (ChannelList, list, tuple)):
        return _out_channels("ReplaceOut", bus, signal)
    return Ugen("ReplaceOut", [bus, signal])


# ---- side-effect UGens: reply / observe, no `out` required ----
# These emit OSC replies or console posts on a trigger instead of audio. A
# SynthDef may contain only these and no `out(...)` at all. Pass them as roots
# of the `SynthDef` (they have no consumer to reach them otherwise). A trigger
# fires on a crossing from ``<= 0`` up to ``> 0``.


def send_trig(trig, id=0, value=0.0) -> Ugen:
    """On each trigger of ``trig``, sends ``/tr nodeID id value`` to ``/notify``
    clients. Output is silence; pass it as a `SynthDef` root."""
    return Ugen("SendTrig", [trig, id, value])


def send_reply(trig, *values, cmd="/reply", reply_id=-1) -> Ugen:
    """On each trigger of ``trig``, sends the OSC message ``cmd nodeID reply_id
    value…`` to ``/notify`` clients (``cmd`` defaults to ``/reply``). ``values``
    is the arbitrary-arity payload. Output is silence; pass it as a `SynthDef`
    root."""
    return Ugen("SendReply", [trig, reply_id, *values], label=cmd)


def poll(trig, signal, label="poll", trig_id=-1) -> Ugen:
    """On each trigger of ``trig``, posts ``label: value`` (the ``signal``
    value) to the server console and, when ``trig_id >= 0``, also sends ``/tr
    nodeID trig_id value``. ``signal`` passes through the output, so ``poll``
    can sit mid-chain."""
    return Ugen("Poll", [trig, signal, trig_id], label=label)


# ---- frequency-domain chain: FFT / PV_* / IFFT (S8) ----
# `fft` opens a spectral chain, the `pv_*` filters transform the frame in place,
# and `ifft` resynthesises audio. Wire them in order (fft -> pv_* -> ... -> ifft).
# The frame is synth-private scratch (no buffer to allocate); only `fft` names
# the window size, and the server propagates it down the chain.


def fft(source, active=1.0, *, fft_size=1024, hop=0.5, wintype=0) -> Ugen:
    """Opens a spectral chain: windows ``source`` (an audio signal) and
    transforms it to a spectral frame once per **hop**. ``active > 0`` runs the
    transform, ``<= 0`` holds. ``fft_size`` is the window size (a power of two:
    256/512/1024/2048/4096), ``hop`` the fraction of the window between frames,
    ``wintype`` the window (a `clausters._native.Window`: 0 Hann, 1 sine, …).
    These size the transform, so they are static fields given **only here** — the
    server propagates them to the rest of the chain. The window is also settable
    live with `Server.u_cmd`. Feed the result to a ``pv_*`` filter or `ifft`."""
    return Ugen(
        "FFT", [source, active],
        static={"fft_size": int(fft_size), "hop": float(hop), "wintype": int(wintype)},
    )


def ifft(chain) -> Ugen:
    """Closes a spectral chain: inverse-transforms each fresh frame and
    overlap-adds it back to audio (window-normalized, so a bare `fft`->`ifft`
    reconstructs at unity gain, delayed by one window). ``chain`` is the output
    of an `fft` or a ``pv_*`` filter."""
    return Ugen("IFFT", [chain])


def pv_mag_above(chain, threshold) -> Ugen:
    """Passes only the bins whose magnitude is **above** ``threshold``, zeroing
    the rest. ``chain`` comes from `fft` or another ``pv_*``."""
    return Ugen("PV_MagAbove", [chain, threshold])


def pv_mag_below(chain, threshold) -> Ugen:
    """Passes only the bins whose magnitude is **below** ``threshold``."""
    return Ugen("PV_MagBelow", [chain, threshold])


def pv_brick_wall(chain, wipe) -> Ugen:
    """Brick-wall band limit: ``wipe > 0`` zeroes the top fraction of bins (a low
    pass), ``wipe < 0`` the bottom (a high pass), ``0`` passes everything
    (``wipe`` in -1..1)."""
    return Ugen("PV_BrickWall", [chain, wipe])


def pv_mag_clip(chain, threshold) -> Ugen:
    """Limits each bin's magnitude **to** ``threshold``: louder bins are scaled
    down to it (phases kept), quieter bins pass untouched."""
    return Ugen("PV_MagClip", [chain, threshold])


def pv_add(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex sum. Both inputs must be spectral
    chains of the same ``fft_size`` (and distinct); the result lands in chain A,
    which the combiner's output carries onward."""
    return Ugen("PV_Add", [chain_a, chain_b])


def pv_mul(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex product (spectral ring modulation)."""
    return Ugen("PV_Mul", [chain_a, chain_b])


def pv_min(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per bin, whichever input has the **smaller**
    magnitude."""
    return Ugen("PV_Min", [chain_a, chain_b])


def pv_max(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per bin, whichever input has the **larger**
    magnitude."""
    return Ugen("PV_Max", [chain_a, chain_b])


def pv_mag_mul(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: A's bins scaled by B's magnitudes — A's phases kept
    (a spectral envelope transfer, the classic "vocoder" cross-synthesis)."""
    return Ugen("PV_MagMul", [chain_a, chain_b])


def pv_copy_phase(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: A's magnitudes with B's phases (the complementary
    cross-synthesis to `pv_mag_mul`)."""
    return Ugen("PV_CopyPhase", [chain_a, chain_b])


def pv_mag_freeze(chain, freeze=0.0) -> Ugen:
    """While ``freeze <= 0`` stores each frame's magnitudes and passes through;
    while ``> 0`` rescales every bin to the stored magnitudes — the spectral
    envelope holds while the phases keep running."""
    return Ugen("PV_MagFreeze", [chain, freeze])


def pv_mag_smear(chain, bins=0.0) -> Ugen:
    """Averages each bin's magnitude over ``bins`` neighbors on each side
    (``0`` is transparent), phases untouched — a spectral blur."""
    return Ugen("PV_MagSmear", [chain, bins])


def pv_bin_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """Remaps bin ``b`` to ``round(b * stretch + shift)``: colliding bins sum,
    out-of-range bins are dropped. ``stretch=1, shift=0`` is transparent; a
    positive ``shift`` moves every partial up by ``shift`` bin widths."""
    return Ugen("PV_BinShift", [chain, stretch, shift])


def pv_mag_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """The `pv_bin_shift` remap applied to the magnitude envelope only, laid
    over the frame's original phases."""
    return Ugen("PV_MagShift", [chain, stretch, shift])


def pv_kernel(chain, mag=None, phase=None, params=()) -> Ugen:
    """The general per-frame mechanism: applies user-written **bin
    expressions** to every bin of each fresh frame. ``mag`` and ``phase`` are
    symbolic per-bin expressions built from `clausters.defs.pv_expr`'s terms
    (its ``mag``/``phase``/``bin_index``/``nbins``/``binfreq``/``param``)
    with ordinary Python operators; each maps one bin's values to that bin's
    new magnitude / phase. An omitted expression is the identity — and an
    identity ``phase`` keeps each bin's phase *exactly* (the cheap path: pure
    magnitude maps skip the polar conversion).

    ``params`` are extra signal inputs (controls, LFOs, constants) the
    expressions read as ``param(0)``, ``param(1)``, … — sampled once per hop.

    An expression is a **pure per-bin map**: no state across bins or frames,
    no reading other bins. Gates, tilts, masks and magnitude algebra belong
    here; freeze/smear (cross-frame state) and shift (bin remaps) stay with
    the dedicated ``pv_*`` filters. The server validates the program at
    ``/d_recv`` (stack discipline, parameter arity, unknown words) and
    rejects a bad def with ``/fail``.

    Note that ``mag`` is a raw transform magnitude — it scales with the input
    level, the window and the ``fft_size``, it is **not** normalized to 0..1 —
    so thresholds and constants must be calibrated to the material (probe a
    render, or ``poll`` a reference).

    ```python
    from clausters.defs.pv_expr import mag, bin_index, nbins, param
    # A tilted spectral gate: the threshold rises with frequency.
    chain = pv_kernel(chain,
                      mag=mag * (mag >= param(0) * (1 + 4 * bin_index / nbins)),
                      params=[control("thresh", 2.0)])
    ```"""
    from .pv_expr import pv_tokens
    static = {}
    if mag is not None:
        static["mag_expr"] = pv_tokens(mag)
    if phase is not None:
        static["phase_expr"] = pv_tokens(phase)
    return Ugen("PV_Kernel", [chain, *params], static=static or None)


def conv(source, kernel, *, fft_size=1024, partitions=16) -> Ugen:
    """Partitioned convolution: convolves ``source`` with a **prepared**
    kernel — a buffer written by ``server.gen_buffer(dest, "prepare_partconv",
    fft_size, ir_bufnum)`` (size ``dest`` with `partconv_frames`). The IR's
    spectra are computed once, off the audio thread; the UGen's steady per-
    block cost is flat (the partition products are spread across the hop).

    ``fft_size`` is the transform size (a supported power of two); the
    partition length — and the intrinsic latency — is ``fft_size / 2``
    samples. ``partitions`` caps the kernel length this instance accepts
    (its pre-allocated state). Moving ``kernel`` to a *different* prepared
    buffer crossfades over one partition; regenerating the same buffer
    switches hard."""
    return Ugen(
        "Conv", [source, kernel],
        static={"fft_size": int(fft_size), "partitions": int(partitions)},
    )


def partconv_frames(ir_frames: int, fft_size: int = 1024) -> int:
    """Frames a kernel buffer needs to hold ``ir_frames`` of impulse response
    prepared at ``fft_size`` (partitions of ``fft_size / 2``, plus the two-
    sample header) — the size to `Server.alloc_buffer` before
    ``gen_buffer(..., "prepare_partconv", fft_size, ir_bufnum)``."""
    part = fft_size // 2
    parts = -(-int(ir_frames) // part)
    return 2 + parts * int(fft_size)


def play_buf(bufnum, chan=0.0, rate=1.0, loop=0.0) -> Ugen:
    """Mono buffer player with linear interpolation; ``rate`` is frames per
    output sample (1.0 = server rate)."""
    return Ugen("PlayBuf", [bufnum, chan, rate, loop])


def buf_rd(bufnum, chan, phase, loop=0.0) -> Ugen:
    """Reads a buffer at a ``phase`` signal in frames (linear interpolation)."""
    return Ugen("BufRd", [bufnum, chan, phase, loop])


# ---- table oscillators & waveshaper (read `/b_gen` tables) ----


def osc(bufnum, freq=440.0, phase=0.0) -> Ugen:
    """Interpolating wavetable oscillator. ``bufnum`` must hold a
    **wavetable-format** buffer (fill it with ``Server.gen_buffer`` and a
    ``/b_gen`` command whose wavetable flag is set); ``phase`` is an offset in
    radians."""
    return Ugen("Osc", [bufnum, freq, phase])


def oscn(bufnum, freq=440.0, phase=0.0) -> Ugen:
    """Non-interpolating oscillator over a **plain** (non-wavetable) buffer;
    rawer and cheaper than `osc`."""
    return Ugen("OscN", [bufnum, freq, phase])


def vosc(bufpos, freq=440.0, phase=0.0) -> Ugen:
    """Like `osc` but the buffer number is a signal: reads wavetables
    ``bufpos`` and ``bufpos + 1`` and crossfades by the fractional part, so
    sweeping ``bufpos`` morphs a bank of adjacent tables (allocate them
    contiguously, all the same size)."""
    return Ugen("VOsc", [bufpos, freq, phase])


def shaper(bufnum, signal) -> Ugen:
    """Waveshaper: maps ``signal`` (in +-1, clamped) through a transfer table
    in wavetable format (typically a ``cheby`` `/b_gen`); the table's first
    point is ``signal = -1``, its last ``signal = +1``."""
    return Ugen("Shaper", [bufnum, signal])


# ---- streaming disk I/O (self-contained: one I/O thread + ring each) ----


def disk_in(path, chan=0.0, loop=False) -> Ugen:
    """Streams a file from disk, one file frame per server sample (no
    resampling — pitch follows the sample-rate ratio). Mono per UGen: ``chan``
    picks the channel, a stereo file is two `disk_in`\\ s. ``loop`` restarts at
    the end of the stream. For a handful of streams, not per-voice (each spawns
    its own I/O thread)."""
    return Ugen("DiskIn", [chan], static={"path": str(path), "loop": bool(loop)})


def disk_out(path, signal, format="int16") -> Ugen:
    """Streams ``signal`` to a mono WAV at ``path`` (``format`` is ``"int16"``,
    ``"int24"`` or ``"float"``) and passes ``signal`` through as its output.
    Record stereo with two `disk_out`\\ s."""
    return Ugen("DiskOut", [signal], static={"path": str(path), "format": str(format)})


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


# ---- fused arithmetic (the forms the server optimizes) ----


def madd(a, b, c) -> Ugen:
    """``a*b + c`` in one UGen (the multiply-accumulate the server fuses). The
    plain expression ``a * b + c`` builds the same value with two op UGens; this
    is the fused equivalent."""
    return Ugen("MulAdd", [a, b, c])


def sum3(a, b, c) -> Ugen:
    """``a + b + c`` in one UGen."""
    return Ugen("Sum3", [a, b, c])


def sum4(a, b, c, d) -> Ugen:
    """``a + b + c + d`` in one UGen."""
    return Ugen("Sum4", [a, b, c, d])


# ---- one-pole smoothers ----


def lag(signal, time=0.1) -> Ugen:
    """One-pole smoother: ``signal`` lagged over ``time`` seconds (symmetric);
    ``time`` 0 passes through. The same UGen the server inserts for a lagged
    control -- use it directly to smooth any signal."""
    return Ugen("Lag", [signal, time])


def var_lag(signal, up=0.1, down=0.1) -> Ugen:
    """One-pole smoother with separate rise (``up``) and fall (``down``) times."""
    return Ugen("VarLag", [signal, up, down])


# ---- scalar / init-rate (ir) ----


def sample_rate() -> Ugen:
    """The engine sample rate in Hz, computed once at init (``ir``)."""
    return Ugen("SampleRate", [], rate="ir")


def buf_frames(bufnum) -> Ugen:
    """The number of frames in a buffer, block-constant (``kr``)."""
    return Ugen("BufFrames", [bufnum], rate="kr")


def buf_sample_rate(bufnum) -> Ugen:
    """The buffer's own sample rate (Hz), block-constant (``kr``)."""
    return Ugen("BufSampleRate", [bufnum], rate="kr")


def buf_rate_scale(bufnum) -> Ugen:
    """``file_sr / server_sr``, block-constant (``kr``); feed `play_buf`'s
    ``rate`` (``buf_rate_scale(buf) * pitch``) to play at the file's true pitch
    without the client knowing either rate."""
    return Ugen("BufRateScale", [bufnum], rate="kr")


def buf_channels(bufnum) -> Ugen:
    """The buffer's channel count, block-constant (``kr``)."""
    return Ugen("BufChannels", [bufnum], rate="kr")


def buf_dur(bufnum) -> Ugen:
    """The buffer's duration in seconds (``frames / file_sr``), block-constant
    (``kr``)."""
    return Ugen("BufDur", [bufnum], rate="kr")


def rand(lo=0.0, hi=1.0) -> Ugen:
    """One uniform random value in ``[lo, hi)``, drawn once at synth init and
    held for the node's life (``ir``); ``lo``/``hi`` must be constants or ``ir``."""
    return Ugen("Rand", [lo, hi], rate="ir")


# ---- demand rate (dr) ----


def dseq(values, repeats=0.0) -> Ugen:
    """A demand-rate sequence source: yields ``values`` in order, ``repeats``
    times (``0`` loops forever), then signals end-of-stream. Only valid as a
    `demand` source."""
    return Ugen("Dseq", [repeats, *values], rate="dr")


def demand(trig, reset, source) -> Ugen:
    """Demand driver: pulls the next value from a demand ``source`` (a `dseq`)
    on each rising edge of ``trig`` and holds it between triggers; a rising
    ``reset`` restarts the source."""
    return Ugen("Demand", [trig, reset, source])


# ---- envelopes (EnvGen) ----


class DoneAction:
    """The action `env_gen` takes when its envelope finishes — scsynth's full
    done-action set (0-15). Pass one as ``done_action``. The relative actions
    (3-13, 15) act on the synth's neighbours in its group; a paused node is
    resumed with `Server.run` (``/n_run``)."""

    #: Do nothing; the envelope just holds its final level.
    NONE = 0
    #: Pause the synth (stops processing; it stays in the tree). Resume with
    #: `Server.run`.
    PAUSE_SELF = 1
    #: Free the synth — the usual choice for a one-shot or a released note.
    FREE_SELF = 2
    #: Free the synth and the preceding node.
    FREE_SELF_AND_PREV = 3
    #: Free the synth and the following node.
    FREE_SELF_AND_NEXT = 4
    #: Free the synth; if the preceding node is a group, free all its children.
    FREE_SELF_AND_FREE_ALL_IN_PREV = 5
    #: Free the synth; if the following node is a group, free all its children.
    FREE_SELF_AND_FREE_ALL_IN_NEXT = 6
    #: Free the synth and every preceding node in its group.
    FREE_SELF_TO_HEAD = 7
    #: Free the synth and every following node in its group.
    FREE_SELF_TO_TAIL = 8
    #: Free the synth and pause the preceding node.
    FREE_SELF_PAUSE_PREV = 9
    #: Free the synth and pause the following node.
    FREE_SELF_PAUSE_NEXT = 10
    #: Free the synth; if the preceding node is a group, deep-free it.
    FREE_SELF_AND_DEEP_FREE_PREV = 11
    #: Free the synth; if the following node is a group, deep-free it.
    FREE_SELF_AND_DEEP_FREE_NEXT = 12
    #: Free the synth and every other node in its group.
    FREE_ALL_IN_GROUP = 13
    #: Free the synth's whole enclosing group.
    FREE_GROUP = 14
    #: Free the synth and resume (unpause) the following node.
    FREE_SELF_RESUME_NEXT = 15


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

    @classmethod
    def step(cls, levels, times, release_node=None, loop_node=None):
        """A step sequence: **each value held for its duration** — `levels`
        and `times` have the *same* length, unlike the raw constructor
        (``Env.step([0, 1], [0.5, 0.5])`` holds 0 for 0.5, then 1 for 0.5).

        This is the conceptual interface of a value-with-duration sequence;
        like SuperCollider's ``Env.step``, it is expressed over the raw
        initial-level + (target, duration) form by prepending the first level
        with the ``"step"`` shape (which jumps to each segment's target at its
        start)."""
        levels = list(levels)
        times = list(times)
        if len(levels) != len(times):
            raise ValueError(
                f"Env.step: levels ({len(levels)}) and times ({len(times)}) "
                "must have the same length"
            )
        if not levels:
            raise ValueError("Env.step needs at least one level")
        return cls([levels[0]] + levels, times, "step",
                   release_node=release_node, loop_node=loop_node)

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


# ---- break-point <-> Env mapping (shared by the bpf widget and automation) ----


def env_to_points(env, *, time_at: float = 0.0) -> list:
    """An `Env` (levels / segment times / curves) as the flat ``bpf`` breakpoint
    list ``[t, v, shape, curve, ...]``, with absolute times starting at
    ``time_at``. The last point carries a linear placeholder (no segment leaves
    it). Feed the result to the ``bpf`` widget or to a live ``points`` set."""
    out: list = []
    t = float(time_at)
    for i, level in enumerate(env.levels):
        if i < len(env.times):
            shape, curve = _resolve_curve(env.curves[i])
        else:
            shape, curve = 1, 0.0
        out += [t, float(level), int(shape), float(curve)]
        if i < len(env.times):
            t += float(env.times[i])
    return out


def points_to_env(points, *, time_at: float = 0.0, **env_kwargs):
    """A ``bpf`` breakpoint list — the flat ``t v shape curve ...`` quads a
    ``"points"`` event carries — as an `Env`: absolute times become segment
    durations and each segment keeps its shape (the numeric curvature for the
    custom shape, the shape name otherwise).

    A first breakpoint later than ``time_at`` (default ``0.0``) is a drawn
    initial delay, encoded as a leading ``hold`` segment (the first level held
    for that duration) so what was drawn and what plays stay identical. Extra
    keywords (``release_node``, ``loop_node``) pass through to `Env`."""
    quads = [points[i:i + 4] for i in range(0, len(points) - len(points) % 4, 4)]
    if len(quads) < 2:
        raise ValueError("an envelope needs at least two breakpoints")
    # First name wins for aliased numbers ("step"/"lin"/"exp"... are listed
    # before their long forms).
    names: dict = {}
    for name, num in _SHAPE_NUMBERS.items():
        names.setdefault(num, name)
    levels = [float(q[1]) for q in quads]
    times = [float(b[0]) - float(a[0]) for a, b in zip(quads, quads[1:])]
    curve = [float(q[3]) if int(q[2]) == 5 else names.get(int(q[2]), "lin")
             for q in quads[:-1]]
    delay = float(quads[0][0]) - float(time_at)
    if delay > 1e-9:
        levels.insert(0, levels[0])
        times.insert(0, delay)
        curve.insert(0, "hold")
    return Env(levels, times, curve, **env_kwargs)


# ---- introspecting a UGen kind's input names (the client's own signature) ----
#
# The level-2 Def-view labels a UGen box's inlets from the client's own
# vocabulary: the parameter names of the callable that builds the kind. That
# callable *is* the client's mirror of the server registry (the /u_query
# contrast test keeps the two in line, see `tests/test_session.py`), so reusing
# it here means the patcher and the builder never disagree on an input's name.

#: Kinds whose builder's positional parameters do **not** line up with the wire
#: input order (variadic runs, static fields sitting between inputs) — the
#: divergences the /u_query contrast test declares. For these the names would
#: mislabel the inlets, so the Def-view falls back to positional labels.
_INPUT_NAMES_MISALIGNED = frozenset(
    {"EnvGen", "SendReply", "Dseq", "Poll", "DiskIn", "DiskOut", "PV_Kernel"}
)

#: Lazily built {kind: [param name, ...]} — see `ugen_input_names`.
_INPUT_NAMES: "dict[str, list[str]] | None" = None


def _build_input_names() -> dict:
    """Map each server UGen kind to its builder callable's positional parameter
    names, read from the ``Ugen("Kind", ...)`` literal in this module's source
    (the function name does not equal the kind: ``in_`` builds ``In``,
    ``oscn`` builds ``OscN``)."""
    import ast
    import inspect

    names: dict[str, list[str]] = {}
    for fname, fn in list(globals().items()):
        if fname.startswith("_") or not inspect.isfunction(fn):
            continue
        try:
            tree = ast.parse(inspect.getsource(fn).lstrip())
        except (OSError, SyntaxError):
            continue
        kind = None
        for node in ast.walk(tree):
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                    and node.func.id == "Ugen" and node.args
                    and isinstance(node.args[0], ast.Constant)):
                kind = node.args[0].value
                break
        if kind is None or kind in names:
            continue
        names[kind] = [
            p.name for p in inspect.signature(fn).parameters.values()
            if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)
        ]
    return names


def ugen_input_names(kind: str) -> "list[str] | None":
    """The positional input names of the callable that builds UGen ``kind``, or
    ``None`` when no single callable maps to it cleanly — the generic op UGens
    (``BinaryOpUGen``/``UnaryOpUGen``, built inline) and the kinds whose builder
    parameters do not line up with the wire order (`_INPUT_NAMES_MISALIGNED`).
    A ``None`` result means the caller labels the inlets positionally."""
    global _INPUT_NAMES
    if kind in _INPUT_NAMES_MISALIGNED:
        return None
    if _INPUT_NAMES is None:
        _INPUT_NAMES = _build_input_names()
    return _INPUT_NAMES.get(kind)
