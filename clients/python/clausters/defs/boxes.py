"""Faust Box API as composable, lowercase callables.

The box counterpart of `clausters.defs.signals`, and a complete def-building
API in its own right: each function returns a `Box` and composing boxes
builds the JSON **box tree** the server's ``/d_faust`` consumes (see the
server's ``faust::boxes`` for the schema). Boxes are Faust's point-free
algebra — ``seq``/``par``/``split``/``merge``/``rec`` compose whole
**processors** by their input/output arities. Where ``signals`` describes
one output at a time referentially (``input(n)``), boxes describe
multi-channel blocks that plug into each other — the natural shape for
routing, chains, and anything conceived as units with inputs and outputs.

On top of the algebra, `faust` compiles any Faust **expression** into a
`Box` that composes like a primitive. That addition puts the whole Faust
library ecosystem (``os.osc``, ``fi.lowpass``, ``re.``, ``pm.``, ...) inside
the same algebra without transcribing anything: library functions become
boxes among boxes.

Choosing a form: a fixed processing chain written top to bottom often reads
best as plain Faust (``FaustDef.from_source``); graphs assembled one output
at a time from arithmetic and feedback suit `clausters.defs.signals`.
Regular banks ("N copies with index-dependent parameters") are best written
in Faust itself — ``par(i, N, ...)``, widget labels with ``%i``, ``ba.take``
— and parametrized from Python by splicing ``N`` and lists through `faust`'s
eval arguments. Boxes shine when the graph is conceived as composed
processors, when its structure is decided by Python data, and whenever
library DSP has to mix with Python-built pieces.

Two stages of application, kept separate on purpose:

- ``faust("fi.lowpass", 3)`` — arguments to `faust` are **evaluation-stage**:
  spliced into the Faust source text (``fi.lowpass(3)``), where structural
  parameters like a filter order must live.
- ``fi_lp(cutoff, wire())`` — arguments to a `Box` **call** are
  **composition-stage**: boxes wired as the box's signal inputs, sugar for
  ``seq(par(cutoff, wire()), fi_lp)``.

The wire rule (the big difference from ``signals``): **each `wire` is a
distinct input**. There is no referential ``input(n)`` here — two wires in
two positions are two input channels. Reusing the *same* ``wire()`` (or
``cut()``) object in more than one position is almost always a mistake, and
`FaustDef.from_box` rejects it; route explicitly with `split`, or write that
stretch inside a `faust` fragment (``_ <: ...``). Every *other* box value can
be reused freely: a repeated subexpression is computed once (the server
shares identical subtrees).

Reserved controls ``in`` and ``out`` (set with ``/s_new ... "in" b "out" b``)
choose the input/output buses; they are added by the server, not declared
here.
"""

from ..base.absobject import AbstractObject

# AbstractObject selector -> box schema op name. The box schema has no
# lsh/rsh/rem; Python `%` maps to Faust's `fmod`.
_BINARY = {
    "add": "add", "sub": "sub", "mul": "mul", "div": "div", "mod": "fmod",
    "pow": "pow", "min": "min", "max": "max", "atan2": "atan2",
    "gt": "gt", "lt": "lt", "ge": "ge", "le": "le", "eq": "eq", "ne": "ne",
    "bitand": "and", "bitor": "or", "bitxor": "xor",
}
_UNARY = {
    "abs": "abs", "floor": "floor", "ceil": "ceil", "sin": "sin", "cos": "cos",
    "tan": "tan", "asin": "asin", "acos": "acos", "atan": "atan", "exp": "exp",
    "log": "log", "log10": "log10", "sqrt": "sqrt", "as_int": "intcast",
    "as_float": "floatcast",
}


def _sum_arity(values):
    """Sums arities, propagating unknown (`None` absorbs)."""
    total = 0
    for v in values:
        if v is None:
            return None
        total += v
    return total


