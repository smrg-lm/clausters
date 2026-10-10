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
