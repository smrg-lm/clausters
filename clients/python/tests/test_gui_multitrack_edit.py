"""`MultitrackEditor`: the handler of the multitrack the GUI host holds, over
a saved session or over a multitrack handed to it whole.

What is checked is this client's half: which call each member makes through
the application door (`GuiHost.open_app`, `GuiHost.call_app`), how it reads
the answer, and the script's multitrack reading and writing the window's while
it is open. What the application does with a call is the host's and is tested
there (`clients/gui/src/host/audio_editor.rs`).
"""

import time

import pytest

from clausters import _native
from clausters.gui import edit
from clausters.gui.editing import MultitrackEditor
from clausters.multitrack import Multitrack, Region
from clausters.multitrack import Session as SavedSession


class FakeHost:
    """A host that answers the door from a table, and records the calls. Its
    window holds a multitrack of its own, which a ``handle`` call reads and
    edits as the application's does."""

    def __init__(self, answers=None):
        self.calls: list = []
        self.verbs: list = []
        self._open: set = set()
        self.held = None
        self.path = None
        self.left = None
        self.snapshot = None
        self.recorded: list = []
        self.closed_handler = None
        self.released_handler = None
        self.answers = {
            "playing": True,
            "save": {"destination": "/songs/song.json"},
            "output": {"tracks": {"1": [0.5]}, "starved": 0},
            "history": {"canUndo": True, "canRedo": False, "undoLabel": "rename",
                        "redoLabel": None},
            **(answers or {}),
        }

    @property
    def loop(self):
        return None

    def open_app(self, app, options=None, timeout=5.0):
        options = dict(options or {})
        self.calls.append(("open", app, options))
        if "snapshot" in options:
            snapshot = options["snapshot"]
            self.bind(snapshot["session"].get("multitrack", {}), snapshot.get("path"))
        elif "path" in options or "multitrack" in options:
            self.bind(options.get("multitrack", {}), options.get("path"))
        else:
            self.bind(None)
        self._open.add(9)
        return 9

    def call_app(self, window, call, timeout=None):
        self.calls.append(("call", int(window), call))
        if "query" in call:
            return self.answers.get(call["query"])
        do = call["do"]
        if do["verb"] == "handle":
            asked = dict(do["call"])
            if asked["verb"] == "record":
                self.recorded.append((do.get("label"), asked["legs"]))
                return True
            answer = self.held.call(**asked)
            if asked["verb"] == "apply" and do.get("record", True):
                self.recorded.append((do.get("label"), [asked["intent"]]))
            return answer
        if do["verb"] == "close":
            self.close_window()
            return False
        if do["verb"] in ("new_file", "close_file") or (
                do["verb"] == "open" and "session" in do):
            # the window lets go of its session, as the host says it
            path = do.get("session")
            self.release(None if do["verb"] == "close_file" else Multitrack().write(), path)
            return None
        return self.answers.get(do["verb"])

    def bind(self, written, path=None):
        """The session the window shows, as the application holds it."""
        self.path = path
        self.held = None if written is None else _native.MultitrackHandle(written)

    def snapshot_of(self):
        """What the window hands back of its session: a ``/gui_app_open``
        snapshot."""
        if self.held is None:
            return None
        return {"session": {"format": SavedSession.format,
                            "multitrack": self.held.call("state"), "sources": {}},
                "path": self.path, "saved": None}

    def close_window(self):
        """The window goes, as the host says it: what it left the multitrack
        as rides with the close."""
        self.left = None if self.held is None else self.held.call("state")
        self.snapshot = self.snapshot_of()
        self._open.discard(9)
        if self.closed_handler is not None:
            self.closed_handler[1]()

    def release(self, written=None, path=None):
        """The hand put another session in the window (``/gui_app_released``):
        its opener is handed the first, and the window goes on."""
        self.snapshot = self.snapshot_of()
        self.bind(written, path)
        if self.released_handler is not None:
            self.released_handler[1]()

    def _closed_snapshot(self, window):
        snapshot, self.snapshot = self.snapshot, None
        return snapshot

    def _closed_with(self, window):
        left, self.left = self.left, None
        return left

    def verb(self, window, verb):
        self.verbs.append((int(window), verb))

    def _wait_while(self, pred, timeout=None):
        return not pred()

    def _set_changed_handler(self, window, func):
        self.changed_handler = (window, func)

    def _set_closed_handler(self, window, func):
        self.closed_handler = (window, func)

    def _set_released_handler(self, window, func):
        self.released_handler = (window, func)


