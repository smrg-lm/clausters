"""Filters, delays and smoothers: what a signal is put through.

One state-variable implementation stands behind every two-pole name, one delay
line behind the nine `delay`/`comb`/`allpass` forms (chosen by interpolation),
and the one-pole smoothers that make a control move instead of jump.
"""

from .graph import Ugen, _channel_binop, _channel_unop

# ---- filters ----------------------------------------------------------------
#
# One state-variable implementation stands behind every two-pole name; the row
# chooses the tap mix. Resonance travels on the wire as ``rq`` (the reciprocal
# of Q), which is scsynth's contract and the parameter with the clean domain:
# ``rq = 0`` is infinite Q, representable exactly, where ``Q = 0`` would divide
# by zero. Because ``rq`` is awkward to *think* in, each resonant builder also
# accepts ``q=``; a constant folds here at graph-build time and a signal
# composes one reciprocal, which is nothing next to the filter it feeds.


def _resonance(rq, q):
    """Resolves the mutually exclusive ``rq`` / ``q`` pair into a wire ``rq``."""
    if q is None:
        return 1.0 if rq is None else rq
    if rq is not None:
        raise TypeError("give either rq or q, not both")
    if isinstance(q, (int, float)):
        if q == 0:
            raise ValueError("q must be non-zero; use rq=0 for infinite Q")
        return 1.0 / q
    return _channel_unop(q, "recip")


def lpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth lowpass: -3 dB at ``freq``, -12 dB/octave."""
    return Ugen("LPF", [signal, freq])


def hpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth highpass: -3 dB at ``freq``, -12 dB/octave."""
    return Ugen("HPF", [signal, freq])


def rlpf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Resonant lowpass. Give the resonance as ``rq`` (1/Q, 0 = infinite) or
    as ``q``; unity gain at DC."""
    return Ugen("RLPF", [signal, freq, _resonance(rq, q)])


def rhpf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Resonant highpass; unity gain at Nyquist. Resonance as in `rlpf`."""
    return Ugen("RHPF", [signal, freq, _resonance(rq, q)])


def bpf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Bandpass with **unity gain at the centre**; ``rq`` is its bandwidth
    ratio. Resonance as in `rlpf`."""
    return Ugen("BPF", [signal, freq, _resonance(rq, q)])


def brf(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Band reject (notch); unity gain in both passbands, a true null at
    ``freq``. Resonance as in `rlpf`."""
    return Ugen("BRF", [signal, freq, _resonance(rq, q)])


def resonz(signal, freq=440.0, rq=None, *, q=None) -> Ugen:
    """Resonator with unity gain at the peak.

    The same structure and parameterization as `bpf` — sclang ships two
    historically distinct two-pole resonators that promise the same thing, and
    here one implementation carries both names.
    """
    return Ugen("Resonz", [signal, freq, _resonance(rq, q)])


def svf(signal, freq=440.0, rq=None, low=1.0, band=0.0, high=0.0, *,
        q=None) -> Ugen:
    """The state-variable filter with its three tap gains as **signal inputs**,
    so the response itself can be modulated.

    Every classic response is a mix of the three taps, and each of these is a
    valid argument triple:

    | response | ``low`` | ``band`` | ``high`` |
    |---|---|---|---|
    | lowpass | 1 | 0 | 0 |
    | bandpass (peak gain Q) | 0 | 1 | 0 |
    | bandpass (unity peak) | 0 | ``rq`` | 0 |
    | highpass | 0 | 0 | 1 |
    | notch | 1 | 0 | 1 |
    | peak | -1 | 0 | 1 |
    | allpass | 1 | ``-rq`` | 1 |

    Sweeping between them costs the mix and nothing else: the three taps come
    out of the same pair of integrator updates. See `svf_morph` for the
    one-knob version.
    """
    return Ugen("Svf", [signal, freq, _resonance(rq, q), low, band, high])


def svf_morph(pos):
    """The ``(low, band, high)`` gains for a continuous lowpass → bandpass →
    highpass sweep, to splat into `svf`: ``svf(sig, freq, rq, *svf_morph(p))``.

    ``pos`` runs 0 → 1 → 2 and may be a signal, so the response becomes an
    automation lane like any other. The ordering lives here rather than on the
    wire, where committing to one arbitrary sequence of responses would exclude
    every other (notch, peak, allpass are all reachable through `svf` itself).
    """
    def clamp01(x):
        return _channel_binop(_channel_binop(x, "max", 0.0), "min", 1.0)

    low = clamp01(_channel_binop(1.0, "sub", pos))
    high = clamp01(_channel_binop(pos, "sub", 1.0))
    # A triangle peaking at pos == 1: 1 - |pos - 1|.
    band = clamp01(
        _channel_binop(1.0, "sub", _channel_unop(_channel_binop(pos, "sub", 1.0), "abs"))
    )
    return low, band, high


def one_pole(signal, coef=0.5) -> Ugen:
    """``y[n] = (1-|coef|)·x[n] + coef·y[n-1]`` — lowpass for a positive
    coefficient, highpass for a negative one, unity in the passband.

    The parameter is the **pole**, not a cutoff, as in sclang. Use `lag` when
    what you want is a time constant.
    """
    return Ugen("OnePole", [signal, coef])


