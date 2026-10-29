"""`MultitrackEditor`: the handler of the multitrack the GUI host holds, over
a saved session or over a multitrack handed to it whole.

What is checked is this client's half: which call each member makes through
the application door (`GuiHost.open_app`, `GuiHost.call_app`), how it reads
the answer, and the script's multitrack reading and writing the window's while
it is open. What the application does with a call is the host's and is tested
there (`clients/gui/src/host/audio_editor.rs`).
"""

import pytest

from clausters import _native
from clausters.gui import edit
from clausters.gui.editing import MultitrackEditor
from clausters.multitrack import Multitrack
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
        self.recorded: list = []
        self.closed_handler = None
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
        self.held = _native.MultitrackHandle(options.get("multitrack", {}))
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
        return self.answers.get(do["verb"])

    def close_window(self):
        """The window goes, as the host says it: what it left the multitrack
        as rides with the close."""
        self.left = self.held.call("state")
        self._open.discard(9)
        if self.closed_handler is not None:
            self.closed_handler[1]()

    def _closed_with(self, window):
        return getattr(self, "left", None)

    def verb(self, window, verb):
        self.verbs.append((int(window), verb))

    def _wait_while(self, pred, timeout=None):
        return not pred()

    def _set_closed_handler(self, window, func):
        self.closed_handler = (window, func)


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
    assert editor.undo() and host.verbs[-1] == (9, "undo")
    assert editor.split() and host.verbs[-1] == (9, "split")


def test_the_grid_is_the_windows():
    host = FakeHost({"verb": {"grid": 0.5, "snap": True, "view": "roll", "set": True}})
    editor = edit(a_session(), host=host)
    assert editor.grid == 0.5 and editor.snap_to_grid
    editor.grid = 0.25
    assert host.calls[-1][2] == {"do": {"verb": "verb", "call": {"verb": "sync", "grid": 0.25}}}
    editor.snap_to_grid = False
    assert host.calls[-1][2] == {"do": {"verb": "verb", "call": {"verb": "sync", "snap": False}}}
    assert editor.notes_view == "roll"
    editor.notes_view = "score"
    assert host.calls[-1][2] == {"do": {"verb": "verb",
                                        "call": {"verb": "notesView", "view": "score"}}}
