# pyright: basic
"""The multitrack: tracks, take lanes, regions, automation and markers.

Private: the names are `clausters.document`'s, which says what they are.
"""

import weakref
from dataclasses import dataclass
from typing import Any, overload

from .. import _native
from ._format import FIRST_VERSION, _unknown_fields
from ._timeline import Meter, Span, Tempo



@dataclass
class Fade:
    """**A region's fade**: how long it lasts, in seconds, and the shape of the
    one envelope segment it is.

    A region's two fades make a trapezoid over its box -- from silence to full
    level, full level, back to silence -- and each sloped side is a segment like
    any other envelope's: ``shape`` is one of the names an `Env` segment takes
    (``"lin"``, ``"exp"``, ``"sin"``, ``"wel"``, ``"curve"`` ...) and ``curve``
    the curvature for ``"curve"``. The default is ``"wel"``, **equal power**:
    quick at first and slow at the end of a rise, so a fade out against a fade
    in of the same length keeps the summed power constant.
    """

    length: float
    shape: str = "wel"
    curve: float = 0.0

    def write(self) -> dict:
        out: dict = {"length": self.length}
        if self.shape != "wel":
            out["shape"] = self.shape
        if self.curve:
            out["curve"] = self.curve
        return out

    @classmethod
    def read(cls, written: dict) -> "Fade":
        shape = written.get("shape")
        return cls(length=float(written.get("length", 0.0)),
                   shape=shape if isinstance(shape, str) else "wel",
                   curve=float(written.get("curve", 0.0)))


@dataclass
class Content:
    """What fills a region: a **window** onto a source.

    Not the clipboard's content, which is what was *copied*. Two nouns in two
    modules, each the right word where it stands: this one is a region's
    ``content`` field, and the format tags it ``fill``.

    One class with a `fill` saying which shape it is, because that is how the
    format writes it. A window is the one fill this build knows; any other is
    carried whole and written back as it came, so a session a newer writer
    saved is not lost by passing through here.

    A window carries its `playrate` (a property of *this* placement: two regions
    over one source may play it at two rates) and the `args` of **this**
    evaluation, for a window onto something generated -- a function placed twice
    is two evaluations, possibly with different arguments, and the document
    carries them without reading them.
    """

    fill: str
    window: "dict | None" = None
    playrate: float = 1.0
    args: "dict | None" = None
    other: "dict | None" = None
    #: Whether the window **wraps**: past the end of the source it begins
    #: again, and before the beginning it shows the source's own tail. What a
    #: box longer than what it reads means -- the alternative being that it
    #: simply stops, which is what a box that does not loop does. A property of
    #: *this* placement, like `playrate`: two regions over one recording may
    #: loop and not loop.
    looping: bool = False

    @classmethod
    def onto(cls, window: dict, *, playrate: float = 1.0,
             args: "dict | None" = None, looping: bool = False) -> "Content":
        """A window onto a source: `{"source": ..., "start": ..., "duration":
        ...}`, the source being one of the session's table.

        `duration` is how much of the source the window **reaches** -- the whole
        take, or the sum of a join's segments -- and not how much the region
        shows: the region's own `length` says that, and a trim that hides part
        of the source leaves `duration` alone so the edge can be pulled back."""
        return cls(fill="window", window=window, playrate=playrate, args=args,
                   looping=looping)

    def write(self) -> dict:
        if self.fill == "window":
            out: dict = {"fill": "window", "window": self.window}
            if self.playrate != 1.0:
                out["playrate"] = self.playrate
            if self.args is not None:
                out["args"] = self.args
            if self.looping:
                out["loop"] = True
            return out
        # A fill this build does not know, carried whole.
        return dict(self.other or {})

    @classmethod
    def read(cls, written: dict) -> "Content":
        fill = written.get("fill")
        if fill == "window":
            return cls(fill="window", window=written.get("window"),
                       playrate=float(written.get("playrate", 1.0)),
                       args=written.get("args"),
                       looping=bool(written.get("loop", False)))
        return cls(fill=str(fill), other=dict(written))


def crate_points(points, curve=None) -> list:
    """A break-point list as the **document's** points: ``{"at", "value",
    "data"}``, with the segment's shape in the point's own ``data``.

    The one place the two vocabularies meet in this direction. Reads every
    spelling `clausters.defs.ugens.quads` does, curve names included, and the
    crate carries the ``data`` without ever reading it -- which is what lets a
    shape survive an undo instead of coming back straight.
    """
    from ..defs.ugens import quads

    return [{"at": at, "value": value,
             "data": {"shape": shape, "curve": curvature}}
            for at, value, shape, curvature in quads(points, curve)]


def flat_points(points) -> list:
    """The document's points back as the flat ``[t, v, shape, curve, ...]``
    quads the ``bpf`` view and a curve both speak -- the other direction.

    A point that says nothing about its segment is linear, which is what a curve
    drawn somewhere that has no shapes means.
    """
    out: list = []
    for point in points:
        data = point.get("data") or {}
        out += [float(point.get("at", 0.0)), float(point.get("value", 0.0)),
                int(data.get("shape", 1)), float(data.get("curve", 0.0))]
    return out


def _points(points) -> list:
    """Points as the document writes them: a ``{"at", "value", ...}`` dict is
    taken as it is, and an ``(at, value)`` pair becomes one."""
    out = []
    for point in points:
        if isinstance(point, dict):
            out.append(dict(point))
        else:
            at, value = point
            out.append({"at": float(at), "value": float(value)})
    return out


