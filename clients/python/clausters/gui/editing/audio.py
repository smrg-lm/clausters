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

from ... import _native
from ..._steps import run_steps
from ...errors import CommandError
from ...defs._wire import resolve as _resolve
from .application import _service_instance
from ._clipboard import exchange, bind_exchange
from ._cleanup import execute_steps, release_buffer
from .domain import Domain
from .editor import Editor
from .marking import Marking
from .samples import MEASURES, SamplesView, _plain, measures

#: How many buffers the crate holds for new takes before a turn: a stroke or a
#: paste takes one, a mix two.
_SPARE = 2


class _AudioPlayback:
    """**What sounds one audio editor** -- the shared crate's playback, the
    steps it answers carried out on the editor's server.

    One per editor, shared by its open files: one file plays at a time and
    all others stay paused, and playing one window moves nothing in another.
    A pass goes on while the window shows another file until it is stopped,
    from any file, and a play is over the file shown.
    """

    def __init__(self, server):
        self.server = server
        self._instance = _service_instance(server)
        self._native = _native.AudioEditorPlayback(chunk=server._bulk_chunk())
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
        if _service_instance(self.server) != self._instance:
            raise RuntimeError("the audio service belongs to a previous server instance")
        answer = self._native.call(verb, self.server.ids, **args)
        steps = answer.get("steps")
        if steps:
            execute_steps(self.server, self._instance, steps)
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
        return self._playback.state().get("focus") == self.editor._file_id

    def playing(self) -> bool:
        was = bool(self._playback.state().get("rolling"))
        rolling = self._playback.rolling()
        if was != rolling:
            self.editor._show_transport()
        return rolling and self._focused()

    def play(self, at=None) -> None:
        if at is None and self._paused and self._focused():
            self._paused = False
            self._playback.call("resume")
            self.editor._show_transport()
            return
        self._paused = False
        if at is not None:
            self.locate(at)
        ed = self.editor
        play = ed._call("play", looping=self.looping)
        self._playback.call("play", file=ed._file_id, start=int(play["start"]),
                            **{"pass": play["pass"]})
        ed._show_transport()

    def pause(self) -> None:
        if self._focused():
            self._playback.call("pause")
            self._paused = True
            self.editor._show_transport()

    def stop(self) -> None:
        """Stop whichever file plays, from the window showing another file
        too, and bring the file shown into focus at its cursor."""
        self._paused = False
        if self._focused() or self._playback.rolling():
            ed = self.editor
            play = ed._call("play", looping=self.looping)
            self._playback.call("stop", back=int(play["back"]), file=ed._file_id)
            ed._show_transport()

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
        editor._check_server()
        run_steps(editor._server, editor._runner, steps)
        return True

    def execute_operation(self, structure, prepared):
        if self.editor is None:
            raise RuntimeError("the operation owner is absent")
        return self.editor._operation({}, prepared=prepared)

    def execute_batch(self, structure, server, steps):
        editor = self.editor
        if editor is None:
            raise RuntimeError("the operation owner is absent")
        editor._check_server()
        if server != editor._instance:
            raise RuntimeError("the batch belongs to another server instance")
        execute_steps(editor._server, server, steps)

    def operation_returned(self, structure, server, buffers):
        editor = self.editor
        if editor is not None and server == editor._instance and server == _service_instance(editor._server):
            for buffer in buffers:
                editor._server.buffers.free(int(buffer))

    def operation_echo(self, structure, answer):
        if self.editor is not None:
            self.editor.echo.send(answer)

    def sound_file(self, structure, file):
        if self.editor is not None:
            self.editor._sync_sound(file)

    def free(self, structure, buffers, spilled=()) -> None:
        """Free takes the context handed back: nothing reaches them any more.
        A take on disk (``spilled``) has no buffer to free, only its number to
        give back."""
        editor = self.editor
        if editor is None:
            return
        editor._check_server()
        for bufnum in buffers:
            release_buffer(editor._server, editor._editing.core,
                           {"server": editor._instance, "buffer": int(bufnum)})

    def store(self, structure, buffer: int, steps, *, operation=None) -> None:
        """Write a take to disk and free its buffer. A write the server refuses
        -- no room -- leaves the take in memory, and the crate is told so."""
        editor = self.editor
        if editor is None:
            return
        editor._check_server()
        try:
            if operation is not None:
                editor._operation({}, prepared=operation)
            else:
                run_steps(editor._server, _native.StepRunner(), steps)
        except CommandError:
            if operation is None:
                editor._call("kept", buffer=int(buffer))


