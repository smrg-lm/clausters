"""The random context: one seedable source for a whole script.

Everything random in a Clausters script -- the random patterns (``pwhite``,
``prand``), these module functions, anything sequenced -- draws from **one
context**, the sclang model, so a single root seed reproduces a take from
beginning to end:

- ``main.seed(n)`` seeds the **root** generator (`clausters.base.main.Main.seed`).
- Every `Routine` (any `Stream`) derives its **own** generator from the context
  that creates it, at creation time (`Rng.spawn` -- the child's seed is the
  parent's next word). Deterministic: same root seed + same creation order =
  same streams, and concurrent routines (several clocks, RT next to NRT) stay
  reproducible **per routine** regardless of how their wakes interleave.
- A draw always uses the generator of the **routine running right now**
  (`current_rng`, via the thread-local ``main.current_routine``). Outside any routine
  it falls back to the **active session's** root -- the explicit
  `clausters.Session` on this thread if any, else the default session
  (``main``) -- so ``seed(n)`` on one session reproduces *its* own sound without
  touching another's.

The generator itself lives in the shared native core (one ``u64`` of state, the
same splitmix64/xorshift64 as the server's ``WhiteNoise``), so the same seed
replays the same values in every client language. There are no per-pattern
seeds: independent seeds would break whole-script consistency -- override
*locally* by playing inside its own routine instead.
"""

from .. import _native
from .main import main

#: The value stream itself -- one ``u64`` of state over the shared core, the
#: same type in every client (`clausters._native.Rng`). You rarely build one:
#: the context hands you the right stream through `current_rng`.
Rng = _native.Rng


def seed(value: int) -> None:
    """Seed the **default** session's root generator (``main.seed``).

    The free spelling of the verb, for a script that names no session. A named
    `clausters.Session` is its own random context: ``session.seed(n)``
    reproduces *that* session's own sound without touching another's, which is
    what lets two sessions in one script stay reproducible independently.
    """
    main.seed(value)


def current_rng():
    """The generator of the routine running on this thread; outside a routine,
    the root generator of the active session -- the explicit `clausters.Session`
    on this thread (`clausters.base.main.Main.current_session`) if any, else the
    default session (``main.rng``). This is where every random value in the
    library comes from, and why each session reproduces independently."""
    tt = main.current_routine
    rng = getattr(tt, "rng", None)
    if rng is not None:
        return rng
    sess = main.current_session
    return sess.rng if sess is not None else main.rng


def spawn_rng():
    """A new generator derived from the current context (`Rng.spawn`):
    how a `Routine` gets its own stream at creation, seeded by its parent."""
    return current_rng().spawn()


# ---- the draws, from the current context ----
#
# sclang's names for its distributions. Each draws from `current_rng`, so one
# root seed reproduces every value. The integer draws are their own functions
# (`irand`, `irand2`, `irrand`) -- see `Rng` for why.

def rand(hi: float = 1.0) -> float:
    """Uniform in ``[0, hi)``."""
    return current_rng().rand(hi)


def rand2(x: float = 1.0) -> float:
    """Uniform in ``[-x, x)``."""
    return current_rng().rand2(x)


def rrand(lo: float, hi: float) -> float:
    """Uniform between ``lo`` and ``hi``."""
    return current_rng().rrand(lo, hi)


def exprand(lo: float, hi: float) -> float:
    """Exponentially distributed between ``lo`` and ``hi`` (same sign,
    nonzero): equal chances per octave -- what a frequency or a duration
    wants."""
    return current_rng().exprand(lo, hi)


def linrand(x: float = 1.0) -> float:
    """In ``[0, x)``, more likely toward 0."""
    return current_rng().linrand(x)


def bilinrand(x: float = 1.0) -> float:
    """In ``(-x, x)``, more likely toward 0."""
    return current_rng().bilinrand(x)


def sum3rand(x: float = 1.0) -> float:
    """In ``(-x, x)``, bell-shaped: three uniforms summed, sclang's loose
    Gaussian."""
    return current_rng().sum3rand(x)


def coin(p: float = 0.5) -> bool:
    """``True`` with probability ``p``."""
    return current_rng().coin(p)


def gauss(mean: float = 0.0, dev: float = 1.0) -> float:
    """Gaussian with ``mean`` and standard deviation ``dev``."""
    return current_rng().gauss(mean, dev)


def lprand(lo: float, hi: float) -> float:
    """The lower of two uniform draws between ``lo`` and ``hi``: more likely
    toward ``lo``."""
    return current_rng().lprand(lo, hi)

def hprand(lo: float, hi: float) -> float:
    """The higher of two uniform draws between ``lo`` and ``hi``: more likely
    toward ``hi``."""
    return current_rng().hprand(lo, hi)

def meanrand(lo: float, hi: float) -> float:
    """The mean of two uniform draws between ``lo`` and ``hi``: more likely
    toward the middle."""
    return current_rng().meanrand(lo, hi)

def betarand(prob1: float, prob2: float) -> float:
    """The beta distribution in ``[0, 1]``: ``prob1`` and ``prob2`` weigh its
    two ends (below 1 pulls toward an end, above 1 away from it)."""
    return current_rng().betarand(prob1, prob2)

def cauchy(mean: float = 0.0, spread: float = 1.0) -> float:
    """Cauchy around ``mean``, ``spread`` wide: a bell with tails that reach
    far."""
    return current_rng().cauchy(mean, spread)

def poisson(mean: float = 1.0) -> int:
    """A whole number of events, ``mean`` of them on average (Poisson)."""
    return current_rng().poisson(mean)


def irand(n: int) -> int:
    """A whole number in ``[0, n)``."""
    return current_rng().irand(n)


def irand2(n: int) -> int:
    """A whole number in ``[-n, n]``, both ends included."""
    return current_rng().irand2(n)


def irrand(lo: int, hi: int) -> int:
    """A whole number between ``lo`` and ``hi``, both ends included."""
    return current_rng().irrand(lo, hi)


def choose(items):
    """A uniformly chosen element of ``items``."""
    return current_rng().choose(items)


def wchoose(items, weights):
    """An element of ``items`` chosen with the relative ``weights``."""
    return current_rng().wchoose(items, weights)


def tablerand(table):
    """One draw shaped by ``table`` -- what `clausters.builtins.asrandomtable`
    builds from a probability shape."""
    return current_rng().tablerand(table)


def scramble(items):
    """``items`` in a uniformly random order (a new list)."""
    return current_rng().scramble(items)


def sputter(items, probability=0.25, maxlen=100):
    """``items`` read in order, each one repeated while a ``probability`` coin
    lands, until they are read through or ``maxlen`` values are out --
    sclang's ``sputter``: a stammer over a list."""
    return current_rng().sputter(items, probability, maxlen)
