"""The engraver: a score in, a ``score`` display list out.

This is the client-side rendering step. `Score` holds a loaded document so it
can be **edited** and re-engraved; `engrave` is the one-shot for a page that
will not change; `svg_to_display_list` is the adapter both flow through, split
out so it is testable on a captured SVG and shared with every other client.
`page_json` is how a re-engraved page replaces the one on screen.

The engraver is libverovio bound in Rust (``clausters-notation``) and the walk
itself is `clausters_core::notation`, both reached through the C ABI — so this
module is a shell of names, dicts and one handle whose lifetime Python owns,
and a client in another language rebinds the same ABI instead of
reimplementing any of it.
"""

from __future__ import annotations

import json
import os

from ... import _libpath, _native
from ._abi import _MISSING, _engraver, _text, _u8
from .mei import from_notes, from_timeline

# Where the SMuFL data the engraver reads lives. verovio bakes a resource path
# in at *build* time -- the configure-time prefix, which is not where a wheel's
# copy ends up -- so an installed package has to say where its own staged data
# is, or every engraving fails with "font resources are not available". The
# native side reads `CLAUSTERS_VEROVIO` and looks for `<dir>/verovio` under it;
# `setdefault` so an explicit override still wins, and only when the bundled
# directory is actually there (a source checkout has none and falls back to the
# build prefix baked in by `clausters-notation`'s build script).
if os.path.isdir(os.path.join(_libpath.LIBS_DIR, "verovio")):
    os.environ.setdefault("CLAUSTERS_VEROVIO", _libpath.LIBS_DIR)

# The display-list keys the host draws from — everything but `notes`, which is
# the client's own layer. `page_json` and `guidef.score` send exactly these.
_PAGE_LAYERS = ("vb", "glyphs", "prims", "cursors", "step")

class Score:
    """A loaded score, kept alive so it can be **edited** and re-engraved.

    `engrave` is the one-shot form — load, draw, discard. This is the stateful
    one: it holds the engraver's document open, so an edit can be applied to the
    same one the display list was drawn from and the page re-engraved against it.
    The MEI ``xml:id``s survive editing, which is what lets the host keep its
    selection across the round trip: the id the user clicked still names the same
    note afterwards.

    The edit cycle and the undo stack are the shared layer's
    (``clausters_notation::Score``), not this shell's: every edit runs the
    editor action, then a commit that re-runs the layout, then a reload that
    refreshes the MIDI/timemap cache an edit does not invalidate; undo is a stack
    of MEI snapshots, because the engraver's own stack cannot survive that reload
    and its ``undo`` on an empty stack takes the process down. What Python owns
    is the **handle's lifetime**: the score is freed when this object is.
    """

    def __init__(self, data: str, *, scale: int = 40, page_width: int = 2100,
                 options: dict | None = None):
        lib = _engraver()
        extra = json.dumps(options).encode("utf-8") if options else b""
        raw = data.encode("utf-8")
        self._h = lib.clausters_score_open(
            _native.as_u8(raw), len(raw), scale, page_width,
            _native.as_u8(extra), len(extra))
        if not self._h:
            raise ValueError("the engraver could not load the score data")

    def __del__(self):
        handle, self._h = getattr(self, "_h", None), None
        if handle:
            _native.lib().clausters_score_free(handle)

    def display_list(self, page: int = 1) -> dict:
        """This score engraved into a ``score`` display list — the same layers
        `engrave` returns, but from the live document, so it reflects every edit
        applied so far."""
        return json.loads(_text(_native.lib().clausters_score_display_list,
                                self._h, page))

    def mei(self) -> str:
        """The score as MEI, ids and all — the format to persist, and what the
        undo stack is made of."""
        return _text(_native.lib().clausters_score_mei, self._h)

    @property
    def can_undo(self) -> bool:
        return bool(_native.lib().clausters_score_can_undo(self._h))

    @property
    def can_redo(self) -> bool:
        return bool(_native.lib().clausters_score_can_redo(self._h))

    def undo(self) -> bool:
        """Step back one edit. False (never a crash) when there is nothing to
        undo."""
        return bool(_native.lib().clausters_score_undo(self._h))

    def redo(self) -> bool:
        """Step forward again after `undo`; False when there is nothing to redo."""
        return bool(_native.lib().clausters_score_redo(self._h))

    def transpose(self, element_id: str, steps: int) -> bool:
        """Move a note by ``steps`` **diatonic** steps along the staff — up when
        positive — as one undo step.

        This is the pitch edit as the *engraver* expresses it, in steps rather
        than in a position, because its coordinate-taking ``drag`` reads an
        absolute page y in a frame that does not line up with the display list's
        (passing a note its own drawn y moves it six steps), so a caller would
        have to carry an unexplained offset. Steps are exact.

        It is **not** the shape an edit travels in — a displacement made against
        a page since re-engraved would have to be rebased. `transpose_to` is
        what applies what a host sends; reach for this one only when the delta
        is what you actually have.
        """
        raw = element_id.encode("utf-8")
        return bool(_native.lib().clausters_score_transpose(
            self._h, _native.as_u8(raw), len(raw), steps))

    def transpose_to(self, element_id: str, position: int, page: int = 1) -> bool:
        """Move a note **to** the diatonic staff position ``position`` on
        ``page`` — whole steps from its staff's top line, positive upward — as
        one undo step.

        The absolute form, and what a ``"transpose"`` edit-back from the GUI
        host carries: applying it twice leaves the note where it is, and a page
        re-engraved under the gesture needs no rebasing. The relative call
        underneath is the engraver's requirement, and the delta is computed
        against the engraving rather than carried from wherever the gesture
        happened — which is the point, since the two can differ.

        Host and engraver read the position off the same drawing, so a position
        named by one and resolved by the other cannot mean two things.

        Returns whether the note is now at ``position`` — **True when it was
        already there**, since the requested state holds and a resend must be
        harmless. False when the element is not on that page, the page has no
        staff to measure against, or the engraver refused the move.
        """
        raw = element_id.encode("utf-8")
        return bool(_native.lib().clausters_score_transpose_to(
            self._h, _native.as_u8(raw), len(raw), position, page))

    def edit(self, action: str, **param) -> bool:
        """Apply one raw editor action (``set``, ``insert``, ``delete``, ...) as
        a single undo step — the escape hatch for what `transpose` does not
        cover. Returns whether the engraver accepted it; a rejected action leaves
        the score untouched."""
        act = action.encode("utf-8")
        par = json.dumps(param).encode("utf-8")
        return bool(_native.lib().clausters_score_edit(
            self._h, _native.as_u8(act), len(act),
            _native.as_u8(par), len(par)))

    @classmethod
    def from_notes(cls, notes, *, meter: str = "4/4", clef: str = "G2",
                   key: str = "C", beat_unit: int = 4, **kw) -> "Score":
        """An editable `Score` built from a **monophonic** run of events — the
        `from_notes` encoder handed straight to the constructor. ``kw`` passes
        ``scale``/``page_width`` through. See `from_notes` for the mapping."""
        return cls(from_notes(notes, meter=meter, clef=clef, key=key,
                              beat_unit=beat_unit), **kw)

    @classmethod
    def from_timeline(cls, timeline, *, meter: str = "4/4", clef: str = "G2",
                      key: str = "C", beat_unit: int = 4, **kw) -> "Score":
        """An editable `Score` built from a `Timeline` (chords from simultaneous
        events, rests from gaps) — the `from_timeline` encoder handed to the
        constructor. ``kw`` passes ``scale``/``page_width`` through."""
        return cls(from_timeline(timeline, meter=meter, clef=clef, key=key,
                                 beat_unit=beat_unit), **kw)


