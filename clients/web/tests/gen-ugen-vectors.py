#!/usr/bin/env python3
"""Generate ugen-vectors.json from the **server's** own UGen catalog.

Unlike the other generators here, the reference is not the Python client: it
is `/ugen_query`, the server's registry reporting every kind with its named
inputs and their defaults. `tests/ugen-catalog.test.ts` contrasts the
TypeScript builders against it, which is what keeps a hand-written mirror from
drifting from the thing it mirrors -- the same job
`clients/python/tests/test_session.py::test_ugen_catalog_matches_the_python_callables`
does for Python, and which the web client did without until eleven builders
had already drifted.

It boots a server to ask, because the catalog is a property of the build: a
server without the `synth` feature has no UGens at all.

The JSON is committed; regenerate with:

    python3 gen-ugen-vectors.py

(from clients/web/tests/, with the Python client importable -- the repo's
.venv has it installed editable.)
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters import Session  # noqa: E402

OUT = pathlib.Path(__file__).with_name("ugen-vectors.json")


def main():
    with Session.live() as session:
        catalog = session.server.query_ugens()
    if not catalog:
        raise SystemExit("the server reported no UGens (a build without `synth`?)")

    kinds = [
        {
            "name": u.name,
            "arity": u.arity,
            "inputs": [{"name": i.name, "default": i.default} for i in u.inputs],
        }
        for u in sorted(catalog, key=lambda u: u.name)
    ]
    OUT.write_text(json.dumps({"kinds": kinds}, indent=2) + "\n")
    print(f"wrote {OUT.name}: {len(kinds)} kinds")


main()