class Box(AbstractObject):
    """One node of a Faust box expression. Wrap a number to make a constant;
    compose with operators, the module functions, or by calling the box.

    `num_inputs`/`num_outputs` are the box's signal arity as computed on the
    client from the composition rules; ``None`` when unknown (a `faust`
    fragment without ``ins=``/``outs=``). The server does not read them — a
    real mismatch is reported by Faust itself when the def compiles.
    """

    def __init__(self, node, num_inputs, num_outputs):
        # `node` is a JSON-able dict, or a bare number (a constant).
        self.node = node
        self.num_inputs = num_inputs
        self.num_outputs = num_outputs

    def to_json(self):
        return self.node

    # --- application sugar ---

    def __call__(self, *args) -> "Box":
        """Applies boxes to this box's inputs: ``f(a, b)`` is
        ``seq(par(a, b), f)`` (with one argument, ``seq(a, f)``) — Faust's
        partial-application style written as a call. The arguments must cover
        *all* the box's inputs; use `wire` for the ones left open."""
        if not args:
            raise TypeError("a box call needs at least one argument box")
        applied = par(*args) if len(args) > 1 else box(args[0])
        return seq(applied, self)

    def __getitem__(self, index) -> "Box":
        """Selects one output channel: ``st[0]`` is ``seq(st, par(wire, cut,
        ...))``. Needs a known `num_outputs` (pass ``outs=`` to `faust` for
        fragments). The selected fragment is shared, not recomputed, when
        several channels of the same box value are used."""
        n = self.num_outputs
        if n is None:
            raise ValueError(
                "cannot select an output: this box's arity is unknown "
                "(pass outs=... to faust())"
            )
        if not isinstance(index, int):
            raise TypeError(f"box output index must be an int, not {index!r}")
        if index < 0:
            index += n
        if not 0 <= index < n:
            raise IndexError(f"output {index} out of range for a {n}-output box")
        if n == 1:
            return self
        taps = [wire() if k == index else cut() for k in range(n)]
        return seq(self, par(*taps))

    def outs(self) -> tuple:
        """All output channels as a tuple: ``l, r = st.outs()``."""
        if self.num_outputs is None:
            raise ValueError(
                "cannot enumerate outputs: this box's arity is unknown "
                "(pass outs=... to faust())"
            )
        return tuple(self[k] for k in range(self.num_outputs))

    # --- AbstractObject hooks: build graph nodes ---

    def _compose_unop(self, selector):
        if selector == "neg":  # Faust has no unary neg; 0 - x
            return Box({"op": "sub", "in": [0.0, self.node]}, self.num_inputs, 1)
        op = _UNARY.get(selector)
        if op is None:
            raise ValueError(f"no Faust box op for unary {selector!r}")
        return Box({"op": op, "in": [self.node]}, self.num_inputs, 1)

    def _compose_binop(self, selector, other):
        op = _BINARY.get(selector)
        if op is None:
            raise ValueError(f"no Faust box op for binary {selector!r}")
        other = box(other)
        ins = _sum_arity((self.num_inputs, other.num_inputs))
        return Box({"op": op, "in": [self.node, other.node]}, ins, 1)

    def _rcompose_binop(self, selector, other):
        op = _BINARY.get(selector)
        if op is None:
            raise ValueError(f"no Faust box op for binary {selector!r}")
        other = box(other)
        ins = _sum_arity((other.num_inputs, self.num_inputs))
        return Box({"op": op, "in": [other.node, self.node]}, ins, 1)

    def __repr__(self):
        return f"Box({self.node!r})"


def box(x) -> Box:
    """Coerces a number or `Box` into a `Box` (numbers are constants:
    Python ``int`` -> Faust int, ``float`` -> real)."""
    if isinstance(x, Box):
        return x
    if isinstance(x, (int, float)) and not isinstance(x, bool):
        return Box(x, 0, 1)
    raise TypeError(f"cannot make a box out of {x!r}")


def _n(x):
    return box(x).node


# ---- primitives ----

