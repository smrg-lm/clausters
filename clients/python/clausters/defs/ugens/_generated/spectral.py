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
    "pv_mag_squared",
    "pv_mag_noise",
    "pv_conj",
    "pv_phase_shift90",
    "pv_phase_shift270",
    "pv_phase_shift",
    "pv_div",
    "pv_mag_div",
    "pv_bin_wipe",
    "pv_rect_comb",
    "pv_rect_comb2",
    "pv_rand_comb",
    "pv_rand_wipe",
    "pv_bin_scramble",
    "pv_diffuser",
    "pv_local_max",
    "pv_conformal_map",
    "pv_copy",
    "unpack1_fft",
    "convolution",
    "convolution2",
    "convolution2_l",
    "convolution3",
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
    """Brick-wall band limit: `wipe > 0` zeroes that fraction of the bins from
    the bottom up, DC included (a high pass), `wipe < 0` from the top down,
    Nyquist included (a low pass), `0` passes everything (`wipe` in -1..1).
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
    envelope holds while the phases keep running. The first frame is always
    stored, so a chain frozen from its start holds its first spectrum.
    """
    return Ugen("PV_MagFreeze", [chain, freeze])


def pv_mag_smear(chain, bins=0.0) -> Ugen:
    """Averages each bin's magnitude over `bins` neighbors on each side
    (`0` is transparent), phases untouched -- a spectral blur.
    """
    return Ugen("PV_MagSmear", [chain, bins])


def pv_bin_shift(chain, stretch=1.0, shift=0.0, interp=0.0) -> Ugen:
    """Remaps bin `b` to `round(b * stretch + shift)`: colliding bins sum,
    out-of-range bins are dropped. `stretch=1, shift=0` is transparent; a
    positive `shift` moves every partial up by `shift` bin widths. With
    `interp` above 0 a bin is split between the two bins around
    `b * stretch + shift`, in proportion to how near it lands to each.
    """
    return Ugen("PV_BinShift", [chain, stretch, shift, interp])


def pv_mag_shift(chain, stretch=1.0, shift=0.0) -> Ugen:
    """The `pv_bin_shift` remap applied to the magnitude envelope only, laid
    over the frame's original phases.
    """
    return Ugen("PV_MagShift", [chain, stretch, shift])


def pv_mag_squared(chain) -> Ugen:
    """Squares every bin's magnitude, phases kept: the loud partials stand out
    further from the rest (and the frame's level rises with its square).
    """
    return Ugen("PV_MagSquared", [chain])


def pv_mag_noise(chain) -> Ugen:
    """Multiplies every bin by its own random factor in `[-1, 1)`, drawn again
    each frame: the spectrum's shape turned to noise.
    """
    return Ugen("PV_MagNoise", [chain])


def pv_conj(chain) -> Ugen:
    """The complex conjugate of every bin: each phase negated, which plays each
    frame backwards in time (DC and Nyquist, which are real, stay).
    """
    return Ugen("PV_Conj", [chain])


def pv_phase_shift90(chain) -> Ugen:
    """Turns every bin's phase a quarter turn forward (DC and Nyquist stay)."""
    return Ugen("PV_PhaseShift90", [chain])


def pv_phase_shift270(chain) -> Ugen:
    """Turns every bin's phase a quarter turn back (DC and Nyquist stay)."""
    return Ugen("PV_PhaseShift270", [chain])


def pv_phase_shift(chain, shift, integrate=0.0) -> Ugen:
    """Turns every bin's phase by `shift` radians. With `integrate` above 0 the
    shift adds up from frame to frame, so a constant `shift` keeps turning.
    """
    return Ugen("PV_PhaseShift", [chain, shift, integrate])


def pv_div(chain_a, chain_b) -> Ugen:
    """Two-chain combiner: per-bin complex division, A / B (a deconvolution).
    A bin where B is silent is 0.
    """
    return Ugen("PV_Div", [chain_a, chain_b])


def pv_mag_div(chain_a, chain_b, zeroed=0.0001) -> Ugen:
    """Two-chain combiner: A's magnitudes divided by B's, A's phases kept; each
    B magnitude is taken as at least `zeroed`, so a quiet bin of B does not
    blow up.
    """
    return Ugen("PV_MagDiv", [chain_a, chain_b, zeroed])


def pv_bin_wipe(chain_a, chain_b, wipe=0.0) -> Ugen:
    """Two-chain combiner: A with a run of B's bins in it. `wipe > 0` takes that
    fraction of B's bins from the bottom up, `wipe < 0` from the top down;
    `0` is A, `1` or `-1` is B.
    """
    return Ugen("PV_BinWipe", [chain_a, chain_b, wipe])


