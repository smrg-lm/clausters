"""The document: the composition's authoritative model, and the one place an
edit is applied.

The model lives in a Rust crate (`crates/clausters-document`) and every client
binds that one — this client, the web client, and a ``standalone`` GUI host with
no language attached. **The tree stays there**: a client opens a `Document`,
applies intents to it, and asks for the JSON when it actually wants the JSON.

The discipline is the point: the crate is the **only** thing that applies an
intent, so no client can apply an edit and then report what it did — which is
what would let three clients mean three different things by the same gesture.

Why this module exists
----------------------

The surface was here all along, in the private `clausters._native`, reachable
only by importing a module whose leading underscore says not to. The web client
has had ``document.ts`` as a public module of its own since it was written, so
one client had a door and the other had a back way in. That is the kind of
asymmetry the non-divergence rule exists to catch, and it went unnoticed because
the only thing that used the door was `clausters.form`'s converter — a frozen,
secondary module the arrangement no longer goes through.

What is here
------------

- `Document` — one composition, held by the crate. `Document.apply` hands over
  an intent and takes back what happened; `Document.snapshot` is how the JSON
  leaves, asked for rather than paid per edit.
- `History` and `Log` — the edit pile. A `Log` is a history over one document,
  which is what an ordinary editing client wants; a `History` spans several
  structures, which is what an application showing a roll and a curve together
  needs so an undo crosses both in one order.
- `apply_intent` and `resolve_selection` — the one-shot forms, for a caller
  that holds no document.
- `domain_edit` and `domain_coalesce_key` — the same two questions for a
  structure that is **not** a document: a curve, a span of samples, a timeline
  of events.
- `TREE`, `MULTITRACK`, `POINTS`, `SAMPLES`, `EVENTS` — the domain names those
  two answer for.
- `FIRST_VERSION` and `SESSION_FORMAT` — the two version numbers the format
  itself carries.

Usage::

    from clausters.document import Document, Log

    log = Log()
    with Document(written) as doc:
        log.apply(doc, {"intent": "place", "node": 3, "offset": 4.0},
                  label="move the clip")
        log.undo(doc)                    # exactly where it was
        written = doc.snapshot()
"""

from ._native import (MULTITRACK, EVENTS, POINTS, SAMPLES, TREE, Document,
                      History, Log, domain_coalesce_key, domain_edit)

#: The version an unedited document carries.
#:
#: One rather than zero, because zero is what an edit means by *unstated* when
#: it names the state it was made against — the same reservation the GUI host's
#: acknowledgement uses.
FIRST_VERSION = 1

#: The session format this build writes. It moves when a reader that does not
#: know the new shape would read a file **wrongly**, never for an added field,
#: which an older reader ignores and a newer one defaults.
SESSION_FORMAT = 1
from ._native import document_apply as apply_intent
from ._native import document_resolve as resolve_selection

__all__ = [
    "MULTITRACK",
    "EVENTS",
    "FIRST_VERSION",
    "SESSION_FORMAT",
    "POINTS",
    "SAMPLES",
    "TREE",
    "Document",
    "History",
    "Log",
    "apply_intent",
    "domain_coalesce_key",
    "domain_edit",
    "resolve_selection",
]
