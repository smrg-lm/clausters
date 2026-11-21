"""The `spectral` builders that mirror the wire, re-exported by `spectral`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/spectral.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "ifft",
    "pv_mag_above",
    "pv_mag_below",
    "pv_brick_wall",
    "pv_mag_clip",
    "pv_add",
    "pv_mul",
    "pv_min",
    "pv_max",
    "pv_mag_mul",
    "pv_copy_phase",
    "pv_mag_freeze",
    "pv_mag_smear",
    "pv_bin_shift",
    "pv_mag_shift",
]


def ifft(chain) -> Ugen:
    """Closes a spectral chain: inverse-transforms each fresh frame and
    overlap-adds it back to audio (window-normalized, so a bare `fft`->`ifft`
    reconstructs at unity gain, delayed by one window). `chain` is the output
    of an `fft` or a `pv_*` filter.
    """
    return Ugen("IFFT", [chain])


def pv_mag_above(chain, threshold) -> Ugen:
    """Passes only the bins whose magnitude is **above** `threshold`, zeroing
    the rest. `chain` comes from `fft` or another `pv_*`.
    """
    return Ugen("PV_MagAbove", [chain, threshold])


def pv_mag_below(chain, threshold) -> Ugen:
    """Passes only the bins whose magnitude is **below** `threshold`."""
    return Ugen("PV_MagBelow", [chain, threshold])


def pv_brick_wall(chain, wipe) -> Ugen:
    """Brick-wall band limit: `wipe > 0` zeroes the top fraction of bins (a low
    pass), `wipe < 0` the bottom (a high pass), `0` passes everything
    (`wipe` in -1..1).
    """
    return Ugen("PV_BrickWall", [chain, wipe])


def pv_mag_clip(chain, threshold) -> Ugen:
    """Limits each bin's magnitude **to** `threshold`: louder bins are scaled
    down to it (phases kept), quieter bins pass untouched.
    """
    return Ugen("PV_MagClip", [chain, threshold])


def pv_add(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex sum. Both inputs must be spectral
    chains of the same `fft_size`, `hop` and `wintype` (and distinct); the
    result lands in chain A, which the combiner's output carries onward.
    """
    return Ugen("PV_Add", [chain_a, chain_b])


def pv_mul(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex product (spectral ring modulation).
    """
    return Ugen("PV_Mul", [chain_a, chain_b])


def pv_min(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per bin, whichever input has the **smaller**
    magnitude.
    """
    return Ugen("PV_Min", [chain_a, chain_b])


def pv_max(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per bin, whichever input has the **larger**
    magnitude.
    """
    return Ugen("PV_Max", [chain_a, chain_b])


def pv_mag_mul(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: A's bins scaled by B's magnitudes -- A's phases kept
    (a spectral envelope transfer, the classic "vocoder" cross-synthesis).
    """
    return Ugen("PV_MagMul", [chain_a, chain_b])


def pv_copy_phase(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: A's magnitudes with B's phases (the complementary
    cross-synthesis to `pv_mag_mul`).
    """
    return Ugen("PV_CopyPhase", [chain_a, chain_b])


def pv_mag_freeze(chain, freeze=0.0) -> Ugen:
    """While `freeze <= 0` stores each frame's magnitudes and passes through;
    while `> 0` rescales every bin to the stored magnitudes -- the spectral
    envelope holds while the phases keep running.
    """
    return Ugen("PV_MagFreeze", [chain, freeze])


def pv_mag_smear(chain, bins=0.0) -> Ugen:
    """Averages each bin's magnitude over `bins` neighbors on each side
    (`0` is transparent), phases untouched -- a spectral blur.
    """
    return Ugen("PV_MagSmear", [chain, bins])


def pv_bin_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """Remaps bin `b` to `round(b * stretch + shift)`: colliding bins sum,
    out-of-range bins are dropped. `stretch=1, shift=0` is transparent; a
    positive `shift` moves every partial up by `shift` bin widths.
    """
    return Ugen("PV_BinShift", [chain, stretch, shift])


def pv_mag_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """The `pv_bin_shift` remap applied to the magnitude envelope only, laid
    over the frame's original phases.
    """
    return Ugen("PV_MagShift", [chain, stretch, shift])
