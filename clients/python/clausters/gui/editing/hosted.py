"""**A handler of an application the GUI host holds**: what a script keeps while
a window of the host's has what it opened.

The roll and the score editor are applications of the host, as the audio editor
and the multitrack are: the window, what a gesture does, the history, the
engraver and the playback all run in the host's process
(`clausters.gui.host.GuiHost.open_app`). An editor here is the script's handle:
each member is one call to the application
(`clausters.gui.host.GuiHost.call_app`), and what it answers is read from there
each time rather than kept here.

**The script's structure is the window's while one is open.** An
`clausters.seq.EventSequence` or a `clausters.gui.notation.Score` keeps its
identity and reads and writes what the window holds: a change is an edit of the
window's, undone by its Ctrl+Z. When the last window over it goes, it holds what
that window left it as.
"""

from .window_verbs import WindowVerbs


class MultitrackTransport:
    """**The multitrack's transport**, as a script plays it: each verb one
    call to the application, in the multitrack's seconds."""

    def __init__(self, editor):
        self._editor = editor

    @property
    def playing(self) -> bool:
        """Whether the transport rolls."""
        return bool(self._editor._do("playing"))

    def play(self, start: "float | None" = None, end: "float | None" = None,
             *, looping: bool = False) -> "MultitrackTransport":
        """**Roll**: over ``start``..``end`` seconds when both are given (and
        over and over with ``looping``), else from where it stands."""
        args = {"looping": bool(looping)}
        if start is not None and end is not None:
            args["range"] = [float(start), float(end)]
        self._editor._do("play", **args)
        return self

    def pause(self) -> "MultitrackTransport":
        """Hold where it is; a `play` goes on from there."""
        self._editor._do("pause")
        return self

    def stop(self, mark: float = 0.0) -> "MultitrackTransport":
        """Stop, and go back to ``mark`` seconds."""
        self._editor._do("stop", mark=float(mark))
        return self

    def locate(self, at: float) -> "MultitrackTransport":
        """Stand the transport at ``at`` seconds."""
        self._editor._do("locate", at=float(at))
        return self

    def wait(self, timeout: "float | None" = None) -> bool:
        """Block until the transport stops, or ``timeout`` seconds pass.
        Answers whether it stopped."""
        import time

        deadline = None if timeout is None else time.monotonic() + float(timeout)
        while self.playing:
            if deadline is not None and time.monotonic() >= deadline:
                return False
            time.sleep(0.05)
        return True


class _RemoteHandle:
    """**The multitrack while the application has it open**: each verb of
    the multitrack's handle (`clausters._native.MultitrackHandle`) asked of
    the application, which answers it over the multitrack its window holds."""

    def __init__(self, editor):
        self._editor = editor

    def call(self, verb: str, **args):
        """One verb: its answer. Raises `ValueError` for a refusal."""
        return self._editor._handle({"verb": verb, **args})


class _RemoteHistory:
    """**The window's history, as the script's multitrack reads it** while
    the application has it open: the undo order the window walks, which the
    script's changes are entries of."""

    def __init__(self, editor):
        self._editor = editor

    def __call__(self, label: str = "edit"):
        return self._editor._block(str(label))

    def undo(self) -> bool:
        """Step back over the last entry -- a script's or a window's -- and
        answer whether anything moved."""
        return bool(self._editor._do("undo"))

    def redo(self) -> bool:
        """Step forward again after `undo`; whether anything moved."""
        return bool(self._editor._do("redo"))

    @property
    def can_undo(self) -> bool:
        """Whether there is an entry to step back over."""
        return bool(self._editor._query("history").get("canUndo"))

    @property
    def can_redo(self) -> bool:
        """Whether there is an undone entry to step forward into."""
        return bool(self._editor._query("history").get("canRedo"))

    @property
    def undo_label(self) -> "str | None":
        """What an undo would take back, as a menu names it."""
        return self._editor._query("history").get("undoLabel")

    @property
    def redo_label(self) -> "str | None":
        """What a redo would bring back."""
        return self._editor._query("history").get("redoLabel")

    def __repr__(self) -> str:
        return f"<UndoHistory, undo {self.undo_label!r}>"


class _Block:
    """A ``with multitrack.history(label):`` over the open window: every
    change inside it applied as it is made and recorded at the end as one
    entry."""

    def __init__(self, editor, label: str):
        self._editor = editor
        self._label = label

    def __enter__(self):
        editor = self._editor
        editor._depth += 1
        if editor._depth == 1:
            editor._legs = []
        return self

    def __exit__(self, *exc):
        editor = self._editor
        editor._depth -= 1
        if editor._depth == 0:
            legs, editor._legs = editor._legs, None
            if legs and not editor.closed:
                editor._handle({"verb": "record", "legs": legs}, label=self._label)
        return False


class _Held:
    """**A structure while windows have it**: the editors over it, the first
    still open answering for it, and what the last of them left."""

    def __init__(self):
        self.editors: list = []

    def live(self):
        """The editor whose window answers for the structure, or ``None``."""
        return next((editor for editor in self.editors if not editor.closed), None)

    def host(self):
        """The host its windows are on, or ``None`` with none open."""
        live = self.live()
        return None if live is None else live.host


