"""Editing a take as **a list of parts over immutable files**: the audio editor.

What it opens is **a file or a server buffer**, and nothing it was handed is
written until it is saved. The take is a list of spans of files on disk: a cut
takes a span out, a paste of a copy puts its spans in, and an edit that makes
new samples -- a pencil stroke, a pasted block, a mix, a gain curve applied,
a reverse -- **renders** them offline into a file of the editor's working
folder, which the list then names. Nothing a history entry names is ever
written again, so an undo is the list before: it costs the list and not the
samples. The window draws the list from its files, and the take sounds
through a **join** -- a buffer made of the list's spans (`Buffer.stitch`),
streamed from disk.

**The editor is the shared crate's** (`clausters._native.EditingCore`, the
``openAudio`` member): what each gesture does to the list, the renders that
make its new files, and which renders the history can no longer reach. What
is here is what a language owns: the server the steps are walked against,
the renders run on offline servers of their own, the working folder, and
deleting the renders the context hands back.

**What a gesture, a verb or the transport leads to is the shared crate's
coordinator's** (the context's ``audio`` door): every action runs there and
comes back as an ordered list of effects -- steps to carry out, props to set,
the window to draw again, an answer to send -- with, where the next step
depends on it, one thing to wait on. What is here carries those out.

**It sounds through nodes of its own**, the audio editor's (the shared
crate's ``AudioEditorPlayback``, which the coordinator holds; the GUI host's
monitor holds another):
one play graph per open take, all but the one played last paused, on a
transport of the editor's own, and an output beside them that meters the take
and declicks every play and stop on the way to the hardware. The window shows
the level on a meter beside the take, and the space bar and ``L`` are the
editor's -- the window says so (``plays``), so the host's monitor stays out.

**Disk is spent on what the history holds.** Every file a stroke or a paste
rendered is kept while an undo or a redo, or the clipboard, can still reach
it, and deleted when none can. The working folder is a temporary one unless
one is given, and goes with the editor.
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

#: The containers a part of a list reads in place; a file of any other codec
#: is opened through a buffer of it.
_STREAMED = ("wav", "w64", "rf64")


#: Why a verb that acts on a file is refused by an editor with none open.
_NO_FILE = "no file is open"

#: What a run's own error raises, by the coordinator's name for it.
_RAISED = {"value": ValueError, "command": CommandError, "runtime": RuntimeError}


class _AudioPlayback:
    """**What a script reads of one audio editor's playback**, which the
    shared crate's coordinator holds: the transport it plays on and its state.
    One per editor, shared by its open files: one file plays at a time and
    all others stay paused, and playing one window moves nothing in another.
    """

    def __init__(self, editor):
        self._editor = editor

    @property
    def transport_id(self) -> int:
        """The transport it plays on: one of the server's, allocated for it."""
        return int(self._editor._transport_id)

    def state(self) -> dict:
        """``{playing, meters, rolling, focus}``, as the playback was last told."""
        return self._editor._transport_state()["state"]


class _AudioDriver:
    """**What an audio editor's `clausters.defs.Transport` plays through**:
    the coordinator's transport verbs, its positions the take's seconds, and
    the time range and the loop switch the window shows -- the band a sweep
    leaves and `L` -- kept there, stopped or rolling, so a script and a hand
    read and write one state."""

    def __init__(self, editor):
        self.editor = editor

    @property
    def span(self):
        """The time range ``(start, end)`` in seconds, or ``None``."""
        span = self.editor._transport_state()["span"]
        return None if span is None else (float(span[0]), float(span[1]))

    @property
    def looping(self) -> bool:
        """Whether the loop switch is on: the span, or the whole take."""
        return bool(self.editor._transport_state()["looping"])

    def playing(self) -> bool:
        return bool(self.editor._audio("playing"))

    def play(self, at=None) -> None:
        self.editor._audio("play", **({} if at is None else {"at": float(at)}))

    def pause(self) -> None:
        self.editor._audio("pause")

    def stop(self) -> None:
        """Stop whichever file plays, from the window showing another file
        too, and bring the file shown into focus at its cursor."""
        self.editor._audio("stop")

    def locate(self, at: float) -> None:
        self.editor._audio("locate", at=float(at))

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
        self.editor._audio("span", show=bool(show), span=None if span is None
                           else [float(span[0]), float(span[1])])

    def set_looping(self, on: bool) -> None:
        self.editor._audio("looping", on=bool(on))


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
        return self.editor._operation(None, prepared=prepared)

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
            self.editor._audio("syncSound", file=file)

    def free(self, structure, buffers, spilled=()) -> None:
        """Free buffers the context handed back: nothing reaches them any
        more."""
        editor = self.editor
        if editor is None:
            return
        editor._check_server()
        for bufnum in buffers:
            release_buffer(editor._server, editor._editing.core,
                           {"server": editor._instance, "buffer": int(bufnum)})

    def delete_files(self, structure, files) -> None:
        """Delete renders the context handed back: no list, no entry and no
        clipboard names them any more."""
        _delete(files)


