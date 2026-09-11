#!/usr/bin/env python3
"""Generate editing-vectors.json: a structure's projections, frozen.

A projection is what an editable structure owes an endpoint -- the props a host
draws it with, the payloads an edit becomes, the operations that make a server
sound it -- and the whole point of `clausters-editing` is that there is **one**
of each. That is the design; this is what proves it rather than assuming it.
The Python client asks the crate over the C ABI and the answers are frozen
here; `editing-parity.test.ts` asks the same questions over wasm and compares.

What it catches is precisely what nothing else does. Two clients drawing one
curve two ways is not a build failure, not a type error and not a lint: it is
two payloads nobody put side by side. Here they are side by side.

The JSON is committed; regenerate with:

    python3 gen-editing-vectors.py
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))
from clausters import _native  # noqa: E402


def curves():
    """The break-point curves, and what each one is asked with.

    Every case a view actually meets: a curve drawn for the first time, one
    drawn against an axis already in hand, one whose data outgrew that axis,
    one that spans no time at all, one with a ragged tail, and an empty one.
    """
    return [
        ("a curve drawn for the first time",
         [0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 2.0, 0.25, 1.0, 0.0],
         None, 0.0),
        ("an axis in hand is held, not recomputed",
         [0.0, 0.0, 1.0, 0.0, 1.0, 0.5, 1.0, 0.0],
         (-4.0, 4.0), 9.0),
        ("and widened where the data left it",
         [0.0, 0.0, 1.0, 0.0, 1.0, 40.0, 1.0, 0.0],
         (-1.0, 1.0), 0.5),
        ("a curve that spans nothing states no duration",
         [0.0, 0.5, 1.0, 0.0],
         None, 0.0),
        ("a trailing partial quad is not a point",
         [0.0, 0.5, 1.0, 0.0, 3.0, 0.5, 1.0],
         None, 0.0),
        ("shapes and curvature ride through untouched",
         [0.0, 0.0, 5.0, -3.5, 4.0, 1.0, 2.0, 0.0],
         None, 0.0),
        ("an empty curve", [], None, 0.0),
    ]


def main() -> None:
    vectors = []
    for name, points, kept, held in curves():
        vectors.append({
            "name": name,
            "kind": "points_props",
            "points": points,
            "kept": list(kept) if kept is not None else None,
            "held": held,
            "props": _native.points_props(points, kept, held),
        })
    out = pathlib.Path(__file__).with_name("editing-vectors.json")
    out.write_text(json.dumps(vectors, indent=2) + "\n")
    print(f"wrote {out.name}: {len(vectors)} vectors")


if __name__ == "__main__":
    main()
