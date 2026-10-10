#!/usr/bin/env python3
"""Generate pattern-vectors.json from the Python client's patterns.

The Python client is the reference; this script freezes what each pattern
**yields** -- the values a walk gives, in order, for the same arguments -- so
`tests/pattern-parity.test.ts` can build the same patterns in the TS client and
assert the same values. A case that draws seeds the random context first, so the
draws are asserted too: both clients draw from the same core generator.

Each case is a name and the pattern it builds; the TS test holds a table with
the same names, and a name missing on either side fails there.

The JSON is committed; regenerate with:

    python3 gen-pattern-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's
.venv has it installed editable).
"""

import itertools
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.base import builtins as b  # noqa: E402
from clausters.base.main import main  # noqa: E402
from clausters.seq import (pavaroh, pbind, pclump, pcollect,  # noqa: E402
                           pdelta, pdiff, pdrop, pdur, pflatten, pgate,
                           platch, plen, pprorate, preject, pselect,
                           pstutter, ptrace, pwhile, pwrap)
from clausters.seq import (pfuncn, pif, plazy, pproduct,  # noqa: E402
                           prout)
from clausters.seq import (pdfsm, pfsm, place, placep, pshuffle,  # noqa: E402
                           pslide, pswitch, pswitch1, ptuple, pwalk, pwrand,
                           pxrand)
from clausters.seq import (pbeta, pbrown, pcauchy, pconst,  # noqa: E402
                           pexprand, pfunc, pgauss, pgbrown, pgeom, phprand,
                           plprand, pmeanrand, pn, pnarop, ppoisson, pprob,
                           prand, pseq, pser, pseries, pvalue, pwhite)

#: How many values an endless case is read for.
TAKE = 8

def _counted(inval):
    """`prout`'s body: three tens."""
    for i in range(3):
        inval = yield i * 10


def _twice():
    """A condition true twice, then false: `pwhile`'s test."""
    count = [0]

    def test():
        count[0] += 1
        return count[0] <= 2
    return test


