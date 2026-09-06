"""The arrangement: tracks, lanes, regions, and the timeline they sit on.

This is the client's side of `clausters_document::arrangement` — the model a
multitrack editor edits, and the one the three classic applications (audio
editor, multitrack editor, score editor) are built over. The crate defines the
format; this module is the idiomatic way to write one and read one back, the
same way `clausters.document` is the idiomatic way to reach an edit.

The vocabulary is the field's own and not this project's invention:

- A **source** is samples. It lives outside the arrangement — the session's
  table says where — and is never overwritten.
- A `Region` is **one placed thing**: a span of the timeline (where it starts,
  how long, its fades, which of the overlapping ones is on top) plus a
  `Content` saying what fills it. Six regions over one source are six
  identities and one source, referenced rather than copied. That is the whole
  of non-destructive editing.
- A `Lane` is one of a track's several contents, an ordered list of regions.
  Ardour's structure and our name.
- A `Track` holds several lanes and **plays one**, which is what comping is:
  record six passes into six lanes, then take from each.
- An `Automation` is a curve over one parameter, in the arrangement's time.
- An `Arrangement` is the tracks plus what the **piece** has one of: the tempo
  map, the meter map, the markers, the loop and punch spans. They are here and
  not on a track precisely so that no two tracks can disagree about them.

A region is not a clip
----------------------

`Region` is the model's word; **clip** is the picture's. A clip, a lane row, a
waveform are what the host draws; a region is what an edit names. Keeping them
apart is deliberate — the multitrack's defects came from the thing drawn and the
thing addressed being one object.

Time
----

Everything placed here is placed in **beats**, because where a thing sits in a
piece is a musical decision. What fills a region is measured in its own source's
units — frames for samples, beats for a node — and the two are not the same
axis. The crate makes that a type; here it is a rule the field names say
(`position` and `length` are the region's, `start` and `duration` are its
window's), and the conversion between them needs the tempo map, which is why the
map is part of the piece.

Usage::

    from clausters.arrangement import Arrangement, Region, Track, Tempo

    piece = Arrangement()
    piece.set_tempo(Tempo(at=0.0, bpm=96.0))
    drums = Track(id=1, name="drums", lanes=[Lane(id=2)])
    drums.active_lane.place(Region(id=3, position=0.0, length=4.0,
                                   content=Content.window(take)))
    piece.tracks.append(drums)
"""

from dataclasses import dataclass, field

__all__ = [
    "Arrangement",
    "Automation",
    "Content",
    "Fade",
    "Lane",
    "Marker",
    "Meter",
    "Region",
    "Span",
    "Tempo",
    "Track",
]


def _rest(written: dict, *known: str) -> dict:
    """Whatever a newer writer wrote and this build has no field for.

    Carried, never read. A reader that dropped it would lose a piece the next
    version of this client wrote, which for a format with two writers in two
    languages is not hypothetical.
    """
    return {key: value for key, value in written.items() if key not in known}


@dataclass
class Fade:
    """A fade's length in beats, and whatever the client says about its curve.

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
    evaluation, for a window onto something generated — a function placed twice
    is two evaluations, possibly with different arguments, and the document
    carries them without reading them.
    """

    fill: str
    window: "dict | None" = None
    playrate: float = 1.0
    args: "dict | None" = None
    node: "dict | None" = None
    other: "dict | None" = None

    @classmethod
    def onto(cls, window: dict, *, playrate: float = 1.0,
             args: "dict | None" = None) -> "Content":
        """A window onto a source — a `clausters.form` segment reference, or any
        `{"source": …, "start": …, "duration": …}` the crate accepts."""
        return cls(fill="window", window=window, playrate=playrate, args=args)

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
                       args=written.get("args"))
        if fill == "composite":
            return cls(fill="composite", node=written.get("node"))
        return cls(fill=str(fill), other=dict(written))


