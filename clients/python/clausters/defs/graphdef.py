"""GraphDef: a named node-graph "program" ready for ``/d_graph``.

Where `SynthDef` and
`FaustDef` each describe a *single* synthesis
node, a `GraphDef` describes a whole **configuration of member nodes
wired by buses** — an effect chain, a mixer, a layered instrument — that the
server stores and instantiates as one unit. It exposes a **named parameter
surface**: ports that map to inner member controls, so the running instance is
driven through the port names, never the private member node ids (the same
encapsulation a composite SynthDef would give).

It is a thin JSON builder, like the other two def kinds: it composes a spec and
hands it to ``server.add_graphdef`` (``/d_graph``). The server resolves the
member def names (SynthDef *or* FaustDef, identically), allocates the
instance's private buses and wires them.

```python
from clausters.defs import GraphDef

g = GraphDef("chain")
mix = g.bus("mix")                          # a private internal audio bus
src = g.add("gsrc", out=mix, level=1.0)     # a member; `out` control -> the bus
g.add("gsink", {"in": mix, "out": "OUT"})   # `in` reads `mix`, `out` -> hardware
g.port("gain", src["level"], default=0.5)   # surface port -> the source's level
server.add_graphdef(g)                       # /d_graph (blocks on /done in RT)

inst = server.graph("chain", {"gain": 0.8})  # /graph_new
server.set(inst, {"gain": 0.3})              # resolves against the surface
server.free(inst)                            # frees the group + its private buses
```

The reserved control name ``"OUT"`` wires a member's output to hardware bus 0;
any other string value of a member control is the name of an internal bus.
"""

import json


class GraphBusRef:
    """A reference to an internal GraphDef bus, returned by `GraphDef.bus`.
    Used as a member control value (it serializes to the bus name)."""

    def __init__(self, name: str):
        self.name = str(name)


class _Target:
    """One inner target of a surface port: a member's control with optional
    linear scaling (``mul``·v + ``add``)."""

    def __init__(self, member: int, control: str, mul: float = 1.0, add: float = 0.0):
        self.member = member
        self.control = control
        self.mul = mul
        self.add = add

    def scaled(self, mul: float = 1.0, add: float = 0.0) -> "_Target":
        """A copy of this target with linear scaling applied to incoming
        values, e.g. ``filt["cutoff"].scaled(7800, 200)`` maps a 0..1 port to
        200..8000 Hz."""
        return _Target(self.member, self.control, float(mul), float(add))

    def _as_dict(self) -> dict:
        d = {"member": self.member, "control": self.control}
        if self.mul != 1.0:
            d["mul"] = self.mul
        if self.add != 0.0:
            d["add"] = self.add
        return d


class MemberRef:
    """A handle to a member added with `GraphDef.add`. Index a control
    name (``member["cutoff"]`` or ``member.cutoff``) to get a surface
    `_Target`."""

    def __init__(self, index: int):
        self.index = index

    def __getitem__(self, control: str) -> _Target:
        return _Target(self.index, str(control))

    def __getattr__(self, control: str) -> _Target:
        if control.startswith("_"):
            raise AttributeError(control)
        return _Target(self.index, control)


def _control_value(v):
    """Member control values: a bus reference serializes to its name, a plain
    string (e.g. ``"OUT"``) stays a string, anything else is a float."""
    if isinstance(v, GraphBusRef):
        return v.name
    if isinstance(v, str):
        return v
    return float(v)


