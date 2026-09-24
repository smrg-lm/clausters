"""Editing a **multitrack**: its vocabulary, its picture and its editor.

The multitrack, as one of the three fundamental structures gets: a
`clausters.gui.editing.Domain` that puts a step of the history back onto the
multitrack, a `clausters.gui.editing.View` that is the editor's window, and an
editor that is `clausters.gui.editing.Editor` with those two in it.

**Nothing here decides anything about a turn.** What a message from the host
is, what a gesture means, how an edit inverts, what the window is and what the
host is answered with are the shared crate's
(`clausters._native.EditingCore`), which the standalone host runs and
the web client binds too. What this adds is what a language owns: the
`clausters.multitrack.Multitrack` object a script holds and gets written back
onto, which server buffer a source was read into, and the socket.

**Seconds meet frames through the rate alone.** A multitrack is placed in
seconds, so a position and a length are each their seconds times the rate, and
no tempo is involved; the tempo map the multitrack holds
(`clausters.multitrack.Multitrack.tempo_map`) is what its ruler draws beats and
bars from, which the shared crate composes.
"""

from ... import _native
from ...defs import Buffer, Part
from ...multitrack import Multitrack
from .domain import Domain
from .editor import Editor
from .view import View

__all__ = ["MultitrackDomain", "MultitrackEditor", "MultitrackView", "Sources",
           "is_multitrack"]

#: The names the transport row's three widgets carry. A name and not an id,
#: because these are the widgets a **hand** addresses and a handler is hung on a
#: name -- and they are the multitrack's own, so a script's ``extra`` may carry
#: anything it likes beside them.
REWIND = "transport_rewind"
PLAY = "transport_play"
STOP = "transport_stop"
CLOCK = "transport_clock"

#: How often the read-out asks the engine where the multitrack is, in seconds. The
#: *line* asks nothing -- the host draws it from the segment every frame -- so this
#: is the price of the number beside it and nothing else.
CLOCK_TICK = 0.05

class Sources:
    """Which **server buffer** each of the multitrack's sources was read into.

    The one thing about a multitrack that is not in the multitrack: a document names a
    source and a picture is drawn from a buffer, and only whoever loaded the
    samples knows they are the same. It is a class rather than a dict so both
    directions have a name -- a box is *drawn* from a buffer and *read back* into
    a source.
    """

    def __init__(self, buffers=None):
        #: source id -> the buffer number it was read into, or **the object
        #: that holds it** (a `clausters.defs.Buffer`), since a caller usually
        #: has the object; `bufnum` reads the number off either.
        self.buffers = dict(buffers or {})

    def bufnum(self, source) -> int:
        """The buffer a source was read into; ``-1`` for one nobody loaded.

        **Negative and not zero**, because buffer 0 is a buffer -- the first one
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

    def held(self) -> dict:
        """The table as a **join** and a **picture** read it: source id ->
        ``{"buffer", "channels", "frames", "rate"}``.

        `table` plus two facts about the samples themselves, which a plan does
        not need and these do. The **length**, which a join needs: a part that
        names no range contributes the whole of its source, and only whoever
        loaded it knows how much that is. And the **rate they were written
        at**, which a picture needs: a box is placed and drawn in the session's
        samples and filled with its source's frames, and those are the same
        number only while the two rates are -- a 44.1 kHz take on a 48 kHz
        session is drawn 8.8% longer than its samples without it. Zero means
        unknown, and a source that says nothing is read at the session's own
        rate.
        """
        out: dict = {}
        for source, entry in self.table().items():
            held = self.buffers[source]
            entry = dict(entry)
            # A length only where the object states one as a number: a
            # timeline's `frames` is not a count, and 0 is read as unknown.
            frames = getattr(held, "frames", 0)
            known = isinstance(frames, (int, float)) and not isinstance(frames, bool)
            entry["frames"] = max(0, int(frames)) if known else 0
            rate = getattr(held, "sample_rate", 0.0)
            known = isinstance(rate, (int, float)) and not isinstance(rate, bool)
            entry["rate"] = max(0.0, float(rate)) if known else 0.0
            out[source] = entry
        return out

    def table(self) -> dict:
        """The whole table as the instance plan reads it: source id ->
        ``{"buffer": n, "channels": n}``.

        The one fact about a multitrack that is not in the multitrack, handed to the crate
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

    def source(self, bufnum: int):
        """The source a buffer number came from, or ``None``."""
        for source, held in self.buffers.items():
            number = held if isinstance(held, (int, float)) else getattr(held, "bufnum", None)
            if number is not None and int(number) == int(bufnum):
                return int(source)
        return None