@dataclass
class Region:
    """One placed thing on a lane: a span of the timeline, and what fills it.

    `position` and `length` are the region's own, in beats. They are **not** the
    content's: a region may show part of what it holds, and trimming moves these
    without touching the source.
    """

    id: int
    position: float
    length: float
    content: Content
    name: "str | None" = None
    #: Which of the overlapping regions on this lane draws and plays on top.
    #: Overlap is legal and ordinary -- a crossfade *is* an overlap -- so the
    #: stack needs an order that survives a save.
    layer: int = 0
    fade_in: "Fade | None" = None
    fade_out: "Fade | None" = None
    muted: bool = False
    extra: dict = field(default_factory=dict)

    @property
    def end(self) -> float:
        """Where it ends: its position plus its length."""
        return self.position + self.length

    def overlaps(self, other: "Region") -> bool:
        """Whether the two occupy any of the same time.

        Half-open, so a region ending exactly where the next begins does not
        overlap it — which is what makes a cut into two regions not a crossfade.
        """
        return self.position < other.end and other.position < self.end

    def write(self) -> dict:
        out: dict = {"id": self.id, "position": self.position,
                     "length": self.length, "content": self.content.write()}
        if self.name is not None:
            out["name"] = self.name
        if self.layer:
            out["layer"] = self.layer
        if self.fade_in is not None:
            out["fade_in"] = self.fade_in.write()
        if self.fade_out is not None:
            out["fade_out"] = self.fade_out.write()
        if self.muted:
            out["muted"] = True
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Region":
        known = ("id", "position", "length", "content", "name", "layer",
                 "fade_in", "fade_out", "muted")
        fade = written.get("fade_in")
        out = written.get("fade_out")
        return cls(
            id=int(written["id"]),
            position=float(written.get("position", 0.0)),
            length=float(written.get("length", 0.0)),
            content=Content.read(written.get("content") or {}),
            name=written.get("name"),
            layer=int(written.get("layer", 0)),
            fade_in=None if fade is None else Fade.read(fade),
            fade_out=None if out is None else Fade.read(out),
            muted=bool(written.get("muted", False)),
            extra=_rest(written, *known),
        )


@dataclass
class Lane:
    """One of a track's several contents: an ordered list of regions.

    Ardour's structure and our name — its *playlist* is this, and that word is
    spent on something else everywhere. `place` keeps the list in position
    order, so a re-saved session is stable and a diff of two saves is the edits
    rather than the iteration order.
    """

    id: int
    name: "str | None" = None
    regions: list = field(default_factory=list)
    extra: dict = field(default_factory=dict)

    def place(self, region: Region) -> Region:
        """Places a region and keeps the lane in position order. Returns it, so
        a caller can go on holding what it just placed."""
        at = 0
        for at, held in enumerate(self.regions):
            if (held.position, held.layer) > (region.position, region.layer):
                self.regions.insert(at, region)
                return region
        self.regions.append(region)
        return region

    def region(self, id: int) -> "Region | None":
        """The region with this id, if it is here."""
        return next((r for r in self.regions if r.id == id), None)

    @property
    def end(self) -> float:
        """Where the last region ends, or zero when there are none."""
        return max((r.end for r in self.regions), default=0.0)

    def write(self) -> dict:
        out: dict = {"id": self.id}
        if self.name is not None:
            out["name"] = self.name
        if self.regions:
            out["regions"] = [r.write() for r in self.regions]
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Lane":
        return cls(
            id=int(written["id"]),
            name=written.get("name"),
            regions=[Region.read(r) for r in written.get("regions", [])],
            extra=_rest(written, "id", "name", "regions"),
        )


