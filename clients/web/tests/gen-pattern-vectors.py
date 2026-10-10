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
from clausters.seq import (pconst, pfunc, pgeom, pn, pnarop, prand,  # noqa: E402
                           pseq, pser, pseries, pwhite)

#: How many values an endless case is read for.
TAKE = 8

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
}


def values(seed, build):
    if seed is not None:
        main.seed(seed)
    return [float(v) for v in itertools.islice(build(), TAKE)]


def main_():
    vectors = {name: {"seed": seed, "values": values(seed, build)}
               for name, (seed, build) in CASES.items()}
    out = pathlib.Path(__file__).with_name("pattern-vectors.json")
    out.write_text(json.dumps(vectors, indent=1) + "\n")
    print(f"wrote {out} ({len(vectors)} cases)")


if __name__ == "__main__":
    main_()
