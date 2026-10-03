"""Editing a take as **a list of parts over immutable takes**: the audio editor.

What it opens is **a file or a server buffer**, and it edits a private copy:
nothing it was handed is written until it is saved. The window draws a
**join** -- a buffer made of spans of other buffers (`Buffer.stitch`) -- and
every edit leaves a new list of those spans: a cut takes a span out, a paste
puts a new take in, a mix adds the block onto a new take over the frames it
lands on, and a pencil stroke writes **a new take the size of the stroke** and
splices it over the frames it was drawn on. Nothing a history entry names is
ever written again, so an undo is the list before, stitched: it costs the list
and not the samples.

**The editor is the shared crate's** (`clausters._native.EditingCore`, the
``openAudio`` member): what each gesture does to the list, the steps that make
a new take and stitch the join again, and which takes the history can no
longer reach. What is here is what a language owns: the server the steps are
walked against, the buffer numbers the crate is handed for new takes (from this
client's allocator, topped up before every turn), and freeing the takes the
context hands back.

**It sounds through nodes of its own**, the audio editor's (the shared
crate's ``AudioEditorPlayback``, the object the GUI host's monitor holds too):
one play graph per open take, all but the one played last paused, on a
transport of the editor's own, and an output beside them that meters the take
and declicks every play and stop on the way to the hardware. The window shows
the level on a meter beside the take, and the space bar and ``L`` are the
editor's -- the window says so (``plays``), so the host's monitor stays out.

**Memory is spent on what the history holds.** Every take a stroke or a paste
made is kept while an undo or a redo can still reach it, and freed when neither
can -- the history's budget trimmed it, or an edit after an undo dropped the
redo. ``history_bytes`` caps what only the history holds; past it the oldest
entries go first.
"""

import weakref

from ... import _native
from ..._steps import run_steps
from ...errors import CommandError
from ...defs._wire import resolve as _resolve
from .domain import Domain
from .editor import Editor
from .samples import MEASURES, SamplesView, _plain, measures

#: How many buffers the crate holds for new takes before a turn: a stroke or a
#: paste takes one, a mix two.
_SPARE = 2


class _AudioPlayback:
    """**What sounds the audio editors of one server** -- the shared crate's
    playback, the steps it answers carried out on that server.

    One per server, since the editors on it share one structure and one
    transport: each open take is a file of it, and the one played last is the
    one that sounds.
    """

    _of = weakref.WeakKeyDictionary()

    @classmethod
    def of(cls, server) -> "_AudioPlayback":
        """The playback of ``server``, made the first time it is asked for."""
        found = cls._of.get(server)
        if found is None:
            found = cls._of[server] = cls(server)
        return found

    def __init__(self, server):
        self.server = server
        self._native = _native.AudioEditorPlayback(chunk=server._bulk_chunk())
        self._runner = _native.StepRunner()
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        server._ensure_recycler()
        self._rate = None
        try:
            taken = self._native.call("open", server.ids)
        except ValueError as refused:
            raise RuntimeError(str(refused)) from None
        #: The transport it plays on: one of the server's, allocated for it.
        self.transport_id = int(taken["transport"])

    @property
    def rate(self) -> float:
        """The engine's sample rate, asked once; 0 when it will not say."""
        if self._rate is None:
            try:
                self._rate = float(self.server.query_info().nominal_sample_rate)
            except (RuntimeError, OSError, TimeoutError):
                self._rate = 0.0
        return self._rate

    def call(self, verb: str, **args) -> dict:
        """One verb of the crate's playback, its steps carried out."""
        answer = self._native.call(verb, self.server.ids, **args)
        steps = answer.get("steps")
        if steps:
            run_steps(self.server, self._runner, steps)
        return answer

    def state(self) -> dict:
        """``{transport, rolling, focus, meters, nodes}``."""
        return self._native.call("state", self.server.ids)

    def rolling(self) -> bool:
        """Whether the transport rolls, as the engine answers -- a pass that
        ended on its mark stopped without anybody here saying so."""
        playing = bool(self.server.transport_at(self.transport_id).state()["playing"])
        self._native.call("setRolling", self.server.ids, rolling=playing)
        return playing


