"""Engrave a score into the host's ``score`` display list.

This is the client-side rendering step: an engraver lays a digital score (MEI,
MusicXML, ABC or Plaine & Easie) out into SVG, and that SVG is walked into the
flat, resolution-independent display list the GUI host's ``score`` widget
consumes — a SMuFL glyph-outline table plus placed primitives (glyphs, staff
lines, stems, beams, slurs) in page units, each carrying the MEI ``xml:id`` it
was engraved from. The host tessellates it; **the engraver lives on the client,
never in the host**, so any language client reuses the same host renderer by
sending the same display list.

The whole layer is **native and shared**: the engraver is libverovio, bound in
Rust (``clausters-notation``), and the format-agnostic parts — the SVG-to-display
-list walk, the MEI writer, the timemap-to-cursor fold — live in
``clausters-core``, reached here through the C ABI (`clausters._native`). This
module is the Python shell over that: idiomatic names, dicts and a handle whose
lifetime Python owns. A second client in another language rebinds the same ABI
instead of reimplementing any of it.

The library ships **inside the wheel** (``clausters/_libs``) exactly as the Faust
compiler and its LLVM do: an installed package engraves with nothing else on the
machine. In a source checkout, build libverovio with
``third_party/build-verovio.sh``, build the ABI with the ``verovio`` feature on,
and stage both with ``build_native.py``.

There are three ways into the engraver: typed score text (ABC/PAE/MEI/MusicXML)
handed to `engrave`/`Score`; `from_notes`/`from_timeline`, which turn the
client's own `clausters.seq` data into MEI (the inverse direction, data->score);
and `svg_to_display_list`, the adapter the first two both flow through.

`score_view` and `transport` are the two helpers for putting a page on screen
and *playing* it: the first wraps the display list in a scrollable view, the
second hands back the shared `clausters.gui.transport.Transport` with the page's
own unit filled in.

**Module layout.** The layer's growth is semantic rather than graphic, so it is
a package split by what each part knows: `engraver` holds the engraver and its
output (`Score`, `engrave`, `svg_to_display_list`, `page_json`), `mei` is the
encoder that turns the client's own sequencing data into a score, and `view` is
the pair of helpers that put a page on screen and play it (`score_view`,
`transport`). Every name is re-exported here, so ``clausters.gui.notation.Score``
keeps meaning what it always did.
"""

from .engraver import Score, engrave, page_json, svg_to_display_list
from .mei import from_notes, from_timeline
from .view import score_view, transport

__all__ = [
    "Score",
    "engrave",
    "from_notes",
    "from_timeline",
    "page_json",
    "score_view",
    "svg_to_display_list",
    "transport",
]