class Automation:
    """A curve over one parameter, in its holder's time.

    `target` says **what this automates** in the client's terms and is never
    read here -- a control name, a bus, a plugin's parameter index -- the same
    door a leaf's configuration is, and for the same reason. The points are
    `{"at": t, "value": v, "data": ...}`, the shape `clausters.document`'s
    points vocabulary already carries; ``(at, value)`` pairs are read as
    such points. What ``at`` counts is the holder's: seconds on a `Track`,
    seconds from the region's start on a `Region`, beats on a
    `clausters.seq.EventSequence`, beats from the note's start on one of its
    events.

    **One class, free or held.** Built by a script, a curve is a **value**
    that nothing holds. Added to a holder -- a sequence or one of its events
    (``seq.automation.add``, ``event.automation.add``), a track or a region of
    a multitrack (``track.automation.add``, ``region.automation.add``) -- it is
    a **live view** of the curve the holder keeps: reading a field asks the
    holder, so it reads what an editor left there, writing one writes the
    curve back, and the same curve read twice is the same object. A curve its
    holder no longer keeps -- removed, or undone away -- is **detached**:
    `held` is ``False`` and reading it raises `ValueError`, until an undo
    brings it back.
    """

    _FIELDS = ("id", "target", "name", "points", "visible", "enabled", "extra")

    def __init__(self, target=None, points=(), name: "str | None" = None, *,
                 visible: bool = False, enabled: bool = True,
                 extra: "dict | None" = None, id: int = 0):
        #: ``(holder, scope)`` while something holds it: a sequence and the
        #: event id or ``None``, or a multitrack and ``("track"|"region", id)``.
        self._holder = None
        self._id = int(id)
        self._value = {"target": target, "name": name, "points": _points(points),
                       "visible": bool(visible), "enabled": bool(enabled),
                       "extra": dict(extra or {})}

    @classmethod
    def _held_by(cls, holder, scope, id: int) -> "Automation":
        """The view of curve ``id`` that ``holder`` keeps in ``scope``."""
        curve = cls.__new__(cls)
        curve._bind(holder, scope, id)
        return curve

    def _bind(self, holder, scope, id: int) -> None:
        """Become the view of curve ``id`` the holder now keeps."""
        self._holder = (holder, scope)
        self._id = int(id)
        self._value = None

    # ---- the fields ----

    def _written(self) -> dict:
        """The curve as its holder writes it: the value, or what the sequence
        holds now."""
        if self._holder is None:
            out = {"id": self._id, **self._own()}
            out["points"] = list(out["points"])
            return out
        holder, scope = self._holder
        written = holder._curve(scope, self._id)
        if written is None:
            raise ValueError("its holder no longer keeps this curve")
        return written

    @property
    def _history_owner(self):
        """The structure that holds the curve -- a sequence, a multitrack --
        whose history it shares (`clausters.history.Editing.of`), or ``None``
        for a free value."""
        return None if self._holder is None else self._holder[0]

    @property
    def held(self) -> bool:
        """Whether something holds the curve: ``False`` for a free value, and
        for a view whose curve was removed."""
        if self._holder is None:
            return False
        try:
            self._written()
        except ValueError:
            return False
        return True

    @property
    def id(self) -> int:
        """The curve's identity in its holder; ``0`` for a value nothing holds
        yet."""
        return self._id

    @id.setter
    def id(self, value: int) -> None:
        if self._holder is not None:
            raise AttributeError("a held curve keeps the id its holder gave it")
        self._id = int(value)

    def _own(self) -> dict:
        """The value of a curve no holder keeps: it holds its own."""
        assert self._value is not None, "a curve with no holder holds its value"
        return self._value

    def _field(self, name: str) -> Any:
        written = self._written()
        defaults = {"target": None, "name": None, "points": [], "visible": False,
                    "enabled": True}
        if name == "extra":
            if self._holder is None:
                return written["extra"]
            return {k: v for k, v in written.items() if k not in self._FIELDS}
        return written.get(name, defaults[name])

    def _write(self, name: str, value) -> None:
        """Write one field: into the value, or -- for a held curve -- the
        whole curve back into its holder, which keeps its id."""
        if self._holder is None:
            self._own()[name] = value
            return
        written = dict(self._written())
        if name == "extra":
            written = {k: v for k, v in written.items() if k in self._FIELDS}
            written.update(value)
        else:
            written[name] = value
        holder, scope = self._holder
        holder._write_curve(scope, written, "edit a curve")

    def remove(self) -> None:
        """Remove the curve from what holds it. This object is left detached,
        and an undo that brings the curve back brings it back too. A free curve
        is held by nothing, so there is nothing to remove it from:
        `ValueError`."""
        if self._holder is None:
            raise ValueError("a free curve is held by nothing")
        self._written()
        holder, scope = self._holder
        holder._remove_curve(scope, self._id)

    @property
    def target(self):
        """What the curve drives, in its holder's vocabulary."""
        return self._field("target")

    @target.setter
    def target(self, value) -> None:
        self._write("target", value)

    @property
    def name(self) -> "str | None":
        """What a reader calls the curve."""
        return self._field("name")

    @name.setter
    def name(self, value: "str | None") -> None:
        self._write("name", value)

    @property
    def points(self) -> list:
        """The points, ``{"at", "value", "data"}``, in order. A copy: change a
        curve by assigning its points, or with `set_points`."""
        return list(self._field("points"))

    @points.setter
    def points(self, value) -> None:
        self._write("points", _points(value))

    @property
    def visible(self) -> bool:
        """Whether the curve is shown. The **view's**, and kept here because
        which curves a person had open is part of reopening the work as they
        left it."""
        return bool(self._field("visible"))

    @visible.setter
    def visible(self, value: bool) -> None:
        self._write("visible", bool(value))

    @property
    def enabled(self) -> bool:
        """Whether the curve is being applied. A curve can be kept and switched
        off without being deleted, which is what an arm or a bypass is."""
        return bool(self._field("enabled"))

    @enabled.setter
    def enabled(self, value: bool) -> None:
        self._write("enabled", bool(value))

    @property
    def extra(self) -> dict:
        """Fields a newer writer wrote, carried as they are."""
        return dict(self._field("extra"))

    @extra.setter
    def extra(self, value: dict) -> None:
        self._write("extra", dict(value))

    # ---- the curve protocol ----

    def to_points(self) -> list:
        """The curve as the flat ``[t, v, shape, curve, ...]`` break points the
        ``bpf`` view and a ``"points"`` event speak -- the curve protocol
        `clausters.gui.edit` opens a curve by, shared with
        `clausters.defs.ugens.Env` and `clausters.defs.ugens.Bpf`.

        A point that says nothing about its segment is linear, which is what a
        curve drawn somewhere that has no shapes means."""
        return flat_points(self.points)

    def set_points(self, points, curve=None) -> "Automation":
        """Take the curve's break points from a break-point list, in place --
        the other half of `to_points`, and what an editor writes back through.
        Reads every spelling `clausters.defs.ugens.quads` does, curve names
        included.

        The shape of each segment travels in the point's own ``data``, which the
        document crate carries and never reads."""
        self.points = crate_points(points, curve)
        return self

    # ---- as a channel's curve ----

    def play(self, destination=None) -> "Automation":
        """**Play it as a channel's curve** on ``destination`` (the ambient
        server when ``None``): from now on it sets the control its `target`
        names on the notes of its channel -- ``{"control": "amp", "channel":
        0}``, or every channel without one -- those sounding and those to
        come, its points counted in beats from this moment, and it holds its
        last value past its end (`clausters.defs.Server.play_automation`).

        The timeline-item protocol, so a curve is placed on a
        `clausters.seq.Timeline` like an event. A note's own curve is not
        played: it is the event's (``Event(automation=[curve])``). Returns the
        curve, whose `stop` ends it."""
        if destination is None:
            from ..base.main import main

            destination = main.resolve_server()
        handler = getattr(destination, "play_automation", None)
        if handler is None:
            raise TypeError(
                f"a curve plays on a server, where it sets a control of the notes "
                f"of its channel; {type(destination).__name__} does not play one")
        if self.enabled:
            self.stop()
            self._playing = (destination, handler(self))
        return self

    def stop(self) -> None:
        """Stop it as a channel's curve: the notes it reached keep the last
        value it set. Nothing when it is not playing."""
        playing, self._playing = getattr(self, "_playing", None), None
        if playing is not None and playing[1] is not None:
            playing[0].curves.stop(playing[1])

    # ---- as data ----

    def write(self) -> dict:
        written = self._written()
        out: dict = {"id": self._id}
        if written.get("name") is not None:
            out["name"] = written["name"]
        if written.get("target") is not None:
            out["target"] = written["target"]
        if written.get("points"):
            out["points"] = list(written["points"])
        if written.get("visible"):
            out["visible"] = True
        if written.get("enabled") is False:
            out["enabled"] = False
        out.update(self._field("extra"))
        return out

    @classmethod
    def read(cls, written: dict) -> "Automation":
        known = ("id", "name", "target", "points", "visible", "enabled")
        return cls(
            id=int(written["id"]),
            target=written.get("target"),
            name=written.get("name"),
            points=list(written.get("points", [])),
            visible=bool(written.get("visible", False)),
            enabled=bool(written.get("enabled", True)),
            extra=_unknown_fields(written, *known),
        )

    def __eq__(self, other) -> bool:
        if not isinstance(other, Automation):
            return NotImplemented
        if self._holder is not None or other._holder is not None:
            return self is other
        return self.write() == other.write()

    __hash__ = object.__hash__

    def __repr__(self) -> str:
        if self._holder is not None and not self.held:
            return "<Automation, detached>"
        written = self._written()
        name = f" {written['name']!r}" if written.get("name") else ""
        count = len(written.get("points") or ())
        return (f"<Automation{name} {written.get('target')!r}, "
                f"{count} point{'' if count == 1 else 's'}>")


