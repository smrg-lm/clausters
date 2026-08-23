"""The graph itself: a UGen node, a control, a channel list.

The types every other module in this package builds on — `Ugen` (one node, one
output), `Control` (a def's parameter) and `ChannelList` (multichannel as an
explicit container, never implicit expansion) — plus the fused arithmetic the
server has dedicated kinds for.
"""

from ...base import builtins as _builtins
from ...base.absobject import AbstractObject
from ..expr import SynthExpr


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


class _Node(SynthExpr):
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
    """A named control (a ``/synth_new``/``/node_set`` parameter) with a default and an
    optional **type** and **lag** (S2), mirroring the server's control types:

    - ``rate="tr"`` — a **trigger**: a ``/node_set`` holds for one block, then the
      server resets it to 0 (drives an `env_gen` gate, a sample-and-hold).
    - ``rate="ir"`` — a **scalar**: read once at init and frozen; a later
      ``/node_set`` is ignored. As ``ir`` it may feed an ``ir`` input (`rand`,
      buffer-info UGens).
    - ``lag`` (seconds) — smooth a ``kr`` control's changes with an implicit
      one-pole (a `lag`/`var_lag` UGen the server inserts); ``lag_down`` gives a
      separate downward time.

    Used as a UGen input it serializes to a ``{"control": index}`` reference;
    the `SynthDef` gathers the controls a graph references, in first-seen
    order."""

    def __init__(self, name, default=0.0, rate=None, lag=None, lag_down=None,
                 min=None, max=None, step=None):
        self.name = str(name)
        self.default = float(default)
        self.rate = None if rate is None else str(rate)
        self.lag = None if lag is None else float(lag)
        self.lag_down = None if lag_down is None else float(lag_down)
        #: The range this control is meant to be driven over — what a GUI
        #: control needs to draw it, and the one thing about a control only its
        #: author knows. It rides no wire: the server takes any float, so this
        #: is a statement about the surface, not a constraint (see
        #: `clausters.defs.info.ControlInfo`, which a FaustDef fills from its
        #: own ``hslider`` declaration).
        self.min = None if min is None else float(min)
        self.max = None if max is None else float(max)
        self.step = None if step is None else float(step)
        if (self.min is None) != (self.max is None):
            raise ValueError(
                f"control {self.name!r}: a range is min *and* max, and it is "
                "either declared or not")
        if self.rate is not None and self.rate not in _CONTROL_RATES:
            raise ValueError(
                f"unknown control type {self.rate!r}; use one of "
                f"{sorted(_CONTROL_RATES)}"
            )
        if self.lag_down is not None and self.lag is None:
            raise ValueError("lag_down requires lag (the up time)")

    def _signature(self):
        """The full identity used to detect conflicting reuses of a name."""
        return (self.default, self.rate, self.lag, self.lag_down,
                self.min, self.max, self.step)

    def __repr__(self):
        return f"Control({self.name!r}, {self.default!r})"


def control(name, default=0.0, rate=None, lag=None, lag_down=None,
            min=None, max=None, step=None) -> Control:
    """A named control (``/synth_new``/``/node_set`` parameter). ``rate`` is its type
    (``"tr"`` trigger, ``"ir"`` scalar, or the default ``kr``); ``lag`` (with an
    optional ``lag_down``) smooths a ``kr`` control.

    ``min``/``max`` (with an optional ``step``) declare the **range it is meant
    to be driven over** — the one thing about a control only the person writing
    the graph knows, and what a GUI control reads instead of being handed the
    same two numbers a second time::

        freq = control("freq", 220.0, min=110.0, max=880.0)
        sd = SynthDef("voice", out(0.0, sine(freq=freq)))
        knob(freq)                      # name, value and range, all from here

    It rides no wire: the server takes any float for any control, so a range is
    a statement about the surface rather than a constraint. See `Control`.
    """
    return Control(name, default, rate=rate, lag=lag, lag_down=lag_down,
                   min=min, max=max, step=step)


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


class ChannelList(SynthExpr):
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
