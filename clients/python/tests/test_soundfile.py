"""`SoundFile`: a file's header and samples through the server's codecs, its
writer, the empty file, and how it meets a server `Buffer`."""

from array import array

import pytest

from clausters import SoundFile
from clausters.defs import Buffer


def _lib_or_skip():
    try:
        from clausters import ipc
        ipc._load(None)
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embed library not built: {e}")


def test_a_written_file_opens_with_its_shape_and_reads_back(tmp_path):
    _lib_or_skip()
    samples = array("f", [((i % 7) - 3) / 8 for i in range(600)])
    wav = tmp_path / "take.wav"
    written = SoundFile.write(wav, samples, 3, 44_100.0, sample_format="float")
    opened = SoundFile.open(wav)
    assert opened == written
    assert (opened.frames, opened.channels, opened.sample_rate) == (200, 3, 44_100.0)
    assert (opened.codec, opened.sample_format) == ("wav", "float")
    assert opened.duration == pytest.approx(200 / 44_100.0)
    assert opened.read() == samples
    assert opened.read(10, 5) == samples[30:45]


def test_an_integer_file_reads_at_its_precision(tmp_path):
    _lib_or_skip()
    samples = [0.5, -0.25, 0.125, 0.0]
    for fmt, step in (("int16", 1 / 32767), ("int24", 1 / 8388607)):
        f = SoundFile.write(tmp_path / f"{fmt}.wav", samples, 1, 48_000.0, sample_format=fmt)
        assert SoundFile.open(f.path).sample_format == fmt
        assert all(abs(a - b) <= step for a, b in zip(f.read(), samples))


def test_what_cannot_be_a_file_is_refused(tmp_path):
    _lib_or_skip()
    from clausters.ipc import RenderError

    with pytest.raises(RenderError):
        SoundFile.open(tmp_path / "missing.wav")
    with pytest.raises(RenderError):
        SoundFile.write(tmp_path / "ragged.wav", [0.0] * 5, 2, 48_000.0)
    with pytest.raises(ValueError):
        SoundFile.empty(0, 10, 48_000.0)


def test_an_empty_file_is_a_shape_with_no_path():
    empty = SoundFile.empty(2, 4, 48_000.0)
    assert empty.path is None and empty.codec is None
    assert empty.read() == array("f", [0.0] * 8)
    assert empty.read(3) == array("f", [0.0] * 2)
    source = empty.source
    assert source.location == {"at": "volatile"}
    assert (source.channels, source.frames, source.sample_rate) == (2, 4, 48_000.0)
    with pytest.raises(ValueError):
        Buffer.read(empty)


def test_a_file_is_a_file_source(tmp_path):
    _lib_or_skip()
    f = SoundFile.write(tmp_path / "a.wav", [0.0] * 6, 2, 48_000.0)
    assert f.source.path == f.path
    assert f.source.frames == 3


def test_a_sound_file_becomes_a_buffer_and_a_buffer_a_sound_file(tmp_path):
    _lib_or_skip()
    try:
        from clausters import Session
        session = Session.embed()
    except (OSError, RuntimeError) as e:
        pytest.skip(f"embedded server unavailable: {e}")
    server = session.server
    samples = array("f", [i / 16 for i in range(16)])
    f = SoundFile.write(tmp_path / "ramp.wav", samples, 2, 48_000.0, sample_format="float")

    buf = f.to_buffer(server=server)
    assert (buf.frames, buf.channels) == (8, 2)
    assert list(buf.get_samples()) == list(samples)
    assert Buffer.read(f, server=server).frames == 8
    assert Buffer.read_channels(f, [1], server=server).channels == 1

    out = buf.write(str(tmp_path / "copy.wav"), sample_format="float")
    assert (out.path, out.frames, out.channels, out.sample_format) == (
        str(tmp_path / "copy.wav"), 8, 2, "float")
    assert SoundFile.open(out.path).read() == samples

    silence = SoundFile.empty(1, 32, 48_000.0).to_buffer(server=server)
    assert (silence.frames, silence.channels) == (32, 1)
    assert set(silence.get_samples()) == {0.0}