def one_zero(signal, coef=0.5) -> Ugen:
    """``y[n] = (1-|coef|)·x[n] + coef·x[n-1]`` — the zero-only sibling of
    `one_pole`."""
    return Ugen("OneZero", [signal, coef])


def leak_dc(signal, coef=0.995) -> Ugen:
    """Removes DC: a zero exactly at 0 Hz with a pole just inside it. The
    default corner is low enough to leave audio untouched."""
    return Ugen("LeakDC", [signal, coef])


def integrator(signal, coef=0.999) -> Ugen:
    """Leaky accumulator, ``y[n] = x[n] + coef·y[n-1]``. The coefficient is
    clamped just inside 1 on the server, so it always forgets eventually
    instead of running away on a DC input."""
    return Ugen("Integrator", [signal, coef])


# ---- delays -----------------------------------------------------------------
#
# One line implementation behind nine names, chosen by interpolation (``N``
# none, ``L`` linear, ``C`` cubic) and by feedback (none, comb, allpass).
#
# ``max_delay`` is **static**: it sizes the line the server allocates when the
# synth is built, so it cannot grow later and a `delaytime` past it is clamped.
# Left unset it follows a constant ``delaytime``, which is what you want for a
# fixed delay; a *modulated* delaytime has to state the longest it will reach.


def _line(kind, delaytime, max_delay):
    """The static ``max_delay`` field, defaulted from a constant delay time."""
    if max_delay is None:
        if not isinstance(delaytime, (int, float)):
            raise TypeError(
                f"{kind}: a modulated delaytime needs an explicit max_delay "
                "(it sizes the line, and the line is allocated once)"
            )
        max_delay = delaytime
    return {"max_delay": float(max_delay)}


def delay_n(signal, delaytime=0.2, *, max_delay=None) -> Ugen:
    """Pure delay, no interpolation: the delay is rounded to whole samples."""
    return Ugen("DelayN", [signal, delaytime],
                static=_line("DelayN", delaytime, max_delay))


def delay_l(signal, delaytime=0.2, *, max_delay=None) -> Ugen:
    """Pure delay with linear interpolation — a fractional delay, at the cost of
    a gentle lowpass that deepens toward Nyquist (about -1.6 dB at 9 kHz on a
    half-sample delay)."""
    return Ugen("DelayL", [signal, delaytime],
                static=_line("DelayL", delaytime, max_delay))


def delay_c(signal, delaytime=0.2, *, max_delay=None) -> Ugen:
    """Pure delay with four-point cubic interpolation — about -0.36 dB at 9 kHz
    where `delay_l` loses 1.6 dB. The one to modulate."""
    return Ugen("DelayC", [signal, delaytime],
                static=_line("DelayC", delaytime, max_delay))


def comb_n(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Feedback comb, no interpolation. ``decaytime`` is the time for the echo
    train to fall 60 dB **counting from the first echo**, which is the direct
    path and always comes back at full level. A negative decay time inverts
    alternate echoes; zero leaves a single echo."""
    return Ugen("CombN", [signal, delaytime, decaytime],
                static=_line("CombN", delaytime, max_delay))


def comb_l(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Feedback comb with linear interpolation. Decay as in `comb_n`."""
    return Ugen("CombL", [signal, delaytime, decaytime],
                static=_line("CombL", delaytime, max_delay))


def comb_c(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Feedback comb with cubic interpolation. Decay as in `comb_n`."""
    return Ugen("CombC", [signal, delaytime, decaytime],
                static=_line("CombC", delaytime, max_delay))


def allpass_n(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Schroeder allpass, no interpolation: the magnitude response is exactly
    flat and only the phase is shaped, which is what makes it the diffusion
    stage of a reverb. Decay as in `comb_n`."""
    return Ugen("AllpassN", [signal, delaytime, decaytime],
                static=_line("AllpassN", delaytime, max_delay))


def allpass_l(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Schroeder allpass with linear interpolation."""
    return Ugen("AllpassL", [signal, delaytime, decaytime],
                static=_line("AllpassL", delaytime, max_delay))


def allpass_c(signal, delaytime=0.2, decaytime=1.0, *, max_delay=None) -> Ugen:
    """Schroeder allpass with cubic interpolation — the one to modulate."""
    return Ugen("AllpassC", [signal, delaytime, decaytime],
                static=_line("AllpassC", delaytime, max_delay))

# ---- one-pole smoothers ----


def lag(signal, time=0.1) -> Ugen:
    """One-pole smoother: ``signal`` lagged over ``time`` seconds (symmetric);
    ``time`` 0 passes through. The same UGen the server inserts for a lagged
    control -- use it directly to smooth any signal."""
    return Ugen("Lag", [signal, time])


def var_lag(signal, up=0.1, down=0.1) -> Ugen:
    """One-pole smoother with separate rise (``up``) and fall (``down``) times."""
    return Ugen("VarLag", [signal, up, down])
