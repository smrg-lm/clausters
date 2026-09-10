"""Editing a **piece**: its vocabulary, its picture and its editor.

The multitrack, as one of the three fundamental structures gets: a
`clausters.gui.editing.Domain` that turns the `multitrack` widget's ``clips``
and ``lanes`` payloads into the crate's own vocabulary and back, a
`clausters.gui.editing.View` that is one `clausters.gui.guidef.multitrack`
widget, and an editor that is `clausters.gui.editing.Editor` with those two in
it and nothing else.

**Nothing here derives the picture, and nothing here reads a gesture.** Both are
the crate's (`clausters._native.multitrack_picture` and
`multitrack_read`), which is what makes this client, the web client and the
standalone host draw the same piece and read the same report: what a row and a
box *are*, and what a list of boxes *means*, are one rule each and not one per
language. What this adds is the two things only a client knows — the axis its
window counts in, and which server buffer a source was read into.

**A report is the piece, so a gesture is however many edits it takes.** A block
drag says a move, a trim and a lane's new contents in one message; they go
through `clausters.gui.editing.Domain.payloads` and land as **one** entry, since
they are one thing a hand did.

**Beats meet frames through the piece's own tempo map**, never through a ratio:
a position is the second it falls on times the rate, and a *length* is the
difference of two of those, because four beats last longer later than earlier
under a ritardando.
"""

from ... import _native
from ...base import TempoMap
from ...multitrack import Multitrack
from .domain import Domain
from .editor import Editor
from .view import View

__all__ = ["MultitrackDomain", "MultitrackEditor", "MultitrackView", "Sources",
           "is_piece", "tempo_map"]

#: What the widget's ``lanes`` prop takes: flat ``name label height mute solo
#: gain`` sextuples.
SEXTUPLE = 6

#: What its ``clips`` prop takes: flat ``name lane offset dur start label
#: source`` septuples.
SEPTUPLE = 7

#: The thickness a row is drawn at, in logical pixels.
ROW_H = 96.0

#: The thickness an automation row is drawn at — shorter than a track's row,
#: because what it draws is one line and not a stack of boxes.
CURVE_H = 40.0

#: What the widget's ``curves`` prop takes: flat ``name lane label min max
#: height`` sextuples. Its ``layers`` prop takes the same without the height,
#: since a layer is as tall as the box it is drawn on.
CURVE_SEXTUPLE = 6

#: What its ``points`` prop takes and reports: flat ``curve t v shape amount``
#: quintuples, each naming the curve it is on.
POINT_QUINTUPLE = 5

#: The tempo a piece that never said one is read at, in beats per second — one,
#: so a beat is a second. It is the **reader's** default and not the document's:
#: a piece that said no tempo did not say one, and writing 120 into the format
#: would be deciding a musical question on its behalf.
DEFAULT_TEMPO = 1.0


def tempo_map(piece: Multitrack) -> TempoMap:
    """The piece's beat→second function, with the reader's default where the
    piece states nothing.

    One line, and a **binding** rather than a rule: the three decisions a run of
    authored entries needs — a ramp reaching the next one, the default before
    the first, an empty list being the default alone — are
    `clausters.base.TempoMap.from_changes`'s, in the crate that models tempo.
    """
    return TempoMap.from_changes(
        [{"beats": t.at, "tempo": t.bpm / 60.0, "ramp": bool(t.ramp)}
         for t in piece.tempo],
        DEFAULT_TEMPO)