class Bridge:
    """What a client adds to the crate's picture: an axis and a buffer table.

    Held by the domain and the view alike, because both cross the same seam --
    one drawing a box and the other reading one back -- and two copies of the
    scale is how a box comes back somewhere it was not put.
    """

    def __init__(self, multitrack: Multitrack, *, sample_rate: float,
                 sources: "Sources | None" = None, server=None):
        self.rate = float(sample_rate)
        self.sources = sources or Sources()
        #: The server the takes are on, for the one thing an edit needs one
        #: for: **a source an edit makes**. A join owns no samples, so what
        #: reaches the server is the list of spans and never the audio.
        self.server = server


class MultitrackDomain(Domain):
    """A multitrack's vocabulary, as the **history** walks it.

    It reads no gesture and decides no edit: a gesture is the editor's turn,
    and the turn is the crate's (`clausters._native.EditingCore`).
    What is left is what the history registers a structure for -- putting a step
    back onto the multitrack -- and that goes through the same editor, so an undo and
    an edit apply by one rule.
    """

    name = _native.MULTITRACK

    def __init__(self, bridge: Bridge):
        super().__init__()
        self.bridge = bridge
        #: The editor this domain was made for.
        self.editor = None

    def state(self, structure) -> dict:
        """The multitrack as the crate holds it."""
        return structure.write()

    def stepped(self, structure, applied: dict) -> None:
        """Carry out a step of the history the context applied to the multitrack: a
        source the edit mints, and the multitrack as it now stands written back onto
        the object the script holds.

        **The source first.** A join over fragments mints the source its box is
        a window onto, and a box over a source nothing answers for is left out of
        the plan -- so building it after the multitrack names it would be one pass of
        silence. It runs again on a redo, which is right: the source is gone the
        moment nothing windows it.
        """
        self._mint(applied.get("minted"))
        if applied.get("applied") and applied.get("multitrack") is not None:
            self.write_back(structure, applied["multitrack"])

    def write_back(self, structure, state: dict) -> None:
        """Write a multitrack the crate answered onto **the object the script
        holds**: a multitrack handed back would be a second multitrack, and the caller's
        would go stale."""
        written = Multitrack.read(state)
        structure.version = written.version
        structure.tracks = written.tracks
        structure.tempo = written.tempo
        structure.meter = written.meter
        structure.markers = written.markers
        structure.loop_span = written.loop_span
        structure.punch = written.punch

    def _mint(self, minted) -> None:
        """Install a source an edit made, and put it in the table.

        The one place a client answers a document's statement with a server
        command. A join owns no samples -- it is spans of the takes the table
        already holds -- so this costs the list of parts and not the audio, and
        freeing a take something is stitched over does not silence it.

        **What the join is comes from the crate** (`_native.editing_stitch`):
        its width, its spans and the channel map a narrow part fills it with,
        read once for every endpoint that builds one. What is left here is
        the command and the table.

        A part whose source nobody loaded leaves the join unmade rather than
        half made: a box over it draws empty and does not play, which is what a
        source nobody answered for has always meant here.
        """
        if not isinstance(minted, dict) or minted.get("id") is None:
            return
        made = _native.editing_stitch(minted, self.bridge.sources.held())
        if not made:
            return
        parts = [Part(int(p["buffer"]), start=int(p["start"]),
                      frames=int(p["frames"]), fade_in=int(p["fadeIn"]),
                      fade_out=int(p["fadeOut"]), channels=list(p["channels"]))
                 for p in made["parts"]]
        # **Sent rather than waited on.** This runs inside the answer to a
        # gesture, and a join owns no samples: there is nothing to copy and
        # nothing to load, so the buffer number is known the moment it is
        # handed out and blocking on the `/done` would only stall the hand.
        # **Written over rather than skipped**: the document has just said
        # what this source is.
        self.bridge.sources.buffers[int(minted["id"])] = Buffer.stitch(
            parts, channels=int(made["channels"]),
            sample_rate=float(made["rate"]), wait=False,
            server=self.bridge.server)


