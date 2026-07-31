"""What "an expression" is, as a type.

An **expression** is something that composes a DSP graph rather than a value:
a `clausters.defs.ugens.Ugen` graph, a channel list, a Faust
`clausters.defs.signals.Signal` or a `clausters.defs.boxes.Box`. It is what
the ambient verbs accept where a def is wanted, and what
`clausters.defs.asdef.as_def` coerces.

The distinction the base above this one cannot make: `clausters.base.absobject.
AbstractObject` is the *operator* protocol, and the value side
(`clausters.base.builtins`) shares it — the same written expression composes
concrete numbers or a graph depending on the subclass. `Expr` is the half that
composes a graph.

Its two branches are the two def families, which are peers: `SynthExpr` for the
UGen graph (`Ugen`, `Control`, `ChannelList`) and `FaustExpr` for Faust
(`Signal`, `Box`). A single engine-neutral base would make one family "the
graph" and the other the exception; `Signal` and `Box` do not compose with each
other either, so their shared roof buys a real dispatch. The name avoids
``Graph*``: `clausters.defs.graphdef.GraphDef` is the third def family, so in
this package "graph" already means a configuration of member nodes wired by
buses.

Not every composable thing is one: `clausters.defs.pv_expr.PvExpr` composes a
symbolic **per-bin** expression that serializes to the token list ``PV_Kernel``
interprets. It is never a graph node — it cannot be a def root nor a UGen input
— so it stays outside this hierarchy and the verbs keep rejecting it.

These are markers: they carry no behavior, and the four composition hooks stay
in the classes that implement them.
"""

from ..base.absobject import AbstractObject


class Expr(AbstractObject):
    """Something that composes a DSP graph rather than a value — a `SynthExpr`
    or a `FaustExpr`. The type the ambient verbs (`clausters.play`,
    `clausters.plot`, `clausters.render`) accept as a bare expression."""


class SynthExpr(Expr):
    """An expression of the **UGen graph**: a `clausters.defs.ugens.Ugen`, a
    `clausters.defs.ugens.Control`, or a `clausters.defs.ugens.ChannelList` of
    them. What `clausters.defs.synthdef.SynthDef` serializes and what
    ``out``/``replace_out``/``out_ctl`` accept."""


class FaustExpr(Expr):
    """An expression of a **Faust graph**: a `clausters.defs.signals.Signal`
    (signal API) or a `clausters.defs.boxes.Box` (box API). What
    `clausters.defs.faustdef.FaustDef` compiles."""