def pv_rect_comb(chain, num_teeth=0.0, phase=0.0, width=0.5) -> Ugen:
    """A comb over the bins: `num_teeth` teeth across the spectrum, the comb
    starting at `phase` and each tooth `width` of a period wide (both
    fractions of a period); the bins between the teeth are zeroed.
    """
    return Ugen("PV_RectComb", [chain, num_teeth, phase, width])


def pv_rect_comb2(
    chain_a,
    chain_b,
    num_teeth=0.0,
    phase=0.0,
    width=0.5,
) -> Ugen:
    """Two-chain combiner: `pv_rect_comb`'s comb, with the bins between the
    teeth taken from B instead of zeroed.
    """
    return Ugen("PV_RectComb2", [chain_a, chain_b, num_teeth, phase, width])


def pv_rand_comb(chain, wipe=0.0, trig=0.0) -> Ugen:
    """Zeroes a random `wipe` fraction of the bins, chosen in an order drawn at
    the first frame and again at each `trig`; at `wipe=1` every bin is zeroed.
    """
    return Ugen("PV_RandComb", [chain, wipe, trig])


def pv_rand_wipe(chain_a, chain_b, wipe=0.0, trig=0.0) -> Ugen:
    """Two-chain combiner: a random `wipe` fraction of the bins taken from B, in
    an order drawn at the first frame and again at each `trig`; `0` is A,
    `1` is B.
    """
    return Ugen("PV_RandWipe", [chain_a, chain_b, wipe, trig])


def pv_bin_scramble(chain, wipe=0.0, width=0.2, trig=0.0) -> Ugen:
    """Moves a random `wipe` fraction of the bins to random places nearby: each
    comes from up to `width` (a fraction of the bins) away. The arrangement
    is drawn at the first frame and again at each `trig`.
    """
    return Ugen("PV_BinScramble", [chain, wipe, width, trig])


def pv_diffuser(chain, trig=0.0) -> Ugen:
    """Turns each bin's phase by its own random amount, drawn at the first
    frame and again at each `trig`: the magnitudes stay, the transients
    smear.
    """
    return Ugen("PV_Diffuser", [chain, trig])


def pv_local_max(chain, threshold=0.0) -> Ugen:
    """Keeps the bins louder than both neighbors and at least `threshold`,
    zeroing the rest: the spectrum's peaks alone.
    """
    return Ugen("PV_LocalMax", [chain, threshold])


def pv_conformal_map(chain, areal=0.0, aimag=0.0) -> Ugen:
    """Maps each bin `z` to `(z - a) / (1 - conj(a) z)`, with `a` the complex
    number `areal + i aimag`: a Moebius map of the complex plane, a spectral
    distortion with no counterpart in time.
    """
    return Ugen("PV_ConformalMap", [chain, areal, aimag])


def pv_copy(chain) -> Ugen:
    """A copy of `chain` as a chain of its own, so one analysis feeds two
    branches: what the def does to one after the copy leaves the other as
    it was.
    """
    return Ugen("PV_Copy", [chain])


def unpack1_fft(chain, binindex, whichmeasure=0.0) -> Ugen:
    """One bin of a frame as a control signal: its magnitude (`whichmeasure=0`)
    or its phase (`1`), updated at each frame and held between them. It
    reads the chain and leaves it as it was.
    """
    return Ugen("Unpack1FFT", [chain, binindex, whichmeasure])


def convolution(source, kernel, framesize=512.0) -> Ugen:
    """Convolves two signals: each `framesize` samples of `source` with the
    same samples of `kernel`, so the response changes as the kernel plays --
    a cross-synthesis. `framesize` (a power of two, a constant) is also the
    latency.
    """
    return Ugen("Convolution", [source, kernel, framesize])


def convolution2(source, kernel, trigger=0.0, framesize=2048.0) -> Ugen:
    """Convolves `source` with the samples in buffer `kernel` (channel 0, at
    most `framesize` of them), read at the start and again at each
    `trigger`, switching at the next frame. `framesize` (a power of two, a
    constant) is the latency; for a longer kernel use `part_conv`.
    """
    return Ugen("Convolution2", [source, kernel, trigger, framesize])


def convolution2_l(
    source,
    kernel,
    trigger=0.0,
    framesize=2048.0,
    crossfade=1.0,
) -> Ugen:
    """`convolution2`, fading to a kernel read at a `trigger` over `crossfade`
    frames instead of switching.
    """
    return Ugen(
        "Convolution2L",
        [source, kernel, trigger, framesize, crossfade],
    )


def convolution3(source, kernel, trigger=0.0, framesize=2048.0) -> Ugen:
    """Convolves `source` with the samples in buffer `kernel` (channel 0, at
    most `framesize` of them) directly in time: no latency, and a cost per
    sample of the kernel's length, so for short kernels. The kernel is read
    at the start and again at each `trigger`.
    """
    return Ugen("Convolution3", [source, kernel, trigger, framesize])