class MultitrackView(View):
    """The multitrack editor's window: the multitrack, ruled from above, with the
    transport row under it when the multitrack can be heard.

    **The window is the application's**, composed in the shared crate
    (`clausters._native.EditingCore`), so this client, the web client
    and the standalone host open the same one. What is left here is the two
    widget ids a hand's gestures come back on, named like every widget of a
    picture.
    """

    def __init__(self, bridge: Bridge, *, link=None, transport: bool = False):
        super().__init__()
        self.bridge = bridge
        #: The navigation group the view joins, so a ruler beside it rules it.
        self.link = link
        #: The id of the strip that rules the multitrack, once one has been built.
        self.ruler: int | None = None
        #: The id of the multitrack's own widget, once one has been built -- what a
        #: playhead is drawn on, so whoever moves the line does not have to
        #: guess which of the two ids is the picture.
        self.multitrack: int | None = None
        #: Whether the window carries the transport row. It is the *view's* and
        #: not a script's ``extra``: a multitrack that can be heard is played from the
        #: window it is drawn in, and every window over a multitrack has the same
        #: three controls in the same place.
        self.transport = bool(transport)

    def build(self, editor) -> dict:
        # **The ruler is named like any other widget of this picture**, so what
        # a hand does on it comes back to this editor: the position cursor is
        # placed on the ruler and nowhere else, and an unnamed strip would put
        # that one gesture outside the only object that could hear it.
        wid = self.widget(editor, "multitrack", editor.structure)
        rid = self.widget(editor, "ruler", editor.structure, "ruler")
        self.ruler = rid
        self.multitrack = wid
        editor._sync_core()
        tree = editor._call("window", widget=wid, ruler=rid)
        # **A script's own widgets are its objects**, and a widget built over a
        # live source keeps a binding no JSON carries -- so they are appended
        # here rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        editor._sync_core()
        return editor._call("props", widget=int(widget_id))


def _plain(value):
    """An event's arguments as JSON carries them. A blob belongs to a widget
    this editor did not draw, and it crosses as nothing."""
    if isinstance(value, (bytes, bytearray, memoryview)):
        return None
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    return value


