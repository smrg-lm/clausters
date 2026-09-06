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
- a **publish**: whether it was a difference or a redefine, and how big;
- an **acknowledgement**: the stamp, the version and the corrections that rode
  with it.

Those five are the joints. Everything between them is derivable from what they
say, and a sixth would be a running commentary rather than a trace.

**Silent unless asked.** The logger has no handler of its own, so a library
caller sees nothing and a script can route it wherever it routes the rest of its
logging. `CLAUSTERS_EDIT_LOG=1` is the convenience for the case this exists
for — a person running an example, watching a window, about to do the thing that
breaks — and it prints to stderr so it does not land in whatever the example is
printing on purpose.
"""

import logging
import os

#: The one logger. Named for the subpackage, so `logging.getLogger` reaches it
#: by the name the module already has and a caller can raise or silence it with
#: everything else in `clausters.gui`.
log = logging.getLogger("clausters.gui.editing")

#: The environment variable that arms it, for a script that would rather not
#: configure logging to watch one window.
ENV = "CLAUSTERS_EDIT_LOG"


def watch(stream=None) -> None:
    """Print the editing path to ``stream`` (stderr by default).

    Idempotent: calling it twice does not double every line, which matters
    because the convenience below calls it on import.
    """
    if any(getattr(h, "_clausters_edit_log", False) for h in log.handlers):
        return
    handler = logging.StreamHandler(stream)
    handler.setFormatter(logging.Formatter("edit: %(message)s"))
    handler._clausters_edit_log = True       # type: ignore[attr-defined]
    log.addHandler(handler)
    log.setLevel(logging.DEBUG)


if os.environ.get(ENV, "").strip() not in ("", "0", "false", "no"):
    watch()
