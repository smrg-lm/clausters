"""Analysis: the measures read off an FFT chain, the onset and beat
trackers, and the pitch follower.

The measures of a frame (`spec_centroid`, `loudness`, `mfcc`, ...) and the
onset and beat detectors read a chain an `fft` opens -- they leave it as it
was, so a chain can feed any number of them and an `ifft` besides -- and
hold their value between frames. `pitch` reads a signal and `beat_track2`
features a def writes to control buses. A UGen with several outputs is a
node a channel, and its builder returns them as a `ChannelList`.
"""

from .graph import ChannelList, Ugen, chans
from ._generated.analysis import *  # noqa: F403 -- the builders that mirror the wire


def mfcc(chain, numcoeff=13) -> ChannelList:
    """The spectrum's mel-frequency cepstral coefficients, from the first:
    the power in 42 bands evenly spaced in mels from 80 Hz to 18 kHz, as dB,
    through a cosine transform, scaled into about 0 to 1. ``numcoeff`` (1 to
    42) is how many, a constant. Returns ``numcoeff`` channels."""
    if isinstance(numcoeff, bool) or not isinstance(numcoeff, int) or not 1 <= numcoeff <= 42:
        raise ValueError(f"mfcc needs 1 to 42 coefficients, got {numcoeff!r}")
    return chans(*(Ugen("MFCC", [chain, float(numcoeff), float(c)]) for c in range(numcoeff)))


def beat_track(chain, lock=0.0) -> ChannelList:
    """The beat of the audio ``chain`` analyses, by Davies and Plumbley's
    context-dependent tracker: a trigger on each beat, on each half beat and
    on each quarter, and the tempo in beats a second. While ``lock`` is
    above 0.5 the outputs keep their period and phase and the tracker goes
    on following. Tuned to a 1024-point chain at a half-frame hop at 44.1 or
    48 kHz (about 11 ms a frame). Returns the four channels."""
    return chans(*(Ugen("BeatTrack", [chain, lock, float(c)]) for c in range(4)))


def beat_track2(busindex, numfeatures, windowsize=2.0, phaseaccuracy=0.02, lock=0.0,
                weightingscheme=-2.1) -> ChannelList:
    """The beat of ``numfeatures`` features (onset strengths, say) on the
    control buses from ``busindex``: twice a second their last
    ``windowsize`` seconds are matched against a template of four events a
    beat, straight and swung, at each tempo from 60 to 179 beats a minute
    and every phase to ``phaseaccuracy`` seconds. ``weightingscheme`` is
    -2.5 for flat weights, -1.5 to compensate each tempo for the events that
    fit the window, or a buffer of 120 weights, one a tempo. ``numfeatures``,
    ``windowsize``, ``phaseaccuracy`` and ``weightingscheme`` are constants;
    ``lock`` above 0.5 holds the outputs. Returns six channels: the beat,
    half-beat and quarter-beat triggers, the tempo in beats a second, the
    beat's phase (0 to 1) and the groove (0 straight, 1 swung)."""
    return chans(*(
        Ugen("BeatTrack2", [busindex, numfeatures, windowsize, phaseaccuracy, lock, weightingscheme, float(c)])
        for c in range(6)
    ))


def pitch(signal, init_freq=440.0, min_freq=60.0, max_freq=4000.0, exec_freq=100.0,
          max_bins_per_octave=16, median=1, amp_threshold=0.01, peak_threshold=0.5,
          down_sample=1, clar=0) -> ChannelList:
    """The frequency of ``signal`` by autocorrelation, and whether it has one.
    ``exec_freq`` times a second, if ``signal`` reaches ``amp_threshold``, the
    first autocorrelation peak above ``peak_threshold`` of its zero lag
    (searched at ``max_bins_per_octave`` lags an octave) is the period; a
    frequency between ``min_freq`` and ``max_freq`` is taken, through a
    median of the last ``median``. ``down_sample`` reads every so many
    samples. Every argument but ``signal`` is a constant. Returns two
    channels: the frequency (``init_freq`` before the first), and 1 when the
    last run found one (with ``clar > 0`` the peak's clarity, 0 to 1) or
    0."""
    params = [init_freq, min_freq, max_freq, exec_freq, max_bins_per_octave, median,
              amp_threshold, peak_threshold, down_sample, clar]
    return chans(*(Ugen("Pitch", [signal, *params, float(c)]) for c in range(2)))