class AudioEditor(Marking, Editor):
    """A take on screen, edited as a list of parts over immutable takes.

    The window draws `buffer`, a join the editor owns: the take as the edits
    have left it. Play that buffer to hear the edited take, and read `parts`
    for what it is made of.

    **What is marked** (`selected`, `select`, `unselect`) is the samples a
    drag over the take covers, as a `clausters.segments.Segment` over
    `buffer` -- its ``start`` a frame and its ``duration`` seconds -- or
    ``None`` when nothing is marked. In an audio editor what is marked and
    the transport's `span` are one range, so setting either sets both.

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
        shared_history: whether the take's undo order is the editing
            context's shared one, which the other members of ``context`` walk
            too. By default each file the editor holds has an order of its own,
            whatever was opened before it.
        scratch: the directory, on the server's filesystem, a take leaves
            memory for. A temporary one when not given.
        file: the `clausters.SoundFile` `take` was loaded from, which a save
            writes -- one with no path asks for one. `clausters.gui.edit`
            loads a file and passes it here.
    """

    def __init__(self, take, *, sample_rate: float = 0.0, title: str = "Audio",
                 layers=MEASURES, history_bytes: "int | None" = None,
                 resident_bytes: "int | None" = None,
                 scratch: "str | None" = None, file=None,
                 shared_history: bool = False, **options):
        #: The `clausters.SoundFile` the take was loaded from, when it was.
        self._file = file
        rate = float(sample_rate or getattr(take, "sample_rate", 0.0) or 48_000.0)
        view = SamplesView(layers)
        domain = AudioDomain()
        super().__init__(take, sample_rate=rate, domain=domain, view=view,
                         title=title, **options)
        domain.editor = self
        #: The server the take is on, and every take this editor makes.
        self._server = _resolve(getattr(take, "server", None))
        self._instance = _service_instance(self._server)
        #: The runner the steps are walked through.
        self._runner = _native.StepRunner()
        #: The join the window draws: the take as the edits have left it.
        self._display = self._server.buffers.alloc()
        # The context is the take's; switching files changes the structure,
        # so the window keeps the one it opened in.
        self._context = self._editing
        self._member, self._structure_id = self._editing.open(
            "openAudio", f"audio:{self._instance['connection']}:{self._instance['generation']}:{int(take.bufnum)}",
            {**self._facts(), "frames": int(getattr(take, "frames", 0) or 0),
             "take": int(take.bufnum), "display": self._display,
             "server": self._instance,
             "sharedHistory": bool(shared_history),
             "path": getattr(take, "path", None),
             "untitled": self._file is not None and self._file.path is None,
             "codec": getattr(file, "codec", None),
             "sample_format": getattr(file, "sample_format", None),
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
        opened = self._operation({"kind": "open"})
        # **A private copy of the take**, and the join over it: the buffer the
        # editor was handed is written by a save and by nothing else.

        #: What sounds the take: the server's audio editor playback, where
        #: this editor's take is the file its join is.
        self._playback = _AudioPlayback(self._server)
        #: What the `transport` plays through.
        self._driver = _AudioDriver(self)
        self._sound()

    @property
    def transport(self):
        """**The take's transport**, as the object a script plays: a
        `clausters.defs.Transport` whose verbs (``play``, ``pause``,
        ``stop``, ``locate``, ``loop``, ``wait``) and ``span`` speak the
        take's seconds -- the span is the band a drag sweeps, the loop switch
        is `L`, and each side reads what the other set. It is this editor's
        own -- one of the server's, allocated for it -- so another window's
        play never moves it."""
        transport = self._server.transport_at(self._playback.transport_id)
        transport._driver = self._driver
        return transport

    # ---- what the hand marked ----

    def _marked(self):
        """The marked samples: the transport's span, from the frame it starts
        on, for its seconds."""
        from ...segments import Segment

        span = self._driver.span
        if span is None:
            return None
        return Segment(self.buffer, round(span[0] * self.sample_rate), span[1] - span[0])

    def _mark(self, segment) -> None:
        """Mark ``segment``, which sets the transport's span."""
        if segment is None:
            self._driver.set_span(None)
            return
        start = float(segment.start) / self.sample_rate
        self._driver.set_span((start, start + float(segment.duration)))

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

    @property
    def _file_id(self) -> int:
        return int(self._call("binding")["id"])

    def _sound(self, *, publish_preview: bool = True) -> None:
        """Make what sounds be the take as it now is -- the join and its
        length -- and tell the window where its level is read from."""
        file = self._call("playback")
        if file.get("active") is False:
            return
        self._sync_sound(file)
        preview = file["preview"]
        state = self._playback.state()
        if publish_preview and self._host is not None and self._window is not None:
            buffer = (state.get("previews") or {}).get(str(self._file_id))
            # every view of the take, each drawing the preview as itself
            for wid in preview.get("pictures") or ():
                self._host.set(int(wid), preview_buffer=-1 if buffer is None else buffer)
        self._call("sync", meters=self._playback.state().get("meters"))

    def _sync_sound(self, file):
        self.dirty = bool(self._call("binding").get("dirty"))
        if file.get("active") is False:
            return
        self._playback.call("sync", file=int(file["id"]), buffer=int(file["display"]),
                            channels=int(file["channels"]), frames=int(file["frames"]),
                            takeRate=float(file["rate"]), rate=self._playback.rate)
        preview = file["preview"]
        if preview["installed"]:
            self._playback.call("gain", file=int(file["id"]), points=preview["points"],
                                on=preview["on"], revision=preview["revision"])
        else:
            self._playback.call("removeEffect", file=int(file["id"]))
        self._retain_playback()

    def _cue(self, frame: int) -> None:
        """**The position cursor moved**: the play cursor goes with it while
        nothing plays, and a rolling pass is left alone."""
        self._playback.rolling()
        self._playback.call("cue", frame=frame)
        self._show_transport()

    def _play(self, play: dict) -> None:
        """**The space bar**, as the editor read it: a rolling transport stops
        and goes back to the position cursor; a stopped one plays the pass."""
        if self._playback.rolling():
            # whichever file plays stops, and the one shown comes into focus
            self._playback.call("stop", back=int(play["back"]), file=self._file_id)
        else:
            self._playback.call("play", file=self._file_id, start=int(play["start"]),
                                **{"pass": play["pass"]})
        self._show_transport()

    def _clipboard(self, doc: str, buffer: int, *, ready: bool = False) -> None:
        """**A copy made on the server**: the buffer it is in is the
        clipboard's now, the one the previous copy was in goes, and the host's
        clipboard names it."""
        self._check_server()
        if ready:
            bind_exchange(self._server, self._editing.core)
        else:
            doc = exchange(self._server, "publish", document=doc)["document"]
        if self._host is not None:
            self._host.clipboard(doc)

    def _show_transport(self) -> None:
        """**The window's play cursor and the list's play mark, as the
        playback stands**: the line is drawn over the file shown unless another
        file is playing, and the list marks the file that plays. Read off what
        the playback was last told: an engine asked right after a play may not
        be rolling yet."""
        state = self._playback.state()
        if self._host is None or self._window is None:
            return
        for correction in self._call("playing", file=state.get("playing"), meters=state.get("meters"))["corrections"]:
            self._host.set(int(correction["widget"]), **correction["props"])

    def open(self, host=None, id: "int | None" = None):
        """Open the window, with its play cursor drawn from the transport.

        The window anchors the play cursor at 0, and the counter that makes
        that the take's own frame is the transport's position -- what the
        monitor plays from. A multitrack editor asks for the same clock when
        it opens, for the same reason.
        """
        if self._window is None:
            self._call("attach_view")
        window = super().open(host, id)
        if self._host is not None and window is not None:
            self._host.head_clock(window, "transport", self._playback.transport_id)
        return window

    def _plays_on(self):
        return self._server

    def _new_file(self, shape: dict) -> None:
        """**A new file asked for in the window**: added to this editor."""
        self.new_file(int(shape["channels"]), int(shape["frames"]), float(shape["rate"]))

    def new_file(self, channels: int, frames: int, rate: float) -> None:
        """Add and focus an empty SoundFile of the requested shape."""
        from ...soundfile import SoundFile
        self._add_file(SoundFile.empty(channels, frames, rate))

    def _open_file(self, paths) -> None:
        """**Files asked to be opened**: each read and added, the last in focus."""
        from ...soundfile import SoundFile
        for path in paths:
            self._add_file(SoundFile.open(path))

    def _add_file(self, file) -> None:
        """Read ``file`` into a buffer and hand it to the crate as a new file
        in focus, with a display and spare buffers of its own."""
        take = file.to_buffer(server=self._server)
        answer = self._call(
            "add_file", take=int(take.bufnum), frames=int(take.frames),
            channels=int(take.channels), rate=float(take.sample_rate),
            display=self._server.buffers.alloc(), path=file.path,
            untitled=file.path is None, codec=file.codec,
            sample_format=file.sample_format,
            buffers=[self._server.buffers.alloc() for _ in range(_SPARE)])
        if "error" in answer:
            self._editing._operation_returned(answer.get("returned"))
            self._editing.release(answer.get("freed"), answer.get("stored"))
        self._take(answer)

    def open_file(self, paths) -> None:
        """Open a path or a list of paths in this editor, focusing the last."""
        paths = list(paths) if isinstance(paths, (list, tuple)) else str(paths)
        answer = self._call("open_file", paths=paths)
        if "error" in answer:
            raise ValueError(answer["error"])
        self._open_file(answer["openFile"]["paths"])

    @property
    def files(self) -> tuple:
        """Open SoundFiles in list order; a buffer-only entry is None."""
        from ...soundfile import SoundFile
        return tuple(
            SoundFile(f["path"], int(f["frames"]), int(f["channels"]), float(f["rate"]),
                      f["codec"], f["sample_format"])
            if f["path"] is not None or f["untitled"] else None
            for row in self._call("files")["files"] for f in [row["file"]])

    def focus_file(self, index: int) -> None:
        """Focus the file at zero-based index, restoring its cursor and selection."""
        self._take(self._call("focus_file", index=index))

    def close_file(self, index=None) -> None:
        """Close the focused file, or the file at zero-based index."""
        self._take(self._call("close_file", **({} if index is None else {"index": index})))

    def show_files(self) -> None:
        """Show the independent file list to the left of the editing view."""
        self._take(self._call("show_files"))

    def hide_files(self) -> None:
        """Hide the file list."""
        self._take(self._call("hide_files"))

    def _window_widget(self, role: str, key: str = "") -> int:
        """A component of this window, rebound to the focused file's data.

        Component ids survive file switches. The context version rejects
        gestures made against the previous file's binding.
        """
        return self.app._view_id_for(role, key, drawer=self)

    # ---- the history: the order of the file in focus ----

    def _step(self, direction: str) -> bool:
        """A step of this window's order: the file in focus walks its own."""
        return self.app.stepped(self._editing.step(direction, self._member), self)

    @property
    def can_undo(self) -> bool:
        """Whether the file in focus has an edit to step back over."""
        return bool(self._editing.state(self._member).get("canUndo"))

    @property
    def can_redo(self) -> bool:
        """Whether the file in focus has an undone edit to step forward into."""
        return bool(self._editing.state(self._member).get("canRedo"))

    @property
    def undo_label(self) -> "str | None":
        """What an undo of the file in focus would be called."""
        return self._editing.state(self._member).get("undoLabel")

    @property
    def redo_label(self) -> "str | None":
        """What a redo of the file in focus would be called."""
        return self._editing.state(self._member).get("redoLabel")

    def _bind_file(self, binding: dict) -> None:
        """**Bind the file in focus**: its buffers, its structure in the
        history, and the cursor, selection, layers and loop switch it left."""
        from ...defs.buffer import Buffer
        self._display = int(binding["display"])
        self.sample_rate = float(binding["rate"])
        self.structure = Buffer(int(binding["take"]), int(binding["frames"]),
                                int(binding["channels"]), self.sample_rate, server=self._server)
        self._structure_id = binding.get("structure", self._structure_id)
        self._editing.version = int(binding.get("version", self._editing.version))
        self.cursor = float(binding["cursor"]) / self.sample_rate
        self.dirty = bool(binding["dirty"])
        self.view.layers = tuple(binding["layers"])
        self._driver.span = tuple(binding["span"]) if binding["span"] is not None else None
        self._driver.looping = bool(binding["looping"])
        self._driver._paused = False

    def _closed(self) -> bool:
        """The window closed: the take stops being one the editor plays, and
        the last one closed frees the editor's nodes."""
        self._call("detach_view")
        self._playback.call("close")
        self._drop_playback()
        return super()._closed()

    def close(self):
        """Close the window, and free what sounded the take."""
        self._call("detach_view")
        self._playback.call("close")
        self._drop_playback()
        return super().close()

    def reflect_step(self) -> None:
        """Reflect the committed history position, including its clean checkpoint."""
        super().reflect_step()
        self.dirty = bool(self._call("binding").get("dirty"))

    def _facts(self) -> dict:
        take = self.structure
        name = getattr(take, "name", None)
        return {"channels": max(1, int(getattr(take, "channels", 1) or 1)),
                "name": name if isinstance(name, str) and name else None,
                "rate": self.sample_rate,
                "chunk": int(self._server._bulk_chunk()) if hasattr(self, "_server") else None,
                "title": self.title, "w": int(self.size[0]), "h": int(self.size[1]),
                "chrome": self.chrome, "window": self._window}

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context. The window
        is handed its actions' readings when the call moved them."""
        self._check_server()
        answer = self._editing.member(self._member, verb, **args)
        self._publish_actions()
        return answer

    def _check_server(self):
        if _service_instance(self._server) != self._instance:
            self._editing._service_call("invalidate", server=self._instance)
            raise RuntimeError("the audio service belongs to a previous server instance")

    def _retain_playback(self):
        result = self._editing._service_call("retainMember", member=self._member, kind="playback")
        self._editing.release(result.get("freed"), result.get("stored"))

    def _drop_playback(self):
        result = self._editing._service_call("dropRoot", root={"kind": "playback", "owner": self._member})
        self._editing.release(result.get("freed"), result.get("stored"))

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

    def mix(self) -> bool:
        """Paste onto what is there, mixed with it -- the window's ``mix`` (`verb`)."""
        return self.verb("mix")

    def retry_pending(self) -> None:
        """Retry failed repairs for this editor's files without repeating edits.

        Call after resolving the I/O failure. Pending contents and history stay
        unchanged; successful repair permits editing, saving and closing again.
        Raises `CommandError` if repair fails again. Does nothing if none remain.
        """
        self._check_server()
        pending = self._editing._service_call("operation", action="pendingRepairs", member=self._member)
        for repair in pending.get("repairs") or ():
            prepared = self._editing._service_call("operation", action="retryRepair", ticket=repair["ticket"])
            self._operation({}, prepared=prepared, repairing=True)

    def _operation(self, command, prepared=None, *, repairing=False):
        """Execute typed batches; commit, refusal and cleanup come from Rust."""
        answer = prepared or self._editing._service_call(
            "operation", action="prepare", member=self._member, command=command)
        self._publish_actions()
        while not answer.get("complete"):
            if answer.get("accepted") is False:
                raise RuntimeError("the operation is no longer available")
            if "error" in answer:
                raise ValueError(answer["error"])
            try:
                self._editing._run_batch(answer["member"], answer["server"], answer.get("steps") or [])
                result = {"status": "success"}
            except Exception as error:
                result = {"status": "failed", "reason": str(error)}
            answer = self._editing._service_call("operation", action="complete",
                                                ticket=answer["ticket"], batch=answer["batch"], result=result)
        if answer.get("repair") is not None:
            ticket = answer["repair"]
            try:
                for recovery in answer.get("recoveries") or ():
                    self._editing._run_batch(recovery["member"], recovery["server"], recovery.get("steps") or [])
            except Exception as error:
                answer = self._editing._service_call("operation", action="repairFailed", ticket=ticket, reason=str(error))
            else:
                answer = self._editing._service_call("operation", action="repaired", ticket=ticket)
        self._editing.version = int(answer.get("version", self._editing.version))
        self._editing._operation_returned(answer.get("returned"))
        self._editing._operation_echo(answer.get("echo"))
        self._editing.release(answer.get("freed"), answer.get("stored"))
        self.dirty = bool(self._call("binding").get("dirty"))
        self._publish_actions()
        if not answer.get("committed") and not (repairing and answer.get("repaired")):
            outcome = answer.get("outcome") or {}
            if answer.get("echo") is None:
                self.echo.send(outcome.get("answer"))
            raise CommandError(answer.get("reason") or (outcome.get("answer") or {}).get("reason", "the operation was refused"))
        return answer

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
        self._sync_core()
        completed = self._operation({"kind": "save", **request})
        self._take(completed.get("outcome") or {})
        answer = completed["destination"]
        if "path" in answer:
            return str(answer["path"])
        return Buffer(int(answer["buffer"]), int(answer["frames"]),
                      int(answer["channels"]), float(answer["rate"]), server=self._server)

    @property
    def file(self):
        """**The `clausters.SoundFile` the take is**, at its length now: the
        file it was opened from or last saved to, one with no path when it has
        not been written yet, and ``None`` for a take opened over a buffer and
        saved into none."""
        from ...soundfile import SoundFile
        answer = self._call("file")
        path = answer.get("path")
        frames, channels = int(answer.get("frames", 0)), int(answer.get("channels", 1))
        rate = float(answer.get("rate", self.sample_rate))
        if path is None:
            return SoundFile(None, frames, channels, rate) if answer.get("untitled") else None
        return SoundFile(path, frames, channels, rate, answer.get("codec"),
                         answer.get("sample_format"))

    @property
    def parts(self) -> list:
        """What the take is made of now: one ``dict`` per span, in reading
        order, each naming the buffer it reads and the frames of it."""
        return list(self._call("parts").get("parts") or ())

    # ---- the crate's turns ----

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        prepared = exchange(self._server, "event", args=_plain(list(args))) if addr == "/gui_event" else {"args": _plain(list(args))}
        turned = self._editing.event(self._member, str(addr), prepared["args"])
        outcome = turned.get("outcome") or {}
        outcome["clipboardLease"] = prepared.get("lease")
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
        wid, tag, values = args[0], args[1], list(args[2:])
        return self._deliver("/gui_event", [wid, 0, 0, tag, *values])

    def _take(self, outcome: dict) -> bool:
        try:
            return self._take_outcome(outcome)
        finally:
            self._publish_actions()

    def _take_outcome(self, outcome: dict) -> bool:
        """Carry out what a turn came to, and answer the host. Returns whether
        the take changed."""
        if outcome.get("operation") is not None:
            try:
                completed = self._operation({}, prepared=outcome["operation"])
                return self._take(completed.get("outcome") or {})
            finally:
                if outcome.get("clipboardLease") is not None:
                    exchange(self._server, "release", lease=outcome["clipboardLease"])
        if "error" in outcome:
            raise ValueError(outcome["error"])
        if outcome.get("turn") in (None, "nothing"):
            if outcome.get("clipboardLease") is not None:
                exchange(self._server, "release", lease=outcome["clipboardLease"])
            return False
        action = outcome.get("fileAction")
        if action is not None and "binding" not in outcome:
            self.echo.send(outcome.get("answer"))
            return self._take(self._call(action["verb"], file=action["file"]))
        if action is not None and action["verb"] == "close_file":
            self._playback.call("closeFile", file=int(action["file"]))
        if "binding" in outcome:
            self.domain.run(self.structure, outcome.get("steps") or [])
            self.domain.free(self.structure, outcome.get("freedBuffers") or [])
            if action is not None and action["verb"] == "close_file":
                self._retain_playback()
            outcome = dict(outcome, steps=[])
            if outcome.get("close"):
                self.close()
                return False
            if outcome.get("focusChanged"):
                self._bind_file(outcome["binding"])
                self._sound(publish_preview=False)
                # a pass in progress goes on: the window only looks elsewhere
                self._playback.rolling()
                self._playback.call("view", file=self._file_id,
                                    frame=int(outcome["binding"]["cursor"]))
        if "layers" in outcome:
            self.view.layers = tuple(outcome["layers"])
        # **A view placed in a lane of its own** (the stacked views, or back
        # to one): the window is drawn again and the host keeps what is its own.
        if outcome.get("restructure") and self._window is not None:
            self.app.publish(int(self._window), self.draw())
            # the def carries no preview: each view is handed it again
            self._sound()
        changed = bool(outcome.get("changed"))
        # **The steps are this client's to carry out**: a new take made where
        # the turn needed one and the join stitched over the list -- or, for a
        # save from the window, the file written.
        steps = outcome.get("steps") or []
        published = False
        try:
            if outcome.get("clipboard") is not None or outcome.get("clipboardLease") is not None:
                self._check_server()
                run_steps(self._server, _native.StepRunner(), steps)
            else:
                self.domain.run(self.structure, steps)
            if outcome.get("clipboard") is not None:
                self._clipboard(outcome["clipboard"], int(outcome["clipboardBuffer"]), ready=bool(outcome.get("clipboardReady")))
                published = True
                if outcome.get("clipboardCut") is not None:
                    self._sync_core()
                    continued = self._editing.act(self._member, outcome["clipboardCut"])
                    next_outcome = continued.get("outcome") or {}
                    if next_outcome.get("answer") is not None:
                        next_outcome["answer"]["seq"] = (outcome.get("answer") or {}).get("seq", 0)
                    changed = self._take(next_outcome)
                    self._editing.release(continued.get("freed"), continued.get("stored"))
        except Exception as error:
            if outcome.get("clipboard") is not None:
                self.echo.send(dict(outcome.get("answer") or {}, reason=str(error), docVersion=self._editing.version))
            if not published and outcome.get("clipboardBuffer") is not None and self._instance == _service_instance(self._server):
                buffer = int(outcome["clipboardBuffer"])
                self._server.send_msg("/buffer_free", buffer)
                self._server.buffers.free(buffer)
            raise
        finally:
            if outcome.get("clipboardLease") is not None:
                exchange(self._server, "release", lease=outcome["clipboardLease"])
        if outcome.get("newFile") is not None or outcome.get("openFile") is not None:
            self.echo.send(outcome.get("answer"))
            outcome = dict(outcome, answer=None)
        if outcome.get("newFile") is not None:
            self._new_file(outcome["newFile"])
        if outcome.get("openFile") is not None:
            self._open_file(outcome["openFile"]["paths"])
        if outcome.get("playbackUpdate") is not None:
            update = outcome["playbackUpdate"]
            self._sync_sound(update["file"])
            if update.get("publish"):
                self._sound()
        if changed:
            self.dirty = bool(self._call("binding").get("dirty"))
            self._editing.changed()
            if not outcome.get("preparedComplete"):
                self._sound()
        if outcome.get("preview") is not None and not changed:
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
        if outcome.get("follow") is not None:
            # A selection redrawn while this take loops: the loop is over
            # the new span, and the head goes into it once the sweep is let
            # go. Whether it loops, and whether it is this take, is the
            # playback's to say.
            self._playback.call("follow", file=self._file_id, **outcome["follow"])
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
        if outcome.get("clipboardCut") is None:
            self.echo.send(outcome.get("answer"))
        if outcome.get("focusChanged"):
            self._show_transport()
        self._closing(outcome)
        return changed

    @property
    def layers(self) -> tuple:
        """Visual layers, back to front: names or objects with `draw`,
        `visible`, `solo`, `alpha` and `y`. Two different domains cannot share
        the vertical axis; use `y="box"` for the overlaid amplitude layer.

        Assignment applies the shared application's outcome to the open view.
        """
        return self.view.layers

    @layers.setter
    def layers(self, stack) -> None:
        self._view_layers("layers", stack=stack if isinstance(stack, str) else list(stack))

    def _view_layers(self, verb: str, **args) -> None:
        outcome = self._call(verb, version=self._editing.version, **args)
        if "error" in outcome:
            raise ValueError(outcome["error"])
        self._take(outcome)

    @property
    def envelope(self) -> list[float]:
        """Gain envelope as time/value/shape/curve quads, with time in seconds.

        Values range from zero to one; one is unity, the take as it is. Editing the curve is
        non-destructive. `apply_envelope` renders it into a new take.
        """
        return list(self._call("envelope")["points"])

    def set_envelope(self, points) -> bool:
        """Set gain quads as one undoable edit. Times must increase; the
        shared application keeps time and gain inside the take's ranges."""
        return self._envelope_act("setautomation", points=list(points))

    def show_envelope(self) -> None:
        """Show and select the gain curve body over the take."""
        self._view_layers("show_envelope")

    @property
    def folded(self) -> list[int]:
        """The channels drawn folded, in order: strips one control tall that a
        press on their name unfolds. A view state, outside the history."""
        return list(self._call("folded")["folded"])

    def fold_channel(self, channel: int) -> None:
        """Fold channel ``channel`` (from 0) to a strip, or unfold it when it
        is folded."""
        self._channels("fold_channel", channel=int(channel))

    def solo_channel(self, channel: int) -> None:
        """Show channel ``channel`` alone: every other channel folded."""
        self._channels("solo_channel", channel=int(channel))

    def show_all_channels(self) -> None:
        """Unfold every channel."""
        self._channels("show_all_channels")

    def _channels(self, verb: str, **args) -> None:
        """A channel verb: the crate's answer carried out, and a refusal --
        a channel the take does not have -- raised."""
        outcome = self._call(verb, version=self._editing.version, **args)
        self._take(outcome)
        reason = (outcome.get("answer") or {}).get("reason")
        if reason:
            raise ValueError(reason)

    def apply_envelope(self) -> bool:
        """Render gain into a new take and reset the curve to unity, in one
        undo step. An enabled preview follows the reset curve, so gain is applied once."""
        return self._envelope_act("apply_envelope")

    def preview_on(self) -> None:
        """Hear the gain envelope through a ramped insert on the take."""
        self._envelope_act("preview_on")

    def preview_off(self) -> None:
        """Ramp to dry and pause the insert, retaining its state."""
        self._envelope_act("preview_off")

    def free_preview(self) -> None:
        """Free the preview insert and its table; keep the unapplied curve."""
        self._envelope_act("free_preview")

    def _envelope_act(self, verb: str, **args) -> bool:
        with self._editing.turn(self):
            self._sync_core()
            turned = self._editing.act(self._member, _plain({"verb": verb, **args}))
            outcome = turned.get("outcome") or {}
            changed = self._take(outcome)
            self._editing.release(turned.get("freed"), turned.get("stored"))
            reason = (outcome.get("answer") or {}).get("reason")
            if reason:
                raise ValueError(reason)
            return changed

    @property
    def layer(self) -> str:
        """The active edit body, separate from the take's visual layers."""
        return self._call("layer")["layer"]

    @layer.setter
    def layer(self, name: str) -> None:
        self._view_layers("layer", name=name)

    def show_layer(self, name: str) -> None:
        """Show a visual layer, appending it if absent."""
        self._view_layers("show_layer", name=name)

    def hide_layer(self, name: str) -> None:
        """Hide a visual layer, keeping its place and settings."""
        self._view_layers("hide_layer", name=name)

    def solo_layer(self, name: str) -> None:
        """Toggle a layer's solo; only visible soloed layers are drawn."""
        self._view_layers("solo_layer", name=name)

    def layer_alpha(self, name: str, alpha: float) -> None:
        """Set a layer's opacity between zero and one."""
        self._view_layers("layer_alpha", name=name, alpha=alpha)

    def view_wave(self) -> None:
        """Show the peak and RMS waveform in the take's lane, instead of the
        spectrogram."""
        self._view_layers("view_wave")

    def view_spectrogram(self) -> None:
        """Show the spectrogram in the take's lane, instead of the waveform."""
        self._view_layers("view_spectrogram")

    def view_both(self) -> None:
        """Show both views, stacked: the waveform in the take's lane and the
        spectrogram in a lane under it, on one time axis."""
        self._view_layers("view_both")


def is_take(structure) -> bool:
    """Whether `edit` should open this in the audio editor: anything with a
    buffer number and samples it can write, which is what a
    `clausters.defs.Buffer` answers with."""
    return hasattr(structure, "bufnum") and hasattr(structure, "set_samples")


__all__ = ["AudioDomain", "AudioEditor", "is_take", "measures"]