def a_session(path="/songs/song.json") -> SavedSession:
    session = SavedSession()
    session.path = path
    return session


def test_edit_opens_a_saved_session_in_the_multitrack_application():
    host = FakeHost()
    editor = edit(a_session(), host=host)
    assert isinstance(editor, MultitrackEditor)
    assert host.calls[0] == ("open", "multitrack",
                             {"title": "song.json", "path": "/songs/song.json"})
    assert editor.window is not None and not editor.closed
    assert edit(a_session(), host=host) is editor, "a second edit finds it"


def test_a_session_with_no_file_is_refused():
    with pytest.raises(ValueError, match="save it first"):
        MultitrackEditor(SavedSession())


def test_a_multitrack_opens_as_an_untitled_session():
    host = FakeHost()
    multitrack = Multitrack()
    multitrack.tracks.add("drums")
    editor = edit(multitrack, host=host)
    assert isinstance(editor, MultitrackEditor)
    _, app, options = host.calls[0]
    assert app == "multitrack" and options["title"] == "Untitled"
    assert options["multitrack"]["tracks"][0]["name"] == "drums"
    assert editor.path is None and editor.session is None
    assert edit(multitrack, host=host) is editor, "a second edit finds it"


def test_the_scripts_multitrack_is_the_windows_while_it_is_open():
    host = FakeHost()
    multitrack = Multitrack()
    track = multitrack.tracks.add("a")
    editor = edit(multitrack, host=host)
    # a read answers what the window holds
    host.held.call("apply", intent={"intent": "settracks", "tracks": [
        dict(multitrack.write()["tracks"][0], name="by hand")]})
    assert track.name == "by hand"
    # a change is an edit of the window's, recorded under its label
    track.name = "b"
    assert host.held.call("track", id=track._id)["name"] == "b"
    assert host.recorded[-1][0] is not None
    # a block is one entry, recorded when it ends
    with multitrack.history("tidy"):
        track.muted = True
        track.name = "c"
    label, legs = host.recorded[-1]
    assert label == "tidy" and len(legs) == 2
    assert multitrack.history.undo_label == "rename"
    assert multitrack.history.can_undo
    # the window goes, and the multitrack holds what it left it as
    host.held.call("apply", intent={"intent": "settracks", "tracks": [
        dict(host.held.call("state")["tracks"][0], name="last")]})
    host.close_window()
    assert editor.closed
    assert track.name == "last"
    track.name = "mine again"
    assert track.name == "mine again", "and it is the script's again"


def test_the_transport_and_the_save_are_calls_to_the_application():
    host = FakeHost()
    editor = MultitrackEditor(a_session("/songs/other.json"), title="mix")
    editor.open(host)
    editor.transport.play(1.0, 2.0, looping=True)
    assert host.calls[-1][2] == {"do": {"verb": "play", "looping": True,
                                        "range": [1.0, 2.0]}}
    editor.transport.stop(0.5)
    assert host.calls[-1][2] == {"do": {"verb": "stop", "mark": 0.5}}
    editor.transport.locate(3.0)
    assert host.calls[-1][2] == {"do": {"verb": "locate", "at": 3.0}}
    assert editor.transport.playing
    assert editor.output["starved"] == 0
    assert editor.save("/songs/song.json") == "/songs/song.json"
    assert host.calls[-1][2] == {"do": {"verb": "save", "path": "/songs/song.json"}}
    # an undo is a step of the window's history, carried to its end
    host.answers["undo"] = True
    assert editor.undo() and host.calls[-1][2] == {"do": {"verb": "undo"}}
    assert editor.split() and host.verbs[-1] == (9, "split")


def test_the_grid_is_the_windows():
    host = FakeHost({"verb": {"grid": 0.5, "snap": True}})
    editor = edit(a_session(), host=host)
    assert editor.grid == 0.5 and editor.snap_to_grid
    editor.grid = 0.25
    assert host.calls[-1][2] == {"do": {"verb": "verb", "call": {"verb": "sync", "grid": 0.25}}}
    editor.snap_to_grid = False
    assert host.calls[-1][2] == {"do": {"verb": "verb", "call": {"verb": "sync", "snap": False}}}


