"""Buffer I/O wrappers over the server's ``/buffer_*`` commands: writing a buffer to
a sound file and reading it back (offline, NRT), and the synchronous shape/data
queries (``/buffer_query``, ``/buffer_getRange``) against an in-process embedded server.
"""

import os

import pytest

from clausters import render
from clausters.base import OscNrtInterface, TempoClock
from clausters.base.stream import Routine
from clausters.defs import Buffer, Server, Synth
from clausters.defs.synthdef import SynthDef
from clausters.defs.ugens import control, out, play_buf


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def test_write_then_read_buffer_round_trips(tmp_path):
    _embed_or_skip()
    wav = str(tmp_path / "buf.wav")

    # Generate a normalized sine period, write it to a WAV (both scored at 0).
    s = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)
    buf = Buffer.alloc(1024, 1, server=s)
    buf.gen("sine1", 7, 1.0)
    buf.write(wav, sample_format="float")

    def close():
        yield 0.1
        s.send_bundle(("/node_free", 0))

    clock.play(Routine(close))
    clock.render()
    try:
        render(s.interface.score.bytes())
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert os.path.getsize(wav) > 0

    # Read it back in a fresh score and play it: the readback is audible.
    s2 = Server(interface=OscNrtInterface())
    clock2 = TempoClock(tempo=1.0)
    b2 = Buffer.read(wav, server=s2)
    SynthDef("play",
             out(0.0, play_buf(control("buf", 0.0, "ir"), 0.0, 1.0, 1.0))).send(s2)

    def go():
        Synth("play", {"buf": b2.bufnum}, server=s2)
        yield 0.5
        s2.send_bundle(("/node_free", 0))

    clock2.play(Routine(go))
    clock2.render()
    _st0 = render(s2.interface.score.bytes())
    samples, frames = _st0.samples, _st0.frames
    peak = max(abs(x) for x in samples[: frames * 2])
    assert peak == pytest.approx(1.0, abs=0.05)


def test_query_and_get_samples_via_embed():
    _embed_or_skip()
    try:
        from clausters import Session
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    server = session.server
    buf = Buffer.alloc(8, 1, server=server)
    assert buf.server is server
    # A linear 0 -> 1 ramp across the 8 samples.
    buf.gen("env", 0.0, 1.0, 1.0, 1, 0.0)

    info = buf.info()
    assert (info.frames, info.channels) == (8, 1) and info.exists
    assert (buf.frames, buf.channels) == (8, 1)   # the handle keeps the record

    vals = list(buf.get_samples(0, 8))
    assert vals[0] == pytest.approx(0.0, abs=1e-6)
    assert vals[-1] == pytest.approx(1.0, abs=1e-6)
    assert vals[3] == pytest.approx(3 / 7, abs=1e-6)


def test_zero_and_free_go_through_the_buffer():
    _embed_or_skip()
    try:
        from clausters import Session
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    server = session.server
    buf = Buffer.alloc(8, 1, server=server)
    buf.gen("env", 0.0, 1.0, 1.0, 1, 0.0)
    buf.zero()
    assert max(abs(v) for v in buf.get_samples(0, 8)) == 0.0
    buf.free()
    assert server.buffers.in_use == 0


def test_written_samples_read_back():
    """The read -> edit -> write cycle: what `set_samples` writes is what
    `get_samples` reads, and a scattered touch-up lands too."""
    _embed_or_skip()
    try:
        from clausters import Session
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    server = session.server
    buf = Buffer.alloc(8, 1, server=server)

    buf.set_samples([0.1, 0.2, 0.3, 0.4], start=2)
    buf.set_sample(0, -0.5)
    assert list(buf.get_samples(0, 8)) == pytest.approx(
        [-0.5, 0.0, 0.1, 0.2, 0.3, 0.4, 0.0, 0.0], abs=1e-6)

    # Read, edit, write back: the round trip an editor view makes.
    edited = [v * 2 for v in buf.get_samples(0, 8)]
    buf.set_samples(edited)
    assert list(buf.get_samples(0, 8)) == pytest.approx(
        [-1.0, 0.0, 0.2, 0.4, 0.6, 0.8, 0.0, 0.0], abs=1e-6)

    # Chunking is transparent: several round trips, one result.
    buf.set_samples([1.0] * 8, chunk=3)
    assert list(buf.get_samples(0, 8)) == pytest.approx([1.0] * 8, abs=1e-6)

    buf.free()


def test_a_write_past_the_end_is_refused():
    """Unlike a read, which clamps: a short write would lose samples the
    caller believes it stored."""
    _embed_or_skip()
    try:
        from clausters import Session
        from clausters.errors import CommandError
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    buf = Buffer.alloc(4, 1, server=session.server)
    with pytest.raises(CommandError):
        buf.set_samples([1.0, 1.0, 1.0], start=2)
    # And the refusal left the buffer alone.
    assert max(abs(v) for v in buf.get_samples(0, 4)) == 0.0
    buf.free()
