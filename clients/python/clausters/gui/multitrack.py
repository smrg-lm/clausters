"""`Multitrack`: the piece a `clausters.gui.multitrack` widget draws, kept here.

The widget owns the lanes and the clips and reports **the piece as it now
stands** after any gesture; this is the object that holds that on the client's
side, so a script says what the piece *is* and never what a hand did to it.

**It wires nothing that a script would otherwise have to.** `attach` subscribes
once, to one widget, and turns both edit-backs into this object's own lists —
so there is no handler per clip, no widget id anywhere, and nothing to keep in
step by hand. Identity is your own name: a clip is placed, drawn and reported
by the same word you called it.

**The words here are the picture's.** A `Lane` is a row of the view and a `Clip`
is a box on it, which is what the protocol has always called them; the model's
words for the same things are `clausters.multitrack`'s `Track`, `Lane` and
`Region`, and a region is not a clip. This object is the view's side.
"""

from dataclasses import dataclass, field, replace

from .guidef import multitrack as multitrack_view

__all__ = ["Clip", "Lane", "Multitrack"]


@dataclass
class Lane:
    """One row of the view: what it is called, how thick it is, and its strip."""

    #: Its identity, and your own word for it.
    name: str
    #: What the header draws; the name when empty.
    label: str = ""
    #: Its thickness in logical pixels.
    height: float = 96.0
    #: Silenced. Carried by the host, never interpreted — what a solo does to
    #: *other* lanes is the mixer's rule, and the mixer is yours.
    mute: bool = False
    solo: bool = False
    #: The fader, over ``[0, 1]``.
    gain: float = 1.0


@dataclass
class Clip:
    """One box: which lane it is on, and where it sits there.

    ``at``, ``dur`` and ``start`` are in the axis' own unit (timeline samples),
    and ``start`` is the source frame the box's own time zero reads — so
    trimming the left edge moves ``at``, ``dur`` and ``start`` together, which is
    what makes a trim hide frames instead of compressing them.
    """

    name: str
    lane: str
    at: float = 0.0
    dur: float = 0.0
    start: float = 0.0
    label: str = ""

    @property
    def end(self) -> float:
        """Where it ends on the axis."""
        return self.at + self.dur


