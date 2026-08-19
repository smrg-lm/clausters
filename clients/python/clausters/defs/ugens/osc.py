"""Oscillators and noise: what a graph starts from.

The band-limited pair (`saw`, `pulse`) beside the naive `lf_*` forms, the noise
family by spectrum, and `phasor` as the ramp the table readers run on.
"""

from .graph import Ugen

def sine(freq=440.0) -> Ugen:
    """Sine by f64 phase accumulation, starting at phase 0."""
    return Ugen("Sine", [freq])


def impulse(freq=1.0) -> Ugen:
    """A single-sample ``1.0`` every ``freq`` Hz, ``0.0`` between (``freq`` 0 =
    one impulse then silence). The first sample is always an impulse."""
    return Ugen("Impulse", [freq])


def white_noise() -> Ugen:
    """Uniform white noise in ±1."""
    return Ugen("WhiteNoise", [])


def pink_noise() -> Ugen:
    """Equal energy per octave, −3 dB/octave (Voss–McCartney).

    A **quiet** signal by construction — around 0.13 RMS against white noise's
    0.58, which is the level a def ported from sclang expects. Scale it up
    rather than assuming something is wrong."""
    return Ugen("PinkNoise", [])


def brown_noise() -> Ugen:
    """A random walk, −6 dB/octave. It **reflects** at ±1 rather than clamping,
    so it never rests against a rail."""
    return Ugen("BrownNoise", [])


def gray_noise() -> Ugen:
    """One randomly chosen bit of a 31-bit word flipped per sample.

    Not white noise with a twist: its spectrum leans low (about −2.9 dB/octave)
    and its steps span every order of magnitude, which is what makes it sound
    grainy rather than smooth."""
    return Ugen("GrayNoise", [])


def clip_noise() -> Ugen:
    """−1 or 1, nothing between — a coin flip per sample. The loudest noise
    available at a given peak, since every sample is at full scale."""
    return Ugen("ClipNoise", [])


def lf_noise0(freq=500.0) -> Ugen:
    """A new random value in ±1 every ``1/freq`` seconds, **held** — steps.
    Not band limited: like the `lf_saw` family it is a modulation shape."""
    return Ugen("LFNoise0", [freq])


def lf_noise1(freq=500.0) -> Ugen:
    """`lf_noise0` with a linear ramp between values, so it has corners but no
    jumps."""
    return Ugen("LFNoise1", [freq])


def lf_noise2(freq=500.0) -> Ugen:
    """`lf_noise0` with a quadratic between values, so the slope is continuous
    too and there are no corners either.

    It aims at the midpoints between draws and carries its slope across them,
    so it **overshoots**: the output reaches about ±1.7, not ±1. scsynth's does
    the same."""
    return Ugen("LFNoise2", [freq])


def lf_clip_noise(freq=500.0) -> Ugen:
    """`lf_noise0` restricted to ±1 — a random square."""
    return Ugen("LFClipNoise", [freq])


def dust(density=1.0) -> Ugen:
    """Random impulses in [0, 1) at a **mean** ``density`` per second.

    Every sample is an independent trial, so the gaps are exponential: this is
    not a clock. Ten per second means ten on average, with clusters and silences
    — use `impulse` when you want them evenly spaced. The amplitudes are random
    too, which matters if you feed it to something that cares."""
    return Ugen("Dust", [density])


def dust2(density=1.0) -> Ugen:
    """`dust` firing both ways, in ±1."""
    return Ugen("Dust2", [density])


def crackle(chaos=1.5) -> Ugen:
    """The chaotic map ``y[n] = |chaos·y[n-1] − y[n-2] − 0.05|``.

    It has no RNG: the same ``chaos`` always gives the same signal, so it is
    reproducible without a seed. The parameter changes the sound drastically and
    **not** monotonically, so reach for it by ear. The output is one-sided and
    carries DC — put a `leak_dc` after it before summing it into a bus."""
    return Ugen("Crackle", [chaos])


def saw(freq=440.0) -> Ugen:
    """Band-limited rising sawtooth in ±1, starting at 0.

    Anti-aliased with PolyBLEP, which is very clean over the low and middle
    range and progressively less so toward Nyquist (its residual grows about
    as the square of the frequency). It carries no DC offset.
    """
    return Ugen("Saw", [freq])


def pulse(freq=440.0, width=0.5) -> Ugen:
    """Band-limited pulse in ±1; ``width`` is the duty cycle (0.5 = square).

    Anti-aliased like `saw`. The width is clamped just inside ``(0, 1)``,
    where the two edges would coincide.
    """
    return Ugen("Pulse", [freq, width])


def lf_saw(freq=440.0, iphase=0.0) -> Ugen:
    """Rising sawtooth in ±1, **not** band-limited — a modulation shape.

    ``iphase`` is the initial phase in **cycles**, ``[0, 1)``, read once at the
    first sample. (sclang measures the same argument in ``[0, 2)``; every phase
    in this client is in cycles.)
    """
    return Ugen("LFSaw", [freq, iphase])


def lf_pulse(freq=440.0, iphase=0.0, width=0.5) -> Ugen:
    """Square in ``[0, 1]`` — a gate, not a bipolar waveform like `pulse` — with
    ``width`` as its duty cycle. Not band-limited. ``iphase`` as in `lf_saw`."""
    return Ugen("LFPulse", [freq, iphase, width])


def lf_tri(freq=440.0, iphase=0.0) -> Ugen:
    """Triangle in ±1, starting at 0 and rising. Not band-limited.
    ``iphase`` as in `lf_saw`."""
    return Ugen("LFTri", [freq, iphase])


def var_saw(freq=440.0, iphase=0.0, width=0.5) -> Ugen:
    """Triangle whose peak sits at ``width`` of the cycle, in ±1: sweeps from a
    falling ramp through a triangle to a rising one. Not band-limited.
    ``iphase`` as in `lf_saw`."""
    return Ugen("VarSaw", [freq, iphase, width])


def phasor(trig=0.0, rate=1.0, start=0.0, end=1.0, reset_pos=0.0) -> Ugen:
    """Ramp from ``start`` to ``end`` advancing by ``rate`` **per sample**,
    wrapping at ``end``; a rising ``trig`` jumps to ``reset_pos``.

    ``rate`` is in output units per sample, not Hz, which is what makes this the
    index source for a buffer reader: a rate of 1 advances one frame per sample.
    """
    return Ugen("Phasor", [trig, rate, start, end, reset_pos])


def transport_pos(offset=0.0) -> Ugen:
    """The **transport's position in the piece**, in frames, minus ``offset``.

    A buffer reader whose phase is this one follows the transport instead of
    carrying a position of its own, so seeking
    (`Server.transport_locate_sample`), looping (`Server.transport_loop`) and
    pausing (`Server.transport_stop` over a governed group) belong to the
    transport and not to the def. That is the shape a multitrack needs — many
    readers, one time — and it is why a locate never has to reach into a node.

    It ramps one frame per sample while the transport rolls and holds while it
    is stopped. ``offset`` is where this signal starts in the piece, so a clip
    reads its own frame 0 when the transport reaches it; the subtraction happens
    in double precision inside the UGen, which is what keeps the value exact
    deep into a long piece (a signal is 32-bit, and past about six minutes at
    48 kHz it can no longer count single frames — subtracting afterwards with
    `sub` has already lost that).

    ```python
    take = buf_rd(buf.bufnum, chan=0, phase=transport_pos())
    ```
    """
    return Ugen("TransportPos", [offset])
