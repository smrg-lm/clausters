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

from .hosted import HostedEditor, MultitrackTransport, _Block, _Held, _RemoteHandle, _RemoteHistory
from .marking import Marking

# **What a window left each multitrack as**, by the multitrack: its session,
# its file and its last save (a `/gui_app_open` snapshot), so that opening it
# again, from this handler or from a later `edit`, reads none of it from the
# file over changes the window left behind
_snapshots = weakref.WeakKeyDictionary()


class MultitrackEditor(Marking, HostedEditor):
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
        HostedEditor.__init__(self, title=str(title) if title is not None else default)
        self._local = None
        self._holders = _Held()
        self._snapshot = _snapshots.get(self._multitrack) if self._multitrack is not None else None
        if self._snapshot is not None:
            self._path = self._snapshot.get("path")

    _app = "multitrack"

    # ---- the window ----

    def _held(self) -> _Held:
        return self._holders

    def _options(self, beside) -> dict:
        """Opened again after its window went, it is opened over what that
        window left it as (the module's docstring)."""
        if self._snapshot is not None:
            snapshot = dict(self._snapshot)
            snapshot["session"] = dict(snapshot["session"], multitrack=self._multitrack.write())
            return {"snapshot": snapshot}
        if self._path is not None:
            return {"path": self._path}
        if self._multitrack is not None:
            return {"multitrack": self._multitrack.write()}
        return {}

    def _take(self) -> None:
        # the script's multitrack reads and writes the window's from now on
        if self._multitrack is not None:
            self._local = self._multitrack._mt
            self._multitrack._mt = _RemoteHandle(self)

    def _settle(self, left: "dict | None", last: bool) -> None:
        """Hand the script's multitrack back, once the window is done with
        it: what the window left it as, and the session it was opened over
        brought to the same. A window that let go hands its snapshot over as
        ``left``; one that closed, the multitrack, with the snapshot beside
        it."""
        window = int(self._window)
        if self._released:
            snapshot, left = left, self._host._closed_with(window)
        else:
            snapshot = self._host._closed_snapshot(window)
        if self._local is None:
            return
        from ... import _native
        from ...multitrack import Session

        if snapshot is not None:
            left = snapshot["session"].get("multitrack", left)
        local, self._local = self._local, None
        self._multitrack._mt = _native.MultitrackHandle(left) if left is not None else local
        if snapshot is None:
            return
        _snapshots[self._multitrack] = snapshot
        self._snapshot = snapshot
        self._path = snapshot.get("path")
        if self._session is not None:
            restored = Session.read(snapshot["session"])
            restored.multitrack = self._multitrack
            restored.path = self._path
            self._session.__dict__.update(restored.__dict__)

    @property
    def multitrack(self):
        """The `clausters.multitrack.Multitrack` it was opened over: the
        window's while it is open, ``None`` for one opened over nothing."""
        return self._multitrack

    @property
    def session(self):
        """The `clausters.multitrack.Session` it was opened over, or ``None``
        for an untitled one."""
        return self._session

    @property
    def path(self) -> "str | None":
        """The session file it opened or last saved, or ``None`` for an
        untitled one."""
        return self._path

    # ---- the door ----

    def _handle(self, call: dict, *, label: "str | None" = None):
        """One verb of the multitrack's handle, asked of the window's. An edit
        inside a ``with multitrack.history(...)`` block is applied now and
        recorded when the block ends."""
        if self.closed:
            if self._window is not None:
                self._leave()
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

    # ---- the grid ----

    def _editor(self, call: dict):
        """One verb of the window's editor (``grid``, ``sync``), and its
        answer."""
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

    # ---- the window's session ----

    def new_file(self) -> None:
        """**A new, untitled session in the window**, as its File > New puts
        one there. The window lets go of the session it was opened over:
        this multitrack holds what the window left it as, as a close leaves
        it, and the window goes on as the host's own."""
        self._do("new_file")

    def open_file(self, path) -> None:
        """**The session file at ``path`` in the window**, as its File > Open
        opens one, letting go of the session it was opened over as
        `new_file` does. A session another window of the host holds is not
        opened twice: that window comes forward and this raises."""
        self._do("open", session=str(path))

    def close_file(self) -> None:
        """**No session in the window**, as its File > Close session leaves
        it, letting go of the one it was opened over as `new_file` does."""
        self._do("close_file")


def is_multitrack(structure) -> bool:
    """Whether ``structure`` is a `clausters.multitrack.Multitrack`."""
    from ...multitrack import Multitrack

    return isinstance(structure, Multitrack)
