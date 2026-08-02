"""Panning, the stereo field and selection.

A UGen has one output, so every function here that produces two channels
returns a `ChannelList` built from single-output nodes — the package's
multichannel rule, applied to the stereo primitives.
"""

from .graph import ChannelList, Ugen, chans, mix

# ---- panning, the stereo field and selection --------------------------------
# A UGen has one output, so every row that produces two channels is built
# twice, once per channel index, and returned as a `ChannelList` — the same
# container `dup` builds and `out` lays on consecutive buses. The index is the
# builder's business: it is always the last input, and never an argument here.


def pan2(signal, pos=0.0, level=1.0) -> ChannelList:
    """Places a mono ``signal`` between two channels at ``pos`` (−1 left, 0
    centre, 1 right), at **equal power**: the two gains hold ``l² + r² = 1``, so
    a source keeps one loudness as it crosses the field. The price is that the
    centre is 0.707 in each channel, not 1 — use `lin_pan2` when it is the
    summed amplitude that has to stay put.

    Out of range the position clamps rather than wrapping. Returns the two
    channels: ``out(0, pan2(sine(440), pos))``."""
    return chans(*(Ugen("Pan2", [signal, pos, level, c]) for c in (0.0, 1.0)))


def lin_pan2(signal, pos=0.0, level=1.0) -> ChannelList:
    """`pan2` with the **constant-amplitude** law: the two gains sum to
    ``level`` at every position, 0.5 each at the centre. What a mono fold-down
    wants; it dips 3 dB in the middle for anything that sums by power."""
    return chans(*(Ugen("LinPan2", [signal, pos, level, c]) for c in (0.0, 1.0)))


def balance2(left, right, pos=0.0, level=1.0) -> ChannelList:
    """Shifts an **already stereo** pair towards one side by attenuating the
    other, at equal power. ``pos=-1`` leaves the left input alone and silences
    the right one.

    Note that a centred `balance2` is not a pass-through: both sides come back
    at 0.707, 3 dB down. That is scsynth's behaviour, and the reason to reach
    for it only when something is actually being balanced."""
    return chans(*(Ugen("Balance2", [left, right, pos, level, c])
                   for c in (0.0, 1.0)))


def rotate2(x, y, pos=0.0) -> ChannelList:
    """Rotates the plane the two signals span by ``pos`` **half turns** (0.25 is
    45°, 1 is a half turn). On a stereo pair it turns the image without
    changing its size or its level — the rotation is equal power at every angle.

    At a quarter turn the rotation *is* the change of basis between left/right
    and mid/side, which is what `mid_side` names directly. To move an image
    rather than resize it, this is the tool; for the size, see `stereo_width`."""
    return chans(*(Ugen("Rotate2", [x, y, pos, c]) for c in (0.0, 1.0)))


def mid_side(a, b) -> ChannelList:
    """The mid/side matrix, normalized so it is **its own inverse**: the same
    call encodes ``(left, right)`` into ``(mid, side)`` and decodes it back.

    Its point is what you can do in between — treat the centre and the sides of
    a mix as separate signals::

        m, s = mid_side(left, right)
        left2, right2 = mid_side(lpf(m, 400), s * 1.5)

    A mono pair has no side at all (exactly zero). The normalization is
    ``1/√2`` rather than the ``1/2`` a DAW meter shows, which is what makes the
    round trip exact; it puts the mid 3 dB above the convention, a plain gain.
    For a width knob and nothing in between, `stereo_width` is one row instead
    of two."""
    return chans(*(Ugen("MidSide", [a, b, c]) for c in (0.0, 1.0)))


def stereo_width(left, right, width=1.0) -> ChannelList:
    """Widens or narrows a stereo image by scaling its side component: ``0``
    collapses to mono, ``1`` is exactly the identity, ``2`` is the textbook
    widening, and a negative width swaps the sides.

    The same thing `mid_side` does in two steps, in one row. Note what widening
    does **not** do: it leaves the mono sum exactly where it was, because only
    the side component is scaled and the mid is what survives a fold-down. So
    every dB it adds to a channel is a dB a mono listener never hears — which is
    the real cost of pushing it past 1, and the reason to check a fold-down
    afterwards."""
    return chans(*(Ugen("StereoWidth", [left, right, width, c]) for c in (0.0, 1.0)))


