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

    from clausters.render import read_soundfile

    # In memory: the samples ride in the stats.
    kept = render(sine(440.0) * 0.2, dur=0.25, sample_rate=SR, channels=1)
    assert abs(kept.frames - 0.25 * SR) <= 128
    assert len(kept.samples) == kept.frames
    assert max(abs(x) for x in kept.samples) > 0.1, "the expression actually sounds"

    # With a path: the audio goes to the file instead, exactly as it does for
    # every other kind of render. Same seed, so it is the same take.
    wav = tmp_path / "expr.wav"
    filed = render(sine(440.0) * 0.2, dur=0.25, sample_rate=SR, channels=1,
                   path=wav, seed=kept.seed)
    assert filed.samples is None, "a path sends the audio to the file"
    assert filed.path == str(wav)
    assert filed.frames == kept.frames
    # A 32-bit float WAV, written by the server's writer like every other
    # render's file: hound tags it WAVE_FORMAT_EXTENSIBLE (0xFFFE, whose
    # subformat GUID opens with IEEE_FLOAT) rather than a bare format 3.
    raw = wav.read_bytes()
    assert raw[:4] == b"RIFF" and raw[8:12] == b"WAVE"
    fmt, chans, rate = struct.unpack("<HHI", raw[20:28])
    bits = struct.unpack("<H", raw[34:36])[0]
    assert fmt in (3, 0xFFFE)
    assert (chans, rate, bits) == (1, int(SR), 32)
    # And it holds the take the in-memory render produced, sample for sample.
    assert read_soundfile(wav).samples == kept.samples


def test_render_refuses_an_expression_wider_than_its_outputs():
    # `channels` is the render's own output count, not a property of the
    # graph, so it is never derived -- but an expression laid past it writes
    # onto internal buses that reach no file, and that is misuse, not a
    # truncation to absorb silently.
    _embed_or_skip()
    from clausters.defs import chans, out, sine

    with pytest.raises(ValueError, match="channels=4"):
        render(sine(440.0).dup(4), dur=0.05, sample_rate=SR, channels=2)
    # Explicit routing is the caller's own business: only the buses the
    # coercion itself assigned are checked.
    render(chans(out(8.0, sine(440.0))), dur=0.05, sample_rate=SR, channels=2)


def test_render_bounces_a_channel_list_on_its_own_channels():
    _embed_or_skip()
    from clausters.defs import chans, sine

    stats = render(chans(sine(440.0) * 0.2, sine(660.0) * 0.2),
                   dur=0.1, sample_rate=SR, channels=2)
    assert stats.channels == 2
    left, right = stats.channel(0), stats.channel(1)
    assert max(abs(x) for x in left) > 0.1
    assert max(abs(x) for x in right) > 0.1
    assert left != right, "the two channels carry different signals"


def test_render_bounces_an_event_pattern():
    _embed_or_skip()
    # Three notes, dur 0.5 at tempo 1 (beats == seconds): the last release
    # lands at 1.0 + 0.5 * 0.8 = 1.4 s.
    _st1 = render(
        Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5),
        sample_rate=SR)
    samples, frames = _st1.samples, _st1.frames
    assert abs(frames - 1.4 * SR) <= 4096
    assert len(samples) == frames * 2


def test_render_needs_until_for_an_endless_pattern():
    _embed_or_skip()
    _st2 = render(
        Pbind(instrument="default", freq=Pwhite(200.0, 800.0), dur=0.25),
        until=2.0, sample_rate=SR)
    samples, frames = _st2.samples, _st2.frames
    # Drained at beat 2.0: the last event starts by then, nothing beyond its
    # release survives.
    assert 2.0 * SR <= frames <= 2.5 * SR


def test_render_bounces_a_timeline():
    _embed_or_skip()
    tl = Timeline()
    tl.add(0.0, Event(degree=0, dur=0.5))
    tl.add(1.0, Event(degree=4, dur=0.5))
    _st3 = render(tl, sample_rate=SR)
    samples, frames = _st3.samples, _st3.frames
    assert abs(frames - 1.4 * SR) <= 4096


def test_render_bounces_an_arrangement_element():
    _embed_or_skip()
    from clausters.form import Event as FormEvent

    _st4 = render(FormEvent(Event(degree=0, dur=0.5)),
                             sample_rate=SR)
    samples, frames = _st4.samples, _st4.frames
    assert frames > 0.3 * SR


def test_render_bounces_a_generator():
    _embed_or_skip()

    def gen():
        Event(degree=0, dur=0.5).play()
        yield 0.5
        Event(degree=4, dur=0.5).play()

    _st5 = render(gen, sample_rate=SR)
    samples, frames = _st5.samples, _st5.frames
    assert abs(frames - 0.9 * SR) <= 4096   # second note at 0.5 + release 0.4


def test_render_rejects_a_live_destination_for_a_pattern():
    with pytest.raises(ValueError, match="play"):
        render(Pbind(degree=0), destination=object())


def test_render_rejects_unrenderable():
    with pytest.raises(TypeError, match="render"):
        render(3.14)


# ---- the seed: unpredictable first, reproducible on request ----

def _noisy():
    """A def whose output is nothing but its stochastic UGen."""
    from clausters.defs import SynthDef, out, white_noise

    return SynthDef("noisy", out(0.0, white_noise() * 0.2))


def test_a_render_is_a_new_take_every_time():
    _embed_or_skip()

    a = render(_noisy(), dur=0.05, sample_rate=SR, channels=1)
    b = render(_noisy(), dur=0.05, sample_rate=SR, channels=1)
    assert a.seed != b.seed, "each render must draw its own seed"
    assert a.samples != b.samples, "an unseeded render is a fresh take"


def test_a_reported_seed_replays_its_take():
    _embed_or_skip()

    a = render(_noisy(), dur=0.05, sample_rate=SR, channels=1)
    again = render(_noisy(), dur=0.05, sample_rate=SR, channels=1, seed=a.seed)
    assert again.seed == a.seed
    assert again.samples == a.samples, "the reported seed must get the take back"


def test_the_file_path_reports_its_seed_too(tmp_path):
    _embed_or_skip()
    from clausters import Session
    from clausters.render import read_soundfile

    def score(seed):
        s = Session.nrt(tempo=1.0)
        s.server.add_def(_noisy())
        node = s.server.synth("noisy")
        s.server.send_bundle_after(0.05, ("/n_free", node.id))
        return s.render(sample_rate=SR, channels=1, path=tmp_path / f"{seed}.wav",
                        seed=seed)

    fresh = score(None)
    assert fresh.seed != 0, "the server reports the seed it drew"
    assert fresh.samples is None
    # The same seed through the file writer gives the same audio back.
    again = score(fresh.seed)
    assert read_soundfile(again.path).samples == read_soundfile(fresh.path).samples
