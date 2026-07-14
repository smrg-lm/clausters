"""The random context: one seedable source for a whole script.

Everything random in a Clausters script — the random patterns (``Pwhite``,
``Prand``), these module functions, anything sequenced — draws from **one
context**, the sclang model, so a single root seed reproduces a piece from
beginning to end:

- ``main.seed(n)`` seeds the **root** generator (`clausters.base.main.Main.seed`).
- Every `Routine` (any `Stream`) derives its **own** generator from the context
  that creates it, at creation time (`RngStream.spawn` — the child's seed is the
  parent's next word). Deterministic: same root seed + same creation order =
  same streams, and concurrent routines (several clocks, RT next to NRT) stay
  reproducible **per routine** regardless of how their wakes interleave.
- A draw always uses the generator of the **routine running right now**
  (`current_rng`, via the thread-local ``main.current_tt``). Outside any routine
  it falls back to the **active session's** root — the explicit
  `clausters.Session` on this thread if any, else the default session
  (``main``) — so ``seed(n)`` on one session reproduces *its* material without
  touching another's.

The generator itself lives in the shared native core (one ``u64`` of state, the
same splitmix64/xorshift64 as the server's ``WhiteNoise``), so the same seed
replays the same values in every client language. There are no per-pattern
seeds: independent seeds would break whole-script consistency — override
*locally* by playing material inside its own routine instead.
"""

from .main import main


def current_rng():
    """The generator of the routine running on this thread; outside a routine,
    the root generator of the active session — the explicit `clausters.Session`
    on this thread (`clausters.base.main.Main.current_session`) if any, else the
    default session (``main.rng``). This is where every random value in the
    library comes from, and why each session reproduces independently."""
    tt = main.current_tt
    rng = getattr(tt, "rng", None)
    if rng is not None:
        return rng
    sess = main.current_session
    return sess.rng if sess is not None else main.rng


def spawn_rng():
    """A new generator derived from the current context (`RngStream.spawn`):
    how a `Routine` gets its own stream at creation, seeded by its parent."""
    return current_rng().spawn()


def next_f64() -> float:
    """Uniform in [0, 1) (53-bit resolution) from the current context."""
    return current_rng().next_f64()


def uniform(lo: float, hi: float) -> float:
    """Uniform in [lo, hi) from the current context (``lo`` when ``hi <= lo``)."""
    return current_rng().uniform(lo, hi)


def next_below(n: int) -> int:
    """Uniform integer in [0, n) from the current context (0 when ``n`` is 0)."""
    return current_rng().next_below(n)


def choice(items):
    """A uniformly chosen element of ``items`` from the current context."""
    return current_rng().choice(items)
