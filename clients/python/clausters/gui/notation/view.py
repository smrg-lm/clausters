"""Putting an engraved page on screen, and playing it.

Two helpers over a display list the engraver already produced: `score_view`
wraps it in a `scroll` sized to the page, and `transport` hands back the shared
`clausters.gui.transport.Transport` with the page's own unit filled in — a
``score`` widget places its cursor in **score milliseconds**, not samples, and
that conversion is the only thing a page needs on top of the transport the
timeline views already use.
"""

from __future__ import annotations

from ..transport import Transport

def score_view(display_list: dict, *, scroll_id: int | None = None,
               score_id: int | None = None, name: str | None = None,
               width: float = 1000.0, zoom: bool = True,
               sample_rate: float | None = None,
               editable: bool | None = None) -> dict:
    """Wrap an engraved ``display_list`` in a `scroll` sized to the page, ready
    to drop into a window. The content area is ``width`` wide and as tall as the
    page's aspect needs, so a multi-system score scrolls down the systems.

    ``zoom`` enables cursor-anchored zoom to read a dense passage, and it also
    decides the pan axes: **zoomed in, the page is wider than the view**, so x
    has to pan too (``axis="both"``); without zoom the page always fits the
    width and only y can move (``axis="y"``, a plain vertical scroll view).

    ``sample_rate`` is the rate the playback cursor reads the engine clock
    through (omitted = the server's own). ``editable`` opts the page into pitch
    editing (`clausters.gui.guidef.score`): left off, a drag does nothing and the
    view is read-only, which is what a plain plot of a score wants; a driver that
    applies the ``"transpose"`` round trip passes ``editable=True``.

    Returns the `scroll` node. ``scroll_id``/``score_id`` name the two widgets by
    hand; left ``None`` the host assigns them when the tree is opened. ``name``
    tags the inner `score` so a driver can address it by name — the page the
    transport anchors, and the one a re-engrave pushes back — instead of tracking
    its id (``win[name].set(display_list=…)``)."""
    from ..guidef import score, scroll

    vb = display_list.get("vb") or [1.0, 1.0]
    aspect = (vb[1] / vb[0]) if vb[0] else 1.0
    height = round(width * aspect, 1)
    return scroll(
        score(id=score_id, name=name, display_list=display_list,
              sample_rate=sample_rate, editable=editable,
              x=0.0, y=0.0, w=width, h=height),
        id=scroll_id, axis="both" if zoom else "y", zoom=zoom,
        content_w=width, content_h=height,
    )


def transport(host, score_id: int, *, source, tempo: float, sample_rate: float,
              extent=None):
    """A `clausters.gui.transport.Transport` driving a ``score`` widget's
    playback cursor — play, pause, stop and locate, with the cursor following
    the sound.

    The same transport the timeline views use; what a page needs on top is only
    its unit: a ``score`` widget places its static cursor in **score
    milliseconds**, not samples, so this fills in that conversion (a beat is
    ``1000 / tempo`` ms) and leaves the rest of the arguments as they are —
    ``source(at)`` starts a pass at beat ``at`` and returns the playing
    `clausters.seq.Playhead`, ``extent()`` gives the piece's length in beats.

    The engraving is what makes both easy to write: `Score.display_list` hands
    back the notes with their onsets and lengths, so the timeline a pass plays
    and the end it stops at are read off the page itself."""
    return Transport(host, score_id, source=source, tempo=tempo,
                     sample_rate=sample_rate, extent=extent,
                     to_units=lambda beats: beats * 1000.0 / float(tempo))
