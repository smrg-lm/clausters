"""The arrangement: tracks, take lanes, regions, and the timeline they sit on.

This is the client's side of `clausters_document::arrangement` -- the model a
multitrack editor edits, and the one the three classic applications (audio
editor, multitrack editor, score editor) are built over. The crate defines the
format; this module is the idiomatic way to write one and read one back, the
same way `clausters.document` is the idiomatic way to reach an edit.

The vocabulary is the field's own and not this project's invention:

- A **source** is samples. It lives outside the arrangement -- the session's
  table says where -- and is never overwritten.
- A `Region` is **one placed thing**: a span of the timeline (where it starts,
  how long, its fades, which of the overlapping ones is on top) plus a
  `Content` saying what fills it. Six regions over one source are six
  identities and one source, referenced rather than copied. That is the whole
  of non-destructive editing.
- A `TakeLane` is one of a track's several contents, an ordered list of regions.
  Ardour's structure and our name.
- A `Track` holds several take lanes and **plays one**, which is what comping is:
  record six passes into six take lanes, then take from each.
- An `Automation` is a curve over one parameter, in the arrangement's time.
- An `Multitrack` is the tracks plus what the **multitrack** has one of: the tempo
  map, the meter map, the markers, the loop and punch spans. They are here and
  not on a track precisely so that no two tracks can disagree about them.

A region is not a clip
----------------------

`Region` is the model's word; **clip** is the picture's. A clip, a track row, a
waveform are what the host draws; a region is what an edit names. Keeping them
apart is deliberate -- the multitrack's defects came from the thing drawn and the
thing addressed being one object.

Time
----

Everything placed here is placed in **seconds**: a region's position, length
and fades, every automation point, the markers, the loop and the punch. A
multitrack is governed by physical time, the way the server and the clients are,
and no tempo change moves anything in it. What fills a region is measured in its
own source's units -- seconds of a recording, beats of a node -- and the two are
not the same axis. The crate makes that a type; here it is a rule the field
names say (`position` and `length` are the region's, `start` and `duration` are
its window's).

The **tempo map and the meter map** are structures the multitrack holds, not its
axis: their entries are stated at beats, in beats per second, and what reads
them is a ruler drawing beats and bars over the seconds and a snap to them.
`Multitrack.tempo_map` is that map as a `clausters.base.TempoMap`, so a script
that wants a region on bar five asks it where bar five is.

Usage::

    from clausters.multitrack import Multitrack, Region, Track, Tempo

    multitrack = Multitrack()
    multitrack.set_tempo(Tempo(at=0.0, tempo=1.6))     # 96 beats a minute
    bar = multitrack.tempo_map().secs_at(4.0)          # where the second bar begins
    drums = Track(id=1, name="drums", take_lanes=[TakeLane(id=2)])
    drums.active_take_lane.place(Region(id=3, position=bar, length=2.5,
                                   content=Content.window(take)))
    multitrack.tracks.append(drums)
"""

import json
import os
import weakref
from dataclasses import dataclass, field

from . import _native
from .document import FIRST_VERSION, SESSION_FORMAT

__all__ = [
    "Multitrack",
    "Automation",
    "Content",
    "Fade",
    "TakeLane",
    "TakeLaneView",
    "Marker",
    "Meter",
    "Region",
    "FrozenSource",
    "Session",
    "Source",
    "Span",
    "Tempo",
    "Track",
    "TrackView",
    "View",
]


def _rest(written: dict, *known: str) -> dict:
    """Whatever a newer writer wrote and this build has no field for.

    Carried, never read. A reader that dropped it would lose a multitrack the next
    version of this client wrote, which for a format with two writers in two
    languages is not hypothetical.
    """
    return {key: value for key, value in written.items() if key not in known}


@dataclass
class Fade:
    """A fade's length in seconds, and whatever the client says about its curve.

    The shape is carried and never interpreted, for the reason a curve's
    interpolation is not this crate's to name: what an exponential fade *is*
    belongs to whoever renders it. Losing it would straighten every fade on a
    reopen, which is a different act from declining to interpret it.
    """

    length: float
    shape: "dict | None" = None

    def write(self) -> dict:
        out: dict = {"length": self.length}
        if self.shape is not None:
            out["shape"] = self.shape
        return out

    @classmethod
    def read(cls, written: dict) -> "Fade":
        return cls(length=float(written.get("length", 0.0)),
                   shape=written.get("shape"))


