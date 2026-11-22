"""Oscillators and noise: what a graph starts from.

The band-limited pair (`saw`, `pulse`) beside the naive `lf_*` forms, the noise
family by spectrum, and `phasor` as the ramp the table readers run on.
"""

from .graph import ChannelList, Ugen
from ._generated.osc import *  # noqa: F403 -- the builders that mirror the wire



def _partials(freqs, amps, third, fill):
    """`freqs`, `amps` (ones unless given) and a third list (`fill` unless
    given), as the flat triples a bank takes."""
    freqs = list(freqs)
    amps = [1.0] * len(freqs) if amps is None else list(amps)
    third = [fill] * len(freqs) if third is None else list(third)
    if not (len(freqs) == len(amps) == len(third)):
        raise ValueError("a bank's lists must have one entry per partial")
    out = []
    for f, a, t in zip(freqs, amps, third):
        out += [f, a, t]
    return out


def klang(freqs, amps=None, phases=None, freqscale=1.0, freqoffset=0.0) -> Ugen:
    """A bank of fixed sines: one per entry of `freqs`, at `amps` (1 unless
    given) and starting at `phases` (cycles, 0 unless given), the frequencies
    times `freqscale` plus `freqoffset`. Read at the synth's start: a fixed
    spectrum, as cheap as sines get."""
    return Ugen("Klang", [freqscale, freqoffset, *_partials(freqs, amps, phases, 0.0)])


def _channels(numchans) -> range:
    """The channels of a granular UGen's rows, one a channel."""
    if isinstance(numchans, bool) or not isinstance(numchans, int) or numchans < 1:
        raise ValueError(f"a granular UGen needs a channel count of at least 1, got {numchans!r}")
    return range(numchans)


def grain_sin(numchans=1, trigger=0.0, dur=1.0, freq=440.0, pan=0.0, envbufnum=-1.0,
              max_grains=512) -> ChannelList:
    """At each rising ``trigger``, a grain ``dur`` seconds long of a sine at
    ``freq``, under the Hann (or the window in buffer ``envbufnum``), panned
    at ``pan`` across ``numchans`` channels: -1 to 1 left to right on a pair,
    a turn round a ring. At most ``max_grains`` (a constant) at once."""
    return ChannelList([Ugen("GrainSin", [trigger, dur, freq, pan, envbufnum, float(max_grains),
                                          float(numchans), float(c)]) for c in _channels(numchans)])


def grain_fm(numchans=1, trigger=0.0, dur=1.0, carfreq=440.0, modfreq=200.0, index=1.0,
             pan=0.0, envbufnum=-1.0, max_grains=512) -> ChannelList:
    """`grain_sin` of an FM pair: a carrier at ``carfreq`` deviated by
    ``index * modfreq`` at ``modfreq``."""
    return ChannelList([Ugen("GrainFM", [trigger, dur, carfreq, modfreq, index, pan, envbufnum,
                                         float(max_grains), float(numchans), float(c)])
                        for c in _channels(numchans)])


def grain_buf(numchans, trigger, dur, sndbuf, rate=1.0, pos=0.0, interp=2.0, pan=0.0,
              envbufnum=-1.0, max_grains=512) -> ChannelList:
    """`grain_sin` of a mono buffer: each grain read from ``pos`` (0 to 1 of
    the buffer) at ``rate`` times its own rate, wrapping round it, with no
    interpolation (``interp`` 1), linear (2) or cubic (4)."""
    return ChannelList([Ugen("GrainBuf", [trigger, dur, sndbuf, rate, pos, interp, pan, envbufnum,
                                          float(max_grains), float(numchans), float(c)])
                        for c in _channels(numchans)])


def grain_in(numchans, trigger, dur, signal, pan=0.0, envbufnum=-1.0,
             max_grains=512) -> ChannelList:
    """`grain_sin` of ``signal``: each grain the input while it sounds."""
    return ChannelList([Ugen("GrainIn", [trigger, dur, signal, pan, envbufnum, float(max_grains),
                                         float(numchans), float(c)]) for c in _channels(numchans)])


def t_grains(numchans, trigger=0.0, bufnum=0.0, rate=1.0, center_pos=0.0, dur=0.1, pan=0.0,
             amp=0.1, interp=4.0) -> ChannelList:
    """At each rising ``trigger``, a grain of a mono buffer centred on
    ``center_pos`` seconds into it, ``dur`` long under the Hann, read at
    ``rate``, at ``amp``, panned at ``pan`` as `grain_sin`'s."""
    return ChannelList([Ugen("TGrains", [trigger, bufnum, rate, center_pos, dur, pan, amp, interp,
                                         float(numchans), float(c)]) for c in _channels(numchans)])


def warp1(numchans, bufnum=0.0, pointer=0.0, freq_scale=1.0, window_size=0.2, envbufnum=-1.0,
          overlaps=8.0, window_rand_ratio=0.0, interp=1.0) -> ChannelList:
    """A buffer stretched in time, ``numchans`` of its channels: grains
    ``window_size`` long, ``overlaps`` at once, each read from ``pointer``
    (0 to 1 of the buffer) at ``freq_scale`` times its pitch. The length
    moves at random by up to ``window_rand_ratio`` of itself."""
    return ChannelList([Ugen("Warp1", [bufnum, pointer, freq_scale, window_size, envbufnum, overlaps,
                                       window_rand_ratio, interp, float(c)])
                        for c in _channels(numchans)])

