"""The patterns, by family (port of ``sc3/seq/patterns``).

Each module is one family, and each pattern says what it yields and which kind
it is -- a **value pattern** (its stream yields values) or an **event pattern**
(its stream yields `clausters.seq.Event`s, and it plays) -- or, for a list or
filter pattern, how it decides when it is built (an event pattern when what it
holds is):

- `valuepatterns` -- values computed from the arguments: a constant, a value
  once, an arithmetic or geometric series.
- `randompatterns` -- random values, drawn from the random context, so one
  seed reproduces them with everything else in the script: uniform,
  exponential, toward an end or the middle, beta, Cauchy, Gaussian, Poisson,
  a random walk, a drawn probability shape.
- `listpatterns` -- values read from a list: in order, cycled, chosen at
  random.
- `filterpatterns` -- a pattern made of another: repeated.
- `funcpatterns` -- values a function computes when the walk asks.
- `eventpatterns` -- the patterns whose stream yields events: `pbind`.
- `oppatterns` -- what an operator composes: `punop`, `pbinop`, `pnarop`.

What a pattern is, and how one is written, is `clausters.seq.pattern`.
"""

from .eventpatterns import pbind
from .filterpatterns import pn
from .funcpatterns import pfunc
from .listpatterns import prand, pseq, pser
from .oppatterns import pbinop, pnarop, punop
from .randompatterns import (pbeta, pbrown, pcauchy, pexprand, pgauss,
                             pgbrown, phprand, plprand, pmeanrand, ppoisson,
                             pprob, pwhite)
from .valuepatterns import pconst, pgeom, pseries, pvalue

__all__ = [
    "pbeta", "pbind", "pbinop", "pbrown", "pcauchy", "pconst", "pexprand",
    "pfunc", "pgauss", "pgbrown", "pgeom", "phprand", "plprand", "pmeanrand",
    "pn", "pnarop", "ppoisson", "pprob", "prand", "pseq", "pser", "pseries",
    "punop", "pvalue", "pwhite",
]
