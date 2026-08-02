"""The frequency-domain chain: FFT, the `pv_*` transforms, IFFT.

A chain is a **frame**, not a signal: `fft` opens one, each `pv_*` transforms it
in place and `ifft` closes it back to samples. The frame is synth-private
scratch, which is why only `fft` names a size.
"""

from .graph import Ugen

# ---- frequency-domain chain: FFT / PV_* / IFFT (S8) ----
# `fft` opens a spectral chain, the `pv_*` filters transform the frame in place,
# and `ifft` resynthesises audio. Wire them in order (fft -> pv_* -> ... -> ifft).
# The frame is synth-private scratch (no buffer to allocate); only `fft` names
# the window size, and the server propagates it down the chain.


def fft(source, active=1.0, *, fft_size=1024, hop=0.5, wintype=0) -> Ugen:
    """Opens a spectral chain: windows ``source`` (an audio signal) and
    transforms it to a spectral frame once per **hop**. ``active > 0`` runs the
    transform, ``<= 0`` holds. ``fft_size`` is the window size (a power of two:
    256/512/1024/2048/4096), ``hop`` the fraction of the window between frames,
    ``wintype`` the window (a `clausters._native.Window`: 0 Hann, 1 sine, …).
    These size the transform, so they are static fields given **only here** — the
    server propagates them to the rest of the chain. The window is also settable
    live with `Server.u_cmd`. Feed the result to a ``pv_*`` filter or `ifft`."""
    return Ugen(
        "FFT", [source, active],
        static={"fft_size": int(fft_size), "hop": float(hop), "wintype": int(wintype)},
    )


def ifft(chain) -> Ugen:
    """Closes a spectral chain: inverse-transforms each fresh frame and
    overlap-adds it back to audio (window-normalized, so a bare `fft`->`ifft`
    reconstructs at unity gain, delayed by one window). ``chain`` is the output
    of an `fft` or a ``pv_*`` filter."""
    return Ugen("IFFT", [chain])


def pv_mag_above(chain, threshold) -> Ugen:
    """Passes only the bins whose magnitude is **above** ``threshold``, zeroing
    the rest. ``chain`` comes from `fft` or another ``pv_*``."""
    return Ugen("PV_MagAbove", [chain, threshold])


def pv_mag_below(chain, threshold) -> Ugen:
    """Passes only the bins whose magnitude is **below** ``threshold``."""
    return Ugen("PV_MagBelow", [chain, threshold])


def pv_brick_wall(chain, wipe) -> Ugen:
    """Brick-wall band limit: ``wipe > 0`` zeroes the top fraction of bins (a low
    pass), ``wipe < 0`` the bottom (a high pass), ``0`` passes everything
    (``wipe`` in -1..1)."""
    return Ugen("PV_BrickWall", [chain, wipe])


def pv_mag_clip(chain, threshold) -> Ugen:
    """Limits each bin's magnitude **to** ``threshold``: louder bins are scaled
    down to it (phases kept), quieter bins pass untouched."""
    return Ugen("PV_MagClip", [chain, threshold])