def wire() -> Box:
    """The identity box ``_``: one open signal input. Every call is a **new,
    distinct input** — reusing one wire object in two positions is an error
    (see the module docs for the rule and the escapes)."""
    return Box({"op": "wire"}, 1, 1)


def cut() -> Box:
    """The ``!`` box: swallows one signal. Like `wire`, each call is a new,
    distinct position."""
    return Box({"op": "cut"}, 1, 0)


# ---- composition (n-ary, folded left, like the server) ----

def seq(*items) -> Box:
    """Sequential composition ``a : b : ...`` (needs at least 2)."""
    return _compose("seq", items)


def par(*items) -> Box:
    """Parallel composition ``a , b , ...`` (needs at least 2)."""
    boxes = [box(i) for i in _at_least_two("par", items)]
    return Box(
        {"op": "par", "in": [b.node for b in boxes]},
        _sum_arity(b.num_inputs for b in boxes),
        _sum_arity(b.num_outputs for b in boxes),
    )


def split(*items) -> Box:
    """Split composition ``a <: b`` (needs at least 2)."""
    return _compose("split", items)


def merge(*items) -> Box:
    """Merge composition ``a :> b`` — excess outputs are summed (needs at
    least 2)."""
    return _compose("merge", items)


def _at_least_two(op, items):
    if len(items) < 2:
        raise TypeError(f"{op} needs at least 2 boxes, got {len(items)}")
    return items


def _compose(op, items):
    # seq/split/merge folded left: the composite reads like the first box
    # and writes like the last.
    boxes = [box(i) for i in _at_least_two(op, items)]
    return Box(
        {"op": op, "in": [b.node for b in boxes]},
        boxes[0].num_inputs,
        boxes[-1].num_outputs,
    )


def rec(a, b) -> Box:
    """Recursive composition ``a ~ b``: `b` feeds `a`'s first inputs back
    from `a`'s first outputs, with one implicit sample of delay. Point-free —
    for the ``rec(lambda s: ...)`` style, build the loop in a `faust`
    fragment or with `clausters.defs.signals` instead."""
    a, b = box(a), box(b)
    ins = None
    if a.num_inputs is not None and b.num_outputs is not None:
        # Not the shadowed module-level max: that one builds a Box.
        ins = a.num_inputs - b.num_outputs
        ins = 0 if ins < 0 else ins
    return Box({"op": "rec", "in": [a.node, b.node]}, ins, a.num_outputs)


# ---- the escape hatch: Faust source fragments ----

def faust(src: str, *eval_args, defs: str = "", ins: int | None = None,
          outs: int | None = None) -> Box:
    """A Faust **expression** compiled into a box — the door to the Faust
    libraries (``stdfaust.lib`` is imported for you). The resulting box is
    indistinguishable from a primitive: compose it, call it, do arithmetic
    on it.

    ``eval_args`` are **evaluation-stage** arguments, spliced into the source
    text as Faust application — ``faust("fi.lowpass", 3)`` compiles
    ``fi.lowpass(3)``. That is where structural parameters (a filter order,
    a table size, a list of coefficients) must go; they cannot travel as
    signals. Formatting: ``int``/``float`` as literals, a list/tuple as a
    Faust list ``(a, b, c)``, a string verbatim (for expressions or library
    functions passed as arguments). Signal inputs are then wired by calling
    the box:

        lp = faust("fi.lowpass", 3)      # fi.lowpass(3): inputs (fc, x)
        y = lp(cutoff, wire())

    ``defs`` prepends auxiliary Faust definitions (helper functions, pattern
    matching) to the generated program. ``ins``/``outs`` declare the
    fragment's signal arity — only the Faust compiler knows it, so pass
    ``outs=`` when you need channel selection (``st[0]`` / ``.outs()``); a
    wrong declaration is caught by Faust when the def compiles.

    Each distinct generated source is compiled (and cached) separately on
    the server; reusing one fragment *value* many times compiles and
    computes it once.
    """
    applied = src
    if eval_args:
        applied = f"{src}({', '.join(_eval_arg(a) for a in eval_args)})"
    program = f'import("stdfaust.lib"); {defs} process = {applied};' if defs \
        else f'import("stdfaust.lib"); process = {applied};'
    return Box({"op": "faust", "src": program}, ins, outs)