class _Held:
    """**An object that stands for one structure of a multitrack**: the
    multitrack and the structure's id, never a copy. Reading a field asks the
    multitrack, so it reads what an editor left there; writing one is an edit
    in the multitrack's vocabulary. The same structure read twice is the same
    object (`Multitrack` keeps an identity map), and one the multitrack no
    longer holds -- removed, or undone away -- is **detached**: `held` is
    ``False`` and reading it raises `ValueError`, until an undo brings it
    back."""

    __slots__ = ("_multitrack", "_id", "__weakref__")

    #: The door's verb that reads one of these, and what it is called.
    _verb = ""
    _noun = ""

    @classmethod
    def _of(cls, multitrack, id: int):
        held = cls.__new__(cls)
        held._multitrack = multitrack
        held._id = int(id)
        return held

    def _found(self) -> dict:
        """What the door answers for this structure; `ValueError` when the
        multitrack no longer holds it."""
        found = self._multitrack._mt.call(self._verb, id=self._id)
        if found is None:
            raise ValueError(f"the multitrack no longer holds this {self._noun}")
        return found

    @property
    def held(self) -> bool:
        """Whether the multitrack still holds it."""
        return self._multitrack._mt.call(self._verb, id=self._id) is not None

    @property
    def multitrack(self) -> "Multitrack":
        """The multitrack it belongs to."""
        return self._multitrack

    @property
    def _history_owner(self):
        """The multitrack, whose history it shares
        (`clausters.history.Editing.of`)."""
        return self._multitrack


class Region(_Held):
    """**One placed thing on a take lane**: a span of the timeline, and what
    fills it -- a view of a region the multitrack holds, made by
    ``lane.regions.add``.

    `position` and `length` are the region's own, in seconds. They are **not**
    the content's: a region may show part of what it holds, and trimming moves
    these without touching the source. Each field is written through the
    multitrack's own verb: the position and the layer are where it is placed,
    the length and the content what it shows, the fades its fades, and the rest
    a rewrite of its take lane.
    """

    __slots__ = ()
    _verb = "region"
    _noun = "region"

    def _region(self) -> dict:
        return self._found()["region"]

    @property
    def take_lane(self) -> "TakeLane":
        """The take lane it sits on."""
        return self._multitrack._view(TakeLane, self._found()["takeLane"])

    @property
    def track(self) -> "Track":
        """The track it belongs to."""
        return self._multitrack._view(Track, self._found()["track"])

    @property
    def position(self) -> float:
        """Where it starts on the timeline, in seconds."""
        return float(self._region().get("position", 0.0))

    @position.setter
    def position(self, value: float) -> None:
        self.place(position=value)

    @property
    def layer(self) -> int:
        """Which of the overlapping regions on its take lane is on top --
        higher is nearer the front."""
        return int(self._region().get("layer", 0))

    @layer.setter
    def layer(self, value: int) -> None:
        self.place(layer=value)

    def place(self, take_lane: "TakeLane | None" = None, *,
              position: "float | None" = None, layer: "int | None" = None) -> None:
        """**Place it**: on ``take_lane`` -- of this track or of another --
        at ``position``, on ``layer``, each left as it is when not given. One
        edit, whatever moved, so it undoes in one step."""
        found = self._found()
        region = found["region"]
        lane = found["takeLane"] if take_lane is None else take_lane._id
        track = found["track"] if take_lane is None else take_lane._found()["track"]
        self._multitrack._edit({
            "intent": "placeregion", "region": self._id, "track": int(track),
            "take_lane": int(lane),
            "position": float(region.get("position", 0.0) if position is None else position),
            "layer": int(region.get("layer", 0) if layer is None else layer),
        }, "move a region")

    @property
    def length(self) -> float:
        """How long it occupies, in seconds -- not the content's length."""
        return float(self._region().get("length", 0.0))

    @length.setter
    def length(self, value: float) -> None:
        self.trim(length=value)

    @property
    def content(self) -> Content:
        """What fills it, as a value: change it by assigning one."""
        return Content.read(self._region().get("content") or {})

    @content.setter
    def content(self, value: Content) -> None:
        self.trim(content=value)

    def trim(self, *, position: "float | None" = None, length: "float | None" = None,
             content: "Content | None" = None) -> None:
        """**How much of it shows, and from where**: a right-hand trim moves the
        length, a left-hand one the position, the length and the window into the
        source -- which is why the content is part of it. What is not given is
        left as it is."""
        region = self._region()
        intent = {"intent": "trimregion", "region": self._id,
                  "position": float(region.get("position", 0.0) if position is None else position),
                  "length": float(region.get("length", 0.0) if length is None else length)}
        if content is not None:
            intent["content"] = content.write()
        self._multitrack._edit(intent, "trim a region")

    @property
    def fade_in(self) -> "Fade | None":
        """Its fade in, as a value, or ``None``."""
        fade = self._region().get("fade_in")
        return None if fade is None else Fade.read(fade)

    @fade_in.setter
    def fade_in(self, value: "Fade | None") -> None:
        self._fades(value, self.fade_out)

    @property
    def fade_out(self) -> "Fade | None":
        """Its fade out, as a value, or ``None``."""
        fade = self._region().get("fade_out")
        return None if fade is None else Fade.read(fade)

    @fade_out.setter
    def fade_out(self, value: "Fade | None") -> None:
        self._fades(self.fade_in, value)

    def _fades(self, fade_in, fade_out) -> None:
        intent = {"intent": "faderegion", "region": self._id}
        if fade_in is not None:
            intent["fade_in"] = fade_in.write()
        if fade_out is not None:
            intent["fade_out"] = fade_out.write()
        self._multitrack._edit(intent, "fade a region")

    @property
    def name(self) -> "str | None":
        """What a reader calls it -- a label, never a second identity."""
        return self._region().get("name")

    @name.setter
    def name(self, value: "str | None") -> None:
        self._write("name", value, "rename a region")

    @property
    def muted(self) -> bool:
        """Silenced without being removed. The region's own, not its track's."""
        return bool(self._region().get("muted", False))

    @muted.setter
    def muted(self, value: bool) -> None:
        self._write("muted", bool(value), "mute a region")

    @property
    def extra(self) -> dict:
        """Fields a newer writer wrote, carried as they are."""
        return _unknown_fields(self._region(), *_REGION_FIELDS)

    @property
    def automation(self) -> "Curves":
        """**The curves that act on this placement alone**, as a live
        collection: drawn inside the region, their points in seconds from its
        start."""
        return Curves(self._multitrack, ("region", self._id))

    @property
    def end(self) -> float:
        """Where it ends: its position plus its length."""
        region = self._region()
        return float(region.get("position", 0.0)) + float(region.get("length", 0.0))

    def overlaps(self, other: "Region") -> bool:
        """Whether the two occupy any of the same time.

        Half-open, so a region ending exactly where the next begins does not
        overlap it -- which is what makes a cut into two regions not a crossfade.
        """
        return self.position < other.end and other.position < self.end

    def _write(self, field: str, value, label: str) -> None:
        """One field written by rewriting its take lane, which keeps every
        region's identity."""
        lane = self._found()["takeLane"]
        regions = self._multitrack._mt.call("takeLane", id=int(lane))["takeLane"].get("regions", [])
        for region in regions:
            if int(region["id"]) == self._id:
                if value is None:
                    region.pop(field, None)
                else:
                    region[field] = value
        self._multitrack._edit({"intent": "settakelane", "take_lane": int(lane),
                                "regions": regions}, label)

    def remove(self) -> None:
        """Take it off its take lane. This object is left detached, and an undo
        that brings the region back brings it back too."""
        lane = self._found()["takeLane"]
        regions = self._multitrack._mt.call("takeLane", id=int(lane))["takeLane"].get("regions", [])
        self._multitrack._edit({"intent": "settakelane", "take_lane": int(lane),
                                "regions": [r for r in regions if int(r["id"]) != self._id]},
                               "remove a region")

    def write(self) -> dict:
        """The region as the crate writes it."""
        return dict(self._region())

    def __repr__(self) -> str:
        if not self.held:
            return "<Region, detached>"
        region = self._region()
        name = f" {region['name']!r}" if region.get("name") else ""
        return (f"<Region{name} at {float(region.get('position', 0.0)):g} s, "
                f"{float(region.get('length', 0.0)):g} s>")


