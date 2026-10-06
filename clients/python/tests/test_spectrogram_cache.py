"""The spectrogram cache a client writes: the shared core's analysis, as the
bytes a ``spectrogram(cache=...)`` maps."""

import math
import struct

from clausters import _native
from clausters.gui import spectrogram_cache_file


def test_stft_cache_is_the_size_it_was_asked_and_refuses_what_has_no_transform():
    """A spectrogram cache is the core's analysis as bytes: a header and
    ``frames x bins`` magnitudes, as long as the size call said -- and a window
    the FFT has no size for is an error here rather than an empty file."""
    samples = [math.sin(i * 0.05) for i in range(9000)]
    cache = _native.stft_cache(samples, window_size=512, hop=128, sample_rate=44100.0)
    assert cache[:4] == b"CLSG"
    frames = 1 + (len(samples) - 512) // 128
    assert len(cache) == 52 + 4 * frames * 256
    # The header says what was analyzed: the samples, the frames, the bins,
    # the hop and the window, then the rate.
    total, n_frames, n_bins, hop, window = struct.unpack_from("<5Q", cache, 8)
    assert (total, n_frames, n_bins, hop, window) == (9000, frames, 256, 128, 512)
    assert struct.unpack_from("<f", cache, 48)[0] == 44100.0
    # Every magnitude is in the normalized range the texture stores.
    mags = struct.unpack_from(f"<{frames * 256}f", cache, 52)
    assert 0.0 <= min(mags) and max(mags) <= 1.0 and max(mags) > 0.5
    for window_size, hop in ((300, 128), (512, 0)):
        try:
            _native.stft_cache(samples, window_size=window_size, hop=hop)
        except ValueError:
            pass
        else:
            raise AssertionError(f"window {window_size}, hop {hop} has no transform")


def test_spectrogram_cache_file_writes_the_cache_and_answers_its_path(tmp_path):
    """The file is the bytes, at the path it was asked for."""
    samples = [math.sin(i * 0.05) for i in range(4096)]
    path = str(tmp_path / "take.stft")
    assert spectrogram_cache_file(samples, path, window_size=256, hop=64) == path
    with open(path, "rb") as f:
        assert f.read() == _native.stft_cache(samples, window_size=256, hop=64)