def _eval_arg(a) -> str:
    if isinstance(a, bool) or isinstance(a, Box):
        raise TypeError(
            f"{a!r} cannot be an evaluation-stage argument; boxes are applied "
            "by calling the fragment: faust(src, ...)(box, ...)"
        )
    if isinstance(a, (int, float)):
        return repr(a)
    if isinstance(a, str):
        return a
    if isinstance(a, (list, tuple)):
        return f"({', '.join(_eval_arg(x) for x in a)})"
    raise TypeError(f"cannot splice {a!r} into Faust source")


# ---- structure ----

def delay(x, n) -> Box:
    """``x`` delayed by ``n`` samples (Faust ``@``)."""
    x, n = box(x), box(n)
    ins = _sum_arity((x.num_inputs, n.num_inputs))
    return Box({"op": "delay", "in": [x.node, n.node]}, ins, 1)


def delay1(x) -> Box:
    """One sample of delay (Faust ``'``), sugar for ``delay(x, 1)``."""
    return delay(x, 1)


def select2(sel, a, b) -> Box:
    sel, a, b = box(sel), box(a), box(b)
    ins = _sum_arity((sel.num_inputs, a.num_inputs, b.num_inputs))
    return Box({"op": "select2", "in": [sel.node, a.node, b.node]}, ins, 1)


def select3(sel, a, b, c) -> Box:
    sel, a, b, c = box(sel), box(a), box(b), box(c)
    ins = _sum_arity(
        (sel.num_inputs, a.num_inputs, b.num_inputs, c.num_inputs))
    return Box(
        {"op": "select3", "in": [sel.node, a.node, b.node, c.node]}, ins, 1)


def fconst(ctype, name, file="") -> Box:
    """A foreign **constant**: a scalar the server resolves once, at
    def-compile time, from its runtime. ``ctype`` is ``"int"`` or ``"real"``.
    The building block of `sr` -- prefer that helper for sample rate."""
    return Box({"op": "fconst", "ctype": ctype, "name": name, "file": file},
               0, 1)


def fvar(ctype, name, file="") -> Box:
    """A foreign **variable**: like `fconst` but re-read each block."""
    return Box({"op": "fvar", "ctype": ctype, "name": name, "file": file},
               0, 1)


def sr() -> Box:
    """The engine's sample rate as a `Box`, read from the server at
    def-compile time -- the port of Faust's ``ma.SR``, with the stdlib's
    ``[1, 192000]`` clamp. Use this instead of baking a Python constant so
    the def is correct at whatever rate the engine or NRT renderer runs."""
    raw = fconst("int", "fSamplingFreq", "<math.h>")
    return min(box(192000.0), max(box(1.0), raw))


# ---- unary functions (also available as methods) ----

def _unary(op):
    return lambda x: Box({"op": op, "in": [_n(x)]}, box(x).num_inputs, 1)


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
abs = _unary("abs")  # noqa: A001 — box schema name, by design
floor = _unary("floor")
ceil = _unary("ceil")
rint = _unary("rint")
round = _unary("round")  # noqa: A001


# ---- binary functions ----

def _binary(op):
    def build(a, b):
        a, b = box(a), box(b)
        ins = _sum_arity((a.num_inputs, b.num_inputs))
        return Box({"op": op, "in": [a.node, b.node]}, ins, 1)
    return build


min = _binary("min")  # noqa: A001
max = _binary("max")  # noqa: A001
pow = _binary("pow")  # noqa: A001
atan2 = _binary("atan2")
fmod = _binary("fmod")


# ---- math constants (literals in Faust too; see signals) ----

PI = 3.141592653589793
TAU = 6.283185307179586  # 2*PI


