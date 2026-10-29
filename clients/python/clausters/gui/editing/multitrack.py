"""The **multitrack**: its tracks read from their files and played by the
application the GUI host holds, and this script its handler.

The multitrack is one program, and it is not this one. The GUI host opens it
(`clausters.gui.host.GuiHost.open_app`) in a window of the host's, on an
engine of the application's own: over a saved session, whose takes are read
where they are -- their headers when it opens, their samples as the transport
reads them, their peaks, kept in the session's ``peaks/`` folder, as the
window draws them -- or over a `clausters.multitrack.Multitrack` handed to it
whole, an **untitled session** with no file behind it until it is saved.
What a gesture does, the history, the playback and the save all run in the
host's process. Every member below is one call to the application
(`clausters.gui.host.GuiHost.call_app`), and what it answers is read from
there each time rather than kept here.

**The script's multitrack is the window's while it is open.** The
`clausters.multitrack.Multitrack` it was opened over -- the session's, or the
one handed over -- reads and writes the multitrack the application holds: a
read answers what the window shows now, a change is an edit of the window's,
undone by its Ctrl+Z, and ``multitrack.history`` is the window's history.
When the window goes, the multitrack holds what the window left it as.

A box of notes plays the sequence the session holds for it, and a double
click on one opens its roll in the same application, one history with the
multitrack.
"""

import os


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


