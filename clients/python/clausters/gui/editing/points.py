"""Editing a **break-point curve**: the points editor.

It is the automation editor seen on its own: what it edits is a curve over one
parameter -- an `clausters.multitrack.Automation`, the curve a track, a region,
a sequence and a note hold -- with nothing around it, which is also how an
envelope is made: an `clausters.defs.ugens.Env` and a
`clausters.defs.ugens.Bpf` are curves nobody holds yet.

**The editor is the crate's** (the ``openPoints`` member of
`clausters._native.EditingCore`): the window, the value axis and the time span
it keeps while open, what a gesture does to the curve, the entry it leaves and
the corrections it answers with. The crate edits a curve of its own, the document's; what is here is
what a language owns -- the socket, handing the crate the curve as the script
holds it, and writing back onto that object the points each edit and each step
leave.

**What a shape is stays the client's.** The crate carries a point's ``data``
and never reads it, so the segment shapes an `clausters.defs.ugens.Env` needs
travel in it -- without that an undo put the curve back straight, which is
losing the data rather than declining to interpret it.
"""

from ... import _native
from .domain import Domain
from .editor import Editor
from .samples import _plain
from .view import View


class PointsDomain(Domain):
    """A curve's vocabulary, the crate's ``points``. The crate applies an edit
    and a step to the curve it holds; what is here is writing the points they
    leave back onto the curve the script holds.

    **What it asks of the structure is `to_points` and `set_points`**, and
    nothing about its type -- an `clausters.defs.ugens.Env`, a
    `clausters.defs.ugens.Bpf` and a `clausters.multitrack.Automation` are all
    curves here, and a fourth thing that learns the pair would be too."""

    name = _native.POINTS
    ingested = True

    def write(self, structure, points) -> None:
        """Write the crate's points -- flat ``t v shape curve`` quads -- onto
        the curve, its segments' shapes as the integers they are."""
        flat = [float(x) for x in points]
        for i in range(2, len(flat), 4):
            flat[i] = int(flat[i])
        structure.set_points(flat)


class PointsView(View):
    """One ``curve`` widget and a row of controls under it, composed by the
    crate."""

    def build(self, editor) -> dict:
        wid = self.widget(editor, "curve", editor.structure)
        shape = self.widget(editor, "shape", editor.structure)
        editor._curve = wid
        editor._sync_core()
        tree = editor._call("window", widget=wid, shape=shape)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate -- into the row under the curve,
        # beside the segment's shape chooser.
        row = tree["children"][1]["children"][0]
        row["children"] = [*row.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class PointsEditor(Editor):
    """A curve on screen, editable back into the curve the caller already holds.

    Nothing is handed back at the end: the object the script passed in *is* the
    edited one, and reading its ``to_points`` after an edit is how a caller sees
    what was drawn.

    Args:
        curve: what to edit -- anything with ``to_points``/``set_points``.
        sample_rate: the rate the curve's time axis counts in.
        title: the window's title.
        min: with ``max``, the **range the curve's values are kept in** -- a
            rule, not a picture: the value axis is the range and holds, and no
            point is left outside it. Without them a curve that automates a
            parameter (a `clausters.multitrack.Automation` with a ``target``)
            is kept in that parameter's range, the one the roll and the
            multitrack draw it over; and a curve that says nothing (an
            `clausters.defs.ugens.Env`, a `clausters.defs.ugens.Bpf`) is drawn
            on an axis derived from its points, which grows to hold one dragged
            past it.
        max: the top of that range.
        start: with ``end``, the range the curve's times are kept in -- a
            normalized envelope is ``start=0.0, end=1.0`` beside ``min=0.0,
            max=1.0``.
        end: the end of that range.

    The window shows the rules: the time ruler under the curve, in its own
    seconds, the value ruler beside it, and a readout of what the pointer is
    over -- a point's value against the range, and the shape of its segment.
    A click on a segment selects it, and the chooser in the row under the curve
    sets its shape; ``extra`` widgets go in that row, beside it.
    """

    def __init__(self, curve, *, sample_rate: float, title: str = "Curve",
                 min=None, max=None, start=None, end=None, **options):
        if (min is None) != (max is None):
            raise ValueError(
                "a declared range needs both ends: pass min and max, or neither")
        if (start is None) != (end is None):
            raise ValueError(
                "a declared range needs both ends: pass start and end, or neither")
        domain = PointsDomain()
        super().__init__(curve, sample_rate=sample_rate, domain=domain,
                         view=PointsView(), title=title, **options)
        #: The curve's widget id, once drawn.
        self._curve = None
        request = {"rate": self.sample_rate, "title": self.title,
                   "w": int(self.size[0]), "h": int(self.size[1]),
                   "points": [float(x) for x in curve.to_points()],
                   "name": _name(curve), "target": _target(curve)}
        if min is not None:
            request.update(min=float(min), max=float(max))
        if start is not None:
            request.update(start=float(start), end=float(end))
        self._member, self._structure_id = self._editing.open(
            "openPoints", f"object:{id(curve)}", request, curve, domain)

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _sync_core(self) -> None:
        """Hand the crate the window it is open in, the chrome, and the curve
        as the script holds it now."""
        self._call("sync", window=self._window, rate=self.sample_rate,
                   title=self.title, w=int(self.size[0]), h=int(self.size[1]),
                   points=[float(x) for x in self.structure.to_points()],
                   name=_name(self.structure), target=_target(self.structure))

    @property
    def rules(self) -> dict:
        """**The ranges the curve is edited inside**: ``{"values": (low,
        high), "time": (low, high)}``, each ``None`` where there is none --
        the declared ones, and where no value range was declared, the range of
        the parameter the curve automates."""
        self._sync_core()
        rules = self._call("rules")
        return {key: None if rules.get(key) is None
                else (float(rules[key][0]), float(rules[key][1]))
                for key in ("values", "time")}

    # ---- the crate's turns ----

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        turned = self._editing.event(self._member, str(addr), _plain(list(args)))
        outcome = turned.get("outcome") or {}
        if outcome.get("turn") == "closed":
            return self._closed()
        if outcome.get("turn") == "step":
            stepped = self.app.stepped(turned.get("stepped") or {}, self)
            self.echo.send(outcome.get("answer"))
            return stepped
        return self._take(outcome)

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off."""
        self._sync_core()
        wid, tag, values = args[0], args[1], list(args[2:])
        turned = self._editing.event(self._member, "/gui_event",
                                     _plain([wid, 0, 0, tag, *values]))
        return self._take(turned.get("outcome") or {})

    def _take(self, outcome: dict) -> bool:
        """Answer the host with what a turn came to; whether the curve
        changed."""
        if outcome.get("turn") in (None, "nothing"):
            return False
        if outcome.get("points") is not None:
            # The crate's curve moved: the script's follows it, as part of the
            # entry the turn recorded rather than as a change of its own.
            with self._editing.applying():
                self.domain.write(self.structure, outcome["points"])
        changed = bool(outcome.get("changed"))
        if changed:
            self.dirty = True
            self._editing.changed()
        if outcome.get("locate") is not None:
            self.cursor = float(outcome["locate"])
            self.locate(self.cursor)
            if self.composed_in is not None:
                self.composed_in.locate(self.cursor)
            if callable(self.on_locate):
                self.on_locate(self.cursor)
        self.echo.send(outcome.get("answer"))
        self._closing(outcome)
        return changed


def _target(curve):
    """What the curve automates, for a curve that says (an
    `clausters.multitrack.Automation`), else ``None``."""
    target = getattr(curve, "target", None)
    return target if isinstance(target, dict) else None


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