class Sources:
    """Which **server buffer** each of the piece's sources was read into.

    The one thing about a piece that is not in the piece: a document names a
    source and a picture is drawn from a buffer, and only whoever loaded the
    samples knows they are the same. It is a class rather than a dict so both
    directions have a name — a box is *drawn* from a buffer and *read back* into
    a source.
    """

    def __init__(self, buffers=None):
        #: source id -> the buffer number it was read into, or **the object
        #: that holds it** — a `clausters.defs.Buffer`, a
        #: `clausters.seq.Timeline`. Both are accepted because they answer two
        #: different questions and a caller usually has the object: which buffer
        #: to draw from is `bufnum`, and what a box **opens as** is `structure`.
        self.buffers = dict(buffers or {})

    def bufnum(self, source) -> int:
        """The buffer a source was read into; ``-1`` for one nobody loaded.

        **Negative and not zero**, because buffer 0 is a buffer — the first one
        an allocator hands out. A source given as an object answers with the
        buffer it holds, and one that holds none is a box with no samples to
        draw, which is honest rather than empty.
        """
        if source is None:
            return -1
        found = self.buffers.get(int(source))
        if found is None:
            return -1
        if isinstance(found, (int, float)):
            return int(found)
        # **Buffer 0 is a buffer**, and `x or -1` says it is not: the first
        # buffer an allocator hands out came back as "nobody loaded this", so
        # the first take a script loads was the one take its boxes could not
        # draw. Asked for explicitly instead -- the same sentence the docstring
        # above has always made.
        held = getattr(found, "bufnum", None)
        return -1 if held is None else int(held)

    def structure(self, source):
        """**What a box over this source opens as** — the object a caller gave,
        or ``None`` for a source it named by number alone.

        A piece names a source and an editor edits a structure; only whoever
        loaded the samples holds both, which is the same reason this class
        exists at all.
        """
        if source is None:
            return None
        found = self.buffers.get(int(source))
        return None if isinstance(found, (int, float)) else found

    def source(self, bufnum: int):
        """The source a buffer number came from, or ``None``."""
        for source, held in self.buffers.items():
            number = held if isinstance(held, (int, float)) else getattr(held, "bufnum", None)
            if number is not None and int(number) == int(bufnum):
                return int(source)
        return None


class Bridge:
    """What a client adds to the crate's picture: an axis and a buffer table.

    Held by the domain and the view alike, because both cross the same seam —
    one drawing a box and the other reading one back — and two copies of the
    scale is how a box comes back somewhere it was not put.
    """

    def __init__(self, piece: Multitrack, *, sample_rate: float,
                 sources: "Sources | None" = None):
        self.rate = float(sample_rate)
        self.sources = sources or Sources()
        self.tempo = tempo_map(piece)

    def refresh(self, piece: Multitrack) -> None:
        """Re-read the tempo map, for an edit that moved one."""
        self.tempo = tempo_map(piece)

    def frame_at(self, beats: float) -> float:
        """Where a beat falls on the timeline, in frames."""
        return self.tempo.secs_at(float(beats)) * self.rate

    def frames_over(self, start: float, length: float) -> float:
        """How long a stretch of beats lasts there — **the difference of two
        positions**, because four beats are not one length."""
        return self.tempo.span_secs(float(start), float(start) + float(length)) * self.rate

    def beat_at(self, frame: float) -> float:
        """The beat a frame falls on: the inverse, and the way an edit comes
        back."""
        return self.tempo.beats_at(float(frame) / (self.rate or 1.0))

    def frame_in(self, base: float, at: float) -> float:
        """Where a beat measured **from ``base``** falls, in frames from
        ``base`` — what a box's own axis counts in.

        A layer is drawn inside its box, so its break-points are the box's own
        time and not the timeline's. That is a *length* from the box's start,
        which is why it goes through `frames_over` rather than through
        `frame_at`: four beats are not one length under a tempo that moves.
        """
        return self.frames_over(base, at)

    def beat_in(self, base: float, frame: float) -> float:
        """The inverse: the beat, measured from ``base``, that a frame from
        ``base`` falls on."""
        return self.beat_at(self.frame_at(base) + float(frame)) - float(base)