class _AudioDriver:
    """**What an audio editor's `clausters.defs.Transport` plays through**:
    the server's audio editor playback, its positions the take's seconds, and
    the time range and the loop switch the window shows -- the band a sweep
    leaves and `L` -- kept here, stopped or rolling, so a script and a hand
    read and write one state. Where a pass starts and how it ends are the
    crate's, as the space bar's are."""

    def __init__(self, editor):
        self.editor = editor
        #: The time range ``(start, end)`` in seconds, or ``None``.
        self.span = None
        #: Whether the loop switch is on: the span, or the whole take.
        self.looping = False
        self._paused = False

    @property
    def _playback(self):
        return self.editor._playback

    def _focused(self) -> bool:
        return self._playback.state().get("focus") == self.editor._display

    def playing(self) -> bool:
        return self._playback.rolling() and self._focused()

    def play(self, at=None) -> None:
        if at is None and self._paused and self._focused():
            self._paused = False
            self._playback.call("resume")
            return
        self._paused = False
        if at is not None:
            self.locate(at)
        ed = self.editor
        play = ed._call("play", looping=self.looping)
        self._playback.call("play", file=ed._display, start=int(play["start"]),
                            **{"pass": play["pass"]})

    def pause(self) -> None:
        if self._focused():
            self._playback.call("pause")
            self._paused = True

    def stop(self) -> None:
        self._paused = False
        if self._focused():
            play = self.editor._call("play", looping=self.looping)
            self._playback.call("stop", back=int(play["back"]))

    def locate(self, at: float) -> None:
        ed = self.editor
        frame = int(ed._call("locate", at=float(at))["cue"])
        ed.cursor = float(at)
        if self._playback.rolling() and self._focused():
            self._playback.call("locate", frame=frame)
        else:
            self._playback.call("cue", frame=frame)

    @property
    def end(self):
        return "contents"

    def set_end(self, end) -> None:
        if end != "contents":
            raise ValueError("an audio editor's pass ends where the take or "
                             "the span does")

    def free(self) -> None:
        """Nothing: the transport is the audio editors' of this server, and a
        take is freed by closing its editor."""

    def set_span(self, span, *, show: bool = True) -> None:
        ed = self.editor
        band = ed._call("span", span=None if span is None
                        else [float(span[0]), float(span[1])])
        self.span = (None if span is None or not band["sel_len"]
                     else (float(span[0]), float(span[1])))
        if show:
            ed._show_band(band)
        self._follow()

    def set_looping(self, on: bool) -> None:
        self.looping = bool(on)
        self._follow()
        self.editor._show_looping(self.looping)

    def _follow(self) -> None:
        """A pass in progress ends as the span and the switch now say."""
        if self._focused():
            play = self.editor._call("play", looping=self.looping)
            self._playback.call("pass", **{"pass": play["pass"]})


class AudioDomain(Domain):
    """A take made of parts: the crate's ``parts`` vocabulary.

    **What a gesture does is the crate's**; what is left is carrying out the
    steps it answers, on the take's server, and freeing the takes nothing
    reaches any more.
    """

    name = _native.PARTS
    ingested = True

    #: The editor whose take this is.
    editor = None

    def run(self, structure, steps) -> bool:
        """Walk ``steps`` against the take's server. Answers whether there were
        any."""
        editor = self.editor
        if editor is None or not steps:
            return False
        run_steps(editor._server, editor._runner, steps)
        return True

    def free(self, structure, buffers, spilled=()) -> None:
        """Free takes the context handed back: nothing reaches them any more.
        A take on disk (``spilled``) has no buffer to free, only its number to
        give back."""
        editor = self.editor
        if editor is None:
            return
        for bufnum in buffers:
            if bufnum not in spilled:
                editor._server.send_msg("/buffer_free", int(bufnum))
            editor._server.buffers.free(int(bufnum))

    def store(self, structure, buffer: int, steps) -> None:
        """Write a take to disk and free its buffer. A write the server refuses
        -- no room -- leaves the take in memory, and the crate is told so."""
        editor = self.editor
        if editor is None:
            return
        try:
            run_steps(editor._server, editor._runner, steps)
        except CommandError:
            editor._call("kept", buffer=int(buffer))


