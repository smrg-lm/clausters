#!/usr/bin/env python3
"""Generate document-vectors.json: the same composition, the same edits.

The document lives in a Rust crate and every client binds *that one* — this
client over wasm, the Python client over the C ABI, a `standalone` host by
linking it. That is the design, and this is what proves it rather than assuming
it: the Python client builds a composition, applies a run of edits through the
crate, and freezes the document after each one. `document-parity.test.ts` runs
the identical edits through the wasm door and compares.

If the two ever disagree, one of the bindings is doing arithmetic the crate is
not — which is exactly the failure the single-implementation rule exists to make
impossible, and the one nothing else in CI would notice.

The JSON is committed; regenerate with:

    python3 gen-document-vectors.py
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))
from clausters import _native  # noqa: E402
from clausters._native import Log  # noqa: E402
from clausters.form import Aggregate, Clang, Vector, to_document  # noqa: E402
from clausters.seq import Event as SeqEvent  # noqa: E402


class _Buffer:
    """A stand-in for a server buffer: the conversion reads a `bufnum`."""

    bufnum = 100


def composition() -> dict:
    piece = Aggregate()
    piece.add(Clang(SeqEvent(midinote=60, dur=1.0)), offset=0.0, dur=1.0)
    piece.add(Vector(_Buffer(), instrument="take"), offset=2.0, dur=4.0)
    inner = Aggregate()
    inner.add(Clang(SeqEvent(midinote=67, dur=0.5)), offset=0.0, dur=0.5)
    piece.add(inner, offset=8.0, dur=2.0)
    return to_document(piece)


#: (label, intent, against-version-offset or None, quant). The offsets are
#: relative to the document's version at that point, so a case can be *stale*
#: on purpose without hard-coding a number the format might renumber.
#: Node 2 is the event, node 3 the buffer, node 4 the nested set.
CASES = [
    ("a plain placement", {"intent": "place", "node": 2, "offset": 5.0}, 0, 0.0),
    ("snapped to the grid", {"intent": "place", "node": 2, "offset": 4.3}, 0, 1.0),
    ("a placement with a length",
     {"intent": "place", "node": 4, "offset": 1.0, "dur": 3.0}, 0, 0.5),
    ("a configuration replaced whole",
     {"intent": "configure", "node": 2, "config": {"midinote": 72, "amp": 0.4}}, 0, 0.0),
    ("an edit nobody can name", {"intent": "place", "node": 999, "offset": 1.0}, 0, 0.0),
    ("an edit made against a superseded version",
     {"intent": "place", "node": 4, "offset": 9.0}, -3, 0.0),
    ("an unstated edit applies unchecked",
     {"intent": "place", "node": 4, "offset": 9.0}, None, 0.0),
    ("a destructive write moves the source's generation",
     {"intent": "writesamples", "node": 3, "start": 100, "values": [0.5, -0.5]}, 0, 0.0),
    ("the same write again is idempotent on the document",
     {"intent": "writesamples", "node": 3, "start": 100, "values": []}, 0, 0.0),
]

#: Edits driven through the log, as `(label, intent, quant)`. A snap is in
#: there because the *effective* edit is what a redo must replay, and a run on
#: one node because that is what coalescing would have to get right if the
#: caller asked for it (it does not here: each is its own undo).
LOGGED = [
    ("move the event", {"intent": "place", "node": 2, "offset": 1.0}, 0.0),
    ("move the take", {"intent": "place", "node": 3, "offset": 6.0}, 0.0),
    ("drop it on the grid", {"intent": "place", "node": 2, "offset": 4.3}, 1.0),
    ("retune it", {"intent": "configure", "node": 2, "config": {"midinote": 65}}, 0.0),
]

#: Selections resolved against the **starting** composition, as
#: `(start, len, in_beats)` — the mapping is what is under test, not the edit
#: history, and a stable document keeps the cases readable. The buffer sits at
#: beat 2 for four beats, so these are: inside it, over the whole piece, a frame
#: span landing in its second beat, and one that misses everything.
SELECTIONS = [
    (2.0, 2.0, True),
    (0.0, 100.0, True),
    (3.5 * 48_000, 48_000, False),
    (500.0, 10.0, True),
]

if __name__ == "__main__":
    document = composition()
    edits = []
    for label, intent, offset, quant in CASES:
        against = None if offset is None else {"version": document["version"] + offset}
        result = _native.document_apply(document, intent, against=against, quant=quant)
        edits.append({
            "label": label,
            "intent": intent,
            "against": against,
            "quant": quant,
            "document": result["document"],
            "outcome": result["outcome"],
        })
        document = result["document"]

    start_document = composition()
    resolutions = []
    for start, length, in_beats in SELECTIONS:
        selection = {"start": start, "len": length}
        resolutions.append({
            "selection": selection,
            "inBeats": in_beats,
            "framesPerBeat": 48_000.0,
            "framesPerSecond": 48_000.0,
            "spans": _native.document_resolve(
                start_document, selection, frames_per_beat=48_000.0,
                frames_per_second=48_000.0, in_beats=in_beats,
            ),
        })

    # The log, applied and inverted through the crate rather than through a
    # history this client keeps -- the acceptance O11 is written around, frozen
    # so the wasm side has to reach the same states and not merely a consistent
    # one of its own.
    # Since O12 the tree stays behind a handle, so each step's document is read
    # back with `snapshot` rather than carried in the reply -- and the vector
    # keeps freezing it, because the document is what the two sides are being
    # compared on.
    logged = {"applies": [], "undos": [], "redos": []}
    with Log() as log, _native.Document(composition()) as doc:
        for label, intent, quant in LOGGED:
            outcome = log.apply(doc, intent, quant=quant, label=label)
            logged["applies"].append({
                "label": label, "intent": intent, "quant": quant,
                "document": doc.snapshot(), "outcome": outcome,
                "entries": len(log), "undoLabel": log.undo_label,
            })
        while log.can_undo:
            step = log.undo(doc)
            logged["undos"].append({**step, "document": doc.snapshot()})
        logged["inverted"] = doc.snapshot()
        while log.can_redo:
            step = log.redo(doc)
            logged["redos"].append({**step, "document": doc.snapshot()})
        logged["redone"] = doc.snapshot()

    out = pathlib.Path(__file__).with_name("document-vectors.json")
    out.write_text(json.dumps({
        "start": composition(),
        "edits": edits,
        "final": document,
        "resolutions": resolutions,
        "logged": logged,
    }, indent=2) + "\n")
    print(f"wrote {out}")
