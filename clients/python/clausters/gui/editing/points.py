"""Editing a **break-point curve**: its vocabulary, its picture and its editor.

The smallest of the three fundamental structures, and the one that shows the
shape of all of them: a `clausters.gui.editing.Domain` that turns the `bpf`
view's ``points`` payload into the crate's vocabulary and back, a
`clausters.gui.editing.View` that is one `clausters.gui.guidef.bpf` widget, and
an editor that is `clausters.gui.editing.Editor` with those two in it and
nothing else.

**How an edit inverts is the crate's**, reached through
`clausters._native.domain_edit`: the payload goes in with the curve as it
stands, and what comes back is the curve as it now is *and* the payload that
puts it back — one call, because the inverse has to be read before the edit
lands. Nothing here computes an inverse, which is the whole reason the domain
seam exists.

**What a shape is stays the client's.** The crate carries a point's ``data``
and never reads it, so the segment shapes an `clausters.defs.ugens.Env` needs
travel in it — without that an undo put the curve back straight, which is
losing the data rather than declining to interpret it.
"""

from ... import _native
from ...defs.ugens import points_to_env
from ...seq.automation import Automation
from .domain import Domain
from .editor import Editor
from .view import View

#: What the ``bpf`` widget sends and takes: flat ``t v shape curve`` quads.
QUAD = 4


def quads(flat) -> list:
    """A flat ``points`` payload as ``(t, value, shape, curve)`` tuples,
    dropping a trailing partial quad rather than guessing at it."""
    values = [float(v) for v in flat]
    return [(values[i], values[i + 1], int(values[i + 2]), values[i + 3])
            for i in range(0, len(values) - len(values) % QUAD, QUAD)]


class PointsDomain(Domain):
    """A curve's vocabulary: the crate's ``points``, with the shape of each
    segment carried in the point's own ``data``."""

    name = _native.POINTS

    def payload(self, structure, tag: str, values) -> "dict | None":
        if tag != "points" or not values:
            return None
        return {"intent": "setpoints",
                "points": [{"at": t, "value": v, "data": {"shape": shape,
                                                          "curve": curve}}
                           for t, v, shape, curve in quads(values)]}

    def state(self, structure) -> list:
        """The curve as the crate holds it — the state `current` is read
        against and `project` writes back."""
        return [{"at": t, "value": v, "data": {"shape": shape, "curve": curve}}
                for t, v, shape, curve in quads(structure.to_points())]

    def current(self, structure, payload: dict) -> "dict | None":
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        return None if edited is None else edited.get("current")

    def project(self, structure, payload: dict) -> bool:
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        if edited is None or not edited.get("applied"):
            return False
        structure.env = points_to_env(self.flat(edited["state"]))
        # One door: the envelope the script holds and the control buffer the
        # lane synth reads cannot disagree about which of the two happened.
        structure.refill()
        return True

    @staticmethod
    def flat(points) -> list:
        """The crate's points back as the flat quads the view and the `Env`
        both speak. A point that says nothing about its segment is linear,
        which is what a curve drawn somewhere that has no shapes means."""
        out: list = []
        for point in points:
            data = point.get("data") or {}
            out += [float(point.get("at", 0.0)), float(point.get("value", 0.0)),
                    int(data.get("shape", 1)), float(data.get("curve", 0.0))]
        return out

    def label(self, payload: dict) -> str:
        return "draw the curve"


class PointsView(View):
    """One `clausters.gui.guidef.bpf`: the curve on its own axis."""

    def build(self, editor) -> dict:
        from ..guidef import bpf, window

        wid = self.register(editor._new_id(), editor.structure)
        return window(bpf(id=wid, points=editor.structure.to_points(),
                          label=_name(editor.structure)),
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def props(self, editor, widget_id: int) -> dict:
        return {"points": editor.structure.to_points()}


class PointsEditor(Editor):
    """A curve on screen, editable back into the `clausters.seq.Automation` the
    caller already holds.

    Nothing is handed back at the end: the object the script passed in *is* the
    edited one, and reading `clausters.seq.Automation.to_points` after an edit
    is how a caller sees what was drawn.
    """

    def __init__(self, curve, *, sample_rate: float, tempo: float = 1.0,
                 title: str = "Curve", **options):
        super().__init__(curve, sample_rate=sample_rate, tempo=tempo,
                         domain=PointsDomain(), view=PointsView(), title=title,
                         **options)


def _name(curve) -> str:
    name = getattr(curve, "name", None)
    return name if isinstance(name, str) and name else "curve"


def is_curve(structure) -> bool:
    """Whether `edit` should open this as a curve."""
    return isinstance(structure, Automation)
