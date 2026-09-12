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
from ...defs import Buffer, Part
from ...multitrack import Multitrack
from .domain import Domain
from .editor import Editor
from .view import View

__all__ = ["MultitrackDomain", "MultitrackEditor", "MultitrackView", "Sources",
           "is_piece", "tempo_map"]

#: The names the transport row's three widgets carry. A name and not an id,
#: because these are the widgets a **hand** addresses and a handler is hung on a
#: name — and they are the piece's own, so a script's ``extra`` may carry
#: anything it likes beside them.
REWIND = "piece_rewind"
PLAY = "piece_play"
STOP = "piece_stop"
CLOCK = "piece_clock"

#: How often the read-out asks the engine where the piece is, in seconds. The
#: *line* asks nothing — the host draws it from the segment every frame — so this
#: is the price of the number beside it and nothing else.
CLOCK_TICK = 0.05

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

    def table(self) -> dict:
        """The whole table as the instance plan reads it: source id ->
        ``{"buffer": n, "channels": n}``.

        The one fact about a piece that is not in the piece, handed to the crate
        so it can say which slot a box goes in -- a mono take is panned into its
        track and a stereo one is balanced, and that follows from the source's
        width and nothing else. A source nobody loaded is left out, and a box
        over it is simply not playing yet.
        """
        out: dict = {}
        for source in self.buffers:
            bufnum = self.bufnum(source)
            if bufnum < 0:
                continue
            held = self.buffers[source]
            channels = getattr(held, "channels", 1)
            out[int(source)] = {"buffer": bufnum,
                                "channels": max(1, int(channels or 1))}
        return out

    def width(self, source) -> int:
        """How many channels a source has, ``1`` for one that does not say.

        What a **join** needs and the buffer number alone cannot answer: how
        wide the assembled thing is follows from the takes it is over.
        """
        if source is None:
            return 1
        held = self.buffers.get(int(source))
        return max(1, int(getattr(held, "channels", 1) or 1))

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
                 sources: "Sources | None" = None, server=None):
        self.rate = float(sample_rate)
        self.sources = sources or Sources()
        #: The server the takes are on, for the one thing an edit needs one
        #: for: **a source an edit makes**. A join owns no samples, so what
        #: reaches the server is the list of spans and never the audio.
        self.server = server
        self.tempo = tempo_map(piece)
        #: The tempo, in beats per minute, a piece that states none is read at.
        #: The reader's own: a piece that never said a tempo did not say one,
        #: and a document that invented 120 would be deciding a musical
        #: question.
        self.bpm = DEFAULT_TEMPO * 60.0

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
    ingested = True

    def __init__(self, bridge: Bridge):
        super().__init__()
        self.bridge = bridge

    def request(self, structure, tag: str, values) -> dict:
        """The report, the piece it is over, and the axis a beat lands on.

        A report of the boxes, the rows or the break-points is the **whole**
        structure rather than the gesture, so the piece has to be in hand for
        the reading to say what the difference is. The rate and the source table
        are the same two the picture is drawn with, which is what keeps a box
        from going out on one axis and coming back on another.
        """
        return {"values": list(values), "state": self.state(structure),
                "rate": float(self.bridge.rate),
                "defaultBpm": float(self.bridge.bpm),
                "sources": {str(k): v
                            for k, v in self.bridge.sources.table().items()}}

    # ---- the state, and writing one back ----

    def state(self, structure) -> dict:
        """The piece as the crate holds it — what `current` is read against and
        what `project` writes back."""
        return structure.write()

    def current(self, structure, payload: dict) -> "dict | None":
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        return None if edited is None else edited.get("current")

    def project(self, structure, payload: dict) -> bool:
        # **A source the edit makes is made before the edit lands.** A join over
        # fragments mints the source its box is a window onto, and a box over a
        # source nothing answers for is left out of the plan -- so realizing it
        # after the piece already names it would be one pass of silence. It runs
        # again on a redo, which is right: the source is gone the moment nothing
        # windows it.
        self._mint(payload.get("source"))
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

    def _mint(self, minted) -> None:
        """Install a source an edit made, and put it in the table.

        The one place a client answers a document's statement with a server
        command. A join owns no samples -- it is spans of the takes the table
        already holds -- so this costs the list of parts and not the audio, and
        freeing a take something is stitched over does not silence it.

        A part whose source nobody loaded leaves the join unmade rather than
        half made: a box over it draws empty and does not play, which is what a
        source nobody answered for has always meant here.
        """
        if not isinstance(minted, dict):
            return
        source = minted.get("id")
        parts = (minted.get("location") or {}).get("parts")
        if source is None or not parts:
            return
        # **Written over rather than skipped.** The document has just said what
        # this source is; a table entry under that id is either the same join
        # being redone or something the id was reused for, and in both cases
        # what the piece now names is this.
        made = []
        for part in parts:
            ref = part.get("source") or {}
            bufnum = self.bridge.sources.bufnum(ref.get("source"))
            if bufnum < 0:
                return
            span = ref.get("range") or {}
            start = int(span.get("start", 0))
            made.append(Part(bufnum, start=start,
                             frames=int(span.get("end", start)) - start,
                             fade_in=int(part.get("fade_in", 0)),
                             fade_out=int(part.get("fade_out", 0)),
                             channels=part.get("channels")))
        width = minted.get("channels") or max(
            (self.bridge.sources.width(p.get("source", {}).get("source"))
             for p in parts), default=1)
        # **Sent rather than waited on.** This runs inside the answer to a
        # gesture, and a join owns no samples: there is nothing to copy and
        # nothing to load, so the buffer number is known the moment it is
        # handed out and blocking on the `/done` would only stall the hand.
        self.bridge.sources.buffers[int(source)] = Buffer.stitch(
            made, channels=int(width),
            sample_rate=float(minted.get("sample_rate") or 0.0),
            wait=False, server=self.bridge.server)


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

    def __init__(self, bridge: Bridge, *, link=None, transport: bool = False):
        super().__init__()
        self.bridge = bridge
        #: The navigation group the view joins, so a ruler beside it rules it.
        self.link = link
        #: The id of the strip that rules the piece, once one has been built.
        #: Kept so a correction addressed to it answers with the *ruler's* props
        #: and not with the piece's.
        self.ruler: int | None = None
        #: The id of the piece's own widget, once one has been built — what a
        #: playhead is drawn on, so whoever moves the line does not have to
        #: guess which of the two ids is the picture.
        self.piece: int | None = None
        #: The row, box and curve names the host was last told — see `props`.
        self.told: "tuple | None" = None
        #: Whether the window carries the transport row. It is the *view's* and
        #: not a script's ``extra``: a piece that can be heard is played from the
        #: window it is drawn in, and every window over a piece has the same
        #: three controls in the same place.
        self.transport = bool(transport)

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
        self.piece = wid
        return window(timeruler(id=rid, link=self.group(wid), ruler="beats",
                                cursor=_cursor(editor),
                                sample_rate=self.bridge.rate,
                                tempo_map=self.bridge.tempo.dump()),
                      node("multitrack", id=wid, **self.props(editor, wid)),
                      *(self.chrome() if self.transport else ()),
                      *editor.extra,
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def chrome(self) -> tuple:
        """The transport row: rewind, play/pause, stop, and where the piece is.

        Named rather than numbered, because these are the only widgets of this
        window a *hand* addresses and a name is what a handler is hung on. The
        names are the piece's own (``piece_*``), so a script's ``extra`` may
        carry anything it likes beside them.
        """
        from ..guidef import button, label, layout

        # **Rewind is not stop.** Stop goes back to the *mark* -- which is
        # what tells it from pause -- and the mark is wherever a hand last put
        # it, so with nothing else the way back to the top is finding beat zero
        # on screen and clicking it. Rewind puts the mark there, which is a
        # statement about the cursor and not about the transport.
        return (layout(button(label="|<", name=REWIND, w=44.0),
                       button(label="play/pause", name=PLAY, w=110.0),
                       button(label="stop", name=STOP, w=110.0),
                       label("", name=CLOCK, text_size=2.0, weight=1.0),
                       flow="row", h=40.0, gap=6.0),)

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
        # **The piece's own props are the projection's**: the rows, the boxes,
        # the automations over both, their break-points, which of them are
        # hidden and which boxes loop. All of it is a function of the piece and
        # of where a beat lands, so all of it is written once and every client
        # and the standalone host ask the same question
        # (`clausters._native.multitrack_props`).
        props = {
            **_native.multitrack_props(editor.structure.write(), self.bridge.rate,
                                       self.bridge.bpm,
                                       self.bridge.sources.table()),
            # **Where each track's level is read from**: the control buses its
            # meters write, which the host reads every frame straight out of the
            # shared segment. A piece with no playback names none, and a header
            # with nothing to read draws no strip.
            "meters": _meters(editor),
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
        #: **What the host was last told things are called.** The rows, the
        #: boxes and the curves, by name. A row or a box the *host* made carries
        #: a word it minted (``track 1``, ``white 2``); the id is the document's
        #: and is minted when the report is read, so until the picture goes back
        #: the two are naming the same thing differently -- and every later
        #: report about it names something the piece does not have, which mints
        #: it **again**. A **curve** is the other half of the same fact: one the
        #: owner made is one the host cannot have drawn, because it did not make
        #: it. `MultitrackEditor.data_changed` compares this with what the piece
        #: now holds and answers with the picture when they differ.
        named = _native.multitrack_names(editor.structure.write())
        self.told = (frozenset(named["rows"]), frozenset(named["boxes"]),
                     frozenset(named["curves"]))
        return props


def _cursor(editor) -> float:
    """The position cursor in timeline samples: where the editor last saw it
    placed, and the top of the piece until a hand places one."""
    return editor.beats_to_units(editor.cursor if editor.cursor is not None else 0.0)



def _meters(editor) -> list:
    """The playback's meter buses as the widget's flat quadruples: ``lane``,
    the first bus of the level run, the first of the mark run, and how many
    channels each run is."""
    playback = getattr(editor, "playback", None)
    if playback is None:
        return []
    out = []
    for track, (bus, channels) in playback.meters.items():
        out += [str(track), int(bus.index), int(bus.index) + channels, channels]
    return out


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
                 sources=None, link=None, server=None,
                 title: str = "Multitrack", **options):
        bridge = Bridge(piece, sample_rate=sample_rate, server=server,
                        sources=Sources(sources) if not isinstance(sources, Sources)
                        else sources)
        #: The axis and the buffer table this window crosses to — the two things
        #: about a piece that are not in the piece.
        self.bridge = bridge
        #: The editors a hand opened by entering a box, by box name — held so a
        #: second double click on the same box raises the one that is already
        #: open rather than a second window over one structure.
        self.entered: dict = {}
        #: What the piece **sounds** as, when it can be heard at all:
        #: `clausters.gui.editing.playback.Playback` over the server this was
        #: given, and ``None`` for a piece opened with none. A piece nobody can
        #: play still edits, which is why it is an argument and not a
        #: requirement.
        self.playback = None
        super().__init__(piece, sample_rate=sample_rate,
                         tempo_map=bridge.tempo,
                         domain=MultitrackDomain(bridge),
                         view=MultitrackView(bridge, link=link,
                                             transport=server is not None),
                         title=title, **options)
        self._shown = None
        if server is not None:
            from .playback import Playback

            self.playback = Playback(self, server=server)

    @property
    def piece_widget(self) -> "int | None":
        """The id of the piece's own widget — what a playhead is drawn on.
        ``None`` before the picture has been drawn once."""
        return getattr(self.view, "piece", None)

    # ---- the piece, heard ----

    def open(self, host=None, id: "int | None" = None):
        """Open the window, and hang the transport row on the playback.

        The wiring is here rather than in the constructor because that is where
        the window comes into being: `clausters.gui.edit` builds the editor and
        opens it in two steps, so a piece has its readers before it has a screen
        — which is the right order anyway, since a piece can be played by a
        script that never draws it.
        """
        window = super().open(host, id)
        if self.playback is not None and self._window is not None:
            self.playback.attach(self._host)
            self.window[REWIND].on_click(self.rewind)
            self.window[PLAY].on_click(self.toggle)
            self.window[STOP].on_click(self.stop)
            self._tick()
            self._host.clock.sched(CLOCK_TICK, self._tick)
        return window

    def _tick(self):
        """The read-out, and the one round trip: the position is the engine's.

        Returns the delay until the next reading, or ``None`` once the window is
        gone — which is how a scheduled tick stops without anybody stopping it.
        """
        if self.closed or self.playback is None:
            return None
        end = self.structure.end
        text = f"{self.playback.position:8.3f} s   of {end:.3f} s"
        if text != self._shown:
            self.window[CLOCK].set(text=text)
            self._shown = text
        return CLOCK_TICK

    def toggle(self):
        """Play, or pause where it stands. A pause freezes the governed group,
        so playing again continues rather than starting over."""
        if self.playback is None:
            return
        if self.playback.playing:
            self.playback.pause()
        else:
            self.playback.play()

    def rewind(self):
        """Put the **position cursor** back at the top, and cue a stopped
        transport there.

        The cursor's own verb, not the transport's: it is where the next play
        starts, and stop goes back to it rather than to the top. A hand that
        has been working at bar forty otherwise has to find beat zero on screen
        to get back to it.
        """
        self.cursor = 0.0
        self.locate(0.0)
        # The host owns where the cursor *is*, so it is told rather than left
        # to find out on the next redraw -- the same way the transport tells it
        # where the playhead stands.
        if self._host is not None and self.piece_widget is not None:
            self._host.set(self.piece_widget, cursor=0.0)

    def play(self):
        """Play the piece from where the position cursor is."""
        if self.playback is not None:
            self.playback.play()

    def pause(self):
        """Freeze the piece where it stands."""
        if self.playback is not None:
            self.playback.pause()

    def stop(self):
        """Halt and go back to the mark the position cursor is on."""
        if self.playback is not None:
            self.playback.stop()

    def locate(self, beat: float):
        """The position cursor was placed, here or in a window entered from
        here: cue a stopped transport there and leave a rolling one alone.

        This is what a box's own ruler reaches, because a structure inside a
        piece has no transport of its own — the piece is the one that has one.
        """
        if self.playback is not None:
            self.playback.cue(beat)

    def data_changed(self) -> None:
        """The piece changed, whoever changed it: put the readers where it now
        says they are, and then tell the script.

        The readers go first because the script's own handler may look at what is
        sounding, and because a piece is a statement: making it true again is not
        a reaction to an edit, it is the same call the first one was.
        """
        if self.playback is not None:
            self.playback.sync()
        self._answer_with_the_picture()
        super().data_changed()

    def _answer_with_the_picture(self) -> None:
        """**A name the host minted is answered with the one the piece kept.**

        A gesture is normally answered with an acknowledgement and nothing else,
        because the report described the result: the host drew what it sent and
        the piece agreed. The cases where it does not are the ones where the
        host **makes** something -- a track from a double click, a box from a
        split or a paste. There the host mints the word (``track 1``,
        ``white 2``) and the document mints the id, so until the picture goes
        back the two are naming the same thing differently.

        And a name the piece does not know is not ignored: it is read as
        something *new*. So the next report about that row or that box mints it
        again, and again after that -- a split box took a fresh id on every
        drag, losing whatever was hung on it, and a box dropped on a new track
        landed on a track nobody had.

        So when the names the host was last told differ from the ones the piece
        now holds, the whole picture goes back as a correction. It carries the
        `meters` prop with it, which is the other half of the same fact for a
        track: one that reached the server has buses to read, and a host that
        never heard of it draws no strip.
        """
        view, piece = self.view, self.piece_widget
        if self._host is None or self._window is None or piece is None:
            return
        told = getattr(view, "told", None)
        if told is None:
            return
        # **What the piece calls them is the crate's** — a flat prop's shape is
        # not a fact to restate at a call site, and which boxes a piece has is
        # not this client's arithmetic either.
        named = _native.multitrack_names(self.structure.write())
        if told == (frozenset(named["rows"]), frozenset(named["boxes"]),
                    frozenset(named["curves"])):
            return
        self._corrections = []
        self._resync(piece)
        self._acknowledge(0)
        self._corrections = []

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
        # **The piece is what this window is composed inside**, which is what a
        # ruler clicked in there needs: a take has no transport of its own, so
        # the position it places is the piece's to act on.
        opened.composed_in = self
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
        if self.playback is not None:
            self.playback.close()
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