def pv_add(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex sum. Both inputs must be spectral
    chains of the same ``fft_size`` (and distinct); the result lands in chain A,
    which the combiner's output carries onward."""
    return Ugen("PV_Add", [chain_a, chain_b])


def pv_mul(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex product (spectral ring modulation)."""
    return Ugen("PV_Mul", [chain_a, chain_b])


def pv_min(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per bin, whichever input has the **smaller**
    magnitude."""
    return Ugen("PV_Min", [chain_a, chain_b])


def pv_max(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per bin, whichever input has the **larger**
    magnitude."""
    return Ugen("PV_Max", [chain_a, chain_b])


def pv_mag_mul(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: A's bins scaled by B's magnitudes — A's phases kept
    (a spectral envelope transfer, the classic "vocoder" cross-synthesis)."""
    return Ugen("PV_MagMul", [chain_a, chain_b])


def pv_copy_phase(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: A's magnitudes with B's phases (the complementary
    cross-synthesis to `pv_mag_mul`)."""
    return Ugen("PV_CopyPhase", [chain_a, chain_b])


def pv_mag_freeze(chain, freeze=0.0) -> Ugen:
    """While ``freeze <= 0`` stores each frame's magnitudes and passes through;
    while ``> 0`` rescales every bin to the stored magnitudes — the spectral
    envelope holds while the phases keep running."""
    return Ugen("PV_MagFreeze", [chain, freeze])


def pv_mag_smear(chain, bins=0.0) -> Ugen:
    """Averages each bin's magnitude over ``bins`` neighbors on each side
    (``0`` is transparent), phases untouched — a spectral blur."""
    return Ugen("PV_MagSmear", [chain, bins])


def pv_bin_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """Remaps bin ``b`` to ``round(b * stretch + shift)``: colliding bins sum,
    out-of-range bins are dropped. ``stretch=1, shift=0`` is transparent; a
    positive ``shift`` moves every partial up by ``shift`` bin widths."""
    return Ugen("PV_BinShift", [chain, stretch, shift])


def pv_mag_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """The `pv_bin_shift` remap applied to the magnitude envelope only, laid
    over the frame's original phases."""
    return Ugen("PV_MagShift", [chain, stretch, shift])


def pv_kernel(chain, mag=None, phase=None, params=()) -> Ugen:
    """The general per-frame mechanism: applies user-written **bin
    expressions** to every bin of each fresh frame. ``mag`` and ``phase`` are
    symbolic per-bin expressions built from `clausters.defs.pv_expr`'s terms
    (its ``mag``/``phase``/``bin_index``/``nbins``/``binfreq``/``param``)
    with ordinary Python operators; each maps one bin's values to that bin's
    new magnitude / phase. An omitted expression is the identity — and an
    identity ``phase`` keeps each bin's phase *exactly* (the cheap path: pure
    magnitude maps skip the polar conversion).

    ``params`` are extra signal inputs (controls, LFOs, constants) the
    expressions read as ``param(0)``, ``param(1)``, … — sampled once per hop.

    An expression is a **pure per-bin map**: no state across bins or frames,
    no reading other bins. Gates, tilts, masks and magnitude algebra belong
    here; freeze/smear (cross-frame state) and shift (bin remaps) stay with
    the dedicated ``pv_*`` filters. The server validates the program at
    ``/def_send synth`` (stack discipline, parameter arity, unknown words) and
    rejects a bad def with ``/fail``.

    Note that ``mag`` is a raw transform magnitude — it scales with the input
    level, the window and the ``fft_size``, it is **not** normalized to 0..1 —
    so thresholds and constants must be calibrated to the material (probe a
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
    kernel — a buffer written by ``dest.gen("prepare_partconv", fft_size,
    ir_bufnum)`` (size ``dest`` with `partconv_frames`). The IR's
    spectra are computed once, off the audio thread; the UGen's steady per-
    block cost is flat (the partition products are spread across the hop).

    ``fft_size`` is the transform size (a supported power of two); the
    partition length — and the intrinsic latency — is ``fft_size / 2``
    samples. ``partitions`` caps the kernel length this instance accepts
    (its pre-allocated state). Moving ``kernel`` to a *different* prepared
    buffer crossfades over one partition; regenerating the same buffer
    switches hard."""
    return Ugen(
        "Conv", [source, kernel],
        static={"fft_size": int(fft_size), "partitions": int(partitions)},
    )


def partconv_frames(ir_frames: int, fft_size: int = 1024) -> int:
    """Frames a kernel buffer needs to hold ``ir_frames`` of impulse response
    prepared at ``fft_size`` (partitions of ``fft_size / 2``, plus the two-
    sample header) — the size to `clausters.defs.Buffer.alloc` before
    ``buf.gen("prepare_partconv", fft_size, ir_bufnum)``."""
    part = fft_size // 2
    parts = -(-int(ir_frames) // part)
    return 2 + parts * int(fft_size)
