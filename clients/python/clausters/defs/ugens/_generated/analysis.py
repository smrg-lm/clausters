"""The `analysis` builders that mirror the wire, re-exported by `analysis`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/analysis.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "spec_centroid",
    "spec_flatness",
    "spec_pcile",
    "loudness",
    "key_track",
    "onsets",
    "pv_hainsworth_foote",
    "pv_jensen_andersen",
]


def spec_centroid(chain) -> Ugen:
    """The spectrum's centre of mass in hertz: every bin's frequency weighted
    by its magnitude, DC included. Updated at each frame and held between
    them; silence reads 0. It reads the chain and leaves it as it was.
    """
    return Ugen("SpecCentroid", [chain])


def spec_flatness(chain) -> Ugen:
    """The geometric mean of the frame's magnitudes over their arithmetic mean:
    1 for white noise, towards 0 for a few partials (a bin of magnitude 0 is
    left out of the geometric mean). Silence reads 0.8, very quiet white
    noise.
    """
    return Ugen("SpecFlatness", [chain])


def spec_pcile(chain, fraction=0.5, interpolate=0.0, binout=0.0) -> Ugen:
    """The frequency below which `fraction` of the spectrum's magnitude lies,
    in hertz (0.5 the median, 0.95 a rolloff), or as a bin number when
    `binout > 0`. `interpolate > 0` places it between the two bins it falls
    between.
    """
    return Ugen("SpecPcile", [chain, fraction, interpolate, binout])


def loudness(chain, smask=0.25, tmask=1.0) -> Ugen:
    """Perceived loudness in sones: the power in 42 bands of about an ERB each
    (each bin masked by the one below at `smask` of its power), through the
    equal-loudness contours to phons, each band falling by at most `tmask`
    phons a frame, summed. The same through a chain of any size and window;
    silence reads 0.
    """
    return Ugen("Loudness", [chain, smask, tmask])


def key_track(chain, keydecay=2.0, chromaleak=0.5) -> Ugen:
    """The key the music is in: 0 to 11 the major keys from C, 12 to 23 the
    minor ones. The power at six partials of each note from A1 to G#6 makes
    a chroma (keeping `chromaleak` of the last frame's), matched against a
    major and a minor profile in every key; the matches' memory falls to 1%
    over `keydecay` seconds. A 4096-point chain resolves the low notes.
    """
    return Ugen("KeyTrack", [chain, keydecay, chromaleak])


def onsets(
    chain,
    threshold=0.5,
    odftype=3.0,
    relaxtime=1.0,
    floor=0.1,
    mingap=10.0,
    medianspan=11.0,
    whtype=1.0,
    rawodf=0.0,
) -> Ugen:
    """1 on the frames an onset starts in, held to the next frame. Each bin is
    divided by its recent peak (`whtype` 1; the peak falls by 60 dB in
    `relaxtime` seconds, and is at least `floor`), an onset detection
    function of the frame is taken (`odftype`: 0 power, 1 sum of magnitudes,
    2 complex domain, 3 complex domain over the rising bins, 4 phase
    deviation, 5 phase deviation weighted by magnitude, 6 modified
    Kullback-Leibler), the median of its last `medianspan` values is taken
    off, and an onset is where that rises past `threshold`, at least
    `mingap` frames after the last. `rawodf > 0` outputs the detection
    function instead. `odftype` and `medianspan` are constants.
    """
    return Ugen(
        "Onsets",
        [chain, threshold, odftype, relaxtime, floor, mingap, medianspan, whtype, rawodf],
    )


def pv_hainsworth_foote(
    chain,
    proph=0.0,
    propf=0.0,
    threshold=1.0,
    waittime=0.04,
) -> Ugen:
    """An audio-rate 1 over the slice of a frame where `proph` times
    Hainsworth's measure (the mean rise in `log2` magnitude between 30 Hz
    and 5 kHz) plus `propf` times Foote's (one minus the frame's cosine
    similarity with the last) passes `threshold`, then nothing for
    `waittime` seconds.
    """
    return Ugen(
        "PV_HainsworthFoote",
        [chain, proph, propf, threshold, waittime],
    )


def pv_jensen_andersen(
    chain,
    propsc=0.25,
    prophfe=0.25,
    prophfc=0.25,
    propsf=0.25,
    threshold=1.0,
    waittime=0.04,
) -> Ugen:
    """An audio-rate 1 over the slice of a frame where the weighted sum of the
    frame-to-frame changes in four features -- the spectral centroid
    (`propsc`), the energy above 4 kHz (`prophfe`), the high-frequency
    content (`prophfc`) and the spectral flux (`propsf`) -- passes
    `threshold`, then nothing for `waittime` seconds.
    """
    return Ugen(
        "PV_JensenAndersen",
        [chain, propsc, prophfe, prophfc, propsf, threshold, waittime],
    )