def pan_az(numchans, signal, pos=0.0, level=1.0, width=2.0,
           orientation=0.5) -> ChannelList:
    """Places a mono ``signal`` on a **ring** of ``numchans`` channels. ``pos``
    spans the whole ring over ``[-1, 1]``, so −1 and 1 are the same place.

    Each channel gets a raised sine lobe ``width`` channels wide, centred on the
    source: at the default width of two, neighbouring channels hold equal power
    between them and a source parked on a channel is exactly unity there.
    Narrower leaves gaps, wider spreads into more channels at once.
    ``orientation`` turns the ring itself — 0.5, the default, puts the origin
    between two channels, which is what an even ring wants; use 0 to put a
    channel at the front.

    Returns ``numchans`` channels: ``out(0, pan_az(4, sig, pos))``."""
    if isinstance(numchans, bool) or not isinstance(numchans, int) or numchans < 1:
        raise ValueError(f"pan_az needs a channel count of at least 1, got {numchans!r}")
    return ChannelList([
        Ugen("PanAz", [signal, pos, level, width, orientation,
                       float(numchans), float(c)])
        for c in range(numchans)
    ])


def xfade2(a, b, pan=0.0, level=1.0) -> Ugen:
    """Equal-power crossfade between two signals: −1 is all ``a``, 1 is all
    ``b``, and the two gains hold unit power in between — which keeps
    *uncorrelated* material at one loudness across the fade, and lifts
    correlated material by 3 dB in the middle. Use `lin_xfade2` when the two
    sides are the same signal."""
    return Ugen("XFade2", [a, b, pan, level])


def lin_xfade2(a, b, pan=0.0, level=1.0) -> Ugen:
    """Crossfade with the constant-amplitude law — a plain interpolation, half
    of each at the centre. The right one for correlated sources."""
    return Ugen("LinXFade2", [a, b, pan, level])


def select(which, *sources) -> Ugen:
    """Outputs one of ``sources``, chosen by the ``which`` index (truncated,
    and clamped to the ends rather than wrapping). At audio rate the choice is
    made per sample.

    Every source runs whether or not it is selected — they are UGens in the
    graph, not branches — so this picks what is *heard*, never what is
    computed. Accepts the sources as arguments or as one list."""
    return Ugen("Select", [which, *_sources(sources)])


def select_x(which, *sources) -> Ugen:
    """`select` with the index's fraction crossfading to the next source, at
    equal power: ``which=0.5`` is halfway between the first two. A whole index
    lands on its source exactly.

    Off the ends the index clamps, like `select`'s. sclang's pseudo-UGen instead
    folds the crossfade while clipping its two picks, so there a negative index
    gives a *mix of the first two* sources and an index past the end gives the
    last one at 1.414 — worth knowing when porting a def that lets the index
    run out of range."""
    return Ugen("SelectX", [which, *_sources(sources)])


def _sources(sources):
    """The sources of a selector, given as arguments or as one list."""
    if len(sources) == 1 and isinstance(sources[0], (ChannelList, list, tuple)):
        sources = ChannelList(sources[0]).items
    if not sources:
        raise ValueError("a selector needs at least one source")
    return list(sources)


def splay(signals, spread=1.0, level=1.0, center=0.0) -> ChannelList:
    """Spreads ``signals`` evenly across the stereo field and mixes them down to
    two channels — one `pan2` per signal, summed.

    A client-side convenience, not a UGen: the first signal lands at
    ``center - spread``, the last at ``center + spread``, and a single signal
    lands at ``center``. Scale ``level`` yourself if the sum is too hot; unlike
    sclang's, this one does not normalize behind your back."""
    items = ChannelList(signals).items
    n = len(items)
    span = [0.0] if n == 1 else [i / (n - 1) * 2.0 - 1.0 for i in range(n)]
    panned = [pan2(s, center + p * spread, level) for s, p in zip(items, span)]
    # Mix each side down separately, so the fold uses the fused sums instead of
    # an Add chain per channel.
    return chans(mix([p[0] for p in panned]), mix([p[1] for p in panned]))
