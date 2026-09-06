"""What the client did, said out loud — the debugging door for the whole
package.

A client session fails in ways nothing else can see. The library is one half of
a conversation: it sends OSC to an audio server and to a GUI host, both of which
answer on their own schedule and neither of which is inside the process. So when
something does not happen — a note that never sounds, a widget that stays where
the hand left it, a def the server never got — the only question worth asking is
*what actually went out and what came back*, and nothing in a traceback says.

The areas, each a logger under `clausters` so a caller can raise or silence one
without the others:

| Logger | What it says |
|---|---|
| `clausters.server` | every message and bundle sent to the audio server, and every reply |
| `clausters.gui` | every command sent to the GUI host, and every event from it |
| `clausters.gui.editing` | the editing path's five joints (`clausters.gui.editing.trace`) |

**Silent unless asked**, and that is not politeness: these are hot paths. A
library that installed a handler would make every importer pay for the
formatting, so nothing is formatted until somebody asks. Log calls pass their
values as arguments rather than building a string, so a disabled logger costs a
level comparison.

Two doors, and they mean different things. `watch()` is the one a script calls;
`CLAUSTERS_LOG` is for the case this exists for — a person running an example,
watching a window, about to do the thing that breaks — and takes an area or a
list of them (`CLAUSTERS_LOG=1` for everything, `CLAUSTERS_LOG=gui,server` for
two, `CLAUSTERS_LOG=gui.editing` for one).
"""

import logging
import os

#: The package's root logger. Every area is a child, so a handler here catches
#: all of them and `logging.getLogger("clausters.gui")` reaches one.
log = logging.getLogger("clausters")

#: The environment variable that arms it: ``1`` for everything, or a
#: comma-separated list of areas relative to `clausters` (``gui``, ``server``,
#: ``gui.editing``).
ENV = "CLAUSTERS_LOG"

#: What a handler this module installed is marked with, so `watch` is idempotent
#: and a second call does not double every line.
_MARK = "_clausters_log"


def watch(area: str = "", stream=None, level: int = logging.DEBUG) -> logging.Logger:
    """Print one area of the client's log to ``stream`` (stderr by default), and
    answer the logger it armed.

    ``area`` is relative to `clausters` — ``""`` for everything, ``"gui"``,
    ``"server"``, ``"gui.editing"``. Idempotent per logger.
    """
    named = log if not area else logging.getLogger(f"clausters.{area}")
    if not any(getattr(h, _MARK, False) for h in named.handlers):
        handler = logging.StreamHandler(stream)
        handler.setFormatter(logging.Formatter("%(name)s: %(message)s"))
        setattr(handler, _MARK, True)
        named.addHandler(handler)
    named.setLevel(level)
    return named


def _armed_by_environment() -> None:
    """Arm what `ENV` asks for, if it asks for anything."""
    asked = os.environ.get(ENV, "").strip()
    if asked.lower() in ("", "0", "false", "no"):
        return
    if asked.lower() in ("1", "true", "yes", "all"):
        watch()
        return
    for area in asked.split(","):
        area = area.strip().removeprefix("clausters.")
        if area:
            watch(area)


_armed_by_environment()
