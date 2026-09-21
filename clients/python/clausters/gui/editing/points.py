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
puts it back -- one call, because the inverse has to be read before the edit
lands. Nothing here computes an inverse, which is the whole reason the domain
seam exists.

**What a shape is stays the client's.** The crate carries a point's ``data``
and never reads it, so the segment shapes an `clausters.defs.ugens.Env` needs
travel in it -- without that an undo put the curve back straight, which is
losing the data rather than declining to interpret it.
"""

import weakref

from ... import _native
from ...multitrack import crate_points, flat_points
from .domain import Domain
from .editor import Editor
from .view import View

class PointsDomain(Domain):
    """A curve's vocabulary: the crate's ``points``, with the shape of each
    segment carried in the point's own ``data``.

    **What it asks of the structure is `to_points` and `set_points`**, and
    nothing about its type -- an `clausters.defs.ugens.Env`, a
    `clausters.defs.ugens.Bpf` and a `clausters.multitrack.Automation` are all
    curves here, and a fourth thing that learns the pair would be too."""

    name = _native.POINTS
    ingested = True

    def state(self, structure) -> list:
        """The curve as the crate holds it -- the state `current` is read
        against and `project` writes back.

        **The curve seam, not a gesture.** It is here rather than in the crate
        for the reason `project` is: what this crosses is the object *this
        client* holds, and the vocabulary on the other side is already the
        crate's. Both directions are `clausters.multitrack.crate_points` and
        `clausters.multitrack.flat_points`, written once because a
        `clausters.multitrack.Automation` converts the same way.
        """
        return crate_points(structure.to_points())

    def current(self, structure, payload: dict) -> "dict | None":
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        return None if edited is None else edited.get("current")

    def project(self, structure, payload: dict) -> bool:
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        if edited is None or not edited.get("applied"):
            return False
        structure.set_points(flat_points(edited["state"]))
        return True


class PointsView(View):
    """One `clausters.gui.guidef.bpf`: the curve on its own axis.

    **The axis is declared, and it is the view's to keep.** A `bpf` given
    neither range draws against the unipolar default and fits its time to the
    last point, so a curve of any other range is pinned to the top of the field
    and the edit-back reports positions in the *axis*'s values -- one drag
    destroys the data's range -- while the time rescales under the hand on every
    edit. So both ends come from the curve, through
    `clausters.gui.editing.points.PointsView.axis`, and are held: what a window
    is looking at is a view's, and moving it under a gesture is the one thing an
    axis must not do.
    """

    def __init__(self):
        super().__init__()
        #: The value axis this view is drawing against, and the time it spans,
        #: kept per structure so a redraw does not re-fit them. Both only ever
        #: **grow** -- see `axis`.
        #:
        #: Keyed by the **curve itself**, weakly. Screen state is about a thing,
        #: and a thing is not its address: `id()` is reused the moment an object
        #: is freed, so a table keyed by one hands a new curve whatever axis the
        #: last one at that address was drawn against. Weak keys also let the
        #: state go when the curve does, which is what screen state should do.
        self._axis: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()
        self._span: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()

    def drawn(self, structure, points) -> dict:
        """The props this curve is drawn with, and the axis they settled on
        remembered for the next time.

        **The projection is the crate's** (`clausters._native.points_props`):
        the points, the value axis they stand on and the time they span, all in
        one answer, so a script and a page set the same widget with the same
        props. What is kept here is only what a *view* keeps -- the axis and the
        span in hand -- because both of them only ever grow, and a curve that
        refits while a point is being dragged moves every other point on
        screen.
        """
        props = _native.points_props(points, self._axis.get(structure),
                                     self._span.get(structure, 0.0))
        self._axis[structure] = (props["min"], props["max"])
        self._span[structure] = props.get("duration", 0.0)
        return props

    def build(self, editor) -> dict:
        from ..guidef import window

        drawn = self.drawn(editor.structure, editor.structure.to_points())
        picture = self.catalogue(editor, "bpf", "curve", editor.structure,
                                 {**drawn, "duration": drawn.get("duration", 0.0),
                                  "label": _name(editor.structure)})
        return window(picture, *editor.extra,
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def props(self, editor, widget_id: int) -> dict:
        return self.drawn(editor.structure, editor.structure.to_points())


class PointsEditor(Editor):
    """A curve on screen, editable back into the curve the caller already holds.

    Nothing is handed back at the end: the object the script passed in *is* the
    edited one, and reading its ``to_points`` after an edit is how a caller sees
    what was drawn.
    """

    def __init__(self, curve, *, sample_rate: float,
                 title: str = "Curve", **options):
        super().__init__(curve, sample_rate=sample_rate,
                         domain=PointsDomain(), view=PointsView(), title=title,
                         **options)


def _name(curve) -> str:
    name = getattr(curve, "name", None)
    return name if isinstance(name, str) and name else "curve"


def is_curve(structure) -> bool:
    """Whether `edit` should open this as a curve.

    **Asked of the structure, not of a type list.** What a curve editor needs is
    an addressable list of break points it can read and write back, which is the
    ``to_points``/``set_points`` pair -- so an `clausters.defs.ugens.Env`, a
    `clausters.defs.ugens.Bpf` and a `clausters.multitrack.Automation` all open,
    and none of them is named here.
    """
    return (callable(getattr(structure, "to_points", None))
            and callable(getattr(structure, "set_points", None)))
