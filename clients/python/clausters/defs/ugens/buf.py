"""Buffers: playing them, reading them as tables, asking about them.

The players (`play_buf`, `buf_rd`), the table oscillators and waveshaper that
read a `/buffer_gen` table, and the init-rate queries that report a buffer's
shape into the graph.
"""

from .graph import Ugen

def play_buf(bufnum, chan=0.0, rate=1.0, loop=0.0) -> Ugen:
    """Mono buffer player with linear interpolation; ``rate`` is frames per
    output sample (1.0 = server rate)."""
    return Ugen("PlayBuf", [bufnum, chan, rate, loop])


def buf_rd(bufnum, chan, phase, loop=0.0) -> Ugen:
    """Reads a buffer at a ``phase`` signal in frames (linear interpolation)."""
    return Ugen("BufRd", [bufnum, chan, phase, loop])


# ---- table oscillators & waveshaper (read `/buffer_gen` tables) ----


def osc(bufnum, freq=440.0, phase=0.0) -> Ugen:
    """Interpolating wavetable oscillator. ``bufnum`` must hold a
    **wavetable-format** buffer (fill it with ``buf.gen(...)`` and a
    ``/buffer_gen`` command whose wavetable flag is set); ``phase`` is an offset in
    radians."""
    return Ugen("Osc", [bufnum, freq, phase])


def oscn(bufnum, freq=440.0, phase=0.0) -> Ugen:
    """Non-interpolating oscillator over a **plain** (non-wavetable) buffer;
    rawer and cheaper than `osc`."""
    return Ugen("OscN", [bufnum, freq, phase])


def vosc(bufpos, freq=440.0, phase=0.0) -> Ugen:
    """Like `osc` but the buffer number is a signal: reads wavetables
    ``bufpos`` and ``bufpos + 1`` and crossfades by the fractional part, so
    sweeping ``bufpos`` morphs a bank of adjacent tables (allocate them
    contiguously, all the same size)."""
    return Ugen("VOsc", [bufpos, freq, phase])


def shaper(bufnum, signal) -> Ugen:
    """Waveshaper: maps ``signal`` (in +-1, clamped) through a transfer table
    in wavetable format (typically a ``cheby`` `/buffer_gen`); the table's first
    point is ``signal = -1``, its last ``signal = +1``."""
    return Ugen("Shaper", [bufnum, signal])

# ---- scalar / init-rate (ir) ----


def sample_rate() -> Ugen:
    """The engine sample rate in Hz, computed once at init (``ir``)."""
    return Ugen("SampleRate", [], rate="ir")


def buf_frames(bufnum) -> Ugen:
    """The number of frames in a buffer, block-constant (``kr``)."""
    return Ugen("BufFrames", [bufnum], rate="kr")


def buf_sample_rate(bufnum) -> Ugen:
    """The buffer's own sample rate (Hz), block-constant (``kr``)."""
    return Ugen("BufSampleRate", [bufnum], rate="kr")


def buf_rate_scale(bufnum) -> Ugen:
    """``file_sr / server_sr``, block-constant (``kr``); feed `play_buf`'s
    ``rate`` (``buf_rate_scale(buf) * pitch``) to play at the file's true pitch
    without the client knowing either rate."""
    return Ugen("BufRateScale", [bufnum], rate="kr")


def buf_channels(bufnum) -> Ugen:
    """The buffer's channel count, block-constant (``kr``)."""
    return Ugen("BufChannels", [bufnum], rate="kr")


def buf_dur(bufnum) -> Ugen:
    """The buffer's duration in seconds (``frames / file_sr``), block-constant
    (``kr``)."""
    return Ugen("BufDur", [bufnum], rate="kr")


def rand(lo=0.0, hi=1.0) -> Ugen:
    """One uniform random value in ``[lo, hi)``, drawn once at synth init and
    held for the node's life (``ir``); ``lo``/``hi`` must be constants or ``ir``."""
    return Ugen("Rand", [lo, hi], rate="ir")
