"""The free-standing ``render``: dispatch and the offline bounce paths.

Every offline path goes through a real embedded NRT render (skipped when the
ffi library is not built), so the assertions are on actual sample counts.
"""

import struct

import pytest

from clausters import Event, render
from clausters.seq.pattern import Pbind, Pseq, Pwhite
from clausters.seq.timeline import Timeline

SR = 48_000.0


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def test_render_bounces_a_bare_expression(tmp_path):
    _embed_or_skip()
    from clausters.defs import sine

    wav = tmp_path / "expr.wav"
    samples, frames = render(sine(440.0) * 0.2, dur=0.25, sample_rate=SR,
                             channels=1, path=wav)
    assert abs(frames - 0.25 * SR) <= 128
    assert len(samples) == frames
    assert max(abs(x) for x in samples) > 0.1, "the expression actually sounds"
    # The WAV is IEEE-float (format 3) carrying exactly the same frames.
    raw = wav.read_bytes()
    assert raw[:4] == b"RIFF" and raw[8:12] == b"WAVE"
    fmt, chans, rate = struct.unpack("<HHI", raw[20:28])
    assert (fmt, chans, rate) == (3, 1, int(SR))
    assert raw[-frames * 4:] == samples.tobytes()


def test_render_bounces_an_event_pattern():
    _embed_or_skip()
    # Three notes, dur 0.5 at tempo 1 (beats == seconds): the last release
    # lands at 1.0 + 0.5 * 0.8 = 1.4 s.
    samples, frames = render(
        Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5),
        sample_rate=SR)
    assert abs(frames - 1.4 * SR) <= 4096
    assert len(samples) == frames * 2


def test_render_needs_until_for_an_endless_pattern():
    _embed_or_skip()
    samples, frames = render(
        Pbind(instrument="default", freq=Pwhite(200.0, 800.0), dur=0.25),
        until=2.0, sample_rate=SR)
    # Drained at beat 2.0: the last event starts by then, nothing beyond its
    # release survives.
    assert 2.0 * SR <= frames <= 2.5 * SR


def test_render_bounces_a_timeline():
    _embed_or_skip()
    tl = Timeline()
    tl.add(0.0, Event(degree=0, dur=0.5))
    tl.add(1.0, Event(degree=4, dur=0.5))
    samples, frames = render(tl, sample_rate=SR)
    assert abs(frames - 1.4 * SR) <= 4096


def test_render_bounces_an_arrangement_element():
    _embed_or_skip()
    from clausters.form import Event as FormEvent

    samples, frames = render(FormEvent(Event(degree=0, dur=0.5)),
                             sample_rate=SR)
    assert frames > 0.3 * SR


def test_render_bounces_a_generator():
    _embed_or_skip()

    def gen():
        Event(degree=0, dur=0.5).play()
        yield 0.5
        Event(degree=4, dur=0.5).play()

    samples, frames = render(gen, sample_rate=SR)
    assert abs(frames - 0.9 * SR) <= 4096   # second note at 0.5 + release 0.4


def test_render_rejects_a_live_destination_for_a_pattern():
    with pytest.raises(ValueError, match="play"):
        render(Pbind(degree=0), destination=object())


def test_render_rejects_unrenderable():
    with pytest.raises(TypeError, match="render"):
        render(3.14)
