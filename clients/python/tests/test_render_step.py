"""A render step carried out: the crate's runner holds the queue on it with
nothing sent, the client renders the score offline into its file, and what
was behind it goes out after (``clausters._steps``)."""

from clausters import _native
from clausters._steps import run_steps
from clausters.base._oscinterface import OscNrtInterface
from clausters.defs import SynthDef, out, sine
from clausters.soundfile import SoundFile


class _Server:
    """A server that keeps what it is sent, and is asked nothing."""

    def __init__(self):
        self.sent = []

    def send_msg(self, addr, *args):
        self.sent.append(addr)


def _score() -> bytes:
    nrt = OscNrtInterface()
    nrt.send_msg(None, "/def_send", "synth", SynthDef("tone", out(0, sine(660) * 0.5)).dump_def())
    nrt.send_msg(None, "/synth_new", "tone", 1000, 0, 0)
    nrt.send_bundle(None, 0.5, ("/node_free", 1000))
    return nrt.score.bytes()


def test_a_render_step_writes_its_file_and_releases_what_follows(tmp_path):
    path = tmp_path / "tone.w64"
    server = _Server()
    steps = [
        {"send": {"addr": "/before", "args": []}},
        {"render": {"score": _score().hex(), "path": str(path), "rate": 48_000.0,
                    "channels": 1, "format": "float"}},
        {"send": {"addr": "/after", "args": []}},
    ]
    run_steps(server, _native.StepRunner(), steps)
    assert server.sent == ["/before", "/after"]
    written = SoundFile.open(path)
    assert (written.frames, written.channels) == (24_000, 1)


def test_a_render_is_followed_as_it_goes(tmp_path):
    from clausters._steps import render_step

    seen = []
    render_step({"score": _score().hex(), "path": str(tmp_path / "tone.wav"), "rate": 48_000.0,
                 "channels": 1, "format": "float"},
                progress=lambda done, total: seen.append((done, total)))
    assert seen and seen[-1] == (24_000, 24_000)


def test_a_render_with_no_path_keeps_no_output_and_does_its_work(tmp_path):
    from clausters._steps import render_step
    from clausters.defs import disk_out

    written = tmp_path / "tone.wav"
    nrt = OscNrtInterface()
    nrt.send_msg(None, "/def_send", "synth",
                 SynthDef("recorded", disk_out(sine(660), path=str(written),
                                               format="float")).dump_def())
    nrt.send_msg(None, "/synth_new", "recorded", 1000, 0, 0)
    nrt.send_bundle(None, 0.5, ("/node_free", 1000))
    render_step({"score": nrt.score.bytes().hex(), "path": "", "rate": 48_000.0,
                 "channels": 1, "format": "float"})
    assert written.read_bytes()[:4] == b"RIFF"
    # what the DiskOut wrote, and no rendered file
    assert sorted(tmp_path.iterdir()) == [written]


def test_renders_called_off_are_not_begun(tmp_path):
    """**A batch called off renders nothing more**: the window's work-in-
    progress panel calls a long reverse off, and the renders after the call
    are not begun; the runner hears it, and so does the caller."""
    import threading

    import pytest

    from clausters.errors import CommandError

    first, second = tmp_path / "piece-0.w64", tmp_path / "piece-1.w64"
    called_off = threading.Event()
    steps = [
        {"render": {"score": _score().hex(), "path": str(path), "rate": 48_000.0,
                    "channels": 1, "format": "float"}}
        for path in (first, second)
    ]

    def progress(render, done, total):
        # the call comes while the first piece renders
        if done is not None:
            called_off.set()

    with pytest.raises(CommandError, match="called off"):
        run_steps(_Server(), _native.StepRunner(), steps, progress=progress,
                  called_off=called_off)
    assert first.exists(), "the piece under way is finished"
    assert not second.exists(), "the next one is not begun"