_REGION_FIELDS = ("id", "position", "length", "content", "name", "layer",
                  "fade_in", "fade_out", "muted", "automation")


class Regions:
    """**The regions a take lane holds, as a live collection**, in position
    order: iterate it, index it, and ``add`` one."""

    __slots__ = ("_lane",)

    def __init__(self, lane: "TakeLane"):
        self._lane = lane

    def _ids(self) -> list:
        ids = self._lane._multitrack._ids("regions", takeLane=self._lane._id)
        if ids is None:
            raise ValueError("the multitrack no longer holds this take lane")
        return ids

    def __len__(self) -> int:
        return len(self._ids())

    def __iter__(self):
        return iter([self._lane._multitrack._view(Region, i) for i in self._ids()])

    def __getitem__(self, i):
        ids = self._ids()
        if isinstance(i, slice):
            return [self._lane._multitrack._view(Region, n) for n in ids[i]]
        return self._lane._multitrack._view(Region, ids[i])

    def add(self, position: float, length: float, content: Content, *,
            name: "str | None" = None, layer: int = 0, fade_in: "Fade | None" = None,
            fade_out: "Fade | None" = None, muted: bool = False) -> Region:
        """**Place a region** at ``position`` for ``length`` seconds, filled with
        ``content``, and answer it. The multitrack names it."""
        multitrack = self._lane._multitrack
        (id,) = multitrack._mint(1)
        written: dict = {"id": id, "position": float(position), "length": float(length),
                         "content": content.write()}
        if name is not None:
            written["name"] = str(name)
        if layer:
            written["layer"] = int(layer)
        if fade_in is not None:
            written["fade_in"] = fade_in.write()
        if fade_out is not None:
            written["fade_out"] = fade_out.write()
        if muted:
            written["muted"] = True
        regions = self._lane._lane().get("regions", [])
        multitrack._edit({"intent": "settakelane", "take_lane": self._lane._id,
                          "regions": [*regions, written]}, "add a region")
        return multitrack._view(Region, id)

    def __repr__(self) -> str:
        return f"<{len(self)} regions>"


class TakeLane(_Held):
    """**One of a track's several contents**: an ordered list of regions -- a
    view of a take lane the multitrack holds, made by ``track.take_lanes.add``
    (a track starts with one).
    """

    __slots__ = ()
    _verb = "takeLane"
    _noun = "take lane"

    def _lane(self) -> dict:
        return self._found()["takeLane"]

    @property
    def track(self) -> "Track":
        """The track that holds it."""
        return self._multitrack._view(Track, self._found()["track"])

    @property
    def name(self) -> "str | None":
        """What a reader calls it."""
        return self._lane().get("name")

    @name.setter
    def name(self, value: "str | None") -> None:
        track = self._found()["track"]
        self._multitrack._rewrite_track(track, lambda t: _set_lane_field(
            t, self._id, "name", value), "rename a take lane")

    @property
    def regions(self) -> Regions:
        """Its regions, as a live collection in position order."""
        return Regions(self)

    @property
    def end(self) -> float:
        """Where its last region ends, or zero when there are none."""
        return max((float(r.get("position", 0.0)) + float(r.get("length", 0.0))
                    for r in self._lane().get("regions", [])), default=0.0)

    def remove(self) -> None:
        """Take it off its track, with its regions. This object is left
        detached."""
        track = self._found()["track"]
        self._multitrack._rewrite_track(track, lambda t: t.update(
            take_lanes=[lane for lane in t.get("take_lanes", [])
                        if int(lane["id"]) != self._id]), "remove a take lane")

    def write(self) -> dict:
        """The take lane as the crate writes it."""
        return dict(self._lane())

    def __repr__(self) -> str:
        if not self.held:
            return "<TakeLane, detached>"
        lane = self._lane()
        name = f" {lane['name']!r}" if lane.get("name") else ""
        return f"<TakeLane{name}, {len(lane.get('regions', []))} regions>"


