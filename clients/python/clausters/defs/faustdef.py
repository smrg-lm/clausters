"""FaustDef: a named Faust definition ready for ``/d_faust``.

Wraps a graph built with `clausters.defs.signals` (the **signal tree**
form), one built with `clausters.defs.boxes` (the **box tree** form — a `Box`,
or a raw dict for machine-generated trees), or a Faust **source** string: the
three payloads the server's ``/d_faust`` accepts, on equal footing (it sniffs
which by the first byte; see the server's ``faust`` module). They are three ways
of writing Faust, not a main road and two detours — pick the one that says what
you mean. Sending and instantiating is the
`Server`'s job; this only builds the payload and
exposes the declared control names (UI labels), plus the reserved ``in``/``out``
bus controls the server adds.
"""

import json

from .boxes import Box, check_wires
from .signals import Signal


class FaustDef:
    def __init__(self, name: str, payload, kind: str):
        #: the def name (also what `/d_faust` replies with on success)
        self.name = name
        self._payload = payload  # dict (signal/box tree) or str (source)
        self.kind = kind         # 'signals' | 'box' | 'source'

    # --- constructors ---

    @classmethod
    def from_signals(cls, name: str, *outputs) -> "FaustDef":
        """One output per argument (``Signal`` or number)."""
        if not outputs:
            raise ValueError("a signal def needs at least one output")
        nodes = [o.to_json() if isinstance(o, Signal) else o for o in outputs]
        return cls(name, {"signals": nodes}, "signals")

    @classmethod
    def from_source(cls, name: str, src: str) -> "FaustDef":
        return cls(name, src, "source")

    @classmethod
    def from_box(cls, name: str, box) -> "FaustDef":
        """From a `clausters.defs.boxes.Box` (or a raw box-tree dict, kept
        for machine-generated graphs). A `Box` is checked for the one silent
        mistake the box algebra allows: reusing the same ``wire()``/``cut()``
        object in two positions (each wire is a distinct input)."""
        if isinstance(box, Box):
            check_wires(box.node)
            box = box.to_json()
        return cls(name, box, "box")

    # --- serialization ---

    def dump_def(self) -> str:
        """The def serialized to text -- the ``/d_faust <name> <payload>`` wire
        payload: a JSON signal/box tree, or the Faust source string verbatim.
        Useful to inspect the built graph before sending it."""
        if self.kind == "source":
            return self._payload
        return json.dumps(self._payload)

    # --- controls ---

    def control_names(self) -> list[str]:
        """The control names this def declares (UI labels), in tree order.
        The reserved ``in``/``out`` bus controls (added by the server) are not
        included; see `reserved`."""
        names: list[str] = []
        if self.kind in ("signals", "box"):
            _collect_labels(self._payload, names)
        return names

    #: bus-selecting controls every Faust synth also accepts.
    reserved = ("out", "in")

    def plot_def(self, *, label: str | None = None, w: int = 1000, h: int = 700,
                 title: str | None = None, host=None):
        """Open this FaustDef's **structure** as a directed `patch` view in its
        own window on the ambient GUI host — the level-2 patcher drawn from the
        def's signal graph (every signal op a box, every operand a cord, the host
        laying them out as an inverted tree). One window per call, the
        `clausters.plot` posture; this shows the def's *structure*, where
        `clausters.plot(self)` renders its *sound*.

        A **signal-tree** def (`from_signals`) decodes node for node; a
        **box-tree** or **source** def is opaque and draws as a single box (its
        internals are the Faust compiler's, not reconstructable client-side).
        ``label`` captions the patch panel (defaults to ``"faustdef"`` — the
        panel names *what* is drawn, not the def's name); ``host`` is an explicit
        `clausters.gui.GuiHost`, ``None`` resolves the ambient one. Returns a
        `clausters.plot.PatchWindow` (``.close()``)."""
        from ..plot import _open_patch_view
        from .patch import DefPatch

        model = DefPatch.from_faustdef(self)
        return _open_patch_view(model, label=label if label is not None else "faustdef",
                                w=w, h=h, title=title or self.name, host=host)

    def __repr__(self):
        return f"FaustDef({self.name!r}, kind={self.kind!r})"


_CONTROL_OPS = {"hslider", "vslider", "nentry", "button", "checkbox"}


def _collect_labels(node, out: list[str]):
    if isinstance(node, dict):
        if node.get("op") in _CONTROL_OPS:
            label = node.get("label")
            if isinstance(label, str) and label not in out:
                out.append(label)
        for value in node.values():
            _collect_labels(value, out)
    elif isinstance(node, list):
        for item in node:
            _collect_labels(item, out)