class HostedEditor(WindowVerbs):
    """**The script's handle of a window the host holds**: its opening, its door,
    its history and its closing, which every application's handler shares (the
    audio editor, the multitrack, a roll, a page). An editor says which
    application it is (``_app``), how it opens (``_options``), what is the
    window's once it opens (``_take``) and what it takes back when a window
    goes (``_settle``).

    Args:
        title: the window's title.
        width, height: its size, in logical pixels; left out, the application's.
        chrome: ``False`` opens what is edited with none of the application's
            chrome around it, the keys kept.
    """

    _app = ""

    def __init__(self, *, title: str, width: "int | None" = None,
                 height: "int | None" = None, chrome: bool = True):
        #: The window's title.
        self.title = str(title)
        self._size = (width, height)
        #: Whether the window carries the application's chrome.
        self.chrome = bool(chrome)
        #: Called with no arguments after any gesture that **changed the
        #: data**, in this window or another over the same structure. ``None``
        #: to be told nothing.
        self.on_change = None
        self._dirty = False
        self._host = None
        self._window = None
        self._on_closed = None
        self._settled = True
        self._released = False
        self._depth = 0
        self._legs = None

    @property
    def dirty(self) -> bool:
        """Whether a hand has changed the data since this was last cleared."""
        return self._dirty

    @dirty.setter
    def dirty(self, value: bool) -> None:
        self._dirty = bool(value)

    # ---- the window ----

    def _has_window(self) -> bool:
        return not self.closed

    def _options(self, beside) -> dict:
        """What the application opens with; ``beside`` is the window it opens
        next to when another already has the structure."""
        raise NotImplementedError

    def _held(self) -> _Held:
        """What says who has this editor's structure."""
        raise NotImplementedError

    def _take(self) -> None:
        """The structure is the window's from now on."""

    def _settle(self, left: "dict | None", last: bool) -> None:
        """The window went, leaving ``left``; with ``last`` it was the last
        over the structure, which is the script's again."""

    def open(self, host=None):
        """**Ask the host to open this editor's window**, on ``host`` or the
        ambient one, and answer it. One already open answers the window it has.
        From then on the structure it was opened over is the window's (the
        module's docstring)."""
        if self._window is not None and not self.closed:
            return self._window
        from .application import _resolve_host

        if self._window is not None:
            # a window that went before its close was heard: what it left is
            # taken back first
            self._leave()
        host = _resolve_host(host, None)
        held = self._held()
        beside = held.live()
        if beside is not None and beside.host is not host:
            raise ValueError("this structure is open in a window of another host: "
                             "close it there first")
        options = {"title": self.title, **self._options(beside)}
        if self._size[0] is not None:
            options["w"] = int(self._size[0])
        if self._size[1] is not None:
            options["h"] = int(self._size[1])
        if not self.chrome:
            options["chrome"] = False
        if beside is not None:
            options["over"] = int(beside.window)
        window = host.open_app(self._app, options)
        return self._hold(host, int(window))

    def _hold(self, host, window: int):
        """**The handler of ``window``** from now on, a window of ``host``'s
        this editor's application holds, opened by `open` or found by another
        editor (a box's roll). The structure is the window's unless another
        window of the script's has it already."""
        from ..handle import WindowHandle

        take = self._held().live() is None
        self._host = host
        self._window = WindowHandle(host, window, {})
        self._settled = False
        self._released = False
        self._window.on_closed(self._closed)
        host._set_changed_handler(window, self._changed)
        host._set_released_handler(window, self._gave_up)
        self._held().editors.append(self)
        # the host's loop delivers the window's close from now on, and the
        # answers of what follows
        host.loop
        if take:
            self._take()
        return self._window

    def _changed(self) -> None:
        """A hand moved what the window holds."""
        self._dirty = True
        if self.on_change is not None:
            self.on_change()

    def _closed(self) -> None:
        """The window went: the structure holds what it left it as."""
        self._leave()
        if self._on_closed is not None:
            self._on_closed()

    def _gave_up(self) -> None:
        """The window let go of what it had and goes on as the host's own: the
        structure holds what it left it as, and this handler is done with it."""
        self._released = True
        self._leave(self._host._closed_snapshot(int(self._window)))
        if self._on_closed is not None:
            self._on_closed()

    def _leave(self, left: "dict | None" = None) -> None:
        if self._settled:
            return
        self._settled = True
        window = int(self._window)
        if left is None:
            left = self._host._closed_with(window)
        self._host._set_changed_handler(window, None)
        held = self._held()
        held.editors = [editor for editor in held.editors if editor is not self]
        self._settle(left, held.live() is None)

    @property
    def host(self):
        """The `clausters.gui.host.GuiHost` it is open on, or ``None``."""
        return self._host

    @property
    def window(self):
        """The open window's handle, or ``None``."""
        return self._window

    @property
    def id(self):
        """The open window's id, or ``None``."""
        return None if self._window is None else int(self._window)

    @property
    def closed(self) -> bool:
        """Whether it is done with its window: the window is gone, or it let go
        of what this handler opened (the window goes on as the host's own)."""
        return self._window is None or self._window.closed or self._released

    def wait(self, timeout: "float | None" = None) -> bool:
        """Hold the calling thread until the window is closed: ``True`` when it
        closed, ``False`` when ``timeout`` ran out first."""
        if self._window is None or self._released:
            return True
        done = self._window.wait(timeout)
        if done:
            self._leave()
        return done

    def on_closed(self, func):
        """Call ``func()`` when its window goes, and return ``self``. ``None``
        clears it."""
        self._opened()
        self._on_closed = func
        return self

    def close(self):
        """Close the window, and what plays it, without asking: what it edits
        is the script's, and is left as the window has it."""
        if not self.closed:
            self._do("close")
            self._host._wait_while(lambda: not self.closed, 5.0)
        if self._window is not None:
            self._leave()
        return self

    def _opened(self):
        if self._window is None:
            raise RuntimeError(f"open() the {self._app} editor first")
        return self._window

    # ---- the door ----

    def _app_call(self, call: dict):
        """One call to the application through the window's door, and its
        answer."""
        window = self._opened()
        return self._host.call_app(window, call)

    def _call(self, verb: str, **args) -> dict:
        """One verb of the window's editor, read; it changes nothing the window
        draws."""
        return self._app_call({"member": {"verb": verb, **args}}) or {}

    def _set(self, **args) -> None:
        """A setting of the window's, which then redraws it."""
        self._do("set", call={"verb": "sync", **args})

    def _query(self, name: str) -> dict:
        return self._app_call({"query": name}) or {}

    def _do(self, verb: str, **args):
        """One action of the application, carried to its end, and its result."""
        return self._app_call({"do": {"verb": verb, **args}})

    # ---- the history its window walks ----

    def undo(self) -> bool:
        """Step back over the last entry of the history its window walks;
        whether anything moved."""
        return bool(self._do("undo"))

    def redo(self) -> bool:
        """Step forward again after `undo`; whether anything moved."""
        return bool(self._do("redo"))

    @property
    def can_undo(self) -> bool:
        """Whether there is an edit to step back over."""
        return bool(self._query("history").get("canUndo"))

    @property
    def can_redo(self) -> bool:
        """Whether there is an undone edit to step forward into."""
        return bool(self._query("history").get("canRedo"))

    @property
    def undo_label(self) -> "str | None":
        """What an undo would be called, for a menu item."""
        return self._query("history").get("undoLabel")

    @property
    def redo_label(self) -> "str | None":
        """What a redo would be called, for a menu item."""
        return self._query("history").get("redoLabel")

    #: What a window's time ruler counts in, by the verb that chooses it.
    _RULER_UNITS = ("beats", "time", "samples")

    def _ruler(self) -> str:
        """The unit the window's time ruler counts in, read off its actions."""
        actions = self.actions
        for unit in self._RULER_UNITS:
            if (actions.get(f"ruler_{unit}") or {}).get("checked"):
                return unit
        return ""

    def _set_ruler(self, unit: str) -> None:
        """Ask the window's ruler verb for ``unit``; `ValueError` for no unit."""
        if unit not in self._RULER_UNITS:
            raise ValueError(f"a ruler counts in 'beats', 'time' or 'samples', not {unit!r}")
        self.verb(f"ruler_{unit}")

    @property
    def actions(self) -> dict:
        """**The window's actions as they read now**, by verb: what each
        addresses (``target``), whether it can be taken (``enabled``, and
        ``reason`` when not), a switch's or a choice's ``checked``."""
        return self._query("actions")

    def adopt(self) -> None:
        """Nothing to bring in step: what a script changes is the window's
        already, and every window over it is redrawn when it does."""


class HostedTransport(MultitrackTransport):
    """**The transport a roll or a page plays on**, as a script plays it, in
    beats: its window's own, each verb one call to the application."""

    def __init__(self, editor):
        super().__init__(editor)
        self._span = None
        self._looping = False

    @property
    def span(self) -> "tuple | None":
        """The time range the space bar plays, ``(start, end)`` in beats, which
        an Alt+drag on a roll sweeps; ``None`` with none. Set it to draw one."""
        written = self._editor._call("range").get("range")
        return None if written is None else (float(written[0]), float(written[1]))

    @span.setter
    def span(self, span) -> None:
        self._editor.show_span(span)

    def loop(self, start: "float | None" = None, end: "float | None" = None,
             *, on: bool = True) -> "HostedTransport":
        """The loop switch: over ``start``..``end`` beats when both are given,
        else over the span or every note, followed at once by a pass under
        way."""
        args = {"looping": bool(on)}
        if start is not None and end is not None:
            args["range"] = [float(start), float(end)]
        self._editor._do("loop", **args)
        return self
