"""Buffer I/O wrappers over the server's ``/buffer_*`` commands: writing a buffer to
a sound file and reading it back (offline, NRT), and the synchronous shape/data
queries (``/buffer_query``, ``/buffer_getRange``) against an in-process embedded server.
"""

import os

import pytest

from clausters.base.timebase import LogicalTimebase
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
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())
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
    clock2 = TempoClock(tempo=1.0, timebase=LogicalTimebase())
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


def test_samples_this_program_holds_become_a_buffer():
    """`from_samples` is `alloc` + `set_samples` in one call -- the way back
    from a take that exists here rather than in a file, and the same call the
    web client makes, where it is the only way back at all."""
    _embed_or_skip()
    try:
        from clausters import Session
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    server = session.server
    stereo = [0.1, -0.1, 0.2, -0.2, 0.3, -0.3]
    buf = Buffer.from_samples(stereo, 2, 44100.0, server=server)

    # The shape follows from the layout: the sequence is interleaved, so three
    # frames of two channels rather than six of one.
    assert (buf.frames, buf.channels) == (3, 2)
    assert buf.sample_rate == 44100.0
    assert list(buf.get_samples(0, 6)) == pytest.approx(stereo, abs=1e-6)

    # With no rate given the buffer keeps the server's own, which is what a
    # take rendered at the session's rate wants.
    plain = Buffer.from_samples([1.0, 0.5], server=server)
    assert (plain.frames, plain.channels) == (2, 1)
    assert list(plain.get_samples(0, 2)) == pytest.approx([1.0, 0.5], abs=1e-6)

    buf.free()
    plain.free()


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


def test_a_join_plays_as_one_buffer_and_says_what_it_is_made_of():
    """A cut assembled from two takes is one buffer: `stitch` installs it,
    reading it crosses the seam, and `parts` says what it is made of -- which
    is the question to ask before offering an editable waveform over one, since
    a join refuses every write."""
    _embed_or_skip()
    try:
        from clausters import Session
        from clausters.defs import Part
        from clausters.errors import CommandError
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    server = session.server
    one = Buffer.from_samples([1.0, 2.0, 3.0, 4.0], server=server)
    two = Buffer.from_samples([5.0, 6.0, 7.0, 8.0], server=server)

    # The second take's tail, then the first take's head: one take out of order
    # is the same mechanism as two takes.
    join = Buffer.stitch([Part(two, 2, 2), Part(one, 0, 2)], server=server)
    assert (join.frames, join.channels) == (4, 1)
    assert list(join.get_samples(0, 4)) == pytest.approx([7.0, 8.0, 1.0, 2.0])

    parts = join.parts()
    assert [(p.source, p.start, p.frames) for p in parts] == [
        (two.bufnum, 2, 2), (one.bufnum, 0, 2)]
    assert parts[0].channels == [0], "every part spells its whole map"

    # A buffer that owns its samples answers with no parts, which is the answer
    # to "is this a join" rather than a refusal.
    assert one.parts() == []

    # And a join is read, never written: it is replaced instead.
    with pytest.raises(CommandError):
        join.set_samples([0.0])

    join.free()
    one.free()
    two.free()


def _swapped_session() -> dict:
    """A session whose one box is a join of two takes, the second take's tail
    first, beside a box over samples that were never written down."""
    def window(region, source):
        return {"id": region, "position": 0.0, "length": 1.0,
                "content": {"fill": "window", "window": {
                    "source": {"source": source, "lifetime": "session",
                               "generation": 0},
                    "start": 0.0, "duration": 1.0}}}

    def span(source, start, end):
        return {"source": {"source": source, "lifetime": "session",
                           "generation": 0, "range": {"start": start, "end": end}}}

    def take(name):
        return {"location": {"at": "file", "path": name}, "lifetime": "session",
                "channels": 1, "frames": 4}

    return {
        "format": 2,
        "multitrack": {"tracks": [{"id": 10, "name": "t", "lanes": [
            {"id": 11, "regions": [window(20, 3), window(21, 4)]}]}]},
        "sources": {
            "1": take("one.wav"),
            "2": take("two.wav"),
            "3": {"location": {"at": "segments",
                               "parts": [span(2, 2, 4), span(1, 0, 2)]},
                  "lifetime": "session", "channels": 1, "frames": 4},
            "4": {"location": {"at": "volatile"}, "lifetime": "temporary"},
        },
    }


def test_a_saved_session_loads_its_takes_and_then_its_join(tmp_path):
    """Reopening a session is reading it and loading it: every take its join
    reads is read from the file beside the session, the join is stitched once
    they are there, and it plays the spans the document states."""
    _embed_or_skip()
    try:
        from clausters import Session
        from clausters.errors import CommandError
        from clausters.multitrack import Session as Saved
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")

    server = session.server
    for name, samples in (("one.wav", [1.0, 2.0, 3.0, 4.0]),
                          ("two.wav", [5.0, 6.0, 7.0, 8.0])):
        written = Buffer.from_samples(samples, server=server)
        written.write(str(tmp_path / name), sample_format="float")
        written.free()

    # Saved beside its takes and opened again, so the load finds them against
    # the folder the session came from.
    saved = Saved.open(Saved.read(_swapped_session()).save(tmp_path / "piece.json"))
    assert saved.path == str(tmp_path / "piece.json")
    before = server.buffers.in_use
    with pytest.warns(UserWarning, match="volatile"):
        loaded = saved.load(server)

    assert sorted(loaded) == [1, 2, 3], "the takes only the join reads load too"
    assert server.buffers.in_use == before + 3, "what the load did not take is given back"
    assert (loaded[1].frames, loaded[1].channels) == (4, 1)
    assert loaded[1].path == str(tmp_path / "one.wav")

    join = loaded[3]
    assert list(join.get_samples(0, 4)) == pytest.approx([7.0, 8.0, 1.0, 2.0])
    assert [(p.source, p.start, p.frames) for p in join.parts()] == [
        (loaded[2].bufnum, 2, 2), (loaded[1].bufnum, 0, 2)]
    for buffer in loaded.values():
        buffer.free()

    # A file that is not there is the server's refusal of its read, and the
    # load leaves nothing of itself behind.
    (tmp_path / "two.wav").unlink()
    in_use = server.buffers.in_use
    with pytest.warns(UserWarning), pytest.raises(CommandError):
        saved.load(server)
    assert server.buffers.in_use == in_use
