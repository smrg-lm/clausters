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
"""

from __future__ import annotations

import json

from .. import _native
from .transport import Transport

# 32nd-note resolution: every duration snaps to an integer number of these, so
# the encoder's barline splitting and tie decomposition are exact integer
# arithmetic. Mirrors `clausters_core::notation`, which does that work.
_TPW = 32  # ticks per whole note

# The display-list keys the host draws from — everything but `notes`, which is
# the client's own layer. `page_json` and `guidef.score` send exactly these.
_PAGE_LAYERS = ("vb", "glyphs", "prims", "cursors", "step")

_MISSING = (
    "no engraver in libclausters_ffi: build libverovio with "
    "third_party/build-verovio.sh, build the ABI with `cargo build -p "
    "clausters-ffi --features verovio`, and stage both with build_native.py"
)


def _engraver():
    """The loaded ABI, once it is known to carry the engraver.

    Raises ``RuntimeError`` — never an ``AttributeError`` out of ctypes — when
    the library was built without the ``verovio`` feature, which is the case a
    source checkout hits before staging.
    """
    if not _native.has_engraver():
        raise RuntimeError(_MISSING)
    return _native.lib()


def _text(fn, *args) -> str:
    """A size-then-fill call whose payload is text."""
    return _native.size_then_fill(fn, *args).decode("utf-8")


def _u8(s: str):
    return _native.as_u8(s.encode("utf-8"))


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

        This is the pitch edit, and it is deliberately expressed in steps rather
        than in a position: the engraver's coordinate-taking ``drag`` reads an
        absolute page y in a frame that does not line up with the display list's
        (passing a note its own drawn y moves it six steps), so a caller would
        have to carry an unexplained offset. Steps are exact, and the host
        already knows the staff geometry needed to turn a gesture into them.
        """
        raw = element_id.encode("utf-8")
        return bool(_native.lib().clausters_score_transpose(
            self._h, _native.as_u8(raw), len(raw), steps))

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


# ===========================================================================
# Score generation: sequencing data -> MEI
# ===========================================================================
# The third way into the engraver, beside typed score text and the SVG adapter:
# turn the client's own `clausters.seq` data (Event, Timeline) into MEI — the
# format `engrave`/`Score` already read — so a melody or a bounced timeline is
# *seen* and edited as notation, the inverse of the score->sound flow.
#
# The reduction below is the **client's** half: it reads Python-native types
# (Event, Timeline) and flattens them into a *voice* — a monophonic-per-slot
# stream of ticks and MIDI pitches. Laying that voice out into barred, tied MEI
# is the shared half, in `clausters_core::notation`, so every client writes the
# same document from the same voice. This is where the agnostic/shell line falls.


def from_notes(notes, *, meter: str = "4/4", clef: str = "G2", key: str = "C",
               beat_unit: int = 4) -> str:
    """Engrave a **monophonic** run of events into an MEI string.

    ``notes`` is any iterable of `clausters.seq.event.Event` (a
    `clausters.seq.event.rest` becomes a rest); each occupies its written
    ``dur`` beats back to back, so this is the notation of a melody the way a
    ``Pbind``/``Routine`` sequence reads it. The pitch is the event's
    `Event.midinote` (rounded to the nearest semitone), the value is ``dur``.

    Returns the MEI to hand to `engrave` (a one-shot display list), `Score` (to
    edit and redraw) or `Score.from_notes` (the two in one). ``meter`` (``"4/4"``)
    sets the barring, ``clef`` (``"G2"``/``"F4"``/``"C3"``) the staff, ``key``
    the key signature and sharp-vs-flat spelling, and ``beat_unit`` what one beat
    is worth (``4`` = a quarter, matching ``TEMPO``/``L:1/4``).

    A duration that is not a single note value is written as **tied** notes (a
    dotted value when exact, e.g. ``1.5`` beats -> a dotted quarter), and a note
    that overruns a barline is split and tied across it. Off-grid durations
    (finer than a 32nd, e.g. a triplet) snap to the grid — tuplets are the
    engraving-refinements milestone.
    """
    return _voice_to_mei(_voice_from_notes(notes, beat_unit),
                         meter=meter, clef=clef, key=key)