#: name -> (seed or None, the pattern). Mirrored by name in the TS test.
CASES = {
    "pseq": (None, lambda: pseq([1, 2, 3], 2)),
    "pseq_nested": (None, lambda: pseq([1, pseq([8, 9]), 2])),
    "pseq_endless": (None, lambda: pseq([1, 2], float("inf"))),
    "pser": (None, lambda: pser([1, 2, 3], 5)),
    "pconst": (None, lambda: pconst(7, 3)),
    "pseries": (None, lambda: pseries(0, 2.5, 4)),
    "pgeom": (None, lambda: pgeom(1, 3, 5)),
    "pn": (None, lambda: pn(pseq([1, 2]), 3)),
    "pfunc": (None, lambda: pfunc(lambda: 4, 3)),
    "pwhite": (11, lambda: pwhite(-1, 1, 6)),
    "prand": (11, lambda: prand([10, 20, 30], 6)),
    "add": (None, lambda: pseq([74, 60, 46]) + 2),
    "mul_shorter": (None, lambda: pseq([1, 2, 3]) * pseq([10, 20])),
    "ops_chain": (None, lambda: ((pseries(0, 1, 6) * 7) % 12).max(3)),
    "midicps": (None, lambda: pseq([57, 69, 81]).midicps()),
    "linlin": (None, lambda: pnarop("linlin", pseq([0, 0.25, 1, 2]), 0, 1, 10, 20)),
    "clip": (None, lambda: pnarop("clip", pseq([-2, 0.5, 3]), 0, 1)),
    "random_ops": (7, lambda: (pwhite(0, 1, 6) * 100).floor()),
    "linlin_method": (None, lambda: pseq([0, 0.5, 1, 2]).linlin(0, 1, 200, 800, clip="max")),
    "lincurve_method": (None, lambda: pseq([0, 0.25, 0.5, 1]).lincurve(0, 1, 0, 10, -4)),
    "fold_method": (None, lambda: pseq([-0.5, 1.5, 2.25]).fold(0, 1)),
    "blend": (None, lambda: pseq([0, 10, 20]).blend(pseq([100, 50, 0]), 0.25)),
    "angles": (None, lambda: pseq([0, 90, 270]).degrad().raddeg()),
    "moddif": (None, lambda: pseq([0.1, 0.9, 0.5]).moddif(0.2)),
    "bilin": (None, lambda: pseq([0, 0.25, 0.5, 0.75, 1]).bilin(0.5, 0, 1, 10, 0, 100)),
    "bit_ops": (None, lambda: (~pseq([0, 5]) // 2).bitand(7)),
    "not": (None, lambda: pseq([-1, 0, 1]).not_()),
    "draws": (13, lambda: pseq([10, 10, 10]).rand() + pseq([1, 1, 1]).exprand(2)),
    "builtin_midicps": (None, lambda: b.midicps(pseq([60, 72]))),
    "builtin_reflected": (None, lambda: b.sub(100, pseq([1, 2, 3]))),
    "builtin_by_name": (None, lambda: b.degkey(pseq([0, 3, 7, -1]), [0, 2, 4, 5, 7, 9, 11])),
    "pvalue": (None, lambda: pseq([pvalue(pseq([1, 2])), pvalue(3)])),
    "pseries_stepped": (None, lambda: pseries(0, pseq([1, 2, 3]))),
    "pgeom_grown": (None, lambda: pgeom(1, pseq([2, 3]), 3)),
    "pwhite_bounds": (17, lambda: pwhite(pseq([0, 10, 20]), pseq([1, 11, 21]))),
    "pexprand": (17, lambda: pexprand(100, 1000, 6)),
    "plprand": (17, lambda: plprand(0, 10, 6)),
    "phprand": (17, lambda: phprand(0, 10, 6)),
    "pmeanrand": (17, lambda: pmeanrand(0, 10, 6)),
    "pbeta": (17, lambda: pbeta(-1, 1, 0.5, 2, 6)),
    "pcauchy": (17, lambda: pcauchy(5, 0.5, 6)),
    "pgauss": (17, lambda: pgauss(60, 3, 6)),
    "ppoisson": (17, lambda: ppoisson(2.5, 6)),
    "pbrown": (17, lambda: pbrown(0, 1, 0.25, 8)),
    "pgbrown": (17, lambda: pgbrown(100, 400, 0.2, 8)),
    "pprob": (17, lambda: pprob([0, 1, 4, 1, 0], 48, 72, None, 8)),
    "pseq_offset": (None, lambda: pseq([1, 2, 3], 2, 1)),
    "pser_offset": (None, lambda: pser([1, 2, 3], 5, 2)),
    "pxrand": (19, lambda: pxrand([1, 2, 3], 8)),
    "pwrand": (19, lambda: pwrand([1, 2, 3], [1, 0, 3], 8)),
    "pshuffle": (19, lambda: pshuffle([1, 2, 3, 4], 2)),
    "pslide": (None, lambda: pslide([1, 2, 3, 4, 5], 3, 3, 1)),
    "pslide_nowrap": (None, lambda: pslide([1, 2, 3], 4, 2, 1, 0, False)),
    "pwalk": (None, lambda: pwalk([1, 2, 3, 4], pseq([1], 8), pseq([1, -1], float("inf")))),
    "pwalk_random": (19, lambda: pwalk([1, 2, 3, 4, 5])),
    "place": (None, lambda: place([1, [2, 3], [4, 5, 6]], 3)),
    "placep": (None, lambda: placep([pseq([1, 2]), pseq([10, 20, 30])], 5)),
    "ptuple": (None, lambda: ptuple([pseq([1, 2]), pseq([10, 20, 30])], 2)),
    "pswitch": (None, lambda: pswitch([pseq([1, 2]), 9], pseq([0, 1, 0]))),
    "pswitch1": (None, lambda: pswitch1([pseq([1, 2, 3]), pseq([10, 20])], pseq([0, 1, 0, 1]))),
    "pfsm": (19, lambda: pfsm([[0], 1, [1], 2, [0, 2], 3, [1, 3], None, None], 2)),
    "pcollect": (None, lambda: pcollect(lambda x: x * x, pseq([1, 2, 3]))),
    "pselect": (None, lambda: pselect(lambda x: x % 3 == 0, pseries(0, 1, 12))),
    "preject": (None, lambda: preject(lambda x: x % 3 == 0, pseries(0, 1, 9))),
    "pdrop": (None, lambda: pdrop(pseries(0, 1, 6), 2)),
    "plen": (None, lambda: plen(pseries(5, 5), 4)),
    "pstutter": (None, lambda: pstutter(pseq([1, 2, 3]), pseq([2, 0, 3]))),
    "platch": (None, lambda: platch(pseries(0, 1), pseq([True, False, False, True, False, True]))),
    "pwhile": (None, lambda: pwhile(_twice(), pseq([7, 8]))),
    "pwrap": (None, lambda: pwrap(pseq([-1, 5, 2.5]), 0, 4)),
    "pclump": (None, lambda: pclump(pseries(0, 1, 5), 2)),
    "pflatten": (None, lambda: pflatten(pseq([[1, [2, 3]], 4]), 1)),
    "pdiff": (None, lambda: pdiff(pseq([1, 4, 9, 16]))),
    "pprorate": (None, lambda: pprorate(pseq([1, 2]), pseq([0.25, [0.5, 0.25, 0.25]]))),
    "pavaroh": (None, lambda: pavaroh(pseq([0, 2, 4, 2, 0]), [0, 2, 4, 5, 7, 9, 11], [0, 1, 3, 5, 7, 8, 10])),
    "pdur": (None, lambda: pcollect(lambda e: e.delta(), pdur(2.5, pbind(dur=1, degree=pseq([0, 1, 2, 3]))))),
    "pdelta": (None, lambda: pcollect(lambda e: e.delta(), pdelta(1.5, pbind(dur=0.5, degree=pseq([0, 1]))))),
    "pgate": (None, lambda: pcollect(lambda e: e["x"], plen(pbind(
        n=pn(pseq([0, 1]), float("inf"), "g"),
        x=pgate(pseries(0, 1), float("inf"), "g")), 6))),
    "ptrace_passes": (None, lambda: ptrace(pseq([1, 2]), "| ")),
    "prout": (None, lambda: prout(_counted)),
    "pfuncn": (None, lambda: pfuncn(lambda: pseq([1, 2]), 2)),
    "plazy": (None, lambda: plazy(lambda: pseq([5, 6]))),
    "pif": (None, lambda: pif(pseq([True, False, True, True]), pseries(0, 1), pseries(100, 1))),
    "pproduct": (None, lambda: pproduct(sum, [pseq([1, 2]), pseq([10, 20, 30])])),
    "pproduct_lists": (None, lambda: pproduct(None, [pseq([1, 2]), pseq([10, 20])])),
    "pdfsm": (None, lambda: pdfsm([pseq([0, 1, 1, 0]),
                                   {0: (0, 1), 1: (1, 2), "default": (0, 3)},
                                   {0: (0, 4), "default": (1, 5)}])),
}


def values(seed, build):
    if seed is not None:
        main.seed(seed)
    # A value that is a list (`ptuple`) is written flat, its elements in order.
    out = []
    for v in itertools.islice(build(), TAKE):
        out.extend(float(x) for x in (v if isinstance(v, list) else [v]))
    return out


def main_():
    vectors = {name: {"seed": seed, "values": values(seed, build)}
               for name, (seed, build) in CASES.items()}
    out = pathlib.Path(__file__).with_name("pattern-vectors.json")
    out.write_text(json.dumps(vectors, indent=1) + "\n")
    print(f"wrote {out} ({len(vectors)} cases)")


if __name__ == "__main__":
    main_()
