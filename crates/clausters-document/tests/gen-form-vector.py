"""Write the parity vector the Rust suite reads: a composition built with the
Python client's arrangement, converted through `clausters.form.document`.

Nothing checks that the two sides agree on the format unless something crosses
between them, and no build ever reaches this client's call sites. So the vector
is generated here, committed, and parsed by `tests/form_parity.rs`: if either
side moves, that test fails instead of a user finding out with a document that
will not open.

Run from the repo root, and commit whatever moves:

    python3 crates/clausters-document/tests/gen-form-vector.py
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[3] / "clients/python"))

from clausters.form import Buffer, Event, Generator, Group, Sequence, Track  # noqa: E402
from clausters.form.document import to_document  # noqa: E402
from clausters.seq import Event as SeqEvent  # noqa: E402
from clausters.seq import Timeline  # noqa: E402


class _Buffer:
    """A stand-in for `clausters.defs.Buffer`: the conversion reads a `bufnum`
    and nothing else, and allocating a real one would need a server."""

    bufnum = 7


def composition() -> Group:
    """One of everything the conversion has a body for, nested, so the vector
    exercises the recursion rather than a flat list."""
    piece = Group()
    piece.add(Event(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    piece.add(
        Track(Timeline([(0.0, SeqEvent(midinote=64)), (1.5, SeqEvent(midinote=67))])),
        offset=1.0,
        dur=4.0,
    )
    piece.add(
        Buffer(_Buffer(), instrument="take", controls={"amp": 0.4}),
        offset=4.0,
        dur=2.0,
    )
    piece.add(Sequence([Event(SeqEvent(midinote=72)), Event(SeqEvent(midinote=74))]),
              offset=6.0)

    chain = Group(kind="logical")
    chain.add(Generator("rlpf", controls={"cutoff": 900.0}), offset=0.0)
    piece.add(chain, offset=8.0)

    resident = Generator("granular")
    resident.resident = True
    piece.add(resident, offset=12.0, dur=4.0)
    return piece


if __name__ == "__main__":
    out = pathlib.Path(__file__).with_name("form_vector.json")
    out.write_text(json.dumps(to_document(composition(), version=1), indent=2) + "\n")
    print(f"wrote {out}")
