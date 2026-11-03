"""The client's walk of the crate's steps (``clausters._steps``): what it
sends, what it waits on, and the steps that work on files, which are the
GUI host's application's and are refused here."""

from clausters import _native
from clausters._steps import run_steps
from clausters.base._oscinterface import OscNrtInterface
from clausters.defs import SynthDef, out, sine


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


def test_a_step_that_works_on_files_is_refused_and_does_nothing(tmp_path):
    """**Nothing hands a client a file step**: a render, a store and a move
    are the application's, which holds every editor that saves or renders.
    One that arrives is refused, naming it, before what is sent with it goes
    out, and the disk is left as it was."""
    import pytest

    from clausters.errors import CommandError

    kept = tmp_path / "kept.wav"
    kept.write_bytes(b"RIFF")
    file_steps = {
        "store": {"path": str(tmp_path / "session.json"), "contents": "{}"},
        "render": {"score": _score().hex(), "path": str(tmp_path / "tone.w64"),
                   "rate": 48_000.0, "channels": 1, "format": "float"},
        "move": {"from": str(kept), "to": str(tmp_path / "moved.wav")},
    }
    for kind, step in file_steps.items():
        server = _Server()
        runner = _native.StepRunner()
        with pytest.raises(CommandError, match=f"a {kind} step"):
            run_steps(server, runner, [
                {"send": {"addr": "/before", "args": []}},
                {kind: step},
                {"send": {"addr": "/after", "args": []}},
            ])
        assert server.sent == [], kind
        assert runner.call("idle") == {"idle": True}
    assert sorted(p.name for p in tmp_path.iterdir()) == ["kept.wav"]


def test_a_render_is_followed_as_it_goes(tmp_path):
    from clausters.render import render_to_file

    seen = []
    render_to_file(_score(), str(tmp_path / "tone.wav"), 48_000.0, 1, 0, None, "float",
                   progress=lambda done, total: seen.append((done, total)))
    assert seen and seen[-1] == (24_000, 24_000)


def test_a_render_with_no_path_keeps_no_output_and_does_its_work(tmp_path):
    from clausters.render import render_to_file
    from clausters.defs import disk_out

    written = tmp_path / "tone.wav"
    nrt = OscNrtInterface()
    nrt.send_msg(None, "/def_send", "synth",
                 SynthDef("recorded", disk_out(sine(660), path=str(written),
                                               format="float")).dump_def())
    nrt.send_msg(None, "/synth_new", "recorded", 1000, 0, 0)
    nrt.send_bundle(None, 0.5, ("/node_free", 1000))
    # `--nrt`'s `-`: the render's work is what its synths write
    render_to_file(nrt.score.bytes(), "-", 48_000.0, 1, 0, None, "float")
    assert written.read_bytes()[:4] == b"RIFF"
    # what the DiskOut wrote, and no rendered file
    assert sorted(tmp_path.iterdir()) == [written]


class _Refusing(_Server):
    """A server that refuses every command a step waits on."""

    def request(self, addr, *args, expect=(), timeout=None):
        self.sent.append(addr)
        return "/fail", [addr, "refused"]


def test_a_failed_walk_leaves_nothing_for_the_next_on_its_runner(tmp_path):
    """**A walk given up on sends nothing more**: after a refused server wait
    and a refused file step, the next batch on the same runner goes out alone,
    and the runner is idle after it."""
    import pytest

    from clausters.errors import CommandError

    server = _Refusing()
    runner = _native.StepRunner()
    failing = [
        [{"send": {"addr": "/buffer_alloc", "args": []}},
         {"await": {"command": "/buffer_alloc", "index": None}}],
        [{"store": {"path": str(tmp_path / "missing" / "session.json"), "contents": "{}"}}],
        [{"render": {"score": _score().hex(), "path": str(tmp_path / "missing" / "x.w64"),
                     "rate": 48_000.0, "channels": 1, "format": "float"}}],
    ]
    for walk in failing:
        server.sent.clear()
        with pytest.raises(CommandError):
            run_steps(server, runner, walk + [{"send": {"addr": "/stale", "args": []}}])
        assert "/stale" not in server.sent
        run_steps(server, runner, [{"send": {"addr": "/fresh", "args": []}}])
        assert server.sent[-1] == "/fresh"
        assert "/stale" not in server.sent
        assert runner.call("idle") == {"idle": True}
