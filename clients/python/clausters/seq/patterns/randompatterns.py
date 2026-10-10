"""Value patterns of random values.

Every draw comes from the **random context** (the running routine's generator,
or the root outside one -- see `clausters.base.rand`): ``main.seed(n)``
reproduces them along with everything else in the script. There is no
per-pattern seed -- independent seeds would break whole-script consistency.
"""

from ...base import rand
from ..pattern import INF, pattern
from .valuepatterns import _counting


@pattern
def pwhite(inval, lo=0.0, hi=1.0, length=INF):
    """Uniform random numbers in ``[lo, hi)``, ``length`` values."""
    for _ in _counting(length):
        inval = yield rand.rrand(lo, hi)
    return inval