@dataclass
class Content:
    """What fills a region: a **window** onto a source, or a **composite** tree.

    Not the clipboard's content, which is what was *copied*. Two nouns in two
    modules, each the right word where it stands: this one is a region's
    ``content`` field, and the format tags it ``fill``.

    Two shapes, held here as one class with a `fill` saying which, because that
    is how the format writes it and a client that mirrored it as a class
    hierarchy would spend an inheritance on a tag.

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
    node: "dict | None" = None
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
        """A window onto a source -- a `clausters.form` segment reference, or any
        `{"source": ..., "start": ..., "duration": ...}` the crate accepts.

        `duration` is how much of the source the window **reaches** -- the whole
        take, or the sum of a join's segments -- and not how much the region
        shows: the region's own `length` says that, and a trim that hides part
        of the source leaves `duration` alone so the edge can be pulled back."""
        return cls(fill="window", window=window, playrate=playrate, args=args,
                   looping=looping)

    @classmethod
    def composite(cls, node: dict) -> "Content":
        """The general tree, placed as one region: a section, a nested
        arrangement, anything the document's primitives can build."""
        return cls(fill="composite", node=node)

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
        if self.fill == "composite":
            return {"fill": "composite", "node": self.node}
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
        if fill == "composite":
            return cls(fill="composite", node=written.get("node"))
        return cls(fill=str(fill), other=dict(written))


def crate_points(points, curve=None) -> list:
    """A break-point list as the **document's** points: ``{"at", "value",
    "data"}``, with the segment's shape in the point's own ``data``.

    The one place the two vocabularies meet in this direction. Reads every
    spelling `clausters.defs.ugens.quads` does, curve names included, and the
    crate carries the ``data`` without ever reading it -- which is what lets a
    shape survive an undo instead of coming back straight.
    """
    from .defs.ugens import quads

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
            out = {"id": self._id, **self._value}
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

    def _field(self, name: str):
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
            self._value[name] = value
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
            from .base.main import main

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
            extra=_rest(written, *known),
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
        return _rest(self._region(), *_REGION_FIELDS)

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
        return _rest(self._found(), *_TRACK_FIELDS)

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


@dataclass
class Tempo:
    """One entry of the tempo map: from here on, this tempo.

    `at` is a beat and `tempo` is beats per **second**, the unit every tempo in
    clausters is in; beats per minute is only how a ruler or a text field may
    show it. `ramp` says the tempo runs from here to the next entry rather than
    stepping. A ritardando is a ramp; a section change is a step.
    """

    at: float
    tempo: float
    ramp: bool = False
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"at": self.at, "tempo": self.tempo}
        if self.ramp:
            out["ramp"] = True
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Tempo":
        return cls(at=float(written.get("at", 0.0)), tempo=float(written["tempo"]),
                   ramp=bool(written.get("ramp", False)),
                   extra=_rest(written, "at", "tempo", "ramp"))


@dataclass
class Meter:
    """One entry of the meter map: from here on, this time signature.

    It says how beats make **bars**, which is what a ruler draws and what a
    snap-to-bar means. It never moves a beat: a meter change does not shift what
    is placed, it re-bars it.
    """

    at: float
    beats: int
    unit: int
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"at": self.at, "beats": self.beats, "unit": self.unit}
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Meter":
        return cls(at=float(written.get("at", 0.0)),
                   beats=int(written["beats"]), unit=int(written["unit"]),
                   extra=_rest(written, "at", "beats", "unit"))


@dataclass
class Span:
    """A span of the timeline: the loop, the punch, a named region of the multitrack.

    Half-open, so two spans that meet cover no instant twice. In seconds.
    """

    start: float
    end: float

    @property
    def length(self) -> float:
        return max(0.0, self.end - self.start)

    def write(self) -> dict:
        return {"start": self.start, "end": self.end}

    @classmethod
    def read(cls, written: dict) -> "Span":
        return cls(start=float(written.get("start", 0.0)),
                   end=float(written.get("end", 0.0)))


