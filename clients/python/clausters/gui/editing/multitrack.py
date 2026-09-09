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
        #: source id -> buffer number.
        self.buffers = dict(buffers or {})

    def bufnum(self, source) -> int:
        """The buffer a source was read into; ``-1`` for one nobody loaded.

        **Negative and not zero**, because buffer 0 is a buffer — the first one
        an allocator hands out.
        """
        if source is None:
            return -1
        return int(self.buffers.get(int(source), -1))

    def source(self, bufnum: int):
        """The source a buffer number came from, or ``None``."""
        for source, buf in self.buffers.items():
            if int(buf) == int(bufnum):
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

    def _strips(self, structure, values) -> list:
        """The mixer's payload: mute, solo and the fader, in the piece's one
        verb over a track.

        A strip saying what the track already says is not an edit, which is
        what keeps one fader drag from rewriting every track — and the whole
        list travels because the piece has no verb for one track.
        """
        # **The piece is not touched here.** A payload states what the piece
        # *would* be; the inverse is read against what it is, and mutating
        # first would leave nothing to read -- a fader that moved and an undo
        # that put it back where it already was.
        tracks = [t.write() for t in structure.tracks]
        by_id = {int(t.get("id")): t for t in tracks}
        changed = False
        for group in _groups(values, SEXTUPLE):
            name, _label, _h, mute, solo, gain = group
            try:
                found = by_id.get(int(str(name)))
            except ValueError:
                continue
            if found is None:
                continue
            muted, soloed, level = bool(int(mute)), bool(int(solo)), float(gain)
            held = float((found.get("config") or {}).get("level", 1.0))
            if (bool(found.get("muted")) == muted
                    and bool(found.get("soloed")) == soloed and held == level):
                continue
            found["muted"], found["soloed"] = muted, soloed
            # The fader is this client's key in an opaque table, so it is
            # written over what is there: a track's config is its instrument
            # and its routing too.
            found["config"] = {**(found.get("config") or {}), "level": level}
            changed = True
        if not changed:
            return []
        return [{"intent": "settracks", "tracks": tracks}]

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
              "joinregions": "join the clips"}

    def label(self, payload: dict) -> str:
        return self.LABELS.get(str(payload.get("intent", "")), "edit the piece")


class MultitrackView(View):
    """One `clausters.gui.guidef.multitrack`: the whole piece, in one widget.

    A row per track and a box per region — the crate's own mapping, crossed to
    this window's axis. The widget draws its own ruler, its own headers and its
    own vertical scroll, so there is no stack to compose and nothing per clip to
    register.
    """

    def __init__(self, bridge: Bridge, *, link=None):
        super().__init__()
        self.bridge = bridge
        #: The navigation group the view joins, so a ruler beside it rules it.
        self.link = link

    def build(self, editor) -> dict:
        from ..guidef import node, window

        # **The props are already what the wire takes**, so the node is made
        # from them directly rather than through `clausters.gui.guidef.multitrack`,
        # whose `lanes`/`clips` are the *tuples* a script types and which would
        # flatten an already-flat list a second time — one row per number. The
        # flat form is the one `props` has to answer in anyway, since a
        # correction rides as a `/gui_set`.
        wid = self.widget(editor, "multitrack", editor.structure)
        return window(node("multitrack", id=wid, **self.props(editor, wid)),
                      *editor.extra,
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def props(self, editor, widget_id: int) -> dict:
        picture = _native.multitrack_picture(editor.structure.write())
        props = {
            "lanes": _lanes(picture.get("rows", [])),
            "clips": _clips(picture.get("boxes", []), self.bridge),
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
        }
        if self.link is not None:
            props["link"] = self.link
        return props


def _lanes(rows) -> list:
    """The crate's rows as the widget's flat sextuples."""
    out = []
    for row in rows:
        out += [str(row["track"]), str(row.get("label", "")), ROW_H,
                bool(row.get("mute")), bool(row.get("solo")),
                float(row.get("gain", 1.0))]
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
        super().__init__(piece, sample_rate=sample_rate,
                         tempo_map=bridge.tempo,
                         domain=MultitrackDomain(bridge),
                         view=MultitrackView(bridge, link=link),
                         title=title, **options)


def is_piece(structure) -> bool:
    """Whether `edit` should open this as a multitrack."""
    return isinstance(structure, Multitrack)
