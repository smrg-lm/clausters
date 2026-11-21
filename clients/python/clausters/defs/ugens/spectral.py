"""The frequency-domain chain: FFT, the `pv_*` transforms, IFFT.

A chain is a **frame**, not a signal: `fft` opens one, each `pv_*` transforms it
in place and `ifft` closes it back to samples. The frame is synth-private
scratch, which is why only `fft` names a size.
"""

from .graph import Ugen
from ._generated.spectral import *  # noqa: F403 -- the builders that mirror the wire

# ---- frequency-domain chain: FFT / PV_* / IFFT ----
# `fft` opens a spectral chain, the `pv_*` filters transform the frame in place,
# and `ifft` resynthesises audio. Wire them in order (fft -> pv_* -> ... -> ifft).
# The frame is synth-private scratch (no buffer to allocate); only `fft` names
# the window size, and the server propagates it down the chain.


def fft(source, active=1.0, *, fft_size=1024, hop=0.5, wintype=0) -> Ugen:
    """Opens a spectral chain: windows ``source`` (an audio signal) and
    transforms it to a spectral frame once per **hop**. ``active`` gates the
    input, per sample: ``<= 0`` is analysed as silence, so the round trip is
    the gated input exactly, and a frame whose whole window was gated is not
    taken -- the chain does no work while it stays off. ``fft_size`` is the
    window size (a power of two: 256/512/1024/2048/4096), ``hop`` the fraction
    of the window between frames (in (0, 1], and at least one 64-sample block:
    0.25 of 256), ``wintype`` the window (a `clausters._native.Window`: 0 Hann,
    1 sine, ...). These size the transform, so they are static fields given
    **only here** -- the server propagates them to the rest of the chain. The
    chain's window is also settable live with `Server.u_cmd` addressed to this
    ``fft`` (the `ifft` follows it). Feed the result to a ``pv_*`` filter or
    `ifft`."""
    return Ugen(
        "FFT", [source, active],
        static={"fft_size": int(fft_size), "hop": float(hop), "wintype": int(wintype)},
    )


def pv_kernel(chain, *, mag=None, phase=None, params=()) -> Ugen:
    """The general per-frame mechanism: applies user-written **bin
    expressions** to every bin of each fresh frame. ``mag``, ``phase`` and
    ``params`` are keyword-only -- the expressions are **static** fields and
    ``params`` is the variadic run the wire puts after the chain -- so the one
    positional parameter is the one input the wire names. ``mag`` and ``phase`` are
    symbolic per-bin expressions built from `clausters.defs.pv_expr`'s terms
    (its ``mag``/``phase``/``bin_index``/``nbins``/``binfreq``/``param``)
    with ordinary Python operators; each maps one bin's values to that bin's
    new magnitude / phase. An omitted expression is the identity -- and an
    identity ``phase`` keeps each bin's phase *exactly* (the cheap path: pure
    magnitude maps skip the polar conversion).

    ``params`` are extra signal inputs (controls, LFOs, constants) the
    expressions read as ``param(0)``, ``param(1)``, ... -- sampled once per hop.

    An expression is a **pure per-bin map**: no state across bins or frames,
    no reading other bins. Gates, tilts, masks and magnitude algebra belong
    here; freeze/smear (cross-frame state) and shift (bin remaps) stay with
    the dedicated ``pv_*`` filters. The server validates the program at
    ``/def_send synth`` (stack discipline, parameter arity, unknown words) and
    rejects a bad def with ``/fail``.

    Note that ``mag`` is a raw transform magnitude -- it scales with the input
    level, the window and the ``fft_size``, it is **not** normalized to 0..1 --
    so thresholds and constants must be calibrated to the signal (probe a
    render, or ``poll`` a reference).

    ```python
    from clausters.defs.pv_expr import mag, bin_index, nbins, param
    # A tilted spectral gate: the threshold rises with frequency.
    chain = pv_kernel(chain,
                      mag=mag * (mag >= param(0) * (1 + 4 * bin_index / nbins)),
                      params=[control("thresh", 2.0)])
    ```"""
    from ..pv_expr import pv_tokens
    static = {}
    if mag is not None:
        static["mag_expr"] = pv_tokens(mag)
    if phase is not None:
        static["phase_expr"] = pv_tokens(phase)
    return Ugen("PV_Kernel", [chain, *params], static=static or None)


def conv(source, kernel, *, fft_size=1024, partitions=16) -> Ugen:
    """Partitioned convolution: convolves ``source`` with a **prepared**
    kernel -- a buffer written by ``dest.gen("prepare_partconv", fft_size,
    ir_bufnum)`` (size ``dest`` with `partconv_frames`). The IR's
    spectra are computed once, off the audio thread; the UGen's steady per-
    block cost is flat (the partition products are spread across the hop).

    ``fft_size`` is the transform size (a supported power of two); the
    partition length -- and the intrinsic latency -- is ``fft_size / 2``
    samples. ``partitions`` is the most partitions a kernel may have (its
    pre-allocated state, up to 256): a longer kernel plays silence and is
    reported -- the server logs it, an offline render fails -- rather than
    being cut short. Moving ``kernel`` to a *different* prepared
    buffer crossfades over one partition; regenerating the same buffer
    switches hard."""
    return Ugen(
        "Conv", [source, kernel],
        static={"fft_size": int(fft_size), "partitions": int(partitions)},
    )


def partconv_frames(ir_frames: int, fft_size: int = 1024) -> int:
    """Frames a kernel buffer needs to hold ``ir_frames`` of impulse response
    prepared at ``fft_size`` (partitions of ``fft_size / 2``, plus the two-
    sample header) -- the size to `clausters.defs.Buffer.alloc` before
    ``buf.gen("prepare_partconv", fft_size, ir_bufnum)``."""
    part = fft_size // 2
    parts = -(-int(ir_frames) // part)
    return 2 + parts * int(fft_size)