def engrave(data: str, *, page: int = 1, scale: int = 40,
            page_width: int = 2100, options: dict | None = None) -> dict:
    """Engrave ``data`` (a score in any format the engraver auto-detects) into a
    ``score`` display list.

    One-shot: the score is loaded, drawn and discarded. Use `Score` instead when
    the page has to be **edited** and redrawn.

    The result holds one engraving, in three layers:

    - what the host **draws** — ``vb`` (the ``[w, h]`` page-unit viewBox),
      ``glyphs`` (a SMuFL codepoint-to-outline table), ``prims`` (the placed
      glyphs, lines, fills and texts) and ``step`` (page units per diatonic
      step, the quantum a pitch drag on the page counts in);
    - where the **cursor** goes — ``cursors``, the timemap folded into geometry
      (``{"t", "x", "y0", "y1"}`` per onset, ``t`` in ms);
    - what **sounds** — ``notes``, one ``{"t", "dur", "pitch", "id"}`` per note
      (ms and MIDI pitch). This layer stays on the client: it is what a driver
      plays, and playing it while anchoring the widget's ``playhead_at`` to the
      sample clock of that instant puts the cursor on the sounding note. The
      engraver mints fresh ids per load, so all three layers must come from one
      engraving — which is why one call produces them all.

    Pass the result to `clausters.gui.guidef.score` as its ``display_list``, or
    to `score_view` to get a scrollable page; the builder sends only the drawing
    and cursor layers. The score **wraps into systems** at ``page_width`` (page
    units), and the page grows as tall as the music needs (all systems on one
    page), so a long score reads at ``scale`` instead of being squeezed onto one
    line. ``scale`` sets the staff size; extra engraver ``options`` are merged
    over the defaults. Raises ``RuntimeError`` if the engraver is not built in.
    """
    return Score(data, scale=scale, page_width=page_width,
                 options=options).display_list(page)


def page_json(display_list: dict) -> str:
    """The **drawing** layers of ``display_list`` as the JSON string a live
    ``GuiHost.set(score_id, display_list=…)`` takes — how a re-engraved page
    replaces the one on screen after an edit, without redefining the window.

    The same layers `clausters.gui.guidef.score` sends when it builds the
    widget, so the widget looks the same either way: the client-side ``notes``
    stay here, and so does the host's own chrome (the playhead and the
    selection survive the replacement, which is what keeps the edited note
    selected across the round trip).
    """
    return json.dumps({k: display_list[k] for k in _PAGE_LAYERS
                       if k in display_list})


def svg_to_display_list(svg: str) -> dict:
    """Walk an engraver SVG string into a ``score`` display list. Split out of
    `engrave` so it is testable on a captured SVG, and shared with every other
    client — the walk itself is `clausters_core::notation`, reached through the
    ABI, so a wasm client feeding a wasm engraver's SVG gets the identical list.

    Each primitive carries the id of the element it belongs to, and a **sounding
    element owns everything drawn inside it**: the engraver identifies a note's
    stem and flag separately, and collapsing them onto the note's id is what
    makes one note one thing to select and drag. A chord keeps its notes
    distinct, so one of them can still be transposed alone.
    """
    if not _native.has_notation():
        raise RuntimeError(_MISSING)
    raw = svg.encode("utf-8")
    out = _text(_native.lib().clausters_core_svg_to_display_list,
                _native.as_u8(raw), len(raw))
    return json.loads(out) if out else {}

