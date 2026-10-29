"""The **multitrack over a saved session**: its tracks read from their files
and played by the application the GUI host holds.

The multitrack is one program, and it is not this one. The GUI host opens it
over a session file (`clausters.gui.host.GuiHost.open_app`), in a window of
the host's, on an engine of the application's own: the session's takes are
read where they are -- their headers when it opens, their samples as the
transport reads them, their peaks, kept in the session's ``peaks/`` folder,
as the window draws them -- and what a gesture does, the history, the
playback and the save all run in the host's process. This object is its
**handler**: every member below is one call to the application
(`clausters.gui.host.GuiHost.call_app`), and what it answers is read from
there each time rather than kept here.

A box of notes plays the sequence the session holds for it, and a double
click on one opens its roll in the same application, one history with the
multitrack.
"""


class SessionTransport:
    """**The multitrack's transport**, as a script plays it: each verb one
    call to the application, in the multitrack's seconds."""

    def __init__(self, editor):
        self._editor = editor

    @property
    def playing(self) -> bool:
        """Whether the transport rolls."""
        return bool(self._editor._do("playing"))

    def play(self, start: "float | None" = None, end: "float | None" = None,
             *, looping: bool = False) -> "SessionTransport":
        """**Roll**: over ``start``..``end`` seconds when both are given (and
        over and over with ``looping``), else from where it stands."""
        args = {"looping": bool(looping)}
        if start is not None and end is not None:
            args["range"] = [float(start), float(end)]
        self._editor._do("play", **args)
        return self

    def pause(self) -> "SessionTransport":
        """Hold where it is; a `play` goes on from there."""
        self._editor._do("pause")
        return self

    def stop(self, mark: float = 0.0) -> "SessionTransport":
        """Stop, and go back to ``mark`` seconds."""
        self._editor._do("stop", mark=float(mark))
        return self

    def locate(self, at: float) -> "SessionTransport":
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


class SessionEditor:
    """**The multitrack the GUI host holds, over a saved session, and this
    script its handler.**

    `clausters.gui.edit` over a `clausters.multitrack.Session` that has a
    file answers one, open. Each member is a call to the application in the
    host's process, answered by it.

    Args:
        session: the `clausters.multitrack.Session` to open, by the file it
            was opened from or saved to.
        title: the window's title; the session file's name when not given.

    Raises:
        ValueError: the session has no file: save it first.
    """

    def __init__(self, session, *, title: "str | None" = None):
        path = getattr(session, "path", None)
        if path is None:
            raise ValueError(
                "the multitrack opens a session from its file, and this one has "
                "none: save it first (`session.save(path)`)")
        self._session = session
        self._path = str(path)
        #: The window's title.
        self.title = str(title) if title is not None else self._path.rsplit("/", 1)[-1]
        self._host = None
        self._window = None

    # ---- the window ----

    def open(self, host=None):
        """**Ask the host to open the multitrack**, on ``host`` or the ambient
        one, and answer its window. One already open answers the window it
        has."""
        if self._window is not None:
            return self._window
        from ..handle import WindowHandle
        from .application import _resolve_host

        host = _resolve_host(host, None)
        window = host.open_app("multitrack", {"title": self.title, "path": self._path})
        self._host = host
        self._window = WindowHandle(host, int(window), {})
        # the host's loop delivers the window's close from now on
        host.loop
        return self._window

    @property
    def session(self):
        """The `clausters.multitrack.Session` it was opened over, as it was
        then: what the application saves is read from the file again."""
        return self._session

    @property
    def path(self) -> str:
        """The session file it opened."""
        return self._path

    @property
    def host(self):
        """The `clausters.gui.host.GuiHost` it is open on, or ``None``."""
        return self._host

    @property
    def window(self):
        """The open window's handle, or ``None``."""
        return self._window

    @property
    def closed(self) -> bool:
        """Whether the window is gone."""
        return self._window is None or self._window.closed

    def wait(self, timeout: "float | None" = None) -> bool:
        """Hold the calling thread until the window is closed: ``True`` when
        it closed, ``False`` when ``timeout`` ran out first."""
        return True if self._window is None else self._window.wait(timeout)

    def on_closed(self, func) -> "SessionEditor":
        """Call ``func()`` when the window is closed, and return ``self``.
        ``None`` clears it."""
        self._opened().on_closed(func)
        return self

    def close(self) -> "SessionEditor":
        """Close the window, and what plays it."""
        if not self.closed:
            self._do("close")
        return self

    def _opened(self):
        if self._window is None:
            raise RuntimeError("open() the multitrack first")
        return self._window

    # ---- the door ----

    def _call(self, call: dict, timeout: "float | None" = None):
        """One call to the application, and its answer."""
        return self._host.call_app(self._opened(), call, timeout)

    def _query(self, name: str) -> dict:
        return self._call({"query": name}) or {}

    def _do(self, verb: str, **args):
        """One action of the multitrack, carried to its end, and its result."""
        return self._call({"do": {"verb": verb, **args}})

    # ---- the window's verbs ----

    def verb(self, name: str) -> bool:
        """**Perform the window's verb ``name``**, as its key, its menu entry
        or its tool would (``docs/verbs.md``). Answers whether it was asked:
        ``False`` with no window open."""
        if self.closed:
            return False
        self._host.verb(self._window, str(name))
        return True

    def undo(self) -> bool:
        """Step the history back one edit (`verb`)."""
        return self.verb("undo")

    def redo(self) -> bool:
        """Step the history forward again (`verb`)."""
        return self.verb("redo")

    def split(self) -> bool:
        """Split the box under the cursor: the window's ``split`` (`verb`)."""
        return self.verb("split")

    def join(self) -> bool:
        """Join the selected boxes: the window's ``join`` (`verb`)."""
        return self.verb("join")

    def quantize(self) -> bool:
        """Move the selected boxes onto the grid: the window's ``quantize``
        (`verb`)."""
        return self.verb("quantize")

    # ---- the transport, and what is heard ----

    @property
    def transport(self) -> SessionTransport:
        """**The multitrack's transport** (`SessionTransport`)."""
        return SessionTransport(self)

    @property
    def output(self) -> dict:
        """**What the engine plays**: each track's meter levels by track id
        (``tracks``), and the samples played as silence for want of a frame
        read in time since the engine opened (``starved``)."""
        return self._query("output")

    # ---- saving ----

    def save(self, path: "str | None" = None) -> str:
        """**Write the session as the edits have left it**: over the file it
        was opened from, or, given a ``path``, there, which a later `save`
        then writes over. Every file inside the session's folder is written
        relative to it. Answers the path written."""
        args = {} if path is None else {"path": str(path)}
        answer = self._do("save", **args) or {}
        return answer.get("destination", self._path)