class Multitrack:
    """**The tracks, and the timeline they are placed on** -- a handle over the
    multitrack the shared crate holds, which a multitrack editor opened on it
    edits in place.

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

        A multitrack with a history -- one an editor is open on, or one a
        script asked for `history` -- takes the change as a turn of it:
        recorded, and every window over it redrawn. One with none just
        changes."""
        from .history import ATTR

        context = getattr(self, ATTR, None)
        if context is None:
            answer = self._apply(intent, inverse=False)
        else:
            answer = context.script_edit(self, intent, label)
        if not answer.get("applied") and answer.get("reason"):
            raise ValueError(answer["reason"])
        return answer

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
        everything inside it one entry."""
        from .history import UndoHistory

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
        return _rest(self.write(), "version", "tracks", "channels", "tempo", "meter",
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
        from ._native import TempoMap, editing_default_tempo

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


# ---- the session: the multitrack, and where its samples are ----
#
# Lifted out of `clausters.form.document` rather than written again. What was
# worth keeping there was never the element-to-node conversion -- that is form's
# vocabulary and goes with it -- but this: a source table that says where samples
# are, a frozen reference for one this process does not hold, and a file that
# knows what it is missing. None of it was ever about form's five primitives.


@dataclass
class Source:
    """One entry in a session's source table: where samples are, how long they
    live, and what shape they have.

    The two fields a naive format leaves out and then cannot add are here:
    `provenance`, a reference to whatever produced the samples, carried opaquely
    so re-generating stays possible *without the document knowing how*; and
    `editing`, a destructive edit that has not been confirmed -- a save never
    blocks on a confirmation, so a saved session has to be able to say *this is
    a working copy of that, and the person has not decided yet*.
    """

    #: ``{"at": "file", "path": ...}``, ``{"at": "volatile"}`` or ``{"at":
    #: "events"}`` (the events are `sequence`). A relative path
    #: is resolved against the session's own folder, which is what makes a
    #: session directory movable; an absolute one names the user's own file,
    #: which a session must never copy or rewrite.
    location: dict
    #: ``"external"`` (the user's own file), ``"session"`` (saved beside the
    #: document) or ``"temporary"`` (a working copy that dies with the edit).
    lifetime: str = "session"
    #: Which generation of the content this is -- bumped by a destructive edit,
    #: so a reader holding an older copy knows to re-read.
    generation: int = 0
    channels: "int | None" = None
    frames: "int | None" = None
    #: Carried, never acted on: resampling is an edit.
    sample_rate: "float | None" = None
    provenance: "dict | None" = None
    #: ``{"from": source_id, "confirmed": bool}`` while a destructive edit is
    #: open over these samples.
    editing: "dict | None" = None
    extra: dict = field(default_factory=dict)
    #: **The events, when the source is a sequence of them**: the
    #: `clausters.seq.EventSequence` itself, held rather than copied, so what an
    #: editor does to it is what the next save writes. Left out of equality:
    #: a handle is compared by what it holds, which `write` says.
    sequence: "object | None" = field(default=None, compare=False)

    @classmethod
    def file(cls, path: str, lifetime: str = "session") -> "Source":
        """Samples in a file."""
        return cls(location={"at": "file", "path": path}, lifetime=lifetime)

    @classmethod
    def volatile(cls, lifetime: str = "session") -> "Source":
        """Samples that have not been written down -- a buffer never exported, a
        result never saved. A session may hold one, because saving must not be
        blocked by it, but a reader that finds one knows the samples are not
        there and opens that element unresolved rather than pretending."""
        return cls(location={"at": "volatile"}, lifetime=lifetime)

    @classmethod
    def events(cls, sequence, lifetime: str = "session") -> "Source":
        """A sequence of events -- a `clausters.seq.EventSequence` -- held in
        the session file itself. A region over it is a window onto its beats
        (in seconds, through its tempo map), and it draws the notes."""
        return cls(location={"at": "events"}, lifetime=lifetime, sequence=sequence)

    def shaped(self, channels: int, frames: int, sample_rate: float) -> "Source":
        """Its shape, for a caller that knows it."""
        self.channels, self.frames, self.sample_rate = channels, frames, sample_rate
        return self

    @property
    def path(self) -> "str | None":
        """Where the file is, when the samples are in one."""
        if self.location.get("at") == "file":
            return self.location.get("path") or None
        return None

    @property
    def is_resolvable(self) -> bool:
        """Whether the samples are somewhere a reader could find them. A
        sequence is in the file itself, so it always is."""
        return self.sequence is not None or bool(self.path)

    @property
    def is_being_edited(self) -> bool:
        """Whether a destructive edit is open and undecided over these
        samples."""
        return bool(self.editing) and not self.editing.get("confirmed", False)

    def write(self) -> dict:
        location = self.location
        if self.sequence is not None:
            location = {"at": "events", "sequence": self.sequence.data()}
        out: dict = {"location": location, "lifetime": self.lifetime,
                     "generation": self.generation}
        for name in ("channels", "frames", "sample_rate", "provenance", "editing"):
            value = getattr(self, name)
            if value is not None:
                out[name] = value
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Source":
        known = ("location", "lifetime", "generation", "channels", "frames",
                 "sample_rate", "provenance", "editing")
        location = dict(written.get("location") or {"at": "volatile"})
        sequence = None
        if location.get("at") == "events":
            from .seq.sequence import EventSequence

            sequence = EventSequence.from_data(location.pop("sequence", {}))
        return cls(
            location=location,
            sequence=sequence,
            lifetime=str(written.get("lifetime", "session")),
            generation=int(written.get("generation", 0) or 0),
            channels=written.get("channels"),
            frames=written.get("frames"),
            sample_rate=written.get("sample_rate"),
            provenance=written.get("provenance"),
            editing=written.get("editing"),
            extra=_rest(written, *known),
        )


class FrozenSource:
    """A source a session names and this process does not hold.

    Reading a session written elsewhere -- or written here before a buffer was
    allocated -- gives a reference and not an object. Rather than losing it, a
    region's window holds this: the same ``bufnum`` a real buffer answers with,
    plus what the table said about where the samples are and what shape they
    have, so a re-save keeps every location it was given.

    Without it, a multitrack opened with no way to read its files would be written
    back with every source marked volatile, which is a format that loses its own
    contents on the second save.
    """

    def __init__(self, id: int, entry: "Source | None" = None):
        self.bufnum = int(id)
        self.lifetime = "session"
        self.generation = 0
        self.path: "str | None" = None
        self.frames = 0
        self.channels = 0
        self.sample_rate = 0.0
        self.locate(entry)

    def locate(self, entry: "Source | None") -> None:
        """Take where and what this source is from a session's table entry."""
        if entry is None:
            return
        self.path = entry.path
        self.lifetime = entry.lifetime
        self.generation = entry.generation
        self.frames = int(entry.frames or 0)
        self.channels = int(entry.channels or 0)
        self.sample_rate = float(entry.sample_rate or 0.0)

    def __repr__(self) -> str:
        where = self.path or "volatile"
        return f"<FrozenSource {self.bufnum} {where}>"