@dataclass
class Automation:
    """A curve over one parameter, in the arrangement's own time.

    `target` says **what this automates** in the client's terms and is never
    read here — a control name, a bus, a plugin's parameter index — the same
    door a leaf's configuration is, and for the same reason. The points are
    `{"at": beats, "value": v, "data": …}`, the shape `clausters.document`'s
    points vocabulary already carries.
    """

    id: int
    target: "dict | None" = None
    name: "str | None" = None
    points: list = field(default_factory=list)
    #: Whether the lane is shown. The **view's**, and kept here because which
    #: curves a person had open is part of reopening the piece as they left it.
    visible: bool = False
    #: Whether the curve is being applied. A curve can be kept and switched off
    #: without being deleted, which is what an arm or a bypass is.
    enabled: bool = True
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"id": self.id}
        if self.name is not None:
            out["name"] = self.name
        if self.target is not None:
            out["target"] = self.target
        if self.points:
            out["points"] = list(self.points)
        if self.visible:
            out["visible"] = True
        if not self.enabled:
            out["enabled"] = False
        out.update(self.extra)
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


@dataclass
class Track:
    """A row of the arrangement: several lanes, one of them playing, the curves
    over it, and whatever the client says it is.

    **What a track *is* — an instrument, a bus, a folder — is not here.** That
    is `config`, carried and never interpreted, for the reason a leaf is opaque:
    a def is code in the language of whoever wrote it. What the document owns is
    the structure: which lanes, which one plays, what is placed on them.
    """

    id: int
    name: "str | None" = None
    lanes: list = field(default_factory=list)
    #: Which lane plays, as an index into `lanes`.
    active: int = 0
    automation: list = field(default_factory=list)
    muted: bool = False
    #: Marked as soloed. Whether a solo anywhere silences everything else is the
    #: mixer's rule and not the document's.
    soloed: bool = False
    config: "dict | None" = None
    extra: dict = field(default_factory=dict)

    @property
    def active_lane(self) -> "Lane | None":
        """The lane that plays, or ``None`` when `active` names one that is not
        there."""
        return self.lanes[self.active] if 0 <= self.active < len(self.lanes) else None

    @property
    def end(self) -> float:
        """Where the track's last region ends, across **every** lane — what it
        spans rather than what it plays, since an alternate take is still part
        of the piece."""
        return max((lane.end for lane in self.lanes), default=0.0)

    def write(self) -> dict:
        out: dict = {"id": self.id}
        if self.name is not None:
            out["name"] = self.name
        if self.lanes:
            out["lanes"] = [lane.write() for lane in self.lanes]
        if self.active:
            out["active"] = self.active
        if self.automation:
            out["automation"] = [a.write() for a in self.automation]
        if self.muted:
            out["muted"] = True
        if self.soloed:
            out["soloed"] = True
        if self.config is not None:
            out["config"] = self.config
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Track":
        known = ("id", "name", "lanes", "active", "automation", "muted",
                 "soloed", "config")
        return cls(
            id=int(written["id"]),
            name=written.get("name"),
            lanes=[Lane.read(lane) for lane in written.get("lanes", [])],
            active=int(written.get("active", 0)),
            automation=[Automation.read(a) for a in written.get("automation", [])],
            muted=bool(written.get("muted", False)),
            soloed=bool(written.get("soloed", False)),
            config=written.get("config"),
            extra=_rest(written, *known),
        )


@dataclass
class Tempo:
    """One entry of the tempo map: from here on, this tempo.

    `ramp` says the tempo runs from here to the next entry rather than stepping.
    A ritardando is a ramp; a section change is a step.
    """

    at: float
    bpm: float
    ramp: bool = False
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"at": self.at, "bpm": self.bpm}
        if self.ramp:
            out["ramp"] = True
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Tempo":
        return cls(at=float(written.get("at", 0.0)), bpm=float(written["bpm"]),
                   ramp=bool(written.get("ramp", False)),
                   extra=_rest(written, "at", "bpm", "ramp"))


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
class Marker:
    """A named point on the timeline."""

    id: int
    at: float
    name: "str | None" = None
    extra: dict = field(default_factory=dict)

    def write(self) -> dict:
        out: dict = {"id": self.id, "at": self.at}
        if self.name is not None:
            out["name"] = self.name
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Marker":
        return cls(id=int(written["id"]), at=float(written.get("at", 0.0)),
                   name=written.get("name"),
                   extra=_rest(written, "id", "at", "name"))


