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

def score_view(display_list, *, scroll_id: int | None = None,
               score_id: int | None = None, name: str | None = None,
               scroll_name: str | None = None,
               width: float = 1000.0, zoom: bool = True,
               sample_rate: float | None = None,
               editable: bool | None = None,
               entry: bool | None = None) -> dict:
    """Wrap an engraved ``display_list`` in a `scroll` sized to the page, ready
    to drop into a window. The page is a dict, or a
    `clausters.gui.guidef.Source` holding one so a re-engrave reaches the
    definition and every window at once (``source(display_list=…)``). The content area is ``width`` wide and as tall as the
    page's aspect needs, so a multi-system score scrolls down the systems.

    ``zoom`` enables cursor-anchored zoom to read a dense passage, and it also
    decides the pan axes: **zoomed in, the page is wider than the view**, so x
    has to pan too (``axis="both"``); without zoom the page always fits the
    width and only y can move (``axis="y"``, a plain vertical scroll view).

    ``sample_rate`` is the rate the playback cursor reads the engine clock
    through (omitted = the server's own). ``editable`` opts the page into pitch
    editing (`clausters.gui.guidef.score`): left off, a drag does nothing and the
    view is read-only, which is what a plain plot of a score wants; a driver that
    applies the ``"transpose"`` round trip passes ``editable=True``. ``entry``
    opts it into **note entry**: a press on blank paper inside a staff reports
    ``"insert" <after-xml:id> <position> <staff>`` — a place, not a note, since
    the pitch needs the clef and the key and the duration is nobody's until a
    driver chooses one.

    Returns the `scroll` node. ``scroll_id``/``score_id`` name the two widgets by
    hand; left ``None`` the host assigns them when the tree is opened. ``name``
    tags the inner `score` so a driver can address it by name — the page the
    transport anchors, and the one a re-engrave pushes back — instead of tracking
    its id (``win[name].set(display_list=…)``). ``scroll_name`` tags the
    **scroll**, which a driver needs for one thing: **the page is drawn to fit
    the box it is given**, so an edit that adds a system would shrink the whole
    engraving to keep it inside. Growing the box with the page instead keeps the
    drawn size fixed and lets the scroll do what it is for::

        h = round(width * vb[1] / vb[0], 1)
        win[name].set(h=h)
        win[scroll_name].set(content_h=h)

    Left out, the view fits whatever it is sent, which is what a page that is
    never edited wants."""
    from ..guidef import Source, score, scroll

    # The scroll is sized from the page, so the size has to be readable here
    # whether the page arrived as a dict or as a `clausters.gui.guidef.Source`
    # holding one — the source's own expansion is what a definition carries.
    page = display_list.props() if isinstance(display_list, Source) else display_list
    vb = page.get("vb") or [1.0, 1.0]
    aspect = (vb[1] / vb[0]) if vb[0] else 1.0
    height = round(width * aspect, 1)
    return scroll(
        score(id=score_id, name=name, display_list=display_list,
              sample_rate=sample_rate, editable=editable, entry=entry,
              x=0.0, y=0.0, w=width, h=height),
        id=scroll_id, name=scroll_name, axis="both" if zoom else "y", zoom=zoom,
        content_w=width, content_h=height,
    )


def transport(host, score_id: int, *, source, tempo: float = 1.0, tempo_map=None,
              sample_rate: float, extent=None):
    """A `clausters.gui.transport.Transport` driving a ``score`` widget's
    playback cursor — play, pause, stop and locate, with the cursor following
    the sound.

    The same transport the timeline views use; what a page needs on top is only
    its unit: a ``score`` widget places its static cursor in **score
    milliseconds**, not samples, so this fills in that conversion and leaves the
    rest of the arguments as they are — ``source(at)`` starts a pass at beat
    ``at`` and returns the playing `clausters.seq.Playhead`, ``extent()`` gives
    the piece's length in beats.

    The conversion goes through the piece's time map like every other one, not
    through a division of its own: a page is engraved on the beat axis, and the
    millisecond a beat is drawn at is the second it falls on. Pass ``tempo_map``
    (the clock's, `clausters.base.TempoClock.map`) when the tempo changes along
    the piece; ``tempo`` alone is that tempo as a single segment.

    The engraving is what makes both easy to write: `Score.display_list` hands
    back the notes with their onsets and lengths, so the timeline a pass plays
    and the end it stops at are read off the page itself."""
    tr = Transport(host, score_id, source=source, tempo=tempo,
                   tempo_map=tempo_map, sample_rate=sample_rate, extent=extent)
    tr.to_units = lambda beats: tr.tempo_map.secs_at(float(beats)) * 1000.0
    return tr