def _set_lane_field(track: dict, lane: int, field: str, value) -> None:
    for written in track.get("take_lanes", []):
        if int(written["id"]) == lane:
            if value is None:
                written.pop(field, None)
            else:
                written[field] = value


class TakeLanes:
    """**The take lanes a track holds, as a live collection**: iterate it,
    index it, and ``add`` one."""

    __slots__ = ("_track",)

    def __init__(self, track: "Track"):
        self._track = track

    def _ids(self) -> list:
        ids = self._track._multitrack._ids("takeLanes", track=self._track._id)
        if ids is None:
            raise ValueError("the multitrack no longer holds this track")
        return ids

    def __len__(self) -> int:
        return len(self._ids())

    def __iter__(self):
        return iter([self._track._multitrack._view(TakeLane, i) for i in self._ids()])

    @overload
    def __getitem__(self, i: int) -> TakeLane: ...

    @overload
    def __getitem__(self, i: slice) -> "list[TakeLane]": ...

    def __getitem__(self, i):
        ids = self._ids()
        if isinstance(i, slice):
            return [self._track._multitrack._view(TakeLane, n) for n in ids[i]]
        return self._track._multitrack._view(TakeLane, ids[i])

    def add(self, name: "str | None" = None) -> TakeLane:
        """**Add an empty take lane** under the others, and answer it."""
        multitrack = self._track._multitrack
        (id,) = multitrack._mint(1)
        written: dict = {"id": id}
        if name is not None:
            written["name"] = str(name)
        multitrack._rewrite_track(self._track._id, lambda t: t.update(
            take_lanes=[*t.get("take_lanes", []), written]), "add a take lane")
        return multitrack._view(TakeLane, id)

    def __repr__(self) -> str:
        return f"<{len(self)} take lanes>"


class Track(_Held):
    """**A row of the multitrack**: several take lanes, one of them playing, the
    curves over it, and whatever the client says it is -- a view of a track the
    multitrack holds, made by ``mt.tracks.add``.

    **What a track *is* -- an instrument, a bus, a folder -- is not here.** That
    is `config`, carried and never interpreted, for the reason a leaf is opaque:
    a def is code in the language of whoever wrote it. What the document owns is
    the structure: which take lanes, which one plays, what is placed on them.
    """

    __slots__ = ()
    _verb = "track"
    _noun = "track"

    def _field(self, name: str, default):
        return self._found().get(name, default)

    def _set(self, name: str, value, label: str) -> None:
        def write(track: dict) -> None:
            if value is None:
                track.pop(name, None)
            else:
                track[name] = value
        self._multitrack._rewrite_track(self._id, write, label)

    @property
    def name(self) -> "str | None":
        """What a reader calls it."""
        return self._field("name", None)

    @name.setter
    def name(self, value: "str | None") -> None:
        self._set("name", value, "rename a track")

    @property
    def muted(self) -> bool:
        """Silenced."""
        return bool(self._field("muted", False))

    @muted.setter
    def muted(self, value: bool) -> None:
        self._set("muted", bool(value), "mute a track")

    @property
    def soloed(self) -> bool:
        """Marked as soloed. Whether a solo anywhere silences everything else is
        the mixer's rule and not the document's."""
        return bool(self._field("soloed", False))

    @soloed.setter
    def soloed(self, value: bool) -> None:
        self._set("soloed", bool(value), "solo a track")

    @property
    def level(self) -> float:
        """Where its fader is, as a linear gain."""
        return float(self._field("level", 1.0))

    @level.setter
    def level(self, value: float) -> None:
        self._set("level", float(value), "set a track's level")

    @property
    def channels(self) -> int:
        """How wide it is, in channels -- two unless it says otherwise."""
        return int(self._field("channels", 2))

    @channels.setter
    def channels(self, value: int) -> None:
        self._set("channels", int(value), "set a track's width")

    @property
    def config(self) -> "dict | None":
        """What the client says the track is, carried and never read."""
        return self._field("config", None)

    @config.setter
    def config(self, value: "dict | None") -> None:
        self._set("config", value, "configure a track")

    @property
    def extra(self) -> dict:
        """Fields a newer writer wrote, carried as they are."""
        return _unknown_fields(self._found(), *_TRACK_FIELDS)

    @property
    def take_lanes(self) -> TakeLanes:
        """Its take lanes, as a live collection."""
        return TakeLanes(self)

    @property
    def active(self) -> int:
        """Which take lane plays, as an index into `take_lanes`. Setting it is
        comping's one verb."""
        return int(self._field("active", 0))

    @active.setter
    def active(self, index: int) -> None:
        self.active_take_lane = self.take_lanes[int(index)]

    @property
    def active_take_lane(self) -> "TakeLane | None":
        """The take lane that plays, or ``None`` when `active` names one that
        is not there."""
        ids = self.take_lanes._ids()
        at = self.active
        return self._multitrack._view(TakeLane, ids[at]) if 0 <= at < len(ids) else None

    @active_take_lane.setter
    def active_take_lane(self, lane: TakeLane) -> None:
        self._multitrack._edit({"intent": "setactivetakelane", "track": self._id,
                                "take_lane": lane._id}, "choose a take")

    @property
    def automation(self) -> "Curves":
        """**The curves over the track**, as a live collection: drawn in rows
        beside it, their points in the multitrack's seconds."""
        return Curves(self._multitrack, ("track", self._id))

    @property
    def end(self) -> float:
        """Where its last region ends, across **every** take lane -- what it
        spans rather than what it plays, since an alternate take is still part
        of the multitrack."""
        return max((float(r.get("position", 0.0)) + float(r.get("length", 0.0))
                    for lane in self._found().get("take_lanes", [])
                    for r in lane.get("regions", [])), default=0.0)

    def remove(self) -> None:
        """Take it out of the multitrack, with everything on it. This object is
        left detached."""
        tracks = self._multitrack._tracks_written()
        self._multitrack._edit({"intent": "settracks", "tracks": [
            t for t in tracks if int(t["id"]) != self._id]}, "remove a track")

    def write(self) -> dict:
        """The track as the crate writes it."""
        return dict(self._found())

    def __repr__(self) -> str:
        if not self.held:
            return "<Track, detached>"
        track = self._found()
        name = f" {track['name']!r}" if track.get("name") else ""
        return f"<Track{name}, {len(track.get('take_lanes', []))} take lanes>"


_TRACK_FIELDS = ("id", "name", "take_lanes", "active", "automation", "muted",
                 "soloed", "level", "channels", "config")


