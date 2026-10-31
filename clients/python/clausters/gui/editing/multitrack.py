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
import weakref


_retained = weakref.WeakKeyDictionary()

from .marking import Marking


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


class _Object:
    """A retained session identity; a window binding never retargets it."""

    def __init__(self, editor, identity, multitrack):
        self._owner = editor
        self.identity = int(identity)
        self.multitrack = multitrack
        self.local = multitrack._mt
        self._depth = 0
        self._legs = None
        multitrack._mt = _RemoteHandle(self)

    @property
    def closed(self):
        return self.local is None or self._owner.closed

    def _settle(self):
        self._owner._settle()

    def _do(self, verb, **args):
        if self.closed:
            raise ValueError("the session view is closed")
        return self._owner._call({"do": {"verb": verb, "object": self.identity, **args}})

    def _query(self, name):
        if self.closed:
            raise ValueError("the session view is closed")
        return self._owner._call({"query": name, "object": self.identity}) or {}

    def _handle(self, call, *, label=None):
        if self.closed:
            self._settle()
            return self.multitrack._mt.call(**call)
        args = {"call": call}
        if label is not None:
            args["label"] = label
        block = self._legs is not None and call.get("verb") == "apply"
        if block:
            args["record"] = False
        answer = self._do("handle", **args)
        if block and isinstance(answer, dict) and answer.get("applied") and "current" in answer:
            self._legs.append({"forward": call["intent"], "backward": answer["current"]})
        return answer

    def _edit(self, intent, label):
        return self._handle({"verb": "apply", "intent": intent}, label=label)

    def _block(self, label):
        return _Block(self, label)

    @property
    def _history(self):
        return _RemoteHistory(self)


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


