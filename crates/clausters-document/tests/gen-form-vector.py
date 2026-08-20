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

from clausters.form import (Aggregate, Generator, Clang, Sequence, Track,  # noqa: E402
                            Vector)
from clausters.form.document import to_document, to_session  # noqa: E402
from clausters.seq import Event as SeqEvent  # noqa: E402
from clausters.seq import Timeline  # noqa: E402


class _Buffer:
    """A stand-in for `clausters.defs.Buffer`: the conversion reads a `bufnum`
    and nothing else, and allocating a real one would need a server."""

    bufnum = 7


def composition() -> Aggregate:
    """One of everything the conversion has a body for, nested, so the vector
    exercises the recursion rather than a flat list."""
    piece = Aggregate()
    piece.add(Clang(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    piece.add(
        Track(Timeline([(0.0, SeqEvent(midinote=64)), (1.5, SeqEvent(midinote=67))])),
        offset=1.0,
        dur=4.0,
    )
    piece.add(
        Vector(_Buffer(), instrument="take", controls={"amp": 0.4}),
        offset=4.0,
        dur=2.0,
    )
    piece.add(Sequence([Clang(SeqEvent(midinote=72)), Clang(SeqEvent(midinote=74))]),
              offset=6.0)

    chain = Aggregate(kind="logical")
    chain.add(Generator("rlpf", controls={"cutoff": 900.0}), offset=0.0)
    piece.add(chain, offset=8.0)

    resident = Generator("granular")
    resident.resident = True
    piece.add(resident, offset=12.0, dur=4.0)

    # A generator carrying what it last produced: ordinary tree hanging off an
    # opaque leaf, which is the whole of what a host with no language attached
    # can show, and what a saved session has to reopen with.
    frozen = Generator(
        "melody",
        rendered=Track(
            Timeline([(0.0, SeqEvent(midinote=62)), (1.0, SeqEvent(midinote=65))]),
            duration=2.0,
        ),
    )
    piece.add(frozen, offset=16.0, dur=2.0)
    return piece


def session() -> dict:
    """The same composition, saved: the source table, an open destructive edit,
    and the provenance that makes re-generating possible without the format
    knowing how."""
    return to_session(
        composition(),
        version=1,
        sources={
            7: {
                "location": {"at": "file", "path": "/home/someone/takes/vocal.wav"},
                "lifetime": "external",
                "generation": 0,
                "channels": 1,
                "frames": 48000,
                "sample_rate": 48000.0,
            },
            8: {
                # A working copy with the edit still open: a save promotes the
                # scratch and leaves the decision to the person.
                "location": {"at": "file", "path": "scratch/vocal-edit.wav"},
                "lifetime": "session",
                "generation": 3,
                "editing": {"from": 7, "confirmed": False},
            },
            9: {
                # Samples never written down. Saving is not blocked by it, but
                # the file cannot claim to be complete either.
                "location": {"at": "volatile"},
                "lifetime": "session",
                "generation": 0,
            },
        },
        provenance={"script": "song.py", "client": "clausters-python"},
    )


if __name__ == "__main__":
    out = pathlib.Path(__file__).with_name("form_vector.json")
    out.write_text(json.dumps(to_document(composition(), version=1), indent=2) + "\n")
    print(f"wrote {out}")
    out = pathlib.Path(__file__).with_name("session_vector.json")
    out.write_text(json.dumps(session(), indent=2) + "\n")
    print(f"wrote {out}")