class AudioEditor(Editor):
    """A take on screen, edited as a list of parts over immutable takes.

    The window draws `buffer`, a join the editor owns: the take as the edits
    have left it. Play that buffer to hear the edited take, and read `parts`
    for what it is made of.

    Args:
        take: the `clausters.defs.Buffer` to edit. It is never written.
        sample_rate: the take's rate, when the handle does not know it.
        title: the window's title.
        layers: what the picture measures, innermost last.
        history_bytes: the most bytes of takes only the history may hold, or
            ``None`` for no limit.
        resident_bytes: the most of those kept in memory, or ``None`` for all
            of them; past it the oldest are written to ``scratch`` and read
            back when an undo or a redo needs them.
        scratch: the directory, on the server's filesystem, a take leaves
            memory for. A temporary one when not given.
    """

    def __init__(self, take, *, sample_rate: float = 0.0, title: str = "Audio",
                 layers=MEASURES, history_bytes: "int | None" = None,
                 resident_bytes: "int | None" = None,
                 scratch: "str | None" = None, **options):
        rate = float(sample_rate or getattr(take, "sample_rate", 0.0) or 48_000.0)
        view = SamplesView(layers)
        domain = AudioDomain()
        super().__init__(take, sample_rate=rate, domain=domain, view=view,
                         title=title, **options)
        domain.editor = self
        #: The server the take is on, and every take this editor makes.
        self._server = _resolve(getattr(take, "server", None))
        #: The runner the steps are walked through.
        self._runner = _native.StepRunner()
        #: The join the window draws: the take as the edits have left it.
        self._display = self._server.buffers.alloc()
        self._member, self._structure_id = self._editing.open(
            "openAudio", f"audio:{int(take.bufnum)}",
            {**self._facts(), "frames": int(getattr(take, "frames", 0) or 0),
             "take": int(take.bufnum), "display": self._display,
             "path": getattr(take, "path", None),
             "layers": list(view.layers)}, take, domain)
        if history_bytes is not None:
            self._editing.limit_bytes(history_bytes)
        if resident_bytes is not None:
            if scratch is None:
                import tempfile
                scratch = tempfile.mkdtemp(prefix="clausters-audio-")
            self._call("sync", scratch=str(scratch))
            self._editing.limit_resident(resident_bytes)
        self._top_up()
        opened = self._call("open")
        if "error" in opened:
            raise ValueError(opened["error"])
        # **A private copy of the take**, and the join over it: the buffer the
        # editor was handed is written by a save and by nothing else.
        domain.run(take, opened.get("steps") or [])
        #: What sounds the take: the server's audio editor playback, where
        #: this editor's take is the file its join is.
        self._playback = _AudioPlayback.of(self._server)
        #: What the `transport` plays through.
        self._driver = _AudioDriver(self)
        self._sound()

    @property
    def transport(self):
        """**The take's transport**, as the object a script plays: a
        `clausters.defs.Transport` whose verbs (``play``, ``pause``,
        ``stop``, ``locate``, ``loop``, ``wait``) and ``span`` speak the
        take's seconds -- the span is the band a drag sweeps, the loop switch
        is `L`, and each side reads what the other set. The audio editors of
        a server share it -- one of the server's, allocated for them -- and
        the take played last is the one it plays."""
        transport = self._server.transport_at(self._playback.transport_id)
        transport._driver = self._driver
        return transport

    # ---- what the hand marked ----

    @property
    def selected(self):
        """**The samples the hand marked** -- a drag over the take -- as a
        `clausters.segments.Segment` over `buffer`: from the frame the range
        starts on, for its seconds. ``None`` when nothing is marked. In an
        audio editor what is marked and the transport's `span` are one range,
        so setting either sets both."""
        from ...segments import Segment

        span = self._driver.span
        if span is None:
            return None
        return Segment(self.buffer, round(span[0] * self.sample_rate), span[1] - span[0])

    def select(self, segment) -> None:
        """Mark ``segment`` -- a `clausters.segments.Segment` over `buffer`,
        its ``start`` a frame and its ``duration`` seconds."""
        start = float(segment.start) / self.sample_rate
        self._driver.set_span((start, start + float(segment.duration)))

    def unselect(self) -> None:
        """Mark nothing."""
        self._driver.set_span(None)

    def _show_band(self, band: dict) -> None:
        """The band a sweep leaves, drawn where the take is."""
        if self._host is not None and self._window is not None:
            for wid in self.view.widgets:
                self._host.set(wid, sel_start=band["sel_start"], sel_len=band["sel_len"])

    def _show_looping(self, on: bool) -> None:
        """The window's loop switch, as `L` leaves it."""
        if self._host is not None and self._window is not None:
            for wid in self.view.widgets:
                self._host.set(wid, looping=1 if on else 0)
                break

    def _sound(self) -> None:
        """Make what sounds be the take as it now is -- the join and its
        length -- and tell the window where its level is read from."""
        frames = int(self._call("parts").get("frames", 0))
        self._playback.call(
            "sync", file=self._display, buffer=self._display,
            channels=max(1, int(getattr(self.structure, "channels", 1) or 1)),
            frames=frames, takeRate=self.sample_rate, rate=self._playback.rate)
        self._call("sync", meters=self._playback.state().get("meters"))

    def _cue(self, frame: int) -> None:
        """**The position cursor moved**: the play cursor goes with it while
        nothing plays, and a rolling pass is left alone."""
        self._playback.rolling()
        self._playback.call("cue", frame=frame)

    def _play(self, play: dict) -> None:
        """**The space bar**, as the editor read it: a rolling transport stops
        and goes back to the position cursor; a stopped one plays the pass."""
        if self._playback.rolling():
            self._playback.call("stop", back=int(play["back"]))
        else:
            self._playback.call("play", file=self._display, start=int(play["start"]),
                                **{"pass": play["pass"]})

    def open(self, host=None, id: "int | None" = None):
        """Open the window, with its play cursor drawn from the transport.

        The window anchors the play cursor at 0, and the counter that makes
        that the take's own frame is the transport's position -- what the
        monitor plays from. A multitrack editor asks for the same clock when
        it opens, for the same reason.
        """
        window = super().open(host, id)
        if self._host is not None and window is not None:
            self._host.head_clock(window, "transport", self._playback.transport_id)
        return window

    def _closed(self) -> bool:
        """The window closed: the take stops being one the editor plays, and
        the last one closed frees the editor's nodes."""
        self._playback.call("closeFile", file=self._display)
        return super()._closed()

    def close(self):
        """Close the window, and free what sounded the take."""
        self._playback.call("closeFile", file=self._display)
        return super().close()

    def _facts(self) -> dict:
        take = self.structure
        name = getattr(take, "name", None)
        return {"channels": max(1, int(getattr(take, "channels", 1) or 1)),
                "name": name if isinstance(name, str) and name else None,
                "rate": self.sample_rate,
                "chunk": int(self._server._bulk_chunk()) if hasattr(self, "_server") else None,
                "title": self.title, "w": int(self.size[0]), "h": int(self.size[1]),
                "window": self._window}

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _sync_core(self) -> None:
        """Hand the core the chrome, the window it is open in, and enough
        buffers for the next turn."""
        self._top_up()

    def _top_up(self) -> None:
        facts = {k: v for k, v in self._facts().items() if v is not None}
        spare = int(self._call("sync", **facts).get("spare", 0))
        if spare < _SPARE:
            more = [self._server.buffers.alloc() for _ in range(_SPARE - spare)]
            self._call("sync", buffers=more)

    @property
    def buffer(self):
        """The join the window draws, as a `clausters.defs.Buffer`: the take as
        the edits have left it. It changes under this handle as the take is
        edited; read it again rather than caching what it held."""
        from ...defs.buffer import Buffer
        answer = self._call("parts")
        return Buffer(self._display, int(answer.get("frames", 0)),
                      max(1, int(getattr(self.structure, "channels", 1) or 1)),
                      self.sample_rate, server=self._server)

    def save(self, path: "str | None" = None, *, buffer=None,
             sample_format: str = "float"):
        """**Write the take as the edits have left it** -- over what it was
        opened from (the file it was read from, or the server buffer it was
        opened over), or, given a ``path`` or a ``buffer``, there, which a later
        `save` then writes over. Ctrl+S in the window is the same save.

        A **buffer** is rewritten whole at the take's length, so whatever reads
        it hears the edit from then on -- saving into one that is sounding is
        heard as a glitch, which is yours to avoid. ``buffer`` is a
        `clausters.defs.Buffer` to rewrite, or ``True`` for a new one.
        ``sample_format`` (``"float"``, ``"int24"``, ``"int16"``) is a file's.

        Answers the path written, or the `clausters.defs.Buffer`.

        Raises:
            ValueError: the format is not one of those three, or the buffer is
                one the editor reads.
        """
        from ...defs.buffer import Buffer
        request = {"format": str(sample_format)}
        if path is not None:
            request["path"] = str(path)
        elif buffer is True:
            request["buffer"] = self._server.buffers.alloc()
        elif buffer is not None:
            request["buffer"] = int(buffer.bufnum)
        answer = self._call("save", **request)
        if "error" in answer:
            raise ValueError(answer["error"])
        self.domain.run(self.structure, answer.get("steps") or [])
        if "path" in answer:
            return str(answer["path"])
        frames = int(self._call("parts").get("frames", 0))
        return Buffer(int(answer["buffer"]), frames,
                      max(1, int(getattr(self.structure, "channels", 1) or 1)),
                      self.sample_rate, server=self._server)

    @property
    def parts(self) -> list:
        """What the take is made of now: one ``dict`` per span, in reading
        order, each naming the buffer it reads and the frames of it."""
        return list(self._call("parts").get("parts") or ())

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
            self._editing.release(turned.get("freed"), turned.get("stored"))
            self._sound()
            return stepped
        changed = self._take(outcome)
        self._editing.release(turned.get("freed"), turned.get("stored"))
        return changed

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off."""
        self._sync_core()
        wid, tag, values = args[0], args[1], list(args[2:])
        turned = self._editing.event(self._member, "/gui_event",
                                     _plain([wid, 0, 0, tag, *values]))
        changed = self._take(turned.get("outcome") or {})
        self._editing.release(turned.get("freed"), turned.get("stored"))
        return changed

    def _take(self, outcome: dict) -> bool:
        """Carry out what a turn came to, and answer the host. Returns whether
        the take changed."""
        if outcome.get("turn") in (None, "nothing"):
            return False
        changed = bool(outcome.get("changed"))
        # **The steps are this client's to carry out**: a new take made where
        # the turn needed one and the join stitched over the list -- or, for a
        # save from the window, the file written.
        self.domain.run(self.structure, outcome.get("steps") or [])
        if changed:
            self.dirty = True
            self._editing.changed()
            self._sound()
        if outcome.get("play") is not None:
            # The loop switch rides beside the space bar and `L`: a pass that
            # loops is the switch on.
            self._driver._paused = False
            self._driver.looping = outcome["play"]["pass"].get("kind") == "loop"
            self._play(outcome["play"])
        if outcome.get("pass") is not None:
            self._driver.looping = outcome["pass"].get("kind") == "loop"
            # `L`: the pass in progress ends as the switch now says; a stopped
            # playback reads it on its next play.
            self._playback.call("pass", **{"pass": outcome["pass"]})
        if outcome.get("cue") is not None:
            self._cue(int(outcome["cue"]))
        if outcome.get("locate") is not None:
            self.cursor = float(outcome["locate"])
            self.locate(self.cursor)
            if self.composed_in is not None:
                self.composed_in.locate(self.cursor)
            if callable(self.on_locate):
                self.on_locate(self.cursor)
        if outcome.get("selection") is not None:
            # A sweep's time range is the transport's span, and what the hand
            # marked: the space bar plays it and the loop repeats it.
            swept = outcome["selection"]
            start, length = float(swept.get("start", 0.0)), float(swept.get("len", 0.0))
            self._driver.span = (start, start + length) if length > 0 else None
        self.echo.send(outcome.get("answer"))
        return changed

    @property
    def layers(self) -> tuple:
        """What the picture measures -- `("peak", "rms")` for the editor's
        view, `("peak",)` for the bare envelope.

        **Assigning it on an open view sends one message.** The measure is a
        live `/gui_set` prop, so the body appears and disappears over the peaks
        with the picture, the axis, the zoom, the selection and the playhead all
        exactly where they were.
        """
        return self.view.layers

    @layers.setter
    def layers(self, stack) -> None:
        answer = self._call("layers", stack=[str(name) for name in stack])
        if "error" in answer:
            raise ValueError(answer["error"])
        self.view.layers = tuple(answer["layers"])
        if self._host is not None and self._window is not None:
            for wid in self.view.widgets:
                self._host.set(wid, measure=answer["measure"])


def is_take(structure) -> bool:
    """Whether `edit` should open this in the audio editor: anything with a
    buffer number and samples it can write, which is what a
    `clausters.defs.Buffer` answers with."""
    return hasattr(structure, "bufnum") and hasattr(structure, "set_samples")


__all__ = ["AudioDomain", "AudioEditor", "is_take", "measures"]