# ---- the presentation: what a window shows of a multitrack ----
#
# Parallel to the model and never inside it, which is Live's shape and
# deliberate: `Song.View`, `Track.View` and `Application.View` are objects
# *beside* their model objects rather than children. So a `TrackView` is looked
# up by the track's id, and an `Multitrack` round trips the same whether or not
# a view of it exists.


@dataclass
class TrackView:
    """How one track is drawn."""

    #: How tall its row is, in the window's own units. ``None`` is the window's
    #: default, which is what a track nobody resized has.
    height: "float | None" = None
    #: Whether the row is collapsed to its header.
    collapsed: bool = False
    #: Whether the track's other take lanes are shown under the one that plays --
    #: comping open, in a word. Closed by default: a track with six takes on it
    #: is one row until somebody asks to see them.
    take_lanes_shown: bool = False
    #: The colour the track is drawn in, carried and never read.
    color: "str | None" = None
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {}
        if self.height is not None:
            out["height"] = float(self.height)
        if self.collapsed:
            out["collapsed"] = True
        if self.take_lanes_shown:
            out["take_lanes_shown"] = True
        if self.color is not None:
            out["color"] = self.color
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "TrackView":
        known = ("height", "collapsed", "take_lanes_shown", "color")
        return cls(
            height=written.get("height"),
            collapsed=bool(written.get("collapsed", False)),
            take_lanes_shown=bool(written.get("take_lanes_shown", False)),
            color=written.get("color"),
            extra=_rest(written, *known),
        )


