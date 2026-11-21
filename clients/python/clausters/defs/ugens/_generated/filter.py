"""The `filter` builders that mirror the wire, re-exported by `filter`.

Generated from the server's UGen registry and
`crates/clausters-server/src/dsp/registry/builders/filter.toml` by
`crates/clausters-server/tests/ugen_builders.rs`. Do not edit: change the
description and regenerate with
`CLAUSTERS_BLESS=1 cargo test -p clausters --test ugen_builders`.
"""

from ..graph import Ugen

__all__ = [
    "lpf",
    "hpf",
    "one_pole",
    "one_zero",
    "leak_dc",
    "integrator",
    "lag",
    "lag_ud",
    "buf_delay_n",
    "buf_delay_l",
    "buf_delay_c",
    "buf_comb_n",
    "buf_comb_l",
    "buf_comb_c",
    "buf_allpass_n",
    "buf_allpass_l",
    "buf_allpass_c",
]


def lpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth lowpass: -3 dB at `freq`, -12 dB/octave."""
    return Ugen("LPF", [signal, freq])


def hpf(signal, freq=440.0) -> Ugen:
    """Second-order Butterworth highpass: -3 dB at `freq`, -12 dB/octave."""
    return Ugen("HPF", [signal, freq])


def one_pole(signal, coef=0.5) -> Ugen:
    """`y[n] = (1-|coef|)*x[n] + coef*y[n-1]` -- lowpass for a positive
    coefficient, highpass for a negative one, unity in the passband.

    The parameter is the **pole**, not a cutoff, as in sclang. Use `lag` when
    what you want is a time constant.
    """
    return Ugen("OnePole", [signal, coef])


def one_zero(signal, coef=0.5) -> Ugen:
    """`y[n] = (1-|coef|)*x[n] + coef*x[n-1]` -- the zero-only sibling of
    `one_pole`.
    """
    return Ugen("OneZero", [signal, coef])


def leak_dc(signal, coef=0.995) -> Ugen:
    """Removes DC: a zero exactly at 0 Hz with a pole just inside it. The
    default corner is low enough to leave audio untouched.
    """
    return Ugen("LeakDC", [signal, coef])


def integrator(signal, coef=0.999) -> Ugen:
    """Leaky accumulator, `y[n] = x[n] + coef*y[n-1]`. The coefficient is
    clamped just inside 1 on the server, so it always forgets eventually
    instead of running away on a DC input.
    """
    return Ugen("Integrator", [signal, coef])


def lag(signal, time=0.1) -> Ugen:
    """One-pole smoother: `signal` lagged over `time` seconds (symmetric);
    `time` 0 passes through. The same UGen the server inserts for a lagged
    control -- use it directly to smooth any signal.
    """
    return Ugen("Lag", [signal, time])


def lag_ud(signal, up=0.1, down=0.1) -> Ugen:
    """One-pole smoother with separate rise (`up`) and fall (`down`) times: the
    UGen a control with a separate downward lag inserts.
    """
    return Ugen("LagUD", [signal, up, down])


def buf_delay_n(bufnum, chan, signal, delaytime=0.2) -> Ugen:
    """`delay_n` over a channel of a pool buffer."""
    return Ugen("BufDelayN", [bufnum, chan, signal, delaytime])


def buf_delay_l(bufnum, chan, signal, delaytime=0.2) -> Ugen:
    """`delay_l` over a channel of a pool buffer."""
    return Ugen("BufDelayL", [bufnum, chan, signal, delaytime])


def buf_delay_c(bufnum, chan, signal, delaytime=0.2) -> Ugen:
    """`delay_c` over a channel of a pool buffer."""
    return Ugen("BufDelayC", [bufnum, chan, signal, delaytime])


def buf_comb_n(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`comb_n` over a channel of a pool buffer."""
    return Ugen("BufCombN", [bufnum, chan, signal, delaytime, decaytime])


def buf_comb_l(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`comb_l` over a channel of a pool buffer."""
    return Ugen("BufCombL", [bufnum, chan, signal, delaytime, decaytime])


def buf_comb_c(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`comb_c` over a channel of a pool buffer."""
    return Ugen("BufCombC", [bufnum, chan, signal, delaytime, decaytime])


def buf_allpass_n(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`allpass_n` over a channel of a pool buffer."""
    return Ugen("BufAllpassN", [bufnum, chan, signal, delaytime, decaytime])


def buf_allpass_l(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`allpass_l` over a channel of a pool buffer."""
    return Ugen("BufAllpassL", [bufnum, chan, signal, delaytime, decaytime])


def buf_allpass_c(bufnum, chan, signal, delaytime=0.2, decaytime=1.0) -> Ugen:
    """`allpass_c` over a channel of a pool buffer."""
    return Ugen("BufAllpassC", [bufnum, chan, signal, delaytime, decaytime])