# ---- controls (labels become control names) ----

def hslider(label, init, lo, hi, step) -> Box:
    return Box({"op": "hslider", "label": label, "init": init,
                "min": lo, "max": hi, "step": step}, 0, 1)


def vslider(label, init, lo, hi, step) -> Box:
    return Box({"op": "vslider", "label": label, "init": init,
                "min": lo, "max": hi, "step": step}, 0, 1)


def nentry(label, init, lo, hi, step) -> Box:
    return Box({"op": "nentry", "label": label, "init": init,
                "min": lo, "max": hi, "step": step}, 0, 1)


def button(label) -> Box:
    return Box({"op": "button", "label": label}, 0, 1)


def checkbox(label) -> Box:
    return Box({"op": "checkbox", "label": label}, 0, 1)


def hgroup(label, inner) -> Box:
    inner = box(inner)
    return Box({"op": "hgroup", "label": label, "in": [inner.node]},
               inner.num_inputs, inner.num_outputs)


def vgroup(label, inner) -> Box:
    inner = box(inner)
    return Box({"op": "vgroup", "label": label, "in": [inner.node]},
               inner.num_inputs, inner.num_outputs)


# ---- tables ----

def waveform(values) -> Box:
    """A fixed table; outputs the (size, content) pair, ready to stand in
    for `rdtable`/`rwtable`'s leading (size, init) boxes."""
    return Box({"op": "waveform", "values": [float(v) for v in values]}, 0, 2)


def rdtable(*args) -> Box:
    """``rdtable(size, init, ridx)`` — or ``rdtable(wf, ridx)`` with a
    `waveform` standing in for (size, init)."""
    return _table("rdtable", args, 2, 3)


def rwtable(*args) -> Box:
    """``rwtable(size, init, widx, wsig, ridx)`` — or the 4-argument form
    with a `waveform` up front."""
    return _table("rwtable", args, 4, 5)


def _table(op, args, lo, hi):
    if not lo <= len(args) <= hi:
        raise TypeError(f"{op} takes {lo} or {hi} boxes, got {len(args)}")
    boxes = [box(a) for a in args]
    ins = _sum_arity(b.num_inputs for b in boxes)
    return Box({"op": op, "in": [b.node for b in boxes]}, ins, 1)


# ---- the wire-reuse lint (used by FaustDef.from_box) ----

def check_wires(node):
    """Rejects a tree where the same `wire`/`cut` **object** appears in more
    than one position. Each wire is a distinct input in the box algebra;
    reusing one object almost always means the graph silently reads more bus
    channels than intended. Duplicating any *other* box value is fine (shared
    subtrees are computed once)."""
    counts = _io_counts(node, {})
    if any(n > 1 for _, n in counts.values()):
        which = sorted({op for (op, n) in counts.values() if n > 1})
        raise ValueError(
            f"a {'/'.join(which)} box object was reused; each wire (and cut) "
            "is a distinct position — every input needs its own wire(): "
            "route explicitly with split(), or write that stretch inside a "
            "faust() fragment (e.g. \"_ <: ...\")"
        )


def _io_counts(node, memo):
    """``{id(dict): (op, occurrences)}`` for the wire/cut dicts under `node`,
    counting textual positions (shared subtrees multiply their contents)."""
    if isinstance(node, dict):
        key = id(node)
        if key in memo:
            return memo[key]
        op = node.get("op")
        if op in ("wire", "cut"):
            counts = {key: (op, 1)}
        else:
            counts = {}
            for value in node.values():
                _merge_counts(counts, _io_counts(value, memo))
        memo[key] = counts
        return counts
    if isinstance(node, list):
        counts = {}
        for item in node:
            _merge_counts(counts, _io_counts(item, memo))
        return counts
    return {}


def _merge_counts(into, other):
    for key, (op, n) in other.items():
        prev = into.get(key)
        into[key] = (op, n + (prev[1] if prev else 0))