@dataclass
class TakeLaneView:
    """How one take lane is drawn."""

    #: How tall its row is when the track's take lanes are shown.
    height: "float | None" = None
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {}
        if self.height is not None:
            out["height"] = float(self.height)
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "TakeLaneView":
        return cls(height=written.get("height"), extra=_rest(written, "height"))


@dataclass
class View:
    """One window's picture of one multitrack: where it is looking, how far it is
    zoomed, what the hand is holding, how tall each track is drawn.

    None of that is what the multitrack *is* -- a selection and a zoom are each
    window's and never the multitrack's -- and all of it is state a person
    loses on a reopen unless something writes it down. A session carries a
    **list** of these, because a multitrack drawn in two windows has two views and
    they disagree on purpose.

    Nothing here ever reaches the document or the history: a view is not edited
    through an intent, and an undo never puts a scroll back.
    """

    #: What the window is called, when a person named it.
    name: "str | None" = None
    #: The stretch of the timeline on screen, in seconds -- the zoom and the
    #: horizontal scroll, which are one fact and not two. ``None`` shows the
    #: whole multitrack.
    visible: "Span | None" = None
    #: How far down the tracks the window is scrolled, in its own units.
    scroll: float = 0.0
    #: The grid this window snaps to, in beats. Zero snaps nothing. It is here
    #: rather than in the multitrack because two windows over one multitrack may snap
    #: differently -- the arranger to a bar, the editor below it to a sixteenth.
    #: A musical grid over a multitrack in seconds, taken through its tempo map
    #: by the window: the ruler's configuration, not a unit of the placement.
    quant: float = 0.0
    #: Whether the window follows its content. ``False`` says the window is the
    #: reader's, and nothing moves it, which is what an editor wants.
    autofit: bool = True
    #: The time range the hand swept, in seconds, when it swept one.
    selection: "Span | None" = None
    #: What the hand is holding: regions, take lanes or tracks, by id. One list
    #: rather than one per kind, because the multitrack has one id space.
    selected: list = field(default_factory=list)
    #: What a keystroke is aimed at, which is not the same as what is selected.
    focused: "int | None" = None
    #: The region the detail editor below is showing, when the window has one.
    detail: "int | None" = None
    #: How each track is drawn, by the track's id.
    tracks: dict = field(default_factory=dict)
    #: How each take lane is drawn, by the take lane's id.
    take_lanes: dict = field(default_factory=dict)
    extra: dict = field(default_factory=dict)

    def track(self, track: "Track") -> TrackView:
        """How ``track`` is drawn, or the default when nobody touched it."""
        return self.tracks.get(track._id, TrackView())

    def track_view(self, track: "Track") -> TrackView:
        """How ``track`` is drawn, to be edited -- created on first use, which
        is what makes "nobody has touched it" cost nothing to store."""
        return self.tracks.setdefault(track._id, TrackView())

    def take_lane(self, lane: "TakeLane") -> TakeLaneView:
        """How ``lane`` is drawn, or the default."""
        return self.take_lanes.get(lane._id, TakeLaneView())

    def take_lane_view(self, lane: "TakeLane") -> TakeLaneView:
        """How ``lane`` is drawn, to be edited. See `track_view`."""
        return self.take_lanes.setdefault(lane._id, TakeLaneView())

    def prune(self, multitrack: "Multitrack") -> bool:
        """Drops everything this view says about objects the multitrack no longer
        holds, and answers whether anything went.

        **State goes when the thing goes.** Keeping it is worse than losing it:
        a height kept for a track that is not the same track is a defect that
        looks like a feature.
        """
        held = set()
        for track in multitrack.write().get("tracks", []):
            held.add(int(track["id"]))
            for lane in track.get("take_lanes", []):
                held.add(int(lane["id"]))
                held.update(int(r["id"]) for r in lane.get("regions", []))
            held.update(int(a["id"]) for a in track.get("automation", []))
        before = (len(self.tracks), len(self.take_lanes), len(self.selected),
                  self.focused, self.detail)
        self.tracks = {id: v for id, v in self.tracks.items() if id in held}
        self.take_lanes = {id: v for id, v in self.take_lanes.items() if id in held}
        self.selected = [id for id in self.selected if id in held]
        if self.focused not in held:
            self.focused = None
        if self.detail not in held:
            self.detail = None
        return before != (len(self.tracks), len(self.take_lanes),
                          len(self.selected), self.focused, self.detail)

    def write(self) -> dict:
        """The view as the crate's JSON. Nothing said is nothing written."""
        out: dict = {}
        if self.name is not None:
            out["name"] = self.name
        if self.visible is not None:
            out["visible"] = self.visible.write()
        if self.scroll:
            out["scroll"] = float(self.scroll)
        if self.quant:
            out["quant"] = float(self.quant)
        if not self.autofit:
            out["autofit"] = False
        if self.selection is not None:
            out["selection"] = self.selection.write()
        if self.selected:
            out["selected"] = [int(id) for id in self.selected]
        if self.focused is not None:
            out["focused"] = int(self.focused)
        if self.detail is not None:
            out["detail"] = int(self.detail)
        if self.tracks:
            out["tracks"] = {str(id): v.write()
                             for id, v in sorted(self.tracks.items())}
        if self.take_lanes:
            out["take_lanes"] = {str(id): v.write()
                            for id, v in sorted(self.take_lanes.items())}
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "View":
        """A view from the crate's JSON."""
        known = ("name", "visible", "scroll", "quant", "autofit", "selection",
                 "selected", "focused", "detail", "tracks", "take_lanes")
        visible = written.get("visible")
        selection = written.get("selection")
        return cls(
            name=written.get("name"),
            visible=None if visible is None else Span.read(visible),
            scroll=float(written.get("scroll", 0.0)),
            quant=float(written.get("quant", 0.0)),
            autofit=bool(written.get("autofit", True)),
            selection=None if selection is None else Span.read(selection),
            selected=[int(id) for id in written.get("selected", [])],
            focused=written.get("focused"),
            detail=written.get("detail"),
            tracks={int(id): TrackView.read(v)
                    for id, v in (written.get("tracks") or {}).items()},
            take_lanes={int(id): TakeLaneView.read(v)
                   for id, v in (written.get("take_lanes") or {}).items()},
            extra=_rest(written, *known),
        )


