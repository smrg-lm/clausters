"""`AudioEditor`: the handler of the audio editor the GUI host holds.

What is checked is this client's half: which call each member makes through
the application door (`GuiHost.open_app`, `GuiHost.call_app`), and how it
reads the answer. What the application does with a call is the host's and is
tested there (`clients/gui/src/host/audio_editor.rs`), and what each gesture
does to the take is the crate's.
"""

import pytest

from clausters.gui import edit
from clausters.gui.editing import AudioEditor
from clausters.segments import Segment
from clausters.soundfile import SoundFile

SR = 48_000.0


class FakeHost:
    """A host that answers the door from a table, and records the calls."""

    def __init__(self, answers=None):
        self.calls: list = []
        self.verbs: list = []
        self._open: set = set()
        self.answers = {
            "binding": {"rate": SR, "cursor": 4_800, "dirty": False,
                        "layers": ["waveform"]},
            "history": {"version": 3, "canUndo": True, "canRedo": False,
                        "undoLabel": "cut", "redoLabel": None},
            "transport": {"transport": 2, "span": [0.5, 1.0], "looping": False},
            **(answers or {}),
        }

    @property
    def loop(self):
        return None

    def open_app(self, app, options=None, timeout=5.0):
        self.calls.append(("open", app, dict(options or {})))
        self._open.add(7)
        return 7

    def call_app(self, window, call, timeout=None):
        self.calls.append(("call", int(window), call))
        if "member" in call:
            verb = call["member"]["verb"]
        elif "query" in call:
            verb = call["query"]
        else:
            verb = call["do"]["verb"]
        answer = self.answers.get(verb)
        if isinstance(answer, Exception):
            raise answer
        return answer(call) if callable(answer) else answer

    def verb(self, window, verb):
        self.verbs.append((int(window), verb))

    def _wait_while(self, pred, timeout=None):
        return not pred()

    def _set_closed_handler(self, window, func):
        self.closed_handler = (window, func)

    def did(self) -> list:
        """The calls after the open, as their `do`, `member` or `query` names."""
        out = []
        for kind, *rest in self.calls:
            if kind != "call":
                continue
            call = rest[1]
            key = next(iter(call))
            out.append((key, call[key] if key == "query" else call[key]["verb"]))
        return out


def a_file(path="/takes/take.wav", codec="wav") -> SoundFile:
    return SoundFile(path, 48_000, 1, SR, codec, "float")


def opened(host=None, **answers):
    host = host or FakeHost(answers)
    editor = edit(a_file(), host=host)
    assert isinstance(editor, AudioEditor)
    return editor, host


def test_edit_opens_the_hosts_application_on_the_file():
    editor, host = opened()
    assert host.calls[0] == ("open", "audio", {"title": "Audio", "path": "/takes/take.wav"})
    assert editor.window == 7 and not editor.closed
    # a second edit of the same file is the editor already open
    assert edit(a_file(), host=host) is editor
    assert len([c for c in host.calls if c[0] == "open"]) == 1


def test_a_title_and_layers_are_the_opens():
    host = FakeHost()
    edit(a_file("/takes/other.wav"), host=host, title="other", layers=["spectrogram"])
    assert host.calls[0][2]["title"] == "other"
    assert host.calls[-1][2] == {"do": {"verb": "verb", "call": {
        "verb": "layers", "version": 3, "stack": ["spectrogram"]}}}


def test_a_buffer_is_not_edited():
    class Buffer:
        bufnum = 3

        def set_samples(self, *args):
            pass

    with pytest.raises(TypeError, match="sound files"):
        edit(Buffer(), host=FakeHost())


def test_a_file_the_editor_does_not_read_in_place_is_refused():
    with pytest.raises(ValueError, match="WAV, RF64 or W64"):
        edit(a_file("/takes/take.mp3", "mp3"), host=FakeHost())


def test_what_the_editor_holds_is_read_from_the_application():
    editor, host = opened(
        file={"path": "/takes/take.wav", "frames": 24_000, "channels": 1, "rate": SR,
              "codec": "wav", "sample_format": "float"},
        segments={"parts": [{"path": "/takes/take.wav", "start": 0, "frames": 24_000}]})
    file = editor.file
    assert (file.path, file.frames, file.sample_rate) == ("/takes/take.wav", 24_000, SR)
    assert editor.parts == [{"path": "/takes/take.wav", "start": 0, "frames": 24_000}]
    assert editor.cursor == pytest.approx(0.1)
    assert editor.dirty is False
    assert editor.layers == ("waveform",)
    assert (editor.can_undo, editor.can_redo, editor.undo_label) == (True, False, "cut")
    assert ("member", "file") in host.did() and ("query", "history") in host.did()