class MultitrackDomain(Domain):
    """A piece's vocabulary: the crate's `MultitrackIntent`, both ways.

    It reads nothing itself. A report of the boxes goes to
    `clausters._native.multitrack_read`, which is the same reader the standalone
    host uses, and an edit is applied through
    `clausters._native.domain_edit`, which is where the inverse comes from.
    """

    name = _native.MULTITRACK

    def __init__(self, bridge: Bridge):
        self.bridge = bridge

    # ---- a gesture, as edits ----

    def payloads(self, structure, tag: str, values) -> list:
        if tag == "clips":
            return _native.multitrack_read(self.state(structure),
                                           self._placed(values))
        if tag == "lanes":
            return self._strips(structure, values)
        if tag == "points":
            state = self.state(structure)
            return _native.multitrack_read_points(
                state, self._curved(values,
                                    _bases(_native.multitrack_picture(state))))
        return []

    def payload(self, structure, tag: str, values) -> "dict | None":
        """The singular door, for the one-edit case. `payloads` is what a
        multitrack actually goes through: a report is the piece, so one message
        is however many edits it takes."""
        found = self.payloads(structure, tag, values)
        return found[0] if len(found) == 1 else None

    def _placed(self, values) -> list:
        """The flat ``clips`` payload as the crate's boxes: names as they came,
        positions in beats, the window's own numbers in seconds."""
        out = []
        for group in _groups(values, SEPTUPLE):
            name, lane, at, dur, start, _label, source = group
            try:
                row = int(str(lane))
            except ValueError:
                # A row is named by its track's id and never renamed, so a name
                # that is not one names no row this piece has.
                continue
            at, dur = float(at), float(dur)
            position = self.bridge.beat_at(at)
            length = self.bridge.beat_at(at + dur) - position
            out.append({
                "name": str(name),
                "row": row,
                "position": position,
                "length": length,
                "start": float(start) / (self.bridge.rate or 1.0),
                # How much a **new** box shows: the stretch it occupies,
                # crossed to the wall clock the only way a length may be.
                "content": self.bridge.tempo.span_secs(position,
                                                       position + length),
                "source": self.bridge.sources.source(int(source)),
            })
        return out

    def _curved(self, values, bases: dict) -> list:
        """The flat ``points`` payload as the crate's curves: one entry per
        curve named, its break-points back on the musical axis.

        The widget reports **every** curve there is, in one list, so they are
        gathered by name here — the crate reads the difference and says nothing
        about the ones that did not move.
        """
        found: dict = {}
        for group in _groups(values, POINT_QUINTUPLE):
            name, at, value, shape, amount = group
            # **Against the same base the picture was drawn from**: a layer's
            # time is its box's own, so a break-point inside one comes back as
            # a beat from that box's start.
            base = float(bases.get(str(name), 0.0))
            found.setdefault(str(name), []).append(
                {"at": self.bridge.beat_in(base, float(at)),
                 "value": float(value),
                 # **What a shape is stays the client's**: the crate carries a
                 # point's data and never reads it, which is what keeps an undo
                 # from putting a bent curve back straight.
                 "data": {"shape": int(float(shape)),
                          "curve": float(amount)}})
        return [{"name": name, "points": points}
                for name, points in found.items()]

    def _strips(self, structure, values) -> list:
        """The rows' payload as the crate reads it: what a report of every row
        *means*, in the piece's one verb over its tracks.

        The whole list travels because the piece has no verb for one track — a
        report is the piece here as it is for the boxes — so the difference is
        what comes out, and it is one ``settracks`` whatever changed: a level
        moved, a track added, a track gone with its boxes.

        **The rule is the crate's**, like the boxes' and the curves': a client
        that read this payload itself would be writing the mapping a second
        time in its own language, which is how one client comes to add a track
        the other cannot.
        """
        return _native.multitrack_read_rows(self.state(structure), _rows(values))

    # ---- the state, and writing one back ----

    def state(self, structure) -> dict:
        """The piece as the crate holds it — what `current` is read against and
        what `project` writes back."""
        return structure.write()

    def current(self, structure, payload: dict) -> "dict | None":
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        return None if edited is None else edited.get("current")

    def project(self, structure, payload: dict) -> bool:
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        if edited is None or not edited.get("applied"):
            return False
        written = Multitrack.read(edited["state"])
        # The object the script holds **is** the edited one: a piece handed
        # back would be a second piece, and the caller's would go stale.
        structure.version = written.version
        structure.tracks = written.tracks
        structure.tempo = written.tempo
        structure.meter = written.meter
        structure.markers = written.markers
        structure.loop_span = written.loop_span
        structure.punch = written.punch
        # A tempo that moved changes where every box is drawn.
        self.bridge.refresh(structure)
        return True

    #: What an undo menu calls each of the piece's verbs.
    LABELS = {"placeregion": "move a clip",
              "trimregion": "trim a clip",
              "setlane": "edit the clips",
              "settracks": "mix a track",
              "splitregion": "split a clip",
              "joinregions": "join the clips",
              "setautomation": "draw a curve"}

    def label(self, payload: dict) -> str:
        return self.LABELS.get(str(payload.get("intent", "")), "edit the piece")