def _delete(files) -> None:
    import os
    for path in files or ():
        try:
            os.remove(path)
        except FileNotFoundError:
            pass


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

    **An editor with no file open** is the application on its own:
    ``AudioEditor()`` opens one, and closing its last file leaves it so. Its
    window keeps the menu bar and the file list and offers File -> New file
    and File -> Open file; a verb that acts on a file raises `ValueError`
    until one is added (`new_file`, `open_file`).

    Args:
        take: the `clausters.SoundFile` or the `clausters.defs.Buffer` to
            edit. It is never written. A file is read where it is, a buffer
            written to the working folder first. ``None`` opens the editor
            with no file.
        server: the server an editor with no file opens on; the ambient one
            when not given. An editor over a take uses the take's.
        sample_rate: the take's rate, when the handle does not know it.
        title: the window's title.
        layers: what the picture measures, innermost last.
        shared_history: whether the take's undo order is the editing
            context's shared one, which the other members of ``context`` walk
            too. By default each file the editor holds has an order of its own,
            whatever was opened before it.
        workspace: the directory the editor renders its files into, on the
            server's filesystem. A temporary one when not given, removed when
            the editor closes; one handed over is left where it is.
        file: the `clausters.SoundFile` `take` was loaded from, which a save
            writes -- one with no path asks for one. `clausters.gui.edit`
            loads a file and passes it here.
    """

    def __init__(self, take=None, *, server=None, sample_rate: float = 0.0, title: str = "Audio",
                 layers=MEASURES, workspace: "str | None" = None, file=None,
                 shared_history: bool = False, **options):
        from ...soundfile import SoundFile
        if isinstance(take, SoundFile):
            # **A file is read where it is**; one a part cannot read (a
            # compressed one) is decoded into a buffer first, which `edit`
            # does, and the open writes it to the working folder.
            if not reads_in_place(take):
                raise ValueError(f"{take.path or 'a file not written yet'} is not read in place: "
                                 "open it with `edit`, or pass a buffer of it with `file=`")
            file = take
        #: The `clausters.SoundFile` the take was loaded from, when it was.
        self._file = file
        empty = take is None
        rate = float(sample_rate or getattr(take, "sample_rate", 0.0) or 48_000.0)
        view = SamplesView(layers)
        domain = AudioDomain()
        if empty and options.get("context") is None:
            # no data to reach a context through: the editor has its own
            from ...history import Editing
            options["context"] = Editing()
        super().__init__(take, sample_rate=rate, domain=domain, view=view,
                         title=title, **options)
        domain.editor = self
        #: The server the take is on, and every take this editor makes.
        self._server = _resolve(server if server is not None else getattr(take, "server", None))
        # A file a part reads in place is the take, whatever buffer came with it.
        streamed = file is not None and reads_in_place(file)
        self._instance = _service_instance(self._server)
        #: The runner the steps are walked through.
        self._runner = _native.StepRunner()
        #: The join the window draws: the take as the edits have left it.
        self._display = self._server.buffers.alloc()
        # The context is the take's; switching files changes the structure,
        # so the window keeps the one it opened in.
        self._context = self._editing
        #: The folder the renders are written into, and whether it is this
        #: editor's to remove.
        self._owns_workspace = workspace is None
        if workspace is None:
            import tempfile
            workspace = tempfile.mkdtemp(prefix="clausters-audio-")
        self._workspace = str(workspace)
        if empty:
            key = f"audio:{self._instance['connection']}:{self._instance['generation']}:empty:{self._display}"
            request = {"empty": True}
        else:
            path = getattr(file, "path", None)
            held = f"file:{path}" if streamed else int(take.bufnum)
            key = f"audio:{self._instance['connection']}:{self._instance['generation']}:{held}"
            request = {"frames": int(getattr(take, "frames", 0) or 0),
                       "channels": max(1, int(getattr(take, "channels", 1) or 1)),
                       "rate": rate,
                       "path": path,
                       "untitled": self._file is not None and path is None,
                       "codec": getattr(file, "codec", None),
                       "sample_format": getattr(file, "sample_format", None)}
            # A file read in place is the take; a buffer, or a file a part
            # cannot read, is written to the working folder by the open.
            if not streamed:
                request["take"] = int(take.bufnum)
        self._member, self._structure_id = self._editing.open(
            "openAudio", key,
            {**self._facts(), **request, "display": self._display,
             "server": self._instance, "workspace": self._workspace,
             "sharedHistory": bool(shared_history),
             "layers": list(view.layers)}, self if empty else take, domain)
        self._sync_core()
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        self._server._ensure_recycler()
        #: What a script reads of the playback the coordinator holds.
        self._playback = _AudioPlayback(self)
        #: What the `transport` plays through.
        self._driver = _AudioDriver(self)
        # **The join over the take**: what the editor was handed is written
        # by a save and by nothing else. Then it sounds, on a transport of its
        # own.
        opened = self._audio("open", chunk=int(self._server._bulk_chunk()),
                             rate=self._engine_rate(), empty=empty)
        self._transport_id = int(opened["transport"])

    @property
    def transport(self):
        """**The take's transport**, as the object a script plays: a
        `clausters.defs.Transport` whose verbs (``play``, ``pause``,
        ``stop``, ``locate``, ``loop``, ``wait``) and ``span`` speak the
        take's seconds -- the span is the band a drag sweeps, the loop switch
        is `L`, and each side reads what the other set. It is this editor's
        own -- one of the server's, allocated for it -- so another window's
        play never moves it."""
        transport = self._server.transport_at(self._transport_id)
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

    def _clipboard(self, doc: str, *, ready: bool = False) -> None:
        """**A copy published**: the clipboard names the spans of files it is
        of, and the host's clipboard holds the document."""
        self._check_server()
        if ready:
            bind_exchange(self._server, self._editing.core)
        else:
            doc = exchange(self._server, "publish", document=doc)["document"]
        if self._host is not None:
            self._host.clipboard(doc)

    def _engine_rate(self) -> float:
        """The engine's sample rate, asked once; 0 when it will not say."""
        try:
            return float(self._server.query_info().nominal_sample_rate)
        except (RuntimeError, OSError, TimeoutError, AttributeError):
            return 0.0

    def _transport_state(self) -> dict:
        """What the coordinator holds of the transport: ``{transport, span,
        looping, paused, focused, state}``."""
        self._check_server()
        return self._editing.core.call_ids(self._server.ids, "audio", member=self._member,
                                           query="transport")

    # ---- the coordinator's runs ----

    def _audio(self, verb: str, *, held=None, **args):
        """**One action of the shared coordinator, carried out to its end**:
        its effects performed in order, each await answered, and its result
        handed back -- or what failed raised. ``held`` is the clipboard lease
        an event's payload came with, which a run says when to release."""
        self._check_server()
        action = {"verb": verb, "window": self._host is not None and self._window is not None,
                  **args}
        answer = self._editing.core.call_ids(self._server.ids, "audio", member=self._member,
                                             do=action)
        failure = None
        while True:
            if "run" not in answer:
                raise RuntimeError(answer.get("error", "the audio editor did not answer"))
            for effect in answer.get("effects") or ():
                self._effect(effect, held)
            self._publish_actions()
            if answer.get("done"):
                if answer.get("rethrow") and failure is not None:
                    raise failure
                error = answer.get("error")
                if error is not None:
                    raise _RAISED.get(error.get("type"), RuntimeError)(error.get("message"))
                return answer.get("result")
            try:
                result = self._await(answer["await"]) or {}
            except Exception as error:  # noqa: BLE001 -- the run decides what it means
                failure = error
                result = {"failed": str(error) or type(error).__name__}
            answer = self._editing.core.call_ids(self._server.ids, "audio", member=self._member,
                                                 run=answer["run"], result=result)

    def _await(self, wait: dict) -> dict:
        """What a run waits on, done here: the client's own work."""
        kind = wait["kind"]
        if kind == "steps":
            steps = wait.get("steps") or []
            if wait.get("via") == "service":
                execute_steps(self._server, self._instance, steps)
            elif wait.get("via") == "plain":
                self._check_server()
                run_steps(self._server, _native.StepRunner(), steps)
            else:
                self.domain.run(self.structure, steps)
            return {}
        if kind == "rolling":
            transport = self._server.transport_at(int(wait["transport"]))
            return {"rolling": bool(transport.state()["playing"])}
        if kind == "shape":
            return self._shape(wait)
        if kind == "batch":
            self._editing._run_batch(wait["member"], wait["server"], wait.get("steps") or [])
            return {}
        if kind == "stepped":
            return {"changed": bool(self.app.stepped(wait.get("stepped") or {}, self))}
        if kind == "clipboard":
            self._clipboard(wait["document"], ready=bool(wait.get("ready")))
            return {}
        raise RuntimeError(f"the audio editor cannot wait on {kind!r}")

    def _shape(self, wait: dict) -> dict:
        """**A file asked for in the window or by a script**: its header read,
        and a display buffer for its join. A new file is its shape; a file a
        part cannot read is decoded into a buffer the open writes out."""
        from ...soundfile import SoundFile
        if wait.get("shape") is not None:
            shape = wait["shape"]
            file = SoundFile.empty(int(shape["channels"]), int(shape["frames"]), float(shape["rate"]))
        else:
            file = SoundFile.open(wait["path"])
        answer = {"frames": int(file.frames), "channels": int(file.channels),
                  "rate": float(file.sample_rate),
                  "display": self._server.buffers.alloc(), "path": file.path,
                  "untitled": file.path is None, "codec": file.codec,
                  "sample_format": file.sample_format}
        if file.path is not None and not reads_in_place(file):
            answer["take"] = int(file.to_buffer(server=self._server).bufnum)
        return answer

    def _effect(self, effect: dict, lease) -> None:
        """One effect of a run, carried out here."""
        kind = effect["kind"]
        shown = self._host is not None and self._window is not None
        if kind == "version":
            if effect.get("version") is not None:
                self._editing.version = int(effect["version"])
        elif kind == "echo":
            self.echo.send(effect.get("answer"))
        elif kind == "set":
            if shown:
                self._host.set(int(effect["widget"]), **effect["props"])
        elif kind == "publish":
            if self._window is not None:
                self.app.publish(int(self._window), self.draw())
        elif kind == "band":
            self._show_band(effect)
        elif kind == "looping":
            self._show_looping(bool(effect["on"]))
        elif kind == "bind":
            self._bind_file(effect.get("binding"))
        elif kind == "layers":
            self.view.layers = tuple(effect["layers"])
        elif kind == "cursor":
            self.cursor = float(effect["at"])
        elif kind == "locate":
            self.cursor = float(effect["at"])
            self.locate(self.cursor)
            if self.composed_in is not None:
                self.composed_in.locate(self.cursor)
            if callable(self.on_locate):
                self.on_locate(self.cursor)
        elif kind == "changed":
            self._editing.changed()
        elif kind == "free":
            self.domain.free(self.structure, effect.get("buffers") or [])
        elif kind == "release":
            self._editing.release(effect.get("freed"), effect.get("stored"))
        elif kind == "delete":
            _delete(effect.get("files"))
        elif kind == "returned":
            self._editing._operation_returned(effect.get("returned"))
        elif kind == "operationEcho":
            self._editing._operation_echo(effect.get("echo"))
        elif kind == "bufferFree":
            if self._instance == _service_instance(self._server):
                buffer = int(effect["buffer"])
                self._server.send_msg("/buffer_free", buffer)
                self._server.buffers.free(buffer)
        elif kind == "lease":
            if lease is not None:
                exchange(self._server, "release", lease=lease)
        elif kind == "closeWindow":
            Editor.close(self)
        elif kind == "closedWindow":
            Editor._closed(self)

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
            self._host.head_clock(window, "transport", self._transport_id)
        return window

    def _plays_on(self):
        return self._server

    def new_file(self, channels: int, frames: int, rate: float) -> None:
        """Add and focus an empty SoundFile of the requested shape."""
        self._audio("newFile", shape={"channels": int(channels), "frames": int(frames),
                                      "rate": float(rate)})

    def open_file(self, paths) -> None:
        """Open a path or a list of paths in this editor, focusing the last."""
        paths = list(paths) if isinstance(paths, (list, tuple)) else str(paths)
        self._audio("openFile", paths=paths)

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
        self._audio("verb", call={"verb": "focus_file", "index": index})

    def close_file(self, index=None) -> None:
        """Close the focused file, or the file at zero-based index."""
        self._audio("verb", call={"verb": "close_file", **({} if index is None else {"index": index})})

    def show_files(self) -> None:
        """Show the independent file list to the left of the editing view."""
        self._audio("verb", call={"verb": "show_files"})

    def hide_files(self) -> None:
        """Hide the file list."""
        self._audio("verb", call={"verb": "hide_files"})

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

    def _bind_file(self, binding) -> None:
        """**Bind the file in focus**: its buffers, its structure in the
        history, and the cursor, selection, layers and loop switch it left --
        or, with ``None``, no file: the last one was closed."""
        from ...defs.buffer import Buffer
        if binding is None:
            self.structure = None
            self.cursor = None
            return
        self._display = int(binding["display"])
        self.sample_rate = float(binding["rate"])
        self.structure = Buffer(int(binding["take"]), int(binding["frames"]),
                                int(binding["channels"]), self.sample_rate, server=self._server)
        self._structure_id = binding.get("structure", self._structure_id)
        self._editing.version = int(binding.get("version", self._editing.version))
        self.cursor = float(binding["cursor"]) / self.sample_rate
        self.view.layers = tuple(binding["layers"])

    def _closed(self) -> bool:
        """The window closed: the take stops being one the editor plays, and
        the last one closed frees the editor's nodes."""
        self._audio("closed")
        return False

    def close(self):
        """Close the window, and free what sounded the take. A working folder
        the editor made goes with it, its renders deleted by the close -- all
        but the ones the clipboard still names, which stay where they are."""
        self._audio("close")
        if self._owns_workspace:
            import os
            import shutil
            shutil.rmtree(os.path.join(self._workspace, "peaks"), ignore_errors=True)
            try:
                os.rmdir(self._workspace)
            except OSError:
                pass  # what the clipboard names, kept
        return self

    @property
    def dirty(self) -> bool:
        """**Whether the file in focus has edits its file does not hold**: the
        editor's own reading, asked each time. A save that has not completed
        leaves it dirty; an undo back to what was written leaves it clean."""
        member = getattr(self, "_member", None)
        if member is None:
            return False
        return bool(self._editing.member(member, "binding").get("dirty"))

    @dirty.setter
    def dirty(self, value) -> None:
        """Nothing: the base editor's mark of a change since the last render,
        which an audio editor answers from the file instead."""

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
        if answer.get("error") == _NO_FILE:
            raise ValueError(_NO_FILE)
        return answer

    def _check_server(self):
        if _service_instance(self._server) != self._instance:
            self._editing._service_call("invalidate", server=self._instance)
            raise RuntimeError("the audio service belongs to a previous server instance")

    def _sync_core(self) -> None:
        """Hand the core the chrome and the window it is open in."""
        facts = {k: v for k, v in self._facts().items() if v is not None}
        self._call("sync", **facts)

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

    def reverse(self) -> bool:
        """Play the selection backwards, or the whole take with nothing
        selected -- rendered into a file the list then names. The window's
        ``reverse`` (`verb`)."""
        return self.verb("reverse")

    def retry_pending(self) -> None:
        """Retry failed repairs for this editor's files without repeating edits.

        Call after resolving the I/O failure. Pending contents and history stay
        unchanged; successful repair permits editing, saving and closing again.
        Raises `CommandError` if repair fails again. Does nothing if none remain.
        """
        self._audio("retry")

    def _operation(self, command, prepared=None, *, repairing=False):
        """Execute typed batches; commit, refusal and cleanup come from Rust."""
        result = self._audio("operation", command=command, prepared=prepared,
                             repairing=bool(repairing))
        return result["completed"]

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
        answer = self._audio("save", request=request)["destination"]
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
        """What the take is made of now: one ``dict`` per span of a file, in
        reading order -- its ``path``, the ``start`` and the ``frames`` of it
        read, and its fades and shape."""
        return list(self._call("segments").get("parts") or ())

    # ---- the crate's turns ----

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        prepared = exchange(self._server, "event", args=_plain(list(args))) if addr == "/gui_event" else {"args": _plain(list(args))}
        lease = prepared.get("lease")
        return bool(self._audio("event", held=lease, addr=str(addr), args=prepared["args"],
                                **({} if lease is None else {"lease": True})))

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off."""
        wid, tag, values = args[0], args[1], list(args[2:])
        return self._deliver("/gui_event", [wid, 0, 0, tag, *values])

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
        self._audio("verb", call={"verb": verb, "version": self._editing.version, **args})

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
        result = self._audio("verb", call={"verb": verb, "version": self._editing.version, **args})
        reason = (result or {}).get("reason")
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
            result = self._audio("act", call=_plain({"verb": verb, **args})) or {}
            if result.get("reason"):
                raise ValueError(result["reason"])
            return bool(result.get("changed"))

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


def reads_in_place(file) -> bool:
    """Whether the audio editor reads ``file``, a `clausters.SoundFile`,
    where it is: a PCM file a part streams from. Any other is decoded into a
    buffer first."""
    return file.path is not None and file.codec in _STREAMED


def is_take(structure) -> bool:
    """Whether `edit` should open this in the audio editor: anything with a
    buffer number and samples it can write, which is what a
    `clausters.defs.Buffer` answers with."""
    return hasattr(structure, "bufnum") and hasattr(structure, "set_samples")


__all__ = ["AudioDomain", "AudioEditor", "is_take", "measures", "reads_in_place"]
