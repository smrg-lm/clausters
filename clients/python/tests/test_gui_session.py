"""`SessionEditor`: the handler of the multitrack the GUI host holds, over a
saved session.

What is checked is this client's half: which call each member makes through
the application door (`GuiHost.open_app`, `GuiHost.call_app`), and how it
reads the answer. What the application does with a call is the host's and is
tested there (`clients/gui/src/host/audio_editor.rs`).
"""

import pytest

from clausters.gui import edit
from clausters.gui.editing import SessionEditor
from clausters.multitrack import Session as SavedSession


class FakeHost:
    """A host that answers the door from a table, and records the calls."""

    def __init__(self, answers=None):
        self.calls: list = []
        self.verbs: list = []
        self._open: set = set()
        self.answers = {
            "playing": True,
            "save": {"destination": "/songs/song.json"},
            "output": {"tracks": {"1": [0.5]}, "starved": 0},
            **(answers or {}),
        }

    @property
    def loop(self):
        return None

    def open_app(self, app, options=None, timeout=5.0):
        self.calls.append(("open", app, dict(options or {})))
        self._open.add(9)
        return 9

    def call_app(self, window, call, timeout=None):
        self.calls.append(("call", int(window), call))
        key = "query" if "query" in call else "do"
        verb = call[key] if key == "query" else call[key]["verb"]
        return self.answers.get(verb)

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
    assert isinstance(editor, SessionEditor)
    assert host.calls[0] == ("open", "multitrack",
                             {"title": "song.json", "path": "/songs/song.json"})
    assert editor.window is not None and not editor.closed
    assert edit(a_session(), host=host) is editor, "a second edit finds it"


def test_a_session_with_no_file_is_refused():
    with pytest.raises(ValueError, match="save it first"):
        SessionEditor(SavedSession())


def test_the_transport_and_the_save_are_calls_to_the_application():
    host = FakeHost()
    editor = SessionEditor(a_session("/songs/other.json"), title="mix")
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
