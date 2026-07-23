#!/usr/bin/env python3
"""Engraving music notation into the GUI host: the ``score`` widget.

A read-only view like ``plot`` and the node tree, but of a **musical score**
rather than a signal. The client engraves a score with verovio (an optional
dependency) into a semantic display list -- a SMuFL glyph-outline table plus
placed glyphs, staff lines, stems and beams in page units -- and the host
tessellates it into the same triangle mesh the rest of the chrome uses. verovio
lives entirely on the client side; the host never depends on it.

Only **two** processes are involved -- the **GUI host** and this **script**; no
audio server is needed (nothing sounds yet -- this is the notation view).

Install the optional engraver::

    pip install verovio

Start the windowed GUI host (from ``clients/gui``)::

    cargo run --bin clausters-gui -- -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_score.py

A window opens showing the engraved phrase. Close it to stop. Needs a display
and a GPU adapter.
"""

import sys
import time

from clausters.gui import GuiHost, score, window
from clausters.gui import notation

# A two-bar phrase in Plaine & Easie -- the most compact way to type a score;
# verovio also reads MEI, MusicXML, ABC and Humdrum through the same loader.
PHRASE = "@clef:G-2\n@timesig:4/4\n@data:4CDEF gABc'/ 4{FE}D2 4G8AB gAB4/"


def scene(display_list: dict) -> dict:
    """A window filled by a single score view of the engraved phrase."""
    return window(
        score(10, display_list=display_list),
        title="Engraved score (verovio -> GPU)", w=900, h=320,
    )


def main():
    dl = notation.engrave(PHRASE)
    print(f"engraved: {len(dl['glyphs'])} glyph outlines, "
          f"{len(dl['prims'])} primitives, page {dl['vb']}")
    with GuiHost() as gui:  # 127.0.0.1:57210 by default
        gui.define(1, scene(dl))
        print("a window shows the engraved score; close it to stop")
        start = time.monotonic()
        while time.monotonic() - start < 60.0:
            msg = gui.poll(timeout=0.1)
            if msg is not None and msg[0] == "/gui_closed":
                print("window closed")
                break


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError) as e:
        sys.exit(str(e))