class Tracks:
    """**The multitrack's tracks, as a live collection** in the order shown:
    iterate it, index it, and ``add`` one."""

    __slots__ = ("_multitrack",)

    def __init__(self, multitrack: "Multitrack"):
        self._multitrack = multitrack

    def _ids(self) -> list:
        return self._multitrack._ids("tracks") or []

    def __len__(self) -> int:
        return len(self._ids())

    def __iter__(self):
        return iter([self._multitrack._view(Track, i) for i in self._ids()])

    def __getitem__(self, i):
        ids = self._ids()
        if isinstance(i, slice):
            return [self._multitrack._view(Track, n) for n in ids[i]]
        return self._multitrack._view(Track, ids[i])

    def add(self, name: "str | None" = None, *, muted: bool = False,
            soloed: bool = False, level: float = 1.0, channels: int = 2,
            config: "dict | None" = None) -> Track:
        """**Add a track** under the others, with one empty take lane, and
        answer it."""
        track_id, lane_id = self._multitrack._mint(2)
        written: dict = {"id": track_id, "take_lanes": [{"id": lane_id}]}
        if name is not None:
            written["name"] = str(name)
        if muted:
            written["muted"] = True
        if soloed:
            written["soloed"] = True
        if level != 1.0:
            written["level"] = float(level)
        if channels != 2:
            written["channels"] = int(channels)
        if config is not None:
            written["config"] = config
        self._multitrack._edit({"intent": "settracks", "tracks": [
            *self._multitrack._tracks_written(), written]}, "add a track")
        return self._multitrack._view(Track, track_id)

    def __repr__(self) -> str:
        return f"<{len(self)} tracks>"


class Marker(_Held):
    """**A named point on the timeline**, at a second -- a view of a marker the
    multitrack holds, made by ``mt.markers.add``."""

    __slots__ = ()
    _verb = "marker"
    _noun = "marker"

    @property
    def at(self) -> float:
        """Where it is, in seconds."""
        return float(self._found().get("at", 0.0))

    @at.setter
    def at(self, value: float) -> None:
        self._set(float(value), self.name)

    @property
    def name(self) -> "str | None":
        """What it is called."""
        return self._found().get("name")

    @name.setter
    def name(self, value: "str | None") -> None:
        self._set(self.at, value)

    def _set(self, at: float, name) -> None:
        intent = {"intent": "setmarker", "marker": self._id, "at": float(at)}
        if name is not None:
            intent["name"] = str(name)
        self._multitrack._edit(intent, "set a marker")

    def remove(self) -> None:
        """Take it off the timeline. This object is left detached."""
        self._found()
        self._multitrack._edit({"intent": "removemarker", "marker": self._id},
                               "remove a marker")

    def write(self) -> dict:
        """The marker as the crate writes it."""
        return dict(self._found())

    def __repr__(self) -> str:
        if not self.held:
            return "<Marker, detached>"
        found = self._found()
        name = f" {found['name']!r}" if found.get("name") else ""
        return f"<Marker{name} at {float(found.get('at', 0.0)):g} s>"


class Markers:
    """**The multitrack's markers, as a live collection** in position order:
    iterate it, index it, and ``add`` one. Several may share an instant: unlike
    a tempo, two names for one moment is a thing people do."""

    __slots__ = ("_multitrack",)

    def __init__(self, multitrack: "Multitrack"):
        self._multitrack = multitrack

    def _ids(self) -> list:
        return self._multitrack._ids("markers") or []

    def __len__(self) -> int:
        return len(self._ids())

    def __iter__(self):
        return iter([self._multitrack._view(Marker, i) for i in self._ids()])

    def __getitem__(self, i):
        ids = self._ids()
        if isinstance(i, slice):
            return [self._multitrack._view(Marker, n) for n in ids[i]]
        return self._multitrack._view(Marker, ids[i])

    def add(self, at: float, name: "str | None" = None) -> Marker:
        """**Place a marker** at ``at`` seconds, and answer it."""
        (id,) = self._multitrack._mint(1)
        intent = {"intent": "setmarker", "marker": id, "at": float(at)}
        if name is not None:
            intent["name"] = str(name)
        self._multitrack._edit(intent, "add a marker")
        return self._multitrack._view(Marker, id)

    def __repr__(self) -> str:
        return f"<{len(self)} markers>"


class Curves:
    """**Curves a track or a region holds, as a live collection** of
    `Automation` views: iterate it, index it, and ``add`` one."""

    __slots__ = ("_multitrack", "_scope")

    def __init__(self, multitrack: "Multitrack", scope: tuple):
        self._multitrack = multitrack
        self._scope = scope

    def _ids(self) -> list:
        kind, id = self._scope
        ids = self._multitrack._ids("automation", **{kind: id})
        if ids is None:
            raise ValueError(f"the multitrack no longer holds this {kind}")
        return ids

    def __len__(self) -> int:
        return len(self._ids())

    def __iter__(self):
        return iter([self._multitrack._curve_view(self._scope, i) for i in self._ids()])

    def __getitem__(self, i):
        ids = self._ids()
        if isinstance(i, slice):
            return [self._multitrack._curve_view(self._scope, n) for n in ids[i]]
        return self._multitrack._curve_view(self._scope, ids[i])

    def add(self, target, points=(), name: "str | None" = None, *,
            visible: bool = False, enabled: bool = True) -> "Automation":
        """**Add a curve** and answer it, held: ``target`` says what it moves,
        in the client's terms (``{"port": "gain"}``), ``points`` are ``(second,
        value)`` pairs or the document's points, ``name`` labels it. Or
        ``target`` is a free `Automation`, which is added as it is and becomes
        the view."""
        if isinstance(target, Automation):
            curve = target
            if curve._holder is not None:
                raise ValueError("this curve is held already: add a copy of it "
                                 "(Automation.read(curve.write()))")
            written = curve.write()
        else:
            curve = None
            written = {"target": target, "points": _points(points)}
            if name is not None:
                written["name"] = str(name)
            if visible:
                written["visible"] = True
            if not enabled:
                written["enabled"] = False
        (id,) = self._multitrack._mint(1)
        written["id"] = id
        self._multitrack._add_curve(self._scope, written)
        if curve is None:
            return self._multitrack._curve_view(self._scope, id)
        curve._bind(self._multitrack, self._scope, id)
        self._multitrack._objects[("curve", id)] = curve
        return curve

    def __repr__(self) -> str:
        return f"<{len(self)} curves>"