def test_what_the_hand_holds_is_the_windows():
    host = FakeHost({"selected": {"selected": []}})
    multitrack = Multitrack()
    editor = edit(multitrack, host=host)
    assert editor.selected == []
    host.answers["selected"] = {"selected": [7]}
    region = editor.selected[0]
    assert isinstance(region, Region) and region._id == 7
    assert editor.selected == [region]
    assert host.calls[-1][2] == {"query": "selected"}
    editor.select([region])
    assert host.calls[-1][2] == {"do": {"verb": "select", "regions": [region._id]}}
    editor.unselect()
    assert host.calls[-1][2] == {"do": {"verb": "select", "regions": []}}
    host.close_window()
    assert editor.selected == [], "nothing is marked with no window open"


def test_a_window_that_lets_go_of_the_session_hands_the_multitrack_back():
    host = FakeHost()
    original = Multitrack()
    track = original.tracks.add("original")
    editor = edit(original, host=host)
    track.name = "edited in the window"
    replacement = Multitrack()
    replacement.tracks.add("replacement")
    host.release(replacement.write(), "/songs/next.json")
    assert editor.closed, "the handler is done with the window"
    assert editor.multitrack is original
    assert track.name == "edited in the window", "what the window left it as"
    asked = len(host.calls)
    track.name = "local again"
    assert len(host.calls) == asked, "the multitrack is the script's again"
    assert host.held.call("state")["tracks"][0]["name"] == "replacement"


def test_an_editor_can_open_without_a_session():
    host = FakeHost()
    editor = MultitrackEditor()
    editor.open(host)
    assert host.calls[0] == ("open", "multitrack", {"title": "Multitrack"})
    assert editor.multitrack is None
    assert editor.path is None
    host.close_window()
    assert editor.closed and editor.multitrack is None


def test_reopening_keeps_the_same_object_and_its_local_changes():
    host = FakeHost()
    original = Multitrack()
    track = original.tracks.add("original")
    editor = edit(original, host=host)
    host.close_window()
    track.name = "after close"
    editor.open(host)
    assert editor.multitrack is original
    assert track.name == "after close"
    assert "snapshot" in next(call for call in reversed(host.calls) if call[0] == "open")[2]
    host.close_window()
    assert track.name == "after close"


def test_the_windows_file_verbs_hand_the_session_back():
    host = FakeHost()
    original = Multitrack()
    original.tracks.add("original")
    editor = edit(original, host=host)
    editor.new_file()
    assert host.calls[-1][2] == {"do": {"verb": "new_file"}}
    assert editor.closed and editor.multitrack is original
    editor.open(host)
    editor.open_file("/songs/next.json")
    assert host.calls[-1][2] == {"do": {"verb": "open", "session": "/songs/next.json"}}
    assert editor.closed
    editor.open(host)
    editor.close_file()
    assert host.calls[-1][2] == {"do": {"verb": "close_file"}}
    assert editor.closed and host.held is None


def test_a_window_a_session_left_goes_on_answering_its_own_verbs(gui_host, tmp_path):
    """A window with no session, on a real host: it opens one from a file, Close
    file hands that back and leaves the window empty, and its own close closes
    it."""
    path = str(tmp_path / "kept.json")
    SavedSession().save(path)
    editor = MultitrackEditor(None, title="empty")
    editor.open(gui_host)
    assert gui_host.call_app(editor.window, {"query": "file"})["empty"] is True
    editor.open_file(path)
    assert gui_host.call_app(editor.window, {"query": "file"})["path"] == path
    window = int(editor.window)
    editor.close_file()
    gui_host.pump(0.2)
    assert editor.closed, "the handler is done with the window"
    assert gui_host.call_app(window, {"query": "file"})["empty"] is True
    gui_host.verb(window, "close")
    deadline = time.monotonic() + 5.0
    while window in gui_host.windows and time.monotonic() < deadline:
        gui_host.pump(0.05)
    assert window not in gui_host.windows, "a window with no file still closes"


@pytest.mark.parametrize("app", ["audio", "multitrack", "score"])
def test_every_application_with_no_file_answers_its_window_verbs(gui_host, app):
    """**What a window with no file answers is the same in every application**:
    its close, whichever door it comes through, closes it."""
    window = gui_host.open_app(app, {"title": "nothing", "empty": True})
    assert gui_host.call_app(window, {"query": "history"})["canUndo"] is False
    gui_host.verb(window, "close")
    deadline = time.monotonic() + 5.0
    while window in gui_host.windows and time.monotonic() < deadline:
        gui_host.pump(0.05)
    assert window not in gui_host.windows