class MultitrackEditor(Marking):
    """**The multitrack the GUI host holds, and this script its handler.**

    `clausters.gui.edit` over a `clausters.multitrack.Session` that has a
    file, or over a `clausters.multitrack.Multitrack`, answers one, open.
    Each member is a call to the application in the host's process,
    answered by it.

    **What is marked** (`selected`, `select`, `unselect`) is the boxes the
    hand holds -- a click, Alt+click or a marquee -- as the
    `clausters.multitrack.Region` objects of the multitrack, in the order
    the boxes are drawn. The window's, not the multitrack's: it enters no
    history, and nothing is marked with no window open. (The time range a
    sweep leaves is the transport's.)

    Args:
        structure: ``None`` opens without a session. Otherwise, what to open: a `clausters.multitrack.Session`, by the
            file it was opened from or saved to; or a
            `clausters.multitrack.Multitrack`, as an untitled session whose
            first `save` names its file.
        title: the window's title; the session file's name when not given,
            ``"Untitled"`` for a multitrack.

    Raises:
        ValueError: a session with no file: save it first.
    """

    def __init__(self, structure=None, *, title: "str | None" = None):
        from ...multitrack import Multitrack

        if structure is None:
            self._session = None
            self._multitrack = None
            self._path = None
            default = "Multitrack"
        elif isinstance(structure, Multitrack):
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
        self._objects = {}
        self._active = None
        self._snapshot = _retained.get(self._multitrack) if self._multitrack is not None else None
        if self._snapshot is not None:
            self._path = self._snapshot.get("path")
        self._settled = False
        self._on_closed = None

    # ---- the window ----

    def open(self, host=None):
        """**Ask the host to open the multitrack**, on ``host`` or the ambient
        one, and answer its window. One already open answers the window it
        has. From then on the multitrack it was opened over is the window's
        (the module's docstring)."""
        if self._window is not None and not self.closed:
            return self._window
        from ..handle import WindowHandle
        from .application import _resolve_host

        if self._window is not None:
            self._settle()
        host = _resolve_host(host, None)
        if self._snapshot is not None:
            snapshot = dict(self._snapshot)
            snapshot["session"] = dict(snapshot["session"], multitrack=self._multitrack.write())
            options = {"title": self.title, "snapshot": snapshot}
        elif self._path is not None:
            options = {"title": self.title, "path": self._path}
        elif self._multitrack is not None:
            options = {"title": self.title, "multitrack": self._multitrack.write()}
        else:
            options = {"title": self.title}
        window = host.open_app("multitrack", options)
        self._objects = {}
        self._active = None
        self._settled = False
        self._host = host
        self._window = WindowHandle(host, int(window), {})
        self._window.on_closed(self._closed)
        # Capture the original object before a later binding can replace it.
        self._refresh_binding(seed=self._multitrack)
        # the host's loop delivers the window's close from now on
        host.loop
        return self._window

    def _closed(self) -> None:
        """The window went: the multitrack holds what it left it as."""
        self._settle()
        if self._on_closed is not None:
            self._on_closed()

    def _refresh_binding(self, seed=None):
        if self.closed:
            return
        from ...multitrack import Multitrack

        file = self._query("file")
        self._path = file.get("path")
        if file.get("empty"):
            self._active = None
            self._multitrack = None
            return
        identity = int(file["object"])
        if identity not in self._objects:
            multitrack = seed
            if multitrack is None:
                written = self._do("handle", call={"verb": "state"})
                multitrack = Multitrack.read(written)
            self._objects[identity] = _Object(self, identity, multitrack)
        self._active = identity
        self._multitrack = self._objects[identity].multitrack

    def _settle(self) -> None:
        """Return each retained client object to its own final snapshot."""
        if self._settled or self._window is None:
            return
        from ... import _native
        from ...multitrack import Multitrack, Session

        metadata = self._host._closed_objects(int(self._window))
        if metadata is None:
            left = self._host._closed_with(int(self._window))
            snapshots = {} if left is None else {str(self._active): {"session": {"multitrack": left}, "path":self._path, "saved":None}}
        else:
            snapshots = metadata.get("objects", {})
            self._active = metadata.get("active")
        for identity, written in snapshots.items():
            identity = int(identity)
            if identity not in self._objects:
                self._objects[identity] = _Object(self, identity, Multitrack.read(written["session"].get("multitrack", {})))
        for identity, held in self._objects.items():
            if held.local is None:
                continue
            written = snapshots.get(str(identity))
            held.multitrack._mt = _native.MultitrackHandle(written["session"].get("multitrack", {})) if written is not None else held.local
            held.local = None
            if written is not None:
                _retained[held.multitrack] = written
            if written is not None and self._session is not None and self._session.multitrack is held.multitrack:
                restored = Session.read(written["session"])
                restored.multitrack = held.multitrack
                restored.path = written.get("path")
                self._session.__dict__.update(restored.__dict__)
        self._multitrack = None if self._active is None else self._objects[self._active].multitrack
        self._snapshot = snapshots.get(str(self._active))
        self._path = None if self._snapshot is None else self._snapshot.get("path")
        self._settled = True

    @property
    def multitrack(self):
        """The active `clausters.multitrack.Multitrack`, or ``None`` with no
        session. Earlier client objects keep their own contents and history."""
        self._refresh_binding()
        return self._multitrack

    @property
    def session(self):
        """The `clausters.multitrack.Session` it was opened over, or ``None``
        for an untitled one."""
        self._refresh_binding()
        return self._session if self._session is not None and self._session.multitrack is self._multitrack else None

    @property
    def path(self) -> "str | None":
        """The session file it opened, or ``None`` for an untitled one."""
        self._refresh_binding()
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

    def _object(self):
        self._refresh_binding()
        if self._active is None:
            raise ValueError("no file is open")
        return self._objects[self._active]

    def _handle(self, call: dict, *, label: "str | None" = None):
        return self._object()._handle(call, label=label)

    def _edit(self, intent: dict, label: str) -> dict:
        return self._object()._edit(intent, label)

    def _block(self, label: str) -> _Block:
        return self._object()._block(label)

    @property
    def _history(self) -> _RemoteHistory:
        return self._object()._history

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
        """Ask the window to undo (`verb`); whether the verb was sent.

        The result does not confirm completion. A script waiting for the
        history step uses `multitrack.history.undo()`.
        """
        return self.verb("undo")

    def redo(self) -> bool:
        """Ask the window to redo (`verb`); whether the verb was sent.

        The result does not confirm completion. A script waiting for the
        history step uses `multitrack.history.redo()`.
        """
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

    def view_wave(self) -> bool:
        """Draw every box of samples as its take's waveform, as the audio
        editor draws a take: the window's ``view_wave`` (`verb`)."""
        return self.verb("view_wave")

    def view_spectrogram(self) -> bool:
        """Draw every box of samples as its take's spectrogram, as the audio
        editor draws a take: the window's ``view_spectrogram`` (`verb`)."""
        return self.verb("view_spectrogram")

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

    # ---- what the hand holds ----

    def _marked(self) -> list:
        """The regions of the boxes the window holds."""
        if self.closed:
            return []
        from ...multitrack import Region

        multitrack = self.multitrack
        if multitrack is None:
            return []
        held = self._query("selected").get("selected") or []
        return [multitrack._view(Region, int(region)) for region in held]

    def _mark(self, regions) -> None:
        """Hold the boxes of ``regions`` in the window."""
        if self.closed:
            return
        self._do("select", regions=[int(region._id) for region in regions or ()])

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