@dataclass
class Multitrack:
    """The piece: its lanes, its clips, and the one widget that draws them.

    Args:
        lanes: the rows, top to bottom — `Lane`s, or the tuples one takes.
        clips: the boxes — `Clip`s, or the tuples one takes.
        snap: the drag grid in axis units; ``0`` is no grid.
        on_change: ``on_change(what)`` after a hand edited the piece, with
            ``what`` being ``"clips"`` or ``"lanes"``. It is called *after* this
            object's lists are already the new ones, so a handler reads them
            rather than parsing anything.
    """

    lanes: list = field(default_factory=list)
    clips: list = field(default_factory=list)
    snap: float = 0.0
    on_change: object = None
    _widget: object = field(default=None, repr=False)

    def __post_init__(self):
        self.lanes = [l if isinstance(l, Lane) else Lane(*l) for l in self.lanes]
        self.clips = [c if isinstance(c, Clip) else Clip(*c) for c in self.clips]

    # ---- reading it ----

    def lane(self, name: str):
        """The lane of this name, or ``None``."""
        return next((l for l in self.lanes if l.name == name), None)

    def clip(self, name: str):
        """The clip of this name, or ``None``."""
        return next((c for c in self.clips if c.name == name), None)

    def on(self, lane: str) -> list:
        """The clips on ``lane``, in the order they are drawn."""
        return [c for c in self.clips if c.lane == lane]

    @property
    def extent(self) -> float:
        """Where the piece ends: the furthest clip end, ``0.0`` for none.

        The **end**, not the last onset — a clip dragged past everything else
        lengthens the piece by its whole length.
        """
        return max((c.end for c in self.clips), default=0.0)

    # ---- changing it ----

    def add_lane(self, lane, **props) -> "Multitrack":
        """Append a lane (a `Lane`, its tuple, or a bare name)."""
        if isinstance(lane, str):
            lane = Lane(lane, **props)
        elif not isinstance(lane, Lane):
            lane = Lane(*lane)
        self.lanes.append(lane)
        return self._pushed("lanes")

    def remove_lane(self, name: str) -> "Multitrack":
        """Take a lane away. **The clips on it are kept** — they name a lane
        that is not there, are drawn nowhere, and come back to be re-homed;
        losing them silently is the one thing a removal must not do."""
        self.lanes = [l for l in self.lanes if l.name != name]
        return self._pushed("lanes")

    def place(self, name: str, lane: str, at: float, dur: float,
              start: float = 0.0, label: str = "") -> "Multitrack":
        """Put a clip where you say — adding it, or moving the one of that
        name. The verb is one because *the piece is a statement*: what you hand
        over is where the clip is, not how it got there."""
        found = self.clip(name)
        if found is None:
            self.clips.append(Clip(name, lane, at, dur, start, label))
        else:
            self.clips[self.clips.index(found)] = replace(
                found, lane=lane, at=at, dur=dur, start=start, label=label)
        return self._pushed("clips")

    def remove(self, name: str) -> "Multitrack":
        """Take a clip away."""
        self.clips = [c for c in self.clips if c.name != name]
        return self._pushed("clips")

    def mix(self, lane: str, *, mute=None, solo=None, gain=None) -> "Multitrack":
        """Set a lane's strip. What a solo does to the other lanes is **your**
        rule: the host carries the flag and never reads it."""
        found = self.lane(lane)
        if found is None:
            return self
        if mute is not None:
            found.mute = bool(mute)
        if solo is not None:
            found.solo = bool(solo)
        if gain is not None:
            found.gain = float(gain)
        return self._pushed("lanes")

    # ---- the widget ----

    def view(self, **props):
        """The `clausters.gui.multitrack` widget drawing this piece, built from
        what this object holds. Any widget prop (``name``, ``weight``, ``link``,
        ``ruler``, ``sample_rate``, ``playhead_at``…) passes through."""
        props.setdefault("snap", self.snap)
        return multitrack_view(lanes=self._lane_tuples(),
                               clips=self._clip_tuples(), **props)

    def attach(self, widget) -> "Multitrack":
        """Subscribe to the widget drawing this piece — a `WidgetHandle`, or a
        window and the name to look it up by.

        **One subscription, for the whole piece.** The widget reports what it now
        holds, so this replaces the lists and calls `on_change`; there is nothing
        per clip to register and no id for a script to carry.
        """
        self._widget = widget
        widget.on_event(self._edited)
        return self

    def _edited(self, tag, *vals):
        """Both edit-backs, each the whole list — the piece as it now stands."""
        if tag == "clips":
            self.clips = [Clip(str(n), str(lane), float(at), float(dur),
                               float(start), str(label))
                          for n, lane, at, dur, start, label in _six(vals)]
        elif tag == "lanes":
            self.lanes = [Lane(str(n), str(label), float(h), bool(int(m)),
                               bool(int(s)), float(g))
                          for n, label, h, m, s, g in _six(vals)]
        else:
            return
        if callable(self.on_change):
            self.on_change(tag)

    def _pushed(self, what: str) -> "Multitrack":
        """Send a changed list to the widget, when there is one to send to."""
        if self._widget is None:
            return self
        if what == "clips":
            self._widget.set(clips=self._clip_tuples())
        else:
            self._widget.set(lanes=self._lane_tuples())
        return self

    def _lane_tuples(self) -> list:
        return [(l.name, l.label, l.height, l.mute, l.solo, l.gain)
                for l in self.lanes]

    def _clip_tuples(self) -> list:
        return [(c.name, c.lane, c.at, c.dur, c.start, c.label)
                for c in self.clips]


def _six(vals) -> list:
    """The flat payload as sextuples; a trailing partial group is dropped rather
    than half-read, the rule every flat payload here follows."""
    return [vals[i:i + 6] for i in range(0, len(vals) - len(vals) % 6, 6)]