@dataclass
class Session:
    """A session: the multitrack, saved, and where its samples are.

    An `Multitrack` says *what plays when* and deliberately does not say where
    a source lives, because inside a running system a source is a server buffer,
    a mapped file or a rendered result and the multitrack has no business knowing
    which. A session is the multitrack plus exactly that missing half.

    Not `clausters.Session`, which is a connection to a running server. Two
    nouns, two modules; this one is reached as `clausters.multitrack.Session`
    and is a **file**.

    The `document` field carries the general tree for what is not an
    arrangement. It is the leg being walked off -- what every current reader
    opens -- and what replaces it is already here: a composite region carries
    that same tree, placed.
    """

    #: The format version this build writes -- `clausters.document.SESSION_FORMAT`,
    #: which is the crate's `session::FORMAT`. A session **read** in an older
    #: format is migrated to this one first (`read`), since its numbers are
    #: read differently: format 2 placed the multitrack in beats.
    format: int = SESSION_FORMAT
    #: The multitrack. Always present, possibly empty -- which mirrors the crate,
    #: where an absent arrangement reads as an empty one rather than as nothing.
    multitrack: "Multitrack" = field(default_factory=lambda: Multitrack())
    #: How the multitrack was being **looked at**: one entry per window. Carried for
    #: the reason every program in the field carries it -- reopening a multitrack
    #: into the window it was left in is what a person expects -- and a reader
    #: that ignores it opens the same multitrack.
    views: list = field(default_factory=list)
    #: **Where a pass over the multitrack ends**, as a playback's ``end`` says
    #: it: ``None`` (it rolls on, the default), ``"contents"`` (where the last
    #: region ends) or a number of seconds, an end marker. The transport's own
    #: switch, kept so a session opened again stops where it stopped before --
    #: not in a view, since it changes what is heard. It bounds no axis: how
    #: far a window shows or zooms out reads nothing here.
    end: "str | float | None" = None
    document: "dict | None" = None
    #: Where each source is, keyed by source id.
    sources: dict = field(default_factory=dict)
    #: What produced the session as a whole -- the scripts behind it -- carried
    #: opaquely. The document never knows how to re-run them; it only has to not
    #: lose the reference.
    provenance: "dict | None" = None
    extra: dict = field(default_factory=dict)
    #: The file this session was opened from or last saved to, or ``None``.
    #: Not written into the file: where a session is is not what it says.
    path: "str | None" = field(default=None, compare=False, repr=False)

    def __eq__(self, other) -> bool:
        """Two sessions are equal when they write the same file: the multitrack
        is a handle, and a handle is compared by what it holds."""
        if not isinstance(other, Session):
            return NotImplemented
        return self.write() == other.write()

    @classmethod
    def open(cls, path) -> "Session":
        """A session read from the file at ``path``, remembering where it came
        from: `load` reads relative paths against that file's folder, and
        `save` writes back to it."""
        with open(path) as f:
            session = cls.read(json.load(f))
        session.path = str(path)
        return session

    def save(self, path=None) -> str:
        """Writes the session to ``path``, or back to the file it was opened
        from or last saved to, and answers where it went.

        Raises:
            ValueError: when there is no ``path`` and the session was never
                opened or saved.
        """
        target = str(path) if path is not None else self.path
        if target is None:
            raise ValueError("this session has nowhere to save to: give it a path")
        with open(target, "w") as f:
            f.write(json.dumps(self.write(), indent=1))
        self.path = target
        return target

    def source(self, id: int) -> "Source | None":
        """The source a reference names, if the table has it."""
        return self.sources.get(int(id))

    def sequences(self) -> dict:
        """The sources that are sequences of events: source id ->
        `clausters.seq.EventSequence`, the handles the table holds. With what
        `load` answers, the whole table a multitrack editor is opened with."""
        return {int(id): s.sequence for id, s in self.sources.items()
                if s.sequence is not None}

    def volatile(self) -> list:
        """Sources whose samples are not written down anywhere -- what a save
        consults before promising the file is complete."""
        return sorted(id for id, s in self.sources.items() if not s.is_resolvable)

    def open_edits(self) -> list:
        """Sources with a destructive edit still open and undecided."""
        return sorted(id for id, s in self.sources.items() if s.is_being_edited)

    def dangling(self) -> list:
        """Sources the multitrack names but the table does not hold -- what an opening
        reader reports rather than discovering one element at a time.

        **Every** take lane is walked and not only the ones that play: an alternate
        take names its source whether or not anyone has chosen it yet.
        """
        missing = []
        for region in self.multitrack.regions():
            named = (region.content.window or {}).get("source")
            id = named.get("source") if isinstance(named, dict) else None
            if id is not None and int(id) not in self.sources \
                    and int(id) not in missing:
                missing.append(int(id))
        return missing

    def promote(self, id: int) -> bool:
        """Promotes a temporary working copy to one saved beside the document,
        **leaving the edit open**. What a save mid-edit does: auto-confirming
        would turn a save into an edit, and refusing until the edit is settled
        would make the safest habit in the program the one that is blocked."""
        source = self.sources.get(int(id))
        if source is None or source.lifetime != "temporary":
            return False
        source.lifetime = "session"
        return True

    def confirm(self, id: int) -> bool:
        """Confirms the edit open over a source: the working copy becomes the
        samples, and there is nothing left undecided about it."""
        source = self.sources.get(int(id))
        if source is None or not source.editing:
            return False
        source.editing = dict(source.editing, confirmed=True)
        return True

    def load(self, server=None, *, beside: "str | None" = None,
             timeout: "float | None" = None) -> dict:
        """Loads the sources the multitrack names into ``server``: every take read
        from its file, and every join stitched from the takes it is made of
        once those are there.

        What each source *is* in a running system is not the document's to
        decide, and loading is the half of reopening a session that says it:
        a server buffer per source. What is read and in what order is the
        shared crate's (`clausters._native.editing_load`), so a session opens
        the same here, in the web client and in the GUI host.

        Args:
            server: the server to load into; the default one when omitted.
            beside: the folder a relative path is read against -- the session
                file's own, which is what keeps a session directory movable.
                When omitted, the folder of the file the session was opened
                from or saved to, else the current one.
            timeout: how long each read may take.

        Returns:
            Source id -> `clausters.Buffer`, for every source that loaded (a
            sequence of events takes none: `sequences` hands those over). A
            source that cannot -- a volatile one, one the table does not hold,
            a join over a take that did not load -- is left out and named in
            a warning.

        Raises:
            CommandError: when the server refuses a read or a stitch -- a file
                that is not there -- after freeing what the load had made.

        RT only (it waits on each buffer's ``/done``).
        """
        import warnings

        from . import _native
        from ._steps import run_steps
        from .defs.buffer import Buffer, _resolve
        from .errors import CommandError

        if beside is None:
            beside = os.path.dirname(self.path) if self.path else "."
        srv = _resolve(server)
        aside = [srv.buffers.alloc() for _ in self.sources]
        plan = _native.editing_load(self.write(), str(beside), aside)
        if "error" in plan:
            for bufnum in aside:
                srv.buffers.free(bufnum)
            raise ValueError(plan["error"])
        for bufnum in plan["unused"]:
            srv.buffers.free(int(bufnum))
        for id, why in plan["unresolved"]:
            warnings.warn(f"source {id} is not loadable: {why}", stacklevel=2)
        buffers = {}
        for id, take in plan["takes"].items():
            buffer = Buffer(int(take["buffer"]), int(take["frames"]),
                            max(1, int(take["channels"])), server=srv)
            if "path" in take:
                buffer.path = str(take["path"])
            buffers[int(id)] = buffer
        try:
            run_steps(srv, _native.StepRunner(), plan["steps"], to="samples",
                      timeout=timeout)
        except CommandError:
            for buffer in buffers.values():
                buffer.free()
            raise
        # The shape is the file's, so it is read back rather than trusted.
        for buffer in buffers.values():
            buffer.info(timeout=timeout)
        return buffers

    def write(self) -> dict:
        """The session as the crate's JSON."""
        out: dict = {"format": self.format}
        written = self.multitrack.write()
        if written:
            out["multitrack"] = written
        if self.views:
            out["views"] = [v.write() for v in self.views]
        if self.end is not None:
            out["end"] = self.end if self.end == "contents" else float(self.end)
        if self.document is not None:
            out["document"] = self.document
        if self.sources:
            out["sources"] = {str(id): s.write()
                              for id, s in sorted(self.sources.items())}
        if self.provenance is not None:
            out["provenance"] = self.provenance
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Session":
        """A session from the crate's JSON, migrated first when it was written in
        an older format.

        The migration is the crate's (`clausters._native.session_migrate`), so
        an old file opens the same here, in the web client and in the GUI host.
        """
        if int(written.get("format", 1)) < SESSION_FORMAT:
            from ._native import session_migrate

            written = session_migrate(written)
        known = ("format", "multitrack", "views", "end", "document", "sources",
                 "provenance")
        return cls(
            format=int(written.get("format", 1)),
            # `arrangement` is what the field was called before 2026-09-08, read
            # so a session written then still opens.
            multitrack=Multitrack.read(
                written.get("multitrack") or written.get("arrangement") or {}),
            views=[View.read(v) for v in written.get("views", [])],
            end=written.get("end"),
            document=written.get("document"),
            sources={int(id): Source.read(entry)
                     for id, entry in (written.get("sources") or {}).items()},
            provenance=written.get("provenance"),
            extra=_rest(written, *known),
        )