class Multitrack:
    """**The tracks, and the timeline they are placed on** -- a handle over the
    multitrack the shared crate holds, which is the window's while the
    multitrack application has it open (`clausters.gui.edit`).

    What is here rather than on a track is what the **multitrack** has one of: the
    tempo map, the meter map, the markers, the loop. A track has none of them
    and never disagrees with another track about them, which is the whole
    argument for where they live.

    **What a script reads and writes is objects**: `tracks`, a track's
    `Track.take_lanes`, a take lane's `TakeLane.regions`, the curves of a track
    or a region and the `markers` are live collections whose ``add`` answers the
    object it made, and every object is a view of what the multitrack holds --
    so a change is made through the object it changes (``region.position =
    2.0``, ``track.muted = True``, ``region.remove()``), and no call takes or
    answers an id. A tempo entry, a meter entry, a span, a fade and a region's
    content have no identity of their own: they are read as values and written
    whole through what holds them.

    Args:
        channels: how wide the multitrack is -- the master's own width.
    """

    def __init__(self, *, channels: int = 2):
        data = {} if int(channels) == 2 else {"channels": int(channels)}
        self._mt = _native.MultitrackHandle(data)

    @classmethod
    def read(cls, written: dict) -> "Multitrack":
        """A multitrack from the crate's JSON -- what `write` and a session
        file hold."""
        multitrack = cls.__new__(cls)
        multitrack._mt = _native.MultitrackHandle(dict(written or {}))
        return multitrack

    def write(self) -> dict:
        """The multitrack as the crate's JSON. Nothing said is nothing
        written."""
        return self._mt.call("state")

    # ---- the identity map, and the door ----

    @property
    def _objects(self) -> "weakref.WeakValueDictionary":
        """``(kind, id) ->`` the one object that stands for that structure,
        while anything holds it."""
        objects = self.__dict__.get("_identity")
        if objects is None:
            objects = self.__dict__["_identity"] = weakref.WeakValueDictionary()
        return objects

    def _view(self, cls, id: int):
        """The object of the structure ``id`` is, of class ``cls`` -- the same
        one every time."""
        key = (cls.__name__, int(id))
        found = self._objects.get(key)
        if found is None:
            found = cls._of(self, int(id))
            self._objects[key] = found
        return found

    def _curve_view(self, scope: tuple, id: int) -> "Automation":
        """The object of curve ``id``, held by ``scope`` -- the same one every
        time."""
        found = self._objects.get(("curve", int(id)))
        if found is None:
            found = Automation._held_by(self, scope, int(id))
            self._objects[("curve", int(id))] = found
        return found

    def _ids(self, of: str, **where) -> "list | None":
        ids = self._mt.call("ids", of=of, **where).get("ids")
        return None if ids is None else [int(i) for i in ids]

    def _mint(self, count: int) -> list:
        """``count`` ids nothing in the multitrack names."""
        return [int(i) for i in self._mt.call("mint", count=int(count))["ids"]]

    def _apply(self, intent: dict, *, inverse: bool = True) -> dict:
        """Apply one edit in the multitrack's vocabulary and answer
        ``{"applied", "current"?, "reason"?}``. The door the objects write
        through."""
        return self._mt.call("apply", intent=intent, inverse=bool(inverse))

    def _edit(self, intent: dict, label: str) -> dict:
        """**One change a script makes through an object**: applied, and
        answered as `_apply` answers; `ValueError` with the multitrack's reason
        when it refuses. ``label`` is what an undo would call it.

        A multitrack the multitrack application has open is the window's:
        the change is an edit of the window's, recorded in its history. One
        with a history of this script's -- one a script asked for `history`
        -- takes the change as a turn of it: recorded, and every view over it
        redrawn. One with none just changes."""
        from ..history import ATTR

        window = self._window()
        context = getattr(self, ATTR, None)
        if window is not None:
            answer = window._edit(intent, label)
        elif context is None:
            answer = self._apply(intent, inverse=False)
        else:
            answer = context.script_edit(self, intent, label)
        if not answer.get("applied") and answer.get("reason"):
            raise ValueError(answer["reason"])
        return answer

    def _window(self):
        """The multitrack editor whose window has this multitrack open, or
        ``None``."""
        editor = getattr(self._mt, "_editor", None)
        if editor is None:
            return None
        if editor.closed:
            editor._settle()
            return None
        return editor

    # ---- what a history asks of it ----

    def _script_key(self) -> tuple:
        """The key and the domain it joins a history under -- a multitrack
        editor's, so the two are one structure."""
        return f"multitrack:{id(self)}", _native.MULTITRACK

    def _forward(self, intent: dict, answer: dict) -> dict:
        """The edit a redo applies: the intent itself, which already names
        every identity it makes -- the ids are minted before it is sent."""
        return intent

    @property
    def history(self):
        """**The multitrack's history** (`clausters.history.UndoHistory`): the
        undo order its editors share, made on first ask. From then on every
        change made through the multitrack's objects is an entry of it, and a
        turn the windows over it see; ``with mt.history("tidy"):`` makes
        everything inside it one entry. While the multitrack application has
        it open it is the window's history, which its Ctrl+Z walks."""
        from ..history import UndoHistory

        window = self._window()
        if window is not None:
            return window._history
        return UndoHistory(self)

    def _tracks_written(self) -> list:
        return list(self.write().get("tracks", []))

    def _rewrite_track(self, id: int, change, label: str) -> None:
        """Rewrite one track with ``change`` -- a function over its written
        form -- as the tracks stated whole, which keeps every identity."""
        tracks = self._tracks_written()
        for track in tracks:
            if int(track["id"]) == int(id):
                change(track)
        self._edit({"intent": "settracks", "tracks": tracks}, label)

    # ---- the curve holder ----

    def _curve(self, scope: tuple, id: int) -> "dict | None":
        """Curve ``id`` as written, while ``scope`` holds it."""
        found = self._mt.call("automation", id=int(id))
        if found is None or found.get(scope[0]) != scope[1]:
            return None
        return found["automation"]

    def _curves(self, scope: tuple) -> list:
        kind, id = scope
        if kind == "track":
            return list(self._mt.call("track", id=int(id)).get("automation", []))
        return list(self._mt.call("region", id=int(id))["region"].get("automation", []))

    def _with_curves(self, scope: tuple, change, label: str) -> None:
        """Rewrite the curves ``scope`` holds with ``change``, through the edit
        that states their holder: the tracks for a track's, the take lane for a
        region's."""
        kind, id = scope
        if kind == "track":
            self._rewrite_track(id, lambda t: t.update(
                automation=change(list(t.get("automation", [])))), label)
            return
        found = self._mt.call("region", id=int(id))
        lane = found["takeLane"]
        regions = self._mt.call("takeLane", id=int(lane))["takeLane"].get("regions", [])
        for region in regions:
            if int(region["id"]) == int(id):
                region["automation"] = change(list(region.get("automation", [])))
        self._edit({"intent": "settakelane", "take_lane": int(lane),
                    "regions": regions}, label)

    def _add_curve(self, scope: tuple, written: dict) -> None:
        self._with_curves(scope, lambda curves: [*curves, written], "add a curve")

    def _write_curve(self, scope: tuple, written: dict, label: str) -> int:
        """Write a curve whole and answer its id. Its points alone are the
        multitrack's own curve verb, the one a curve drawn in a row is, and
        whether it is shown alone is the one a menu's check is."""
        current = self._curve(scope, written["id"])

        def without(curve: dict, key: str) -> dict:
            return {k: v for k, v in curve.items() if k != key}

        if current is not None and without(current, "visible") == without(written, "visible"):
            self._edit({"intent": "showautomation", "automation": int(written["id"]),
                        "visible": bool(written.get("visible", False))}, label)
            return int(written["id"])
        if current is not None and without(current, "points") == without(written, "points"):
            self._edit({"intent": "setautomation", "automation": int(written["id"]),
                        "points": list(written.get("points", []))}, label)
            return int(written["id"])
        self._with_curves(scope, lambda curves: [
            written if int(c["id"]) == int(written["id"]) else c for c in curves], label)
        return int(written["id"])

    def _remove_curve(self, scope: tuple, id: int) -> None:
        self._with_curves(scope, lambda curves: [
            c for c in curves if int(c["id"]) != int(id)], "remove a curve")

    # ---- reading ----

    @property
    def version(self) -> int:
        """What this multitrack is *at*, and the whole of what a stale edit is
        stale against."""
        return int(self.write().get("version", FIRST_VERSION))

    @property
    def channels(self) -> int:
        """How wide the multitrack is, in channels -- the master's own width,
        and what a track's output is mixed into."""
        return int(self.write().get("channels", 2))

    @property
    def extra(self) -> dict:
        """Fields a newer writer wrote, carried as they are."""
        return _unknown_fields(self.write(), "version", "tracks", "channels", "tempo", "meter",
                     "markers", "loop_span", "punch")

    @property
    def tracks(self) -> Tracks:
        """The tracks, as a live collection in the order shown."""
        return Tracks(self)

    @property
    def markers(self) -> Markers:
        """The named points, as a live collection in position order."""
        return Markers(self)

    @property
    def end(self) -> float:
        """Where the last region ends, across every track and every take lane --
        how long the multitrack is."""
        return max((float(r.get("position", 0.0)) + float(r.get("length", 0.0))
                    for track in self.write().get("tracks", [])
                    for lane in track.get("take_lanes", [])
                    for r in lane.get("regions", [])), default=0.0)

    def regions(self):
        """Every region, in track then take lane then position order -- **every**
        take lane, not only the ones that play, because an alternate take still
        names the source it plays."""
        for track in self.write().get("tracks", []):
            for lane in track.get("take_lanes", []):
                for region in lane.get("regions", []):
                    yield self._view(Region, int(region["id"]))

    # ---- the timeline's own: the two maps and the two spans ----

    @property
    def tempo(self) -> list:
        """The tempo map's entries, in position order, as values. Change it with
        `set_tempo`."""
        return [Tempo.read(t) for t in self.write().get("tempo", [])]

    @property
    def meter(self) -> list:
        """The meter map's entries, in position order, as values. Change it with
        `set_meter`."""
        return [Meter.read(m) for m in self.write().get("meter", [])]

    def tempo_map(self):
        """The tempo map this multitrack holds, as a `clausters.base.TempoMap`:
        where its beats and bars fall over its seconds, with the reader's
        default of one beat a second where it states no tempo.

        It places nothing -- every position here is already seconds -- and is
        what a ruler draws from and what a script asks to put something on a
        bar. Built afresh on each call, so an edited tempo is the one read.
        """
        from .._native import TempoMap, editing_default_tempo

        return TempoMap.from_changes(
            [{"beats": t.at, "tempo": t.tempo, "ramp": bool(t.ramp)}
             for t in self.tempo],
            editing_default_tempo())

    def tempo_at(self, at: float) -> "Tempo | None":
        """The tempo entry in force at beat `at`, or ``None`` when the map says
        nothing.

        The **entry**, not a converted position; `tempo_map` is the map.
        """
        return next((t for t in reversed(self.tempo) if t.at <= at), None)

    def meter_at(self, at: float) -> "Meter | None":
        """The meter entry in force at `at`, or ``None``."""
        return next((m for m in reversed(self.meter) if m.at <= at), None)

    def set_tempo(self, tempo: Tempo) -> None:
        """Adds a tempo entry, in position order, replacing any already at that
        beat -- two tempos at one position is a state the map should not hold."""
        entries = [t for t in self.tempo if t.at != tempo.at] + [tempo]
        entries.sort(key=lambda t: t.at)
        self._edit({"intent": "settempomap", "tempo": [t.write() for t in entries]},
                   "set the tempo")

    def set_meter(self, meter: Meter) -> None:
        """Adds a meter entry, on the same rule."""
        entries = [m for m in self.meter if m.at != meter.at] + [meter]
        entries.sort(key=lambda m: m.at)
        self._edit({"intent": "setmetermap", "meter": [m.write() for m in entries]},
                   "set the meter")

    @property
    def crossfade(self) -> bool:
        """**Whether an edit by hand that makes two regions overlap crossfades
        them**: on, the edit writes the crossfade into the earlier one's fade
        out and the later one's fade in; off, it writes none and the overlap is
        the two summed. It acts on the edits that follow: turning it off or on
        changes no region. A default of the multitrack, on unless turned off,
        and set like any edit -- recorded, and drawn by every window over it."""
        return bool(self.write().get("defaults", {}).get("crossfade", True))

    @crossfade.setter
    def crossfade(self, on: bool) -> None:
        defaults = dict(self.write().get("defaults", {}))
        defaults["crossfade"] = bool(on)
        self._edit({"intent": "setdefaults", "defaults": defaults},
                   "crossfade overlaps")

    @property
    def default_fade(self) -> "Fade | None":
        """**The fade every region's edge has unless the region states one**:
        10 ms of equal power when nothing was set, and ``None`` for none at all
        in this multitrack, where a region that states nothing meets the
        silence beside it as a step. A region's own `Region.fade_in` and
        `Region.fade_out` win over it; one left at ``None`` follows it, and so
        is an edge the crossfade writes over. Set like any edit."""
        defaults = self.write().get("defaults", {})
        if "fade" not in defaults:
            return Fade(0.010)
        fade = defaults["fade"]
        return None if fade is None else Fade.read(fade)

    @default_fade.setter
    def default_fade(self, fade: "Fade | None") -> None:
        defaults = dict(self.write().get("defaults", {}))
        defaults["fade"] = None if fade is None else fade.write()
        self._edit({"intent": "setdefaults", "defaults": defaults},
                   "set the default fade")

    @property
    def loop_span(self) -> "Span | None":
        """Where the loop is, or ``None``. Whether looping is *on* is the
        transport's; what the multitrack holds is where."""
        span = self.write().get("loop_span")
        return None if span is None else Span.read(span)

    @loop_span.setter
    def loop_span(self, span: "Span | None") -> None:
        self._range("loop", span)

    @property
    def punch(self) -> "Span | None":
        """Where recording punches in and out, or ``None``."""
        span = self.write().get("punch")
        return None if span is None else Span.read(span)

    @punch.setter
    def punch(self, span: "Span | None") -> None:
        self._range("punch", span)

    def _range(self, which: str, span: "Span | None") -> None:
        intent: dict = {"intent": "setrange", "range": which}
        if span is not None:
            intent["span"] = span.write()
        self._edit(intent, f"set the {which}")

    def __repr__(self) -> str:
        return f"<Multitrack, {len(self.tracks)} tracks>"

