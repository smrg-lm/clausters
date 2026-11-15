# pyright: basic
"""The presentation: what a window shows of a multitrack, saved beside it.

Private: the names are `clausters.document`'s.
"""

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from ._format import _unknown_fields
from ._timeline import Span

if TYPE_CHECKING:
    from ._multitrack import Multitrack, TakeLane, Track



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
            extra=_unknown_fields(written, *known),
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
        return cls(height=written.get("height"), extra=_unknown_fields(written, "height"))


@dataclass
class MultitrackView:
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
    def read(cls, written: dict) -> "MultitrackView":
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
            extra=_unknown_fields(written, *known),
        )