class GraphDef:
    """A named node graph. Build it with `bus`, `add` and
    `port`, then send it with ``server.add_graphdef``."""

    def __init__(self, name: str):
        self.name = str(name)
        self._buses: list[dict] = []
        self._members: list[dict] = []
        self._surface: dict[str, list[dict]] = {}
        self._defaults: dict[str, float] = {}

    def bus(self, name: str, *, rate: str = "audio", channels: int = 1) -> GraphBusRef:
        """Declares a private internal bus (``rate`` ``"audio"`` or
        ``"control"``). Each instance allocates its own, so two instances never
        collide."""
        if rate not in ("audio", "control"):
            raise ValueError("bus rate must be 'audio' or 'control'")
        self._buses.append({"name": str(name), "rate": rate, "channels": int(channels)})
        return GraphBusRef(name)

    def add(self, defname: str, controls: dict | None = None, *,
            maps: dict | None = None, voice: bool = False, **control_kw) -> MemberRef:
        """Adds a member: an instance of the SynthDef/FaustDef ``defname``.
        Control values may be numbers, a `GraphBusRef` (to wire the
        control to an internal bus), or ``"OUT"`` (hardware bus 0). ``maps``
        binds controls to internal *control* buses via ``/n_map``. Pass
        controls as a dict (needed for reserved names like ``in``) and/or as
        keywords. ``voice=True`` marks a **per-voice** member: instantiated once
        per `Server.graph_voice` (or MIDI note) instead of at
        instantiation — the per-note part of a polyphonic instrument."""
        merged = dict(controls or {})
        merged.update(control_kw)
        member: dict = {"def": str(defname)}
        if merged:
            member["controls"] = {k: _control_value(v) for k, v in merged.items()}
        if maps:
            member["maps"] = {
                k: (v.name if isinstance(v, GraphBusRef) else str(v))
                for k, v in maps.items()
            }
        if voice:
            member["voice"] = True
        index = len(self._members)
        self._members.append(member)
        return MemberRef(index)

    def members(self) -> list[dict]:
        """The member specs in add order (read-only copies): each a def name and
        its control wiring. This is what reads a stored graph back into a patch
        view -- `clausters.defs.GraphPatch.from_graphdef`."""
        return [dict(m) for m in self._members]

    def port(self, name: str, *targets: _Target, default: float | None = None):
        """Defines a surface port mapping ``name`` to one or more member
        controls (each a `_Target`, optionally ``.scaled(...)``).
        ``default`` is applied at instantiation unless overridden."""
        if not targets:
            raise ValueError(f"surface port {name!r} needs at least one target")
        self._surface[str(name)] = [t._as_dict() for t in targets]
        if default is not None:
            self._defaults[str(name)] = float(default)

    def spec(self) -> dict:
        """The ``GraphDefSpec`` dict the server's ``/d_graph`` validates."""
        if not self._members:
            raise ValueError("a GraphDef needs at least one member")
        spec: dict = {"name": self.name, "members": self._members}
        if self._buses:
            spec["buses"] = self._buses
        if self._surface:
            spec["surface"] = self._surface
        if self._defaults:
            spec["defaults"] = self._defaults
        return spec

    def dump_def(self) -> str:
        """The def serialized to text -- the ``/d_graph`` wire payload, the JSON
        ``GraphDefSpec`` (see `spec`). Useful to inspect the composition before
        sending it."""
        return json.dumps(self.spec())

    def plot_def(self, defs: dict | None = None, *, label: str | None = None,
                 w: int = 1000, h: int = 700, title: str | None = None, host=None):
        """Open this GraphDef's **structure** as a directed `patch` view in its
        own window on the ambient GUI host — the level-1 patcher drawn from the
        def itself (the inverse of building it), the host laying the boxes out as
        an inverted tree. One window per call, the `clausters.plot` posture; this
        shows the def's *structure*, where `clausters.plot(self)` renders its
        *sound*.

        ``defs`` maps a member's def name to the `clausters.defs.SynthDef` it was
        built from, so a box's ports are typed (a control feeding an ``In`` is an
        inlet, one feeding an ``Out`` an outlet); a member whose def is not
        resolvable draws port-less (no cords). ``label`` captions the patch panel
        (defaults to ``"graphdef"`` — the panel names *what* is drawn, not the
        def's name); ``host`` is an explicit `clausters.gui.GuiHost``, ``None``
        resolves the ambient one. Returns a `clausters.plot.PatchWindow`
        (``.close()``)."""
        from ..plot import _open_patch_view
        from .patch import GraphPatch

        model = GraphPatch.from_graphdef(self, defs)
        return _open_patch_view(model, label=label if label is not None else "graphdef",
                                w=w, h=h, title=title or self.name, host=host)