def from_timeline(timeline, *, meter: str = "4/4", clef: str = "G2",
                  key: str = "C", beat_unit: int = 4) -> str:
    """Engrave a `clausters.seq.timeline.Timeline` into an MEI string.

    The timeline's placements become the score's rhythm: events **sharing a
    beat** are written as one chord, a gap between a group's written end and the
    next onset becomes a rest, and a gap before the first onset is a leading
    rest. Non-`Event` items (`OscEvent`/`MidiEvent`, which carry no pitch) are
    skipped, as are rest events (they read as silence, i.e. a gap).

    Each group is written for its **shortest** ``dur`` (one layer, so it is
    clamped never to overrun the next onset — mixed-duration polyphony is the
    engraving-refinements milestone). Options and the tie/barline behaviour are
    as `from_notes`; returns the MEI for `engrave`/`Score`/`Score.from_timeline`.
    """
    return _voice_to_mei(_voice_from_timeline(timeline, beat_unit),
                         meter=meter, clef=clef, key=key)


# -- the intermediate voice: back-to-back slots -----------------------------
# One flat, monophonic-per-slot stream both entry points reduce to; a note slot
# carries one midi, a chord slot several, a rest none. It crosses to the shared
# encoder as JSON, one object per slot, which lays it out into barred, tied
# measures and emits the XML.


def _voice_to_mei(voice: list, *, meter: str, clef: str, key: str) -> str:
    """Hand a reduced voice to the shared MEI encoder."""
    if not _native.has_notation():
        raise RuntimeError(_MISSING)
    raw = json.dumps(voice).encode("utf-8")
    return _text(_native.lib().clausters_core_voice_to_mei,
                 _native.as_u8(raw), len(raw),
                 _u8(meter), len(meter.encode("utf-8")),
                 _u8(clef), len(clef.encode("utf-8")),
                 _u8(key), len(key.encode("utf-8")))


def _dur_ticks(beats: float, beat_unit: int) -> int:
    """A *duration* in beats -> 32nd-note ticks (a whole note is ``beat_unit``
    beats). At least one tick — a sounding note never has zero length."""
    return max(1, round(float(beats) * _TPW / beat_unit))


def _pos_ticks(beat: float, beat_unit: int) -> int:
    """A *position* on the beat axis -> 32nd-note ticks. Unlike a duration this
    may be zero: beat 0 is tick 0, not tick 1, or a downbeat onset would push a
    spurious rest before the first note and knock the whole bar off the grid."""
    return round(float(beat) * _TPW / beat_unit)


def _voice_from_notes(notes, beat_unit: int) -> list:
    voice = []
    for ev in notes:
        ticks = _dur_ticks(ev["dur"], beat_unit)
        if ev.get("type") == "rest":
            voice.append({"ticks": ticks})
        else:
            voice.append({"midis": [round(ev.midinote())], "ticks": ticks})
    return voice


def _voice_from_timeline(timeline, beat_unit: int) -> list:
    """Group the timeline by onset beat into chord/note slots, filling the gaps
    between them with rests."""
    groups: dict[float, list] = {}
    for beat, item in timeline:
        # skip what has no pitch (raw OSC/MIDI items) or is silence (a rest)
        if not hasattr(item, "midinote") or item.get("type") == "rest":
            continue
        groups.setdefault(float(beat), []).append(item)

    beats = sorted(groups)
    voice = []
    end = 0  # ticks consumed so far
    for i, beat in enumerate(beats):
        onset = _pos_ticks(beat, beat_unit)
        if onset > end:  # a leading gap or a gap after a short note -> rest
            voice.append({"ticks": onset - end})
        ticks = _dur_ticks(min(ev["dur"] for ev in groups[beat]), beat_unit)
        if i + 1 < len(beats):  # one layer: never overrun the next onset
            nxt = _pos_ticks(beats[i + 1], beat_unit)
            if nxt > onset:
                ticks = min(ticks, nxt - onset)
        voice.append({"midis": [round(ev.midinote()) for ev in groups[beat]],
                      "ticks": ticks})
        end = onset + ticks
    return voice


def score_view(display_list: dict, *, scroll_id: int, score_id: int,
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
    applies the ``"transpose"`` round trip passes ``editable=True``. Returns the
    `scroll` node; give it and the inner `score` distinct ids
    (``scroll_id``/``score_id``)."""
    from .guidef import score, scroll

    vb = display_list.get("vb") or [1.0, 1.0]
    aspect = (vb[1] / vb[0]) if vb[0] else 1.0
    height = round(width * aspect, 1)
    return scroll(
        scroll_id,
        score(score_id, display_list=display_list, sample_rate=sample_rate,
              editable=editable, x=0.0, y=0.0, w=width, h=height),
        axis="both" if zoom else "y", zoom=zoom,
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