def test_a_verb_that_needs_a_file_says_none_is_open():
    editor, _ = opened(file={"error": "no file is open"})
    with pytest.raises(ValueError, match="no file is open"):
        editor.file


def test_a_save_waits_for_the_application_and_answers_the_path():
    editor, host = opened(save=lambda call: {"destination": {"path": call["do"]["request"]["path"]}})
    assert editor.save("/takes/out.wav", sample_format="int16") == "/takes/out.wav"
    assert host.calls[-1][2] == {"do": {"verb": "save",
                                        "request": {"format": "int16", "path": "/takes/out.wav"}}}


def test_what_the_application_raises_is_raised():
    editor, _ = opened(save=RuntimeError("the window closed"))
    with pytest.raises(RuntimeError, match="window closed"):
        editor.save()


def test_the_transport_is_the_applications():
    editor, host = opened(playing=True)
    t = editor.transport
    t.play(1.5)
    t.locate(0.25)
    t.loop(0.5, 1.0)
    assert t.span == (0.5, 1.0) and t.looping is False and t.playing is True
    assert [c[2]["do"] for c in host.calls if c[0] == "call" and "do" in c[2]][:5] == [
        {"verb": "play", "at": 1.5},
        {"verb": "locate", "at": 0.25},
        {"verb": "span", "show": True, "span": [0.5, 1.0]},
        {"verb": "looping", "on": True},
        {"verb": "playing"},
    ]


def test_what_is_marked_is_the_span_over_the_file():
    editor, host = opened(
        file={"path": "/takes/take.wav", "frames": 48_000, "channels": 1, "rate": SR,
              "codec": "wav", "sample_format": "float"})
    marked = editor.selected
    assert marked.start == 24_000 and marked.duration == pytest.approx(0.5)
    assert marked.source.path == "/takes/take.wav"
    editor.select(Segment(marked.source, 4_800, 0.2))
    assert host.calls[-1][2] == {"do": {"verb": "span", "show": True, "span": [0.1, 0.30000000000000004]}}
    editor.unselect()
    assert host.calls[-1][2] == {"do": {"verb": "span", "show": True, "span": None}}


def test_an_undo_is_the_windows_event_against_the_version_now():
    editor, host = opened()
    assert editor.undo() is True
    assert host.calls[-1][2] == {"do": {"verb": "event", "addr": "/gui_event",
                                        "args": [7, 0, 3, "undo"]}}
    # nothing to redo asks nothing of the take
    before = len(host.calls)
    assert editor.redo() is False
    assert all("do" not in c[2] for c in host.calls[before:])


def test_the_windows_verbs_go_to_its_window():
    editor, host = opened()
    editor.cut()
    editor.reverse()
    assert host.verbs == [(7, "cut"), (7, "reverse")]


def test_a_channel_refused_is_raised():
    editor, _ = opened(verb={"reason": "the take has 1 channel"})
    with pytest.raises(ValueError, match="1 channel"):
        editor.fold_channel(3)


def test_the_envelope_is_one_action():
    editor, host = opened(act={"changed": True})
    assert editor.set_envelope([0, 1, 1, 0]) is True
    assert host.calls[-1][2] == {"do": {"verb": "act", "call": {
        "verb": "setautomation", "points": [0.0, 1.0, 1.0, 0.0]}}}


def test_closing_asks_the_application_and_the_window_goes_when_it_says():
    editor, host = opened()
    editor.close()
    assert host.calls[-1][2] == {"do": {"verb": "close"}}
    assert not editor.closed, "the window goes when the host says so"
    host._open.discard(7)
    assert editor.closed
    before = len(host.calls)
    editor.close()
    assert len(host.calls) == before, "a closed editor asks nothing"


def test_an_editor_not_opened_says_so():
    editor = AudioEditor(a_file())
    assert editor.closed and editor.window is None
    with pytest.raises(RuntimeError, match="open"):
        editor.file