@dataclass
class Span:
    """A span of the timeline: the loop, the punch, a named region of the piece.

    Half-open, so two spans that meet cover no beat twice.
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


@dataclass
class Arrangement:
    """The tracks, and the timeline they are placed on.

    What is here rather than on a track is what the **piece** has one of: the
    tempo map, the meter map, the markers, the loop. A track has none of them
    and never disagrees with another track about them, which is the whole
    argument for where they live.
    """

    tracks: list = field(default_factory=list)
    tempo: list = field(default_factory=list)
    meter: list = field(default_factory=list)
    markers: list = field(default_factory=list)
    loop_span: "Span | None" = None
    punch: "Span | None" = None
    extra: dict = field(default_factory=dict)

    def track(self, id: int) -> "Track | None":
        """The track with this id."""
        return next((t for t in self.tracks if t.id == id), None)

    @property
    def end(self) -> float:
        """Where the last region ends, across every track and every lane — how
        long the piece is."""
        return max((t.end for t in self.tracks), default=0.0)

    def regions(self):
        """Every region, in track then lane then position order — **every**
        lane, not only the ones that play, because an alternate take still names
        the source it plays."""
        for track in self.tracks:
            for lane in track.lanes:
                yield from lane.regions

    def tempo_at(self, at: float) -> "Tempo | None":
        """The tempo entry in force at `at`, or ``None`` when the map says
        nothing.

        The **entry**, not a converted position: turning a beat into seconds
        needs the whole map walked and a ramp integrated, and the shape of a
        ramp is not something the document names.
        """
        return next((t for t in reversed(self.tempo) if t.at <= at), None)

    def meter_at(self, at: float) -> "Meter | None":
        """The meter entry in force at `at`, or ``None``."""
        return next((m for m in reversed(self.meter) if m.at <= at), None)

    def set_tempo(self, tempo: Tempo) -> None:
        """Adds a tempo entry, in position order, replacing any already at that
        beat — two tempos at one position is a state the map should not hold."""
        self.tempo = [t for t in self.tempo if t.at != tempo.at]
        self.tempo.append(tempo)
        self.tempo.sort(key=lambda t: t.at)

    def set_meter(self, meter: Meter) -> None:
        """Adds a meter entry, on the same rule."""
        self.meter = [m for m in self.meter if m.at != meter.at]
        self.meter.append(meter)
        self.meter.sort(key=lambda m: m.at)

    def add_marker(self, marker: Marker) -> None:
        """Adds a marker, in position order. Several may share a beat: unlike a
        tempo, two names for one moment is a thing people do."""
        self.markers.append(marker)
        self.markers.sort(key=lambda m: m.at)

    def write(self) -> dict:
        """The arrangement as the crate's JSON. Nothing said is nothing
        written."""
        out: dict = {}
        if self.tracks:
            out["tracks"] = [t.write() for t in self.tracks]
        if self.tempo:
            out["tempo"] = [t.write() for t in self.tempo]
        if self.meter:
            out["meter"] = [m.write() for m in self.meter]
        if self.markers:
            out["markers"] = [m.write() for m in self.markers]
        if self.loop_span is not None:
            out["loop_span"] = self.loop_span.write()
        if self.punch is not None:
            out["punch"] = self.punch.write()
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Arrangement":
        """An arrangement from the crate's JSON."""
        known = ("tracks", "tempo", "meter", "markers", "loop_span", "punch")
        loop = written.get("loop_span")
        punch = written.get("punch")
        return cls(
            tracks=[Track.read(t) for t in written.get("tracks", [])],
            tempo=[Tempo.read(t) for t in written.get("tempo", [])],
            meter=[Meter.read(m) for m in written.get("meter", [])],
            markers=[Marker.read(m) for m in written.get("markers", [])],
            loop_span=None if loop is None else Span.read(loop),
            punch=None if punch is None else Span.read(punch),
            extra=_rest(written, *known),
        )
