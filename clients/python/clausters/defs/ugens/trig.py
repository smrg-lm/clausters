"""Triggers and control flow.

A **trigger** is a signal crossing from <= 0 up to > 0 — one definition shared
by every function here, so the same crossing means the same thing whatever
produced it.
"""

from .graph import Ugen

# ---- triggers and control ----
#
# A **trigger** is a signal crossing from <= 0 up to > 0 — one definition,
# shared by every callable here and by `demand`, `send_trig` and friends. The
# kinds whose only inputs are triggers default to ``rate="kr"`` on the server,
# because a trigger is the only thing that can move them.


def trig(signal, dur=0.1) -> Ugen:
    """Holds the **level the input had at the trigger** for ``dur`` seconds,
    then 0. Its value carries information, which is what makes it a sampler as
    well as a gate; use `trig1` when all you want is a 1."""
    return Ugen("Trig", [signal, dur])


def trig1(signal, dur=0.1) -> Ugen:
    """Holds 1 for ``dur`` seconds after each trigger, whatever level triggered
    it."""
    return Ugen("Trig1", [signal, dur])


def t_delay(signal, dur=0.1) -> Ugen:
    """One sample of 1, ``dur`` seconds after each trigger. A trigger arriving
    while one is already in flight is **dropped**, not queued, so a burst
    cannot pile up."""
    return Ugen("TDelay", [signal, dur])


def latch(signal, trig=0.0) -> Ugen:
    """Sample and hold: takes one sample of ``signal`` at each rising edge of
    ``trig`` and holds it until the next one."""
    return Ugen("Latch", [signal, trig])


def gate(signal, trig=0.0) -> Ugen:
    """Passes ``signal`` while ``trig`` is above zero and **freezes** at the
    last value when it is not. Unlike `latch` it is transparent for as long as
    the gate is open."""
    return Ugen("Gate", [signal, trig])


def schmidt(signal, lo=0.0, hi=1.0) -> Ugen:
    """A comparator with hysteresis: 1 once ``signal`` rises past ``hi``, 0 once
    it falls past ``lo``, unchanged in between. The gap is what keeps a noisy
    input from chattering the way a plain ``signal > threshold`` would."""
    return Ugen("Schmidt", [signal, lo, hi])


def toggle_ff(trig=0.0) -> Ugen:
    """Flips between 0 and 1 on each trigger — a divider by two of the
    *triggers*, not of the signal."""
    return Ugen("ToggleFF", [trig])


def set_reset_ff(trig=0.0, reset=0.0) -> Ugen:
    """1 from the first ``trig``, 0 from the next ``reset``. Both on the same
    sample leaves it at 0: reset is applied second."""
    return Ugen("SetResetFF", [trig, reset])


def pulse_count(trig=0.0, reset=0.0) -> Ugen:
    """Counts triggers, from 1; a rising ``reset`` puts it back to 0."""
    return Ugen("PulseCount", [trig, reset])


def pulse_divider(trig=0.0, div=2.0, start=0.0) -> Ugen:
    """One trigger out for every ``div`` in. ``start`` is where the counter
    begins, read once — set it to ``div - 1`` to fire on the very first
    trigger, which is how two dividers are phased against each other."""
    return Ugen("PulseDivider", [trig, div, start])


def stepper(trig=0.0, reset=0.0, min=0.0, max=7.0, step=1.0, resetval=0.0) -> Ugen:
    """A counter that walks ``[min, max]`` — **both ends included** — one
    ``step`` per trigger, wrapping. It sits at ``resetval`` until the first
    trigger, which lands on ``resetval + step``: a stepper is defined by its
    transitions. A negative ``step`` walks the same ring backwards."""
    return Ugen("Stepper", [trig, reset, min, max, step, resetval])


def timer(trig=0.0) -> Ugen:
    """The time in seconds between the last two triggers, held between them
    (0 until there are two). The crossing is interpolated, so a period measured
    off a slow oscillator is not rounded to the sample."""
    return Ugen("Timer", [trig])


def sweep(trig=0.0, rate=1.0) -> Ugen:
    """A ramp rising at ``rate`` per second, restarted at each trigger. It is
    already running before the first one, so ``sweep(0, 1)`` is simply the
    node's age in seconds."""
    return Ugen("Sweep", [trig, rate])


def changed(signal, threshold=0.0) -> Ugen:
    """1 on any sample where ``signal`` moved by more than ``threshold``.

    It compares the **halved** difference, ``|(x[n] - x[n-1]) / 2|``, because
    sclang builds this out of ``HPZ1`` whose gain is 0.5 and a ported def must
    not change value: a step of 0.2 registers against 0.09, not against 0.19."""
    return Ugen("Changed", [signal, threshold])


def decay(signal, decaytime=1.0) -> Ugen:
    """Turns each impulse into an exponential falling 60 dB in ``decaytime``
    (``y[n] = x[n] + b·y[n-1]``). Its attack is instantaneous, which clicks —
    see `decay2`."""
    return Ugen("Decay", [signal, decaytime])


def decay2(signal, attacktime=0.01, decaytime=1.0) -> Ugen:
    """`decay` minus a second, faster decay, which rounds the attack. Its peak
    is lower than 1 and sits where the two exponentials' slopes match."""
    return Ugen("Decay2", [signal, attacktime, decaytime])