class MultitrackView(View):
    """One `clausters.gui.guidef.multitrack`: the whole piece, in one widget.

    A row per track and a box per region — the crate's own mapping, crossed to
    this window's axis. The widget draws its own headers and its own vertical
    scroll, so there is no stack to compose and nothing per clip to register.

    **The one thing it does not draw is the ruler**, and an editor is where a
    position is read, so the view places a `clausters.gui.guidef.timeruler`
    above it: a strip of its own, in the same navigation group as the piece, so
    it labels exactly what the lanes show and its ticks stand over the samples
    they name. It rules from above, so its marks hug its bottom edge
    (``dir="down"``, the default there).
    """

    def __init__(self, bridge: Bridge, *, link=None):
        super().__init__()
        self.bridge = bridge
        #: The navigation group the view joins, so a ruler beside it rules it.
        self.link = link
        #: The id of the strip that rules the piece, once one has been built.
        #: Kept so a correction addressed to it answers with the *ruler's* props
        #: and not with the piece's.
        self.ruler: int | None = None

    def build(self, editor) -> dict:
        from ..guidef import node, timeruler, window

        # **The props are already what the wire takes**, so the node is made
        # from them directly rather than through `clausters.gui.guidef.multitrack`,
        # whose `lanes`/`clips` are the *tuples* a script types and which would
        # flatten an already-flat list a second time — one row per number. The
        # flat form is the one `props` has to answer in anyway, since a
        # correction rides as a `/gui_set`.
        wid = self.widget(editor, "multitrack", editor.structure)
        # **The ruler is named like any other widget of this picture**, so what
        # a hand does on it comes back to this editor: the position cursor is
        # placed on the ruler and nowhere else, and an unnamed strip would put
        # that one gesture outside the only object that could hear it.
        rid = self.widget(editor, "ruler", editor.structure, "ruler")
        self.ruler = rid
        return window(timeruler(id=rid, link=self.group(wid), ruler="beats",
                                cursor=_cursor(editor),
                                sample_rate=self.bridge.rate,
                                tempo_map=self.bridge.tempo.dump()),
                      node("multitrack", id=wid, **self.props(editor, wid)),
                      *editor.extra,
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def group(self, widget_id: int) -> int:
        """**The navigation group the piece and its ruler share.**

        A ruler rules by being on the same axis as what it is beside, and an
        unlinked widget is a group of one keyed by itself — so the two would
        pan and zoom apart. The piece's own widget id names the group when the
        caller did not name one, which is the id nothing else can collide with.
        """
        return widget_id if self.link is None else self.link

    def props(self, editor, widget_id: int) -> dict:
        if widget_id == self.ruler:
            # The strip's own state, which is the axis' and nothing else: the
            # piece's payloads are the piece widget's.
            return {"cursor": _cursor(editor)}
        picture = _native.multitrack_picture(editor.structure.write())
        curves = picture.get("curves", [])
        layers = picture.get("layers", [])
        props = {
            "lanes": _lanes(picture.get("rows", [])),
            "clips": _clips(picture.get("boxes", []), self.bridge),
            "curves": _curves(curves),
            "layers": _layers(layers),
            "points": _points(curves + layers, self.bridge,
                              _bases(picture)),
            # **What is drawn is what the piece says was open.** Which curves a
            # person had showing is part of reopening the piece as they left
            # it, so it is read out of the document rather than kept here.
            "hidden": " ".join(str(c["automation"]) for c in curves + layers
                               if not c.get("visible", True)),
            # **Which boxes wrap**, by name — a name set like ``hidden``, and
            # read out of the piece for the same reason: whether a box loops is
            # what it *reads* past the end of its source, so it is the piece's
            # and not this window's. It says what an edge drag may do (a box
            # that loops has always more; one that does not stops at the last
            # frame) and how the samples draw under a box longer than they are.
            "loops": " ".join(str(b["region"]) for b in picture.get("boxes", [])
                              if b.get("looping")),
            "weight": 1.0,
            "ruler": "beats",
            "sample_rate": self.bridge.rate,
            # **The window is the reader's.** In an editor a content change is
            # mostly the reader's own edit, so the axis does not re-frame itself
            # on one; the extent is still registered.
            "autofit": False,
            # The head is anchored at 0 because the counter it sweeps from is
            # already the piece's position.
            "playhead_at": 0.0,
            # The piece's own map rules the beats, so the labels and the boxes
            # cannot disagree.
            "tempo_map": self.bridge.tempo.dump(),
            # **A piece opens with the reader at the top.** The position cursor
            # is where a playback starts, so a piece that stated none would open
            # with nowhere to play from; and it is reported from the editor's
            # own copy rather than fixed at zero, or every resync would drag the
            # mark back to the start.
            "cursor": _cursor(editor),
        }
        props["link"] = self.group(widget_id)
        return props


def _rows(values) -> list:
    """The flat ``lanes`` payload as the crate's strips.

    The label and the height are dropped rather than sent: a row's label is the
    track's name where it has one and a made-up one where it has not, and its
    height is this window's. Neither is a fact about the piece, so neither is
    reported into it.
    """
    out = []
    for group in _groups(values, SEXTUPLE):
        name, _label, _h, mute, solo, gain = group
        out.append({"name": str(name), "mute": bool(int(mute)),
                    "solo": bool(int(solo)), "gain": float(gain)})
    return out


def _cursor(editor) -> float:
    """The position cursor in timeline samples: where the editor last saw it
    placed, and the top of the piece until a hand places one."""
    return editor.beats_to_units(editor.cursor if editor.cursor is not None else 0.0)


def _lanes(rows) -> list:
    """The crate's rows as the widget's flat sextuples."""
    out = []
    for row in rows:
        out += [str(row["track"]), str(row.get("label", "")), ROW_H,
                bool(row.get("mute")), bool(row.get("solo")),
                float(row.get("gain", 1.0))]
    return out


def _curves(curves) -> list:
    """The crate's **track automations** as the widget's flat sextuples: a row
    of its own under the track it names."""
    out = []
    for curve in curves:
        lo, hi = _domain(curve)
        out += [str(curve["automation"]), str(curve["owner"]),
                str(curve.get("label", "")), lo, hi, CURVE_H]
    return out


def _layers(layers) -> list:
    """The crate's **region automations** as the widget's flat quintuples: a
    layer inside the box it names, and no height, because it is as tall as
    that box."""
    out = []
    for curve in layers:
        lo, hi = _domain(curve)
        out += [str(curve["automation"]), str(curve["owner"]),
                str(curve.get("label", "")), lo, hi]
    return out


def _domain(curve) -> tuple:
    """The value range a curve is drawn over.

    **The client's, and read out of the target.** The document says what a
    curve automates and never reads it; which range that parameter has — a gain
    over one, a pan over another — is a fact about the parameter, so it is
    stated where the parameter is. Unity is the default, which is what an
    unlabelled level means.
    """
    target = curve.get("target") or {}
    if not isinstance(target, dict):
        return 0.0, 1.0
    return float(target.get("min", 0.0)), float(target.get("max", 1.0))


def _bases(picture) -> dict:
    """**What each curve's time is measured from**, by curve name.

    A track automation runs the timeline, so it is measured from the origin; a
    clip envelope is drawn inside its box and is measured from where that box
    starts. It is the one thing that differs between the two on the wire, and
    the reason it is worked out here is that the beat→frame crossing is the
    client's.
    """
    where = {str(box["region"]): float(box["position"])
             for box in picture.get("boxes", [])}
    bases = {str(c["automation"]): 0.0 for c in picture.get("curves", [])}
    for curve in picture.get("layers", []):
        bases[str(curve["automation"])] = where.get(str(curve["owner"]), 0.0)
    return bases


def _points(curves, bridge: Bridge, bases: dict) -> list:
    """Every curve's break-points as the widget's flat quintuples, each naming
    the curve it is on — one list for the rows and the layers alike."""
    out = []
    for curve in curves:
        name = str(curve["automation"])
        base = float(bases.get(name, 0.0))
        for point in curve.get("points", []):
            data = point.get("data") or {}
            out += [name, bridge.frame_in(base, float(point.get("at", 0.0))),
                    float(point.get("value", 0.0)),
                    float(data.get("shape", 1)), float(data.get("curve", 0.0))]
    return out


def _clips(boxes, bridge: Bridge) -> list:
    """The crate's boxes as the widget's flat septuples, on this axis."""
    out = []
    for box in boxes:
        position, length = float(box["position"]), float(box["length"])
        out += [str(box["region"]), str(box["row"]),
                bridge.frame_at(position),
                bridge.frames_over(position, length),
                float(box.get("start", 0.0)) * bridge.rate,
                str(box.get("label", "")),
                bridge.sources.bufnum(box.get("source"))]
    return out


def _groups(values, n: int) -> list:
    """A flat payload as groups of ``n``; a trailing partial group is dropped
    rather than half-read, the rule every flat payload here follows."""
    return [values[i:i + n] for i in range(0, len(values) - len(values) % n, n)]


class MultitrackEditor(Editor):
    """A piece on screen, editable back into the `clausters.multitrack.Multitrack`
    the caller already holds.

    Nothing is handed back at the end: the object the script passed in *is* the
    edited one, and reading it after an edit is how a caller sees what a hand
    did. Being an `clausters.gui.editing.Editor`, it has the history every other
    editor has — `undo` and `redo` walk it, and a second window over the same
    piece walks the same one.
    """

    def __init__(self, piece: Multitrack, *, sample_rate: float,
                 sources=None, link=None, title: str = "Multitrack",
                 **options):
        bridge = Bridge(piece, sample_rate=sample_rate,
                        sources=Sources(sources) if not isinstance(sources, Sources)
                        else sources)
        #: The axis and the buffer table this window crosses to — the two things
        #: about a piece that are not in the piece.
        self.bridge = bridge
        #: The editors a hand opened by entering a box, by box name — held so a
        #: second double click on the same box raises the one that is already
        #: open rather than a second window over one structure.
        self.entered: dict = {}
        super().__init__(piece, sample_rate=sample_rate,
                         tempo_map=bridge.tempo,
                         domain=MultitrackDomain(bridge),
                         view=MultitrackView(bridge, link=link),
                         title=title, **options)

    def interface(self, widget_id: int, tag: str, values) -> bool:
        """**A box was entered** — the double click the multitrack reports as
        ``"enter"``, with the box's name."""
        if tag != "enter" or not values:
            return False
        self.enter(str(values[0]))
        return True

    def enter(self, name: str):
        """Open the contents of the box called ``name`` in an editor of its
        own, and return it (``None`` for a box with nothing to open).

        **The multitrack places; a box is entered to edit.** What a box holds
        is a structure like any other — a take's samples, a timeline of notes —
        so entering one is `clausters.gui.editing.edit` over that structure,
        with no second implementation of any editor.

        **One undo order, and it is the piece's.** The editor is opened on this
        piece's editing context, so a note written inside a box and a box
        dragged on the stack walk one history: an undo that needed a window
        reopened to reach it is a hole in the order that does not announce
        itself. What that costs is that the entered structure stays in the
        context while the piece is open even if its window is closed — which
        the context already does, since it holds what it registered.

        The object comes from `Sources`, which is where the one fact about a
        piece that is not in the piece already lives: the document names a
        source and only whoever loaded it holds the structure.
        """
        from .edit import edit

        found = self.entered.get(name)
        if found is not None:
            return found
        region = _region(self.structure, name)
        if region is None:
            return None
        held = self.bridge.sources.structure(_source_of(region))
        if held is None:
            return None
        opened = edit(held, sample_rate=self.bridge.rate,
                      context=self._editing, host=self._host,
                      title=str(region.name or name),
                      # **On the host the piece is on, or on no screen at
                      # all.** A piece that was never opened has no window to
                      # enter one *from*, and resolving an ambient host there
                      # would put a box on screen while the piece it belongs to
                      # is not.
                      open=self._host is not None)
        self.entered[name] = opened
        # **A window the reader closed is enterable again**, and it is the only
        # way one leaves this table: an editor that is merely not on screen is
        # still the one that box is open in, so a second double click raises it
        # rather than making a second editor over one structure.
        if self._host is not None:
            opened.on_closed(lambda: self.entered.pop(name, None))
        return opened

    def close(self):
        """Close this piece's window, and the boxes opened out of it with it.

        A window entered *from* the piece is part of looking at the piece: what
        outlives both is the history, which is the data's and was never a
        window's.
        """
        for opened in list(self.entered.values()):
            if not opened.closed:
                opened.close()
        # The handlers cleared their own entries; this is for the ones that
        # were never on screen to clear.
        self.entered.clear()
        super().close()


def _region(piece: Multitrack, name: str):
    """The region of this name, wherever it is, and ``None`` for a box the
    piece has none of.

    A box is named by its region's id, so the name is the address — the same
    thing that makes a report readable with no map on the side.
    """
    try:
        wanted = int(name)
    except ValueError:
        return None
    for track in piece.tracks:
        for lane in track.lanes:
            for region in lane.regions:
                if int(region.id) == wanted:
                    return region
    return None


def _source_of(region):
    """The source a region is a window onto, or ``None`` for a box that is a
    window onto something else."""
    window = (region.content.write() if hasattr(region.content, "write")
              else region.content)
    if not isinstance(window, dict):
        return None
    onto = window.get("window") or {}
    source = (onto.get("source") or {}).get("source")
    return None if source is None else int(source)


def is_piece(structure) -> bool:
    """Whether `edit` should open this as a multitrack."""
    return isinstance(structure, Multitrack)
