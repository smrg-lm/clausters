"""The editing path, said out loud: what arrived, what was recorded, what was
walked, and what the host was told.

An editing session fails in ways nothing else can see. A gesture reaches a
widget, becomes a payload in some vocabulary, lands as an entry in a pile, and
comes back to the host as a correction — and when the picture and the data
disagree, the interesting question is always *which of those four steps did
something unexpected*. A test answers it for the case somebody thought of; a
window in front of a person does not answer it at all, and "it did the wrong
thing after a few edits" is not a bug report anybody can act on, including the
person who wrote it.

So the path says what it did, at five points and no more:

- an **event** routed: the widget, the tag, and whether the data changed;
- an **entry** recorded: its label and the intent behind it;
- a **step** of the pile: the direction, whether anything moved, and the labels
  on either side of the cursor;
- a **publish**: which widget was redrawn, and how big the tree was;
- an **acknowledgement**: the stamp, the version and the corrections that rode
  with it.

Those five are the joints. Everything between them is derivable from what they
say, and a sixth would be a running commentary rather than a trace.

**Silent unless asked**, like every other area of `clausters.log`, of which
this is one: a library caller sees nothing, and a script routes it wherever it
routes the rest of its logging. `CLAUSTERS_LOG=gui.editing` arms this one alone,
`CLAUSTERS_LOG=1` arms everything, and `watch()` is the same door from a script.
"""

import logging

from ...log import watch as _watch_area

#: This area's logger — a child of the package's, so arming `clausters` catches
#: it and arming this one leaves the rest quiet.
log = logging.getLogger("clausters.gui.editing")


def watch(stream=None) -> logging.Logger:
    """Print the editing path to ``stream`` (stderr by default) — the whole of
    `clausters.log.watch` narrowed to this area."""
    return _watch_area("gui.editing", stream)