class MultitrackEditor(Editor):
    """A multitrack on screen, editable back into the `clausters.multitrack.Multitrack`
    the caller already holds.

    Nothing is handed back at the end: the object the script passed in *is* the
    edited one, and reading it after an edit is how a caller sees what a hand
    did. Being an `clausters.gui.editing.Editor`, it has the history every other
    editor has -- `undo` and `redo` walk it, and a second window over the same
    multitrack walks the same one.
    """

    def __init__(self, multitrack: Multitrack, *, sample_rate: float,
                 sources=None, link=None, server=None,
                 title: str = "Multitrack", **options):
        bridge = Bridge(multitrack, sample_rate=sample_rate, server=server,
                        sources=Sources(sources) if not isinstance(sources, Sources)
                        else sources)
        #: The axis and the buffer table this window crosses to -- the two things
        #: about a multitrack that are not in the multitrack.
        self.bridge = bridge
        #: What the multitrack **sounds** as, when it can be heard at all:
        #: `clausters.gui.editing.playback.Playback` over the server this was
        #: given, and ``None`` for a multitrack opened with none. A multitrack nobody can
        #: play still edits, which is why it is an argument and not a
        #: requirement.
        self.playback = None
        domain = MultitrackDomain(bridge)
        super().__init__(multitrack, sample_rate=sample_rate,
                         domain=domain,
                         view=MultitrackView(bridge, link=link,
                                             transport=server is not None),
                         title=title, **options)
        domain.editor = self
        #: **The editor's turns, in the shared crate**: a member of this multitrack's
        #: editing context, which reads a message, records what a gesture did
        #: and takes the steps of the one order the multitrack shares with whatever
        #: else is open in it.
        self._member, self._structure_id = self._editing.open(
            "openMultitrack", f"multitrack:{id(multitrack)}", {
                "multitrack": multitrack.write(), "rate": float(sample_rate),
                "link": link, "transport": server is not None, "title": title,
                "w": int(self.size[0]), "h": int(self.size[1])},
            multitrack, domain)
        self._shown = None
        #: The transport row's ids, once the window has numbered them.
        self._controls = None
        if server is not None:
            from .playback import Playback

            self.playback = Playback(self, server=server)

    @property
    def multitrack_widget(self) -> "int | None":
        """The id of the multitrack's own widget -- what a playhead is drawn on.
        ``None`` before the picture has been drawn once."""
        return getattr(self.view, "multitrack", None)

    # ---- the crate's turns ----

    def _sync_core(self) -> None:
        """Hand the core what this client holds: the multitrack a script may have
        changed, the buffer table, the meters, the cursor and the window."""
        playback = self.playback
        meters = [] if playback is None else [
            {"track": int(track), "bus": int(bus), "channels": int(channels)}
            for track, (bus, channels) in playback.meters.items()]
        self._call(
            "sync", multitrack=self.structure.write(),
            sources={str(k): v for k, v in self.bridge.sources.held().items()},
            meters=meters, cursor=self.cursor, window=self._window,
            controls=self._controls)

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        turned = self._editing.event(self._member, str(addr), _plain(list(args)))
        outcome = turned.get("outcome") or {}
        if outcome.get("turn") == "closed":
            return self._closed()
        if outcome.get("turn") == "step":
            # **The step is the context's, already taken**; what is left is
            # carrying it out, and the acknowledgement the crate wrote -- with
            # the reason when nothing could apply it.
            stepped = self.app.stepped(turned.get("stepped") or {}, self)
            self.echo.send(outcome.get("answer"))
            return stepped
        return self._take(outcome)

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off: the
        same turn as a message, unstamped."""
        self._sync_core()
        wid, tag, values = args[0], args[1], list(args[2:])
        turned = self._editing.event(self._member, "/gui_event",
                                     _plain([wid, 0, 0, tag, *values]))
        return self._take(turned.get("outcome") or {})

    def _take(self, outcome: dict) -> bool:
        """Carry out what a turn came to, and answer the host. Returns whether
        the multitrack changed."""
        if outcome.get("turn") in (None, "nothing"):
            return False
        for minted in outcome.get("minted") or ():
            self.domain._mint(minted)
        changed = bool(outcome.get("changed"))
        if changed:
            # **The entry is already recorded and the version moved**: both are
            # the context's. What is left is the object the script holds.
            self.domain.write_back(self.structure, outcome["multitrack"])
            self.dirty = True
            self._editing.changed()
        if outcome.get("locate") is not None:
            # **Whoever has the transport is told**: this editor, and the multitrack
            # it is composed inside when it is one.
            self.cursor = float(outcome["locate"])
            self.locate(self.cursor)
            if self.composed_in is not None:
                self.composed_in.locate(self.cursor)
            if callable(self.on_locate):
                self.on_locate(self.cursor)
        if outcome.get("selection") is not None:
            self.selection = outcome["selection"]
        if outcome.get("cursor") is not None:
            self.cursor = float(outcome["cursor"])
        if outcome.get("transport") is not None:
            self._transport(outcome["transport"])
        self.echo.send(outcome.get("answer"))
        return changed

    def reflect_step(self) -> None:
        """Draw what a history walk left behind: every widget corrected, and the
        host told once."""
        self.dirty = True
        self._sync_core()
        self.echo.send(self._call("resync"))

    def adopt(self) -> None:
        """Another view of this multitrack edited it: bring this window in step."""
        if self._host is None or self._window is None:
            return
        self._sync_core()
        self.echo.send(self._call("resync"))

    # ---- the multitrack, heard ----

    def open(self, host=None, id: "int | None" = None):
        """Open the window, and hang the transport row on the playback.

        The wiring is here rather than in the constructor because that is where
        the window comes into being: `clausters.gui.edit` builds the editor and
        opens it in two steps, so a multitrack has its readers before it has a screen
        -- which is the right order anyway, since a multitrack can be played by a
        script that never draws it.
        """
        window = super().open(host, id)
        if self.playback is not None and self._window is not None:
            self.playback.attach(self._host, self._window)
            # **The transport row's buttons are the editor's**, like the multitrack
            # and its ruler: a click on one is a turn the core reads, so it
            # learns their ids once the window has numbered them.
            self._controls = {key: int(self.window[name].id)
                              for key, name in (("rewind", REWIND), ("play", PLAY),
                                                ("stop", STOP), ("clock", CLOCK))}
            self._tick()
            self._host.clock.sched(CLOCK_TICK, self._tick)
            self._tell_meters()
        return window

    def _tell_meters(self) -> None:
        """**Tell the window where the multitrack's meters are.**

        The window is composed before anything sounds -- a multitrack can be played
        by a script that never draws it -- so the buses are only known once the
        playback has made the tracks. Without this the strips read nothing until
        some *unrelated* turn happens to push the picture, which is a gesture
        that may never come: the multitrack plays, the levels move, and every column
        stays at the floor.
        """
        if self.playback is None or self.multitrack_widget is None:
            return
        meters = self.view.props(self, self.multitrack_widget).get("meters")
        if meters:
            self._host.set(self.multitrack_widget, meters=meters)

    def _tick(self):
        """The read-out, and the one round trip: the position is the engine's.

        Returns the delay until the next reading, or ``None`` once the window is
        gone -- which is how a scheduled tick stops without anybody stopping it.
        """
        if self.closed or self.playback is None:
            return None
        self._sync_core()
        text = self._call("clock", position=float(self.playback.position))["text"]
        if text != self._shown:
            self.window[CLOCK].set(text=text)
            self._shown = text
        return CLOCK_TICK

    def toggle(self):
        """Play, or pause where it stands. A pause freezes the governed group,
        so playing again continues rather than starting over."""
        self._sync_core()
        self._take(self._call("toggle"))

    def rewind(self):
        """Put the **position cursor** back at the top, and cue a stopped
        transport there.

        The cursor's own verb, not the transport's: it is where the next play
        starts, and stop goes back to it rather than to the top. A hand that
        has been working at bar forty otherwise has to find the top on screen
        to get back to it.
        """
        self._sync_core()
        self._take(self._call("rewind"))

    def _transport(self, verb: dict) -> None:
        """Carry out what a turn asked the transport to do, on the playback."""
        if self.playback is None:
            return
        kind = verb.get("verb")
        if kind == "toggle":
            # The engine is asked first: a transport another client stopped is
            # stopped, whatever this window last told it.
            if self.playback.playing:
                self.playback.pause()
            else:
                self.playback.play()
        elif kind == "stop":
            self.playback.stop()
        elif kind == "cue":
            self.locate(float(verb.get("secs", 0.0)))

    def play(self):
        """Play the multitrack from where the position cursor is."""
        if self.playback is not None:
            self.playback.play()

    def pause(self):
        """Freeze the multitrack where it stands."""
        if self.playback is not None:
            self.playback.pause()

    def stop(self):
        """Halt and go back to the mark the position cursor is on."""
        self._sync_core()
        self._take(self._call("stop"))

    def locate(self, at: float):
        """The position cursor was placed at ``at`` seconds: cue a stopped
        transport there and leave a rolling one alone."""
        if self.playback is not None:
            self.playback.cue(at)

    def data_changed(self) -> None:
        """The multitrack changed, whoever changed it: put the readers where it now
        says they are, and then tell the script.

        The readers go first because the script's own handler may look at what is
        sounding, and because a multitrack is a statement: making it true again is not
        a reaction to an edit, it is the same call the first one was.
        """
        if self.playback is not None:
            self.playback.sync()
        self._answer_with_the_picture()
        super().data_changed()

    def _answer_with_the_picture(self) -> None:
        """**A name the host minted is answered with the one the multitrack kept.**

        The host mints the word for a track it made or a box it split, and the
        document mints the id; the crate compares what the host was last told
        with what the multitrack now holds and answers with the picture when they
        differ (`settle`). It runs after the readers are synced and a minted
        source has its buffer, so the box that windows it draws.
        """
        if self._host is None or self._window is None:
            return
        self._sync_core()
        self.echo.send(self._call("settle"))

    def close(self):
        """Close this multitrack's window, and its playback with it."""
        if self.playback is not None:
            self.playback.close()
        super().close()


def is_multitrack(structure) -> bool:
    """Whether `edit` should open this as a multitrack."""
    return isinstance(structure, Multitrack)
