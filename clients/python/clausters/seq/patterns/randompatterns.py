"""Value patterns of random values.

Every draw comes from the **random context** (the running routine's generator,
or the root outside one -- see `clausters.base.rand`): ``main.seed(n)``
reproduces them along with everything else in the script. There is no
per-pattern seed -- independent seeds would break whole-script consistency.
Each yields ``length`` values (endless by default), and a bound or a parameter
may itself be a pattern, read one value per step. The distributions are the
core's, drawn by the names `clausters.base.rand` gives them.
"""

from ...base import builtins, rand
from ...base.stream import StopStream
from ..pattern import INF, as_stream, pattern
from .valuepatterns import _counting


def _draws(inval, length, draw, *args):
    """``draw`` of the next value of each argument, ``length`` times."""
    streams = [as_stream(a) for a in args]
    try:
        for _ in _counting(length):
            values = [s.next(inval) for s in streams]
            inval = yield draw(*values)
    except StopStream:
        pass
    return inval


@pattern
def pwhite(inval, lo=0.0, hi=1.0, length=INF):
    """Uniform random numbers in ``[lo, hi)``."""
    return (yield from _draws(inval, length, rand.rrand, lo, hi))


@pattern
def pexprand(inval, lo=0.0001, hi=1.0, length=INF):
    """Exponentially distributed between ``lo`` and ``hi`` (same sign,
    nonzero): equal chances per octave, the distribution a frequency or a
    duration wants."""
    return (yield from _draws(inval, length, rand.exprand, lo, hi))


@pattern
def plprand(inval, lo=0.0, hi=1.0, length=INF):
    """Between ``lo`` and ``hi``, more likely toward ``lo``: the lower of two
    uniform draws."""
    return (yield from _draws(inval, length, rand.lprand, lo, hi))


@pattern
def phprand(inval, lo=0.0, hi=1.0, length=INF):
    """Between ``lo`` and ``hi``, more likely toward ``hi``: the higher of two
    uniform draws."""
    return (yield from _draws(inval, length, rand.hprand, lo, hi))


@pattern
def pmeanrand(inval, lo=0.0, hi=1.0, length=INF):
    """Between ``lo`` and ``hi``, more likely toward the middle: the mean of
    two uniform draws."""
    return (yield from _draws(inval, length, rand.meanrand, lo, hi))


@pattern
def pbeta(inval, lo=0.0, hi=1.0, prob1=1.0, prob2=1.0, length=INF):
    """The beta distribution between ``lo`` and ``hi``: ``prob1`` and
    ``prob2`` weigh the two ends -- below 1 a value gathers at that end, above
    1 it leaves it, and both 1 is uniform."""
    def draw(lo, hi, prob1, prob2):
        return rand.betarand(prob1, prob2) * (hi - lo) + lo
    return (yield from _draws(inval, length, draw, lo, hi, prob1, prob2))


@pattern
def pcauchy(inval, mean=0.0, spread=1.0, length=INF):
    """Cauchy around ``mean``: mostly near it, ``spread`` wide, with tails
    that now and then reach far."""
    return (yield from _draws(inval, length, rand.cauchy, mean, spread))


@pattern
def pgauss(inval, mean=0.0, dev=1.0, length=INF):
    """Gaussian around ``mean`` with standard deviation ``dev``."""
    return (yield from _draws(inval, length, rand.gauss, mean, dev))


@pattern
def ppoisson(inval, mean=1.0, length=INF):
    """A whole number of events, ``mean`` of them on average (Poisson): how
    many notes in a beat, how many voices enter."""
    return (yield from _draws(inval, length, rand.poisson, mean))


@pattern
def pbrown(inval, lo=0.0, hi=1.0, step=0.125, length=INF):
    """A random walk between ``lo`` and ``hi``: each value moves from the last
    by up to ``step`` either way, folded back at the bounds. It starts at a
    uniform draw between them."""
    return (yield from _walk(inval, lo, hi, step, length,
                             lambda value, step: value + rand.rand2(step)))


@pattern
def pgbrown(inval, lo=0.0, hi=1.0, step=0.125, length=INF):
    """`pbrown` by factors: each value is the last times up to ``1 + step``
    or ``1 - step`` -- the walk a frequency takes, even per octave."""
    return (yield from _walk(inval, lo, hi, step, length,
                             lambda value, step: value * (1 + rand.rand2(step))))


def _walk(inval, lo, hi, step, length, move):
    los, his, steps = as_stream(lo), as_stream(hi), as_stream(step)
    try:
        lo, hi, step = los.next(inval), his.next(inval), steps.next(inval)
        value = rand.rrand(lo, hi)
        for _ in _counting(length):
            lo, hi, step = los.next(inval), his.next(inval), steps.next(inval)
            value = builtins.fold(move(value, step), lo, hi)
            inval = yield value
    except StopStream:
        pass
    return inval


@pattern
def pprob(inval, distribution, lo=0.0, hi=1.0, table_size=None, length=INF):
    """Values between ``lo`` and ``hi`` drawn with ``distribution`` as their
    probability shape: a list of relative chances across the range, from
    ``lo`` to ``hi``, read as a table of ``table_size`` points (at least 64)."""
    size = table_size or max(64, len(distribution))
    table = builtins.asrandomtable(distribution, size)

    def draw(lo, hi):
        return rand.tablerand(table) * (hi - lo) + lo
    return (yield from _draws(inval, length, draw, lo, hi))