class MultitrackEditor:
    """**The multitrack the GUI host holds, and this script its handler.**

    `clausters.gui.edit` over a `clausters.multitrack.Session` that has a
    file, or over a `clausters.multitrack.Multitrack`, answers one, open.
    Each member is a call to the application in the host's process,
    answered by it.

    Args:
        structure: what to open: a `clausters.multitrack.Session`, by the
            file it was opened from or saved to; or a
            `clausters.multitrack.Multitrack`, as an untitled session whose
            first `save` names its file.
        title: the window's title; the session file's name when not given,
            ``"Untitled"`` for a multitrack.

    Raises:
        ValueError: a session with no file: save it first.
    """

    def __init__(self, structure, *, title: "str | None" = None):
        from ...multitrack import Multitrack

        if isinstance(structure, Multitrack):
            self._session = None
            self._multitrack = structure
            self._path = None
            default = "Untitled"
        else:
            path = getattr(structure, "path", None)
            if path is None:
                raise ValueError(
                    "the multitrack opens a session from its file, and this one has "
                    "none: save it first (`session.save(path)`)")
            self._session = structure
            self._multitrack = structure.multitrack
            self._path = str(path)
            default = os.path.basename(self._path)
        #: The window's title.
        self.title = str(title) if title is not None else default
        self._host = None
        self._window = None
        self._local = None
        self._on_closed = None
        self._depth = 0
        self._legs = None

    # ---- the window ----

    def open(self, host=None):
        """**Ask the host to open the multitrack**, on ``host`` or the ambient
        one, and answer its window. One already open answers the window it
        has. From then on the multitrack it was opened over is the window's
        (the module's docstring)."""
        if self._window is not None:
            return self._window
        from ..handle import WindowHandle
        from .application import _resolve_host

        host = _resolve_host(host, None)
        if self._path is not None:
            options = {"title": self.title, "path": self._path}
        else:
            options = {"title": self.title, "multitrack": self._multitrack.write()}
        window = host.open_app("multitrack", options)
        self._host = host
        self._window = WindowHandle(host, int(window), {})
        self._window.on_closed(self._closed)
        # the script's multitrack reads and writes the window's from now on
        self._local = self._multitrack._mt
        self._multitrack._mt = _RemoteHandle(self)
        # the host's loop delivers the window's close from now on
        host.loop
        return self._window

    def _closed(self) -> None:
        """The window went: the multitrack holds what it left it as."""
        self._settle()
        if self._on_closed is not None:
            self._on_closed()

    def _settle(self) -> None:
        """Hand the script's multitrack back, once the window is gone."""
        if self._local is None:
            return
        from ... import _native

        left = self._host._closed_with(int(self._window))
        local, self._local = self._local, None
        self._multitrack._mt = _native.MultitrackHandle(left) if left is not None else local

    @property
    def multitrack(self):
        """The `clausters.multitrack.Multitrack` it was opened over: the
        window's while it is open."""
        return self._multitrack

    @property
    def session(self):
        """The `clausters.multitrack.Session` it was opened over, or ``None``
        for an untitled one."""
        return self._session

    @property
    def path(self) -> "str | None":
        """The session file it opened, or ``None`` for an untitled one."""
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
        if self._window is None:
            return True
        done = self._window.wait(timeout)
        if done:
            self._settle()
        return done

    def on_closed(self, func) -> "MultitrackEditor":
        """Call ``func()`` when the window is closed, and return ``self``.
        ``None`` clears it."""
        self._opened()
        self._on_closed = func
        return self

    def close(self) -> "MultitrackEditor":
        """Close the window, and what plays it."""
        if not self.closed:
            self._do("close")
            self._host._wait_while(lambda: not self.closed, 5.0)
        self._settle()
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

    def _handle(self, call: dict, *, label: "str | None" = None):
        """One verb of the multitrack's handle, asked of the window's. An edit
        inside a ``with multitrack.history(...)`` block is applied now and
        recorded when the block ends."""
        if self.closed:
            self._settle()
            return self._multitrack._mt.call(**call)
        args = {"call": call}
        if label is not None:
            args["label"] = label
        block = self._legs is not None and call.get("verb") == "apply"
        if block:
            args["record"] = False
        answer = self._do("handle", **args)
        if block and isinstance(answer, dict) and answer.get("applied") \
                and "current" in answer:
            self._legs.append({"forward": call["intent"], "backward": answer["current"]})
        return answer

    def _edit(self, intent: dict, label: str) -> dict:
        """A change the script's multitrack makes: an edit of the window's,
        under ``label``."""
        return self._handle({"verb": "apply", "intent": intent}, label=label)

    def _block(self, label: str) -> _Block:
        return _Block(self, label)

    @property
    def _history(self) -> _RemoteHistory:
        return _RemoteHistory(self)

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

    def stop_at_end(self) -> bool:
        """Switch whether a pass stops where the contents end: the window's
        ``stop_at_end`` (`verb`)."""
        return self.verb("stop_at_end")

    def reset_heights(self) -> bool:
        """Give every track its default height back: the window's
        ``reset_heights`` (`verb`)."""
        return self.verb("reset_heights")

    def compact_tracks(self) -> bool:
        """Fit every track into the window: the window's ``compact_tracks``
        (`verb`)."""
        return self.verb("compact_tracks")

    # ---- the grid, and how a box of notes is drawn ----

    def _editor(self, call: dict):
        """One verb of the window's editor (``grid``, ``sync``,
        ``notesView``), and its answer."""
        return self._do("verb", call=call) or {}

    @property
    def grid(self) -> float:
        """The grid a box snaps to and `quantize` moves it onto, in beats."""
        return float(self._editor({"verb": "grid"}).get("grid", 0.0))

    @grid.setter
    def grid(self, beats: float) -> None:
        self._editor({"verb": "sync", "grid": float(beats)})

    @property
    def snap_to_grid(self) -> bool:
        """Whether a box the hand moves snaps to the grid."""
        return bool(self._editor({"verb": "grid"}).get("snap"))

    @snap_to_grid.setter
    def snap_to_grid(self, on: bool) -> None:
        self._editor({"verb": "sync", "snap": bool(on)})

    @property
    def notes_view(self) -> str:
        """How a box of notes is drawn: ``"roll"`` or ``"score"``."""
        return str(self._editor({"verb": "notesView"}).get("view"))

    @notes_view.setter
    def notes_view(self, view: str) -> None:
        if not self._editor({"verb": "notesView", "view": str(view)}).get("set"):
            raise ValueError(f"no notes view {view!r}: a box of notes is drawn "
                             "as a 'roll' or a 'score'")

    # ---- the transport, and what is heard ----

    @property
    def transport(self) -> MultitrackTransport:
        """**The multitrack's transport** (`MultitrackTransport`)."""
        return MultitrackTransport(self)

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
        then writes over. An untitled one needs a ``path`` the first time.
        Every file inside the session's folder is written relative to it.
        Answers the path written."""
        args = {} if path is None else {"path": str(path)}
        answer = self._do("save", **args) or {}
        written = answer.get("destination", path if path is not None else self._path)
        if written is not None:
            self._path = str(written)
        return written


def is_multitrack(structure) -> bool:
    """Whether ``structure`` is a `clausters.multitrack.Multitrack`."""
    from ...multitrack import Multitrack

    return isinstance(structure, Multitrack)
