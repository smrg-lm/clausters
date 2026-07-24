"""Engrave a score into the host's ``score`` display list, driving verovio.

This is the client-side rendering step: verovio lays out a digital score (MEI,
MusicXML, ABC or Plaine & Easie) into SVG, and this module walks that SVG into
the flat, resolution-independent display list the GUI host's ``score`` widget
consumes — a SMuFL glyph-outline table plus placed primitives (glyphs, staff
lines, stems, beams, slurs) in verovio page units, each carrying the MEI
``xml:id`` it was engraved from. The host tessellates it; **verovio lives here,
never in the host**, so any language client can reuse the same host renderer by
sending the same display list.

The engraver is **libverovio**, bound here through its C API with `ctypes` and
**bundled in the wheel** (``clausters/_libs``) exactly as the Faust compiler and
its LLVM are: an installed package engraves with nothing else on the machine,
and the client keeps no external dependencies. Reaching the vendored *library*
rather than verovio's SWIG Python module is what keeps it that way — a module
would be a second package in site-packages, under a name pip can replace with
the published one, whose score editor is dead (``third_party/verovio.pin``). In
a source checkout, build it with ``third_party/build-verovio.sh`` and stage it
with ``build_native.py``, or point ``CLAUSTERS_VEROVIO`` at a library or a build
prefix.

There are three ways into the engraver: typed score text (ABC/PAE/MEI/MusicXML)
handed to `engrave`/`Score`; `from_notes`/`from_timeline`, which turn the
client's own `clausters.seq` data into MEI (the inverse direction, data->score);
and `svg_to_display_list`, the adapter the first two both flow through.

The heavy lifting is verovio's; this module is only the SVG-to-display-list
adapter and a thin MEI writer over it.
"""

from __future__ import annotations

import ctypes
import json
import os
import platform
import re
import xml.etree.ElementTree as ET

_SVG = "{http://www.w3.org/2000/svg}"
_XLINK_HREF = "{http://www.w3.org/1999/xlink}href"
_CODEPOINT = re.compile(r"([0-9A-Fa-f]{4,6})")
_NUM = r"[-\d.eE]+"
_TRANSFORM = re.compile(rf"(translate|scale)\(\s*({_NUM})\s*[, ]?\s*({_NUM})?\s*\)")
# a staff line / stem / ledger line: a single "M x y L x y" segment
_LINE = re.compile(rf"^M\s*({_NUM})\s+({_NUM})\s+L\s*({_NUM})\s+({_NUM})\s*$")


_KEY_UP, _KEY_DOWN = 38, 40  # verovio's keyDown codes (vrvdef.h)
# The display-list keys the host draws from — everything but `notes`, which is
# the client's own layer. `page_json` and `guidef.score` send exactly these.
_PAGE_LAYERS = ("vb", "glyphs", "prims", "cursors", "step")
_UNDO_LIMIT = 64  # snapshots kept; an MEI page is small, but not free


class Score:
    """A loaded score, kept alive so it can be **edited** and re-engraved.

    `engrave` is the one-shot form — load, draw, discard. This is the stateful
    one: it holds the verovio toolkit open, so an edit can be applied to the
    same document the display list was drawn from and the page re-engraved
    against it. The MEI ``xml:id``s survive editing, which is what lets the host
    keep its selection across the round trip: the id the user clicked still
    names the same note afterwards.

    Every edit runs the same three steps, because verovio needs all of them:
    the editor action, then ``commit`` (which is what re-runs the layout — an
    action alone changes the document but leaves the drawing stale), then a
    reload of the edited MEI. That last step looks redundant and is not: the
    MIDI/timemap cache is *not* invalidated by an edit, so without it a
    transposed note keeps sounding at its old pitch. It costs ~2 ms.

    Undo is ours, not verovio's — a stack of MEI snapshots. Reloading the
    document to refresh those caches resets the editor's own undo stack, so its
    stack could not survive the cycle anyway; and its `canUndo`/`canRedo` are
    unreliable (a successful edit can leave `canUndo` false) while `undo` on an
    empty stack crashes the process. Owning the stack sidesteps all three.
    """

    def __init__(self, data: str, *, scale: int = 40, page_width: int = 2100,
                 options: dict | None = None):
        self._tk = _open_score(data, scale=scale, page_width=page_width,
                            options=options)
        self._undo: list[str] = []
        self._redo: list[str] = []
        self._drawn = False

    def display_list(self, page: int = 1) -> dict:
        """This score engraved into a ``score`` display list — the same three
        layers `engrave` returns, but from the live document, so it reflects
        every edit applied so far."""
        dl = _display_list(self._tk, page)
        self._drawn = True
        return dl

    def mei(self) -> str:
        """The score as MEI, ids and all — the format to persist, and what the
        undo stack is made of."""
        return self._tk.getMEI({})

    @property
    def can_undo(self) -> bool:
        return bool(self._undo)

    @property
    def can_redo(self) -> bool:
        return bool(self._redo)

    def undo(self) -> bool:
        """Step back one edit. False (never a crash) when there is nothing to
        undo."""
        if not self._undo:
            return False
        self._redo.append(self.mei())
        return self._load(self._undo.pop())

    def redo(self) -> bool:
        """Step forward again after `undo`; False when there is nothing to redo."""
        if not self._redo:
            return False
        self._undo.append(self.mei())
        return self._load(self._redo.pop())

    def transpose(self, element_id: str, steps: int) -> bool:
        """Move a note by ``steps`` **diatonic** steps along the staff — up when
        positive — as one undo step.

        This is the pitch edit, and it is deliberately expressed in steps rather
        than in a position: verovio's coordinate-taking `drag` reads an absolute
        page y in a frame that does not line up with the display list's (passing
        a note its own drawn y moves it six steps), so a caller would have to
        carry an unexplained offset. Steps are exact, and the host already knows
        the staff geometry needed to turn a gesture into them.
        """
        if not steps:
            return False
        key = _KEY_UP if steps > 0 else _KEY_DOWN
        return self._apply([("keyDown", {"elementId": element_id, "key": key})]
                           * abs(steps))

    def edit(self, action: str, **param) -> bool:
        """Apply one raw verovio editor action (``set``, ``insert``, ``delete``,
        ...) as a single undo step — the escape hatch for what `transpose` does
        not cover. Returns whether verovio accepted it; a rejected action leaves
        the score untouched."""
        return self._apply([(action, param)])

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

    # -- internals ----------------------------------------------------------

    def _apply(self, actions: list[tuple[str, dict]]) -> bool:
        """Run ``actions`` as one undo step, then make every derived structure
        agree with the result. Rolls back if verovio rejects any of them, so a
        failed edit is not a half-edited score."""
        self._ensure_drawn()
        before = self.mei()
        ok = True
        for action, param in actions:
            ok = self._tk.edit({"action": action, "param": param}) and ok
        self._tk.edit({"action": "commit"})  # re-runs the layout
        if not ok:
            self._load(before)
            return False
        self._undo.append(before)
        del self._undo[:-_UNDO_LIMIT]
        self._redo.clear()
        # Reload our own edited MEI: the layout is fresh after `commit`, but the
        # MIDI/timemap cache is not, and `notes` is read from it.
        return self._load(self.mei())

    def _load(self, mei: str) -> bool:
        self._drawn = False
        return bool(self._tk.loadData(mei))

    def _ensure_drawn(self) -> None:
        """Draw the page if it has not been drawn since the last load.

        Editing a document that has been loaded but never rendered **segfaults**
        — the editor reaches through drawing state the load does not build.
        Neither ``redoLayout()`` nor ``renderToTimemap()`` builds it; only
        rendering the page does. Since every edit reloads (see `_apply`), two
        edits in a row would hit exactly that, as would editing a `Score` nobody
        has drawn yet. The flag keeps it to one render either way: the common
        path draws the page anyway, to send it.
        """
        if not self._drawn:
            self._tk.renderToSVG(1)
            self._drawn = True


def engrave(data: str, *, page: int = 1, scale: int = 40,
            page_width: int = 2100, options: dict | None = None) -> dict:
    """Engrave ``data`` (a score in any format verovio auto-detects) into a
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
      sample clock of that instant puts the cursor on the sounding note. verovio
      mints fresh ids per load, so all three layers must come from one engraving
      — which is why one call produces them all.

    Pass the result to `clausters.gui.guidef.score` as its ``display_list``, or
    to `score_view` to get a scrollable page; the builder sends only the drawing
    and cursor layers. The score **wraps into systems** at ``page_width``
    (verovio page units), and the page grows as tall as the music needs (all
    systems on one page), so a long score reads at ``scale`` instead of being
    squeezed onto one line. ``scale`` sets the staff size; extra verovio
    ``options`` are merged over the defaults. Raises ``RuntimeError`` if verovio
    is not installed.
    """
    tk = _open_score(data, scale=scale, page_width=page_width, options=options)
    return _display_list(tk, page)


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


# ===========================================================================
# Score generation: sequencing data -> MEI
# ===========================================================================
# The third way into the engraver, beside typed score text and the SVG adapter:
# turn the client's own `clausters.seq` data (Event, Timeline) into MEI — the
# format `engrave`/`Score` already read — so a melody or a bounced timeline is
# *seen* and edited as notation, the inverse of the score->sound flow.
#
# MEI is the target because it is explicit: every note spells its pitch
# (pname/oct/accid) and value (dur/dots), with none of ABC's contextual traps
# (accidentals persisting through a bar, spacing-driven beaming). No xml:ids are
# emitted — verovio mints them on load, exactly as on the ABC path, so id
# stability across editing is unchanged.
#
# Two seams are kept deliberately narrow so the undated engraving-refinements
# milestone (see clients/PLAN.md) can extend rather than rewrite them: the
# pitch spelling (`_spell`) and the beats->written-value step (`_pieces`). v1
# reads only the written `dur`; performance nuance (sustain/legato -> staccato,
# a note drawn shorter than its slot) is that milestone, as are tuplets, full
# polyphony and tonal spelling.

# 32nd-note resolution: every duration snaps to an integer number of these, so
# barline splitting and tie decomposition are exact integer arithmetic.
_TPW = 32  # ticks per whole note
# (MEI @dur value, ticks it lasts), longest first — whole(1)..32nd(32).
_VALUES = [(v, _TPW // v) for v in (1, 2, 4, 8, 16, 32)]

# Chromatic spelling: pitch-class -> (pname, accid), one table per accidental
# world. `accid` is "" (natural, no <accid> child), "s" (sharp) or "f" (flat).
_SHARP = [("c", ""), ("c", "s"), ("d", ""), ("d", "s"), ("e", ""), ("f", ""),
          ("f", "s"), ("g", ""), ("g", "s"), ("a", ""), ("a", "s"), ("b", "")]
_FLAT = [("c", ""), ("d", "f"), ("d", ""), ("e", "f"), ("e", ""), ("f", ""),
         ("g", "f"), ("g", ""), ("a", "f"), ("a", ""), ("b", "f"), ("b", "")]

# key name -> (MEI key.sig, prefer flats when spelling chromatic notes).
_KEYS = {
    "C": ("0", False), "G": ("1s", False), "D": ("2s", False),
    "A": ("3s", False), "E": ("4s", False), "B": ("5s", False),
    "F#": ("6s", False), "C#": ("7s", False),
    "F": ("1f", True), "Bb": ("2f", True), "Eb": ("3f", True),
    "Ab": ("4f", True), "Db": ("5f", True), "Gb": ("6f", True),
    "Cb": ("7f", True),
}


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
    return _mei_document(_voice_from_notes(notes, beat_unit),
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
    return _mei_document(_voice_from_timeline(timeline, beat_unit),
                         meter=meter, clef=clef, key=key)


# -- the intermediate voice: back-to-back (kind, ticks, [midi, ...]) ---------
# One flat, monophonic-per-slot stream both entry points reduce to; a note slot
# carries one midi, a chord slot several, a rest none. `_mei_document` lays it
# out into barred, tied measures and emits the XML.

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
            voice.append(("rest", ticks, []))
        else:
            voice.append(("note", ticks, [round(ev.midinote())]))
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
            voice.append(("rest", onset - end, []))
        ticks = _dur_ticks(min(ev["dur"] for ev in groups[beat]), beat_unit)
        if i + 1 < len(beats):  # one layer: never overrun the next onset
            nxt = _pos_ticks(beats[i + 1], beat_unit)
            if nxt > onset:
                ticks = min(ticks, nxt - onset)
        midis = [round(ev.midinote()) for ev in groups[beat]]
        voice.append(("note", ticks, midis))
        end = onset + ticks
    return voice


# -- laying the voice into barred, tied MEI ---------------------------------

def _pieces(ticks: int) -> list:
    """Decompose a tick count (within one bar) into ``(mei_dur, dots)`` note
    values, largest-first, to be tied. A count that is one plain or dotted value
    is that single value; otherwise the largest value that fits is split off and
    the remainder decomposed on."""
    single = _single_value(ticks)
    if single is not None:
        return [single]
    out = []
    while ticks > 0:
        single = _single_value(ticks)
        if single is not None:
            out.append(single)
            break
        for value, vt in _VALUES:
            if vt <= ticks:
                out.append((value, 0))
                ticks -= vt
                break
    return out


def _single_value(ticks: int):
    """``(mei_dur, dots)`` if ``ticks`` is exactly one plain or single-dotted
    note value, else None."""
    for value, vt in _VALUES:
        if ticks == vt:
            return (value, 0)
        if vt % 2 == 0 and ticks == vt + vt // 2:  # dotted: 1.5x, and dottable
            return (value, 1)
    return None


def _mei_document(voice: list, *, meter: str, clef: str, key: str) -> str:
    """Lay a voice stream out into measures (splitting and tying across
    barlines) and wrap it in a minimal MEI document."""
    num, den = _parse_meter(meter)
    bar = num * _TPW // den  # ticks per measure
    keysig, flats = _KEYS.get(key, ("0", False))
    shape, line = _parse_clef(clef)

    measures: list[list[str]] = [[]]
    pos = 0  # ticks into the current (last) measure
    for kind, total, midis in voice:
        specs = []  # (mei_dur, dots, measure_index) across the whole slot
        remaining = total
        while remaining > 0:
            if pos == bar:
                measures.append([])
                pos = 0
            take = min(remaining, bar - pos)
            for value, dots in _pieces(take):
                specs.append((value, dots, len(measures) - 1))
            pos += take
            remaining -= take
        n = len(specs)
        for idx, (value, dots, mi) in enumerate(specs):
            tie = None
            if kind == "note" and n > 1:  # a split note ties its pieces
                tie = "i" if idx == 0 else ("t" if idx == n - 1 else "m")
            measures[mi].append(_element(kind, value, dots, midis, tie, flats))

    if not any(measures):  # an empty voice still needs a drawable bar
        measures[0] = [_element("rest", value, dots, [], None, flats)
                       for value, dots in _pieces(bar)]

    body = "\n".join(_measure_xml(i, cells, i == len(measures) - 1)
                     for i, cells in enumerate(measures))
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<mei xmlns="http://www.music-encoding.org/ns/mei" meiversion="5.0">\n'
        ' <meiHead><fileDesc><titleStmt><title/></titleStmt>'
        '<pubStmt/></fileDesc></meiHead>\n'
        ' <music><body><mdiv><score>\n'
        f'  <scoreDef meter.count="{num}" meter.unit="{den}" key.sig="{keysig}">\n'
        f'   <staffGrp><staffDef n="1" lines="5" clef.shape="{shape}"'
        f' clef.line="{line}"/></staffGrp>\n'
        '  </scoreDef>\n'
        f'  <section>\n{body}\n  </section>\n'
        ' </score></mdiv></body></music>\n'
        '</mei>\n'
    )


def _measure_xml(index: int, cells: list, last: bool) -> str:
    right = ' right="end"' if last else ""
    inner = "".join(cells)
    return (f'   <measure n="{index + 1}"{right}><staff n="1"><layer n="1">'
            f'{inner}</layer></staff></measure>')


def _element(kind: str, value: int, dots: int, midis: list, tie, flats: bool) -> str:
    d = ' dots="1"' if dots else ""
    if kind == "rest":
        return f'<rest dur="{value}"{d}/>'
    if len(midis) == 1:
        return _note_xml(midis[0], value, dots, tie, flats)
    inner = "".join(_note_xml(m, None, 0, tie, flats) for m in midis)
    return f'<chord dur="{value}"{d}>{inner}</chord>'


def _note_xml(midi: int, value, dots: int, tie, flats: bool) -> str:
    pname, octave, accid = _spell(midi, flats)
    head = f'<note dur="{value}"' if value is not None else "<note"
    if dots:
        head += ' dots="1"'
    head += f' oct="{octave}" pname="{pname}"'
    if tie:
        head += f' tie="{tie}"'
    if accid:
        return f'{head}><accid accid="{accid}"/></note>'
    return f'{head}/>'


def _spell(midi: int, flats: bool):
    """MIDI note -> ``(pname, octave, accid)`` in scientific pitch (60 -> c4)."""
    pname, accid = (_FLAT if flats else _SHARP)[midi % 12]
    return pname, midi // 12 - 1, accid


def _parse_meter(meter: str) -> tuple[int, int]:
    num, den = meter.split("/")
    return int(num), int(den)


def _parse_clef(clef: str) -> tuple[str, int]:
    return clef[0].upper(), int(clef[1:])


def _display_list(tk, page: int) -> dict:
    """The three layers, from a toolkit that already holds a laid-out score."""
    dl = svg_to_display_list(tk.renderToSVG(page))
    timemap = _timemap(tk)
    dl["cursors"] = _cursor_track(dl, timemap)
    dl["notes"] = _note_events(tk, timemap)
    return dl


def _note_events(tk, timemap: list) -> list:
    """The score's **sounding events**, from the same layout the page was drawn
    from: one dict per note with ``t``/``dur`` in ms, the MIDI ``pitch`` and the
    MEI ``id``. verovio mints fresh xml:ids on every load, so these only line up
    with the drawn primitives (and with `_cursor_track`) because they come out of
    one toolkit — which is why this is folded into `engrave` rather than offered
    as a second entry point."""
    events = []
    for entry in timemap:
        for mei_id in entry.get("on", []):
            midi = tk.getMIDIValuesForElement(mei_id)
            if not midi or not midi.get("pitch"):
                continue
            events.append({"t": float(midi.get("time", entry.get("tstamp", 0.0))),
                           "dur": float(midi.get("duration", 0.0)),
                           "pitch": int(midi["pitch"]), "id": mei_id})
    events.sort(key=lambda e: e["t"])
    return events


# The C entry points we bind, with their ctypes signatures. `restype` matters:
# without it ctypes truncates a returned pointer to a 32-bit int, so every string
# would come back corrupt on a 64-bit build.
_VRV_API = {
    "vrvToolkit_constructor": ([], ctypes.c_void_p),
    "vrvToolkit_constructorResourcePath": ([ctypes.c_char_p], ctypes.c_void_p),
    "vrvToolkit_destructor": ([ctypes.c_void_p], None),
    "vrvToolkit_getVersion": ([ctypes.c_void_p], ctypes.c_char_p),
    "vrvToolkit_setOptions": ([ctypes.c_void_p, ctypes.c_char_p], ctypes.c_bool),
    "vrvToolkit_loadData": ([ctypes.c_void_p, ctypes.c_char_p], ctypes.c_bool),
    "vrvToolkit_renderToSVG": ([ctypes.c_void_p, ctypes.c_int, ctypes.c_bool],
                               ctypes.c_char_p),
    "vrvToolkit_renderToTimemap": ([ctypes.c_void_p, ctypes.c_char_p],
                                   ctypes.c_char_p),
    "vrvToolkit_getMEI": ([ctypes.c_void_p, ctypes.c_char_p], ctypes.c_char_p),
    "vrvToolkit_getMIDIValuesForElement": ([ctypes.c_void_p, ctypes.c_char_p],
                                           ctypes.c_char_p),
    "vrvToolkit_edit": ([ctypes.c_void_p, ctypes.c_char_p], ctypes.c_bool),
    "vrvToolkit_editInfo": ([ctypes.c_void_p], ctypes.c_char_p),
}

_LIB_NAMES = {"Linux": "libverovio.so", "Darwin": "libverovio.dylib",
              "Windows": "verovio.dll"}

_engraver: tuple | None = None


def _verovio():
    """Load ``libverovio`` and its resource data, cached for the process.

    Returns ``(lib, resources)``: the bound library and the SMuFL data directory
    to construct a toolkit with (``None`` when the library's built-in path is
    already right). Resolution follows the precedence every native artifact in
    this package uses:

    - ``CLAUSTERS_VEROVIO`` — a library file, or a prefix containing
      ``lib/`` and ``share/verovio/``;
    - the copy bundled in ``clausters/_libs`` — what the wheel ships;
    - a system-wide install, last.

    The data directory has to be passed explicitly because verovio bakes its
    resource path in at *configure* time, pointing at the prefix it was built
    for; a copy staged into the wheel is somewhere else entirely, and a toolkit
    that cannot find its SMuFL data engraves nothing.
    """
    global _engraver
    if _engraver is None:
        _engraver = _load_verovio()
    return _engraver


def _verovio_roots() -> list[str]:
    override = os.environ.get("CLAUSTERS_VEROVIO")
    libs = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "_libs")
    return [p for p in (override, libs) if p]


def _load_verovio():
    name = _LIB_NAMES.get(platform.system(), "libverovio.so")
    tried = []
    for root in _verovio_roots():
        # A file names the library itself; a directory holds it directly (the
        # staged layout) or under lib/ (a build prefix).
        candidates = ([root] if os.path.isfile(root) else
                      [os.path.join(root, name), os.path.join(root, "lib", name)])
        for path in candidates:
            tried.append(path)
            if os.path.exists(path):
                return _bind(path), _resources_for(path)
    try:
        return _bind(name), None       # whatever the loader finds system-wide
    except OSError as exc:
        raise RuntimeError(
            f"engraving a score needs {name}, which ships inside this package. "
            "This install has no bundled copy and none is on the library path "
            f"(looked in: {', '.join(tried) or 'nothing'}). Build it with "
            "third_party/build-verovio.sh and stage it with "
            "clients/python/build_native.py, or point CLAUSTERS_VEROVIO at a "
            "library or a build prefix"
        ) from exc


def _bind(path: str):
    lib = ctypes.CDLL(path)
    for fn, (argtypes, restype) in _VRV_API.items():
        func = getattr(lib, fn)
        func.argtypes, func.restype = argtypes, restype
    return lib


def _resources_for(path: str) -> str | None:
    """The SMuFL data directory beside a resolved library, if it is there:
    ``<dir>/verovio`` in the staged layout, ``<prefix>/share/verovio`` in a
    build prefix."""
    lib_dir = os.path.dirname(os.path.abspath(path))
    for cand in (os.path.join(lib_dir, "verovio"),
                 os.path.join(os.path.dirname(lib_dir), "share", "verovio")):
        if os.path.isdir(cand):
            return cand
    return None


class _VerovioTk:
    """One verovio toolkit, over the library's C API.

    A thin ctypes surface rather than verovio's SWIG module, so the engraver is
    a *library* we vendor and bundle — the arrangement libfaust already has here
    — with no second package for pip to shadow. The methods are named after the
    C entry points they call so the mapping stays obvious; strings cross as
    UTF-8, and every ``const char *`` verovio returns points into storage it
    owns until the next call, so each is copied out immediately.
    """

    def __init__(self, lib, resources: str | None):
        self._lib = lib
        self._tk = (lib.vrvToolkit_constructorResourcePath(resources.encode())
                    if resources else lib.vrvToolkit_constructor())
        if not self._tk:
            raise RuntimeError("verovio could not create a toolkit")

    def __del__(self):
        tk, lib = getattr(self, "_tk", None), getattr(self, "_lib", None)
        if tk and lib is not None:
            lib.vrvToolkit_destructor(tk)
            self._tk = None

    def getVersion(self) -> str:
        return self._lib.vrvToolkit_getVersion(self._tk).decode()

    def setOptions(self, options: dict) -> bool:
        return bool(self._lib.vrvToolkit_setOptions(
            self._tk, json.dumps(options).encode()))

    def loadData(self, data: str) -> bool:
        return bool(self._lib.vrvToolkit_loadData(self._tk, data.encode()))

    def renderToSVG(self, page: int = 1) -> str:
        return self._lib.vrvToolkit_renderToSVG(self._tk, page, False).decode()

    def renderToTimemap(self, options: dict | None = None) -> list:
        out = self._lib.vrvToolkit_renderToTimemap(
            self._tk, json.dumps(options or {}).encode())
        return json.loads(out.decode())

    def getMEI(self, options: dict | None = None) -> str:
        return self._lib.vrvToolkit_getMEI(
            self._tk, json.dumps(options or {}).encode()).decode()

    def getMIDIValuesForElement(self, xml_id: str) -> dict:
        out = self._lib.vrvToolkit_getMIDIValuesForElement(
            self._tk, xml_id.encode()).decode()
        return json.loads(out) if out else {}

    def edit(self, action: dict) -> bool:
        return bool(self._lib.vrvToolkit_edit(
            self._tk, json.dumps(action).encode()))

    def editInfo(self) -> dict:
        out = self._lib.vrvToolkit_editInfo(self._tk).decode()
        return json.loads(out) if out else {}


def _open_score(data: str, *, scale: int, page_width: int, options: dict | None):
    """A `_VerovioTk` with the score loaded and laid out — the single place the
    engraver is reached and the layout options are set."""
    lib, resources = _verovio()

    tk = _VerovioTk(lib, resources)
    opts = {"scale": scale, "adjustPageHeight": True, "svgViewBox": True,
            "breaks": "auto", "pageWidth": page_width}
    if options:
        opts.update(options)
    tk.setOptions(opts)
    if not tk.loadData(data):
        raise ValueError("verovio could not load the score data")
    return tk


def _timemap(tk) -> list:
    """The score's timemap: onset ms -> the MEI ids starting and stopping then."""
    return tk.renderToTimemap({"includeMeasures": False})


def _cursor_track(display_list: dict, timemap: list) -> list:
    """Fold the timemap (onset ms -> the MEI ids sounding then) together with
    the placed geometry into a playback-cursor track: for each onset, the page-x
    of its leftmost note and the y-span of that note's system. This is the
    bridge from musical time to score geometry — the same id that carries the
    onset carries the glyph position (see the Phase-1 finding). Sorted by time,
    ready for the host's ``playhead``."""
    id_x, id_y = _id_positions(display_list["prims"])
    systems = _staff_systems(display_list["prims"])
    track = []
    for entry in timemap:
        t = entry.get("tstamp")
        ons = [i for i in entry.get("on", []) if i in id_x]
        if t is None or not ons:
            continue
        lead = min(ons, key=lambda i: id_x[i])  # the leftmost onset note
        y0, y1 = _system_bounds(systems, id_y[lead])
        track.append({"t": round(float(t), 1), "x": round(id_x[lead], 1),
                      "y0": round(y0, 1), "y1": round(y1, 1)})
    track.sort(key=lambda c: c["t"])
    return track


def _id_positions(prims):
    """Map each MEI id to a page position, preferring the glyph (notehead)
    placement — its transform origin ``(tx, ty)``."""
    id_x, id_y = {}, {}
    for p in prims:
        pid = p.get("id")
        if not pid or pid in id_x:
            continue
        if p["k"] == "glyph":
            id_x[pid], id_y[pid] = p["xf"][0], p["xf"][1]
        elif p["k"] == "line" and p["pts"]:
            id_x[pid], id_y[pid] = p["pts"][0][0], p["pts"][0][1]
    return id_x, id_y


def _staff_line_ys(prims):
    """The page-y of every staff line, ascending. A staff line is a wide
    horizontal ``line`` prim — the one geometry that is the same on every
    system, which makes it the ruler both the system clustering and the
    diatonic step are measured against."""
    return sorted({round(p["pts"][0][1], 1) for p in prims
                   if p["k"] == "line" and len(p["pts"]) == 2
                   and abs(p["pts"][0][1] - p["pts"][1][1]) < 1.0
                   and abs(p["pts"][0][0] - p["pts"][1][0]) > 500.0})


def _staff_step(prims) -> float:
    """Page units per **diatonic step**: half the staff-line spacing, since one
    step is a line-to-space move. It goes in the display list because it is what
    turns a vertical drag on the page into a pitch — the host quantizes the
    gesture with it, and it depends on verovio's ``unit`` option rather than on
    the staff ``scale``, so it cannot be assumed. Measured from the drawing
    itself (the median gap within a system) rather than read back from the
    options, so any producer of a display list gets it right; falls back to
    verovio's default when the page has no staff to measure."""
    ys = _staff_line_ys(prims)
    gaps = sorted(b - a for a, b in zip(ys, ys[1:]) if b - a < 500.0)
    if not gaps:
        return 90.0  # the default unit (9) times verovio's definition factor
    return round(gaps[len(gaps) // 2] / 2.0, 3)


def _staff_systems(prims):
    """Cluster the horizontal staff lines into systems, each a ``(y_top,
    y_bottom)`` pair. A large vertical gap between lines starts a new system."""
    ys = _staff_line_ys(prims)
    if not ys:
        return []
    systems, group = [], [ys[0]]
    for y in ys[1:]:
        if y - group[-1] > 500.0:  # gap between systems >> gap between lines
            systems.append((group[0], group[-1]))
            group = [y]
        else:
            group.append(y)
    systems.append((group[0], group[-1]))
    return systems


def _system_bounds(systems, y):
    """The ``(y0, y1)`` cursor span for a note at page-y ``y``: its system's
    staff extent, padded for stems above and below."""
    if not systems:
        return (y - 400.0, y + 400.0)
    top, bot = min(systems, key=lambda s: min(abs(y - s[0]), abs(y - s[1])))
    pad = (bot - top) * 0.6 + 100.0
    return (top - pad, bot + pad)


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


def svg_to_display_list(svg: str) -> dict:
    """Walk a verovio SVG string into a ``score`` display list. Split out of
    `engrave` so it is testable on a captured SVG without verovio installed.

    Each primitive carries the id of the element it belongs to, and a **sounding
    element owns everything drawn inside it**: verovio identifies a note's stem
    and flag separately, and collapsing them onto the note's id is what makes
    one note one thing to select and drag. A chord keeps its notes distinct, so
    one of them can still be transposed alone.
    """
    root = ET.fromstring(svg)
    glyph_defs = _collect_glyph_defs(root)
    # the drawing lives inside the inner <svg class="definition-scale">.
    inner = _find_definition_scale(root)
    target, vb = (inner, _viewbox(inner)) if inner is not None else (root, _viewbox(root))

    glyphs: dict[str, str] = {}
    prims: list[dict] = []
    _walk(target, _IDENTITY, None, glyph_defs, glyphs, prims)
    return {"vb": vb, "glyphs": glyphs, "prims": prims,
            "step": _staff_step(prims)}


# -- the SVG walk -----------------------------------------------------------
# verovio only emits translate()/scale() transforms; an (offset, scale) pair
# composes them exactly, so we carry that instead of a full matrix.
_IDENTITY = (0.0, 0.0, 1.0, 1.0)  # (tx, ty, sx, sy)
# The classes that name a *sounding element* rather than a piece of one: a
# chord is absent on purpose, since its notes nest inside it and each one has
# to stay addressable on its own.
_ELEMENT = frozenset({"note", "rest", "mRest"})


def _compose(parent, child):
    ptx, pty, psx, psy = parent
    ctx, cty, csx, csy = child
    return (ptx + psx * ctx, pty + psy * cty, psx * csx, psy * csy)


def _apply(xf, x, y):
    tx, ty, sx, sy = xf
    return (tx + sx * x, ty + sy * y)


def _parse_transform(s):
    if not s:
        return _IDENTITY
    xf = _IDENTITY
    for kind, a, b in _TRANSFORM.findall(s):
        a = float(a)
        b = float(b) if b else (a if kind == "scale" else 0.0)
        local = (a, b, 1.0, 1.0) if kind == "translate" else (0.0, 0.0, a, b)
        xf = _compose(xf, local)
    return xf


def _collect_glyph_defs(root) -> dict[str, str]:
    """Map each glyph id (e.g. ``E0A4-n1sc384i``) to the raw outline path ``d``
    verovio inlines in ``<defs>``, keyed by its bare SMuFL codepoint. The glyph
    paths carry an inner ``scale(1,-1)``; we fold that flip into the instance
    transform at placement time (`_walk`), so the stored ``d`` is verbatim."""
    out: dict[str, str] = {}
    for g in root.iter(f"{_SVG}g"):
        gid = g.get("id") or ""
        m = _CODEPOINT.match(gid)
        path = g.find(f"{_SVG}path")
        if m and path is not None and path.get("d"):
            out[m.group(1).upper()] = path.get("d")
    return out


def _walk(node, xf, mei_id, glyph_defs, glyphs, prims, owned=False):
    xf = _compose(xf, _parse_transform(node.get("transform")))
    # Which element a primitive belongs to. Verovio gives its *parts* ids of
    # their own -- a note is a notehead plus a `stem` group holding the stem
    # and its `flag`, each with an id -- and taking the innermost would scatter
    # one note across three ids: the host would then select and drag a stem
    # apart from the notehead it grows out of. So a sounding element claims
    # everything drawn inside it (`owned`), and the ids of its parts are
    # dropped. Everything above it still takes its own id, or the layer and
    # staff would swallow the clefs and bar lines.
    own = node.get("id")
    if own and not owned:
        nid = own
        owned = bool(_ELEMENT.intersection((node.get("class") or "").split()))
    else:
        nid = mei_id
    tag = node.tag.replace(_SVG, "")

    if tag == "use":
        href = node.get(_XLINK_HREF) or node.get("href") or ""
        m = _CODEPOINT.search(href.lstrip("#"))
        if m:
            cp = m.group(1).upper()
            if cp in glyph_defs:
                glyphs.setdefault(cp, glyph_defs[cp])
                # the placed transform, with the glyph's inner scale(1,-1) flip
                # folded into a negative sy so the host maps font units -> page.
                tx, ty, sx, sy = xf
                prims.append({"k": "glyph", "cp": cp,
                              "xf": [round(tx, 2), round(ty, 2),
                                     round(sx, 4), round(-sy, 4)],
                              "id": nid})
        return
    if tag == "path" and node.get("d"):
        d = node.get("d").strip()
        line = _LINE.match(d)
        if line:
            x1, y1, x2, y2 = (float(v) for v in line.groups())
            p1, p2 = _apply(xf, x1, y1), _apply(xf, x2, y2)
            prims.append({"k": "line",
                          "pts": [[round(p1[0], 1), round(p1[1], 1)],
                                  [round(p2[0], 1), round(p2[1], 1)]],
                          "w": _stroke_width(node, xf), "id": nid})
        else:
            # a filled outline (slur, tie): keep `d` verbatim in its own units
            # and let the host apply the transform, so comma/space coordinate
            # separators are never rewritten.
            prims.append({"k": "fill", "d": d, "xf": _xf_list(xf), "id": nid})
        return
    if tag == "polygon" and node.get("points"):
        prims.append({"k": "fill", "d": _points_to_path(node.get("points")),
                      "xf": _xf_list(xf), "id": nid})
        return
    if tag == "polyline" and node.get("points"):
        # a stroked open path (hairpin, some brackets): a thick polyline, not a
        # fill — filling its endpoints would paint a solid wedge.
        pts = _points(node.get("points"))
        if len(pts) >= 2:
            prims.append({"k": "line",
                          "pts": [[round(a, 1), round(b, 1)]
                                  for a, b in (_apply(xf, x, y) for x, y in pts)],
                          "w": _stroke_width(node, xf), "id": nid})
        return
    if tag == "rect":
        prims.append({"k": "fill", "d": _rect_to_path(node), "xf": _xf_list(xf),
                      "id": nid})
        return
    if tag == "ellipse":
        prims.append({"k": "fill", "d": _ellipse_to_path(node), "xf": _xf_list(xf),
                      "id": nid})
        return
    if tag == "text":
        prim = _text_prim(node, xf, nid)
        if prim:
            prims.append(prim)
        return  # its tspans are consumed here, not walked as elements

    for child in node:
        _walk(child, xf, nid, glyph_defs, glyphs, prims, owned)


def _stroke_width(node, xf):
    w = node.get("stroke-width")
    _, _, sx, _ = xf
    return round(float(w) * sx, 1) if w else 1.0


def _xf_list(xf):
    tx, ty, sx, sy = xf
    return [round(tx, 2), round(ty, 2), round(sx, 4), round(sy, 4)]


def _points(points):
    """Parse an SVG ``points`` list into ``[(x, y), ...]`` (local coordinates)."""
    coords = [float(v) for v in re.split(r"[ ,]+", points.strip()) if v]
    return [(coords[i], coords[i + 1]) for i in range(0, len(coords) - 1, 2)]


def _points_to_path(points):
    parts = [f"{'M' if i == 0 else 'L'}{x:.1f} {y:.1f}"
             for i, (x, y) in enumerate(_points(points))]
    return " ".join(parts) + " Z"


def _text_prim(node, xf, nid):
    """A verovio ``<text>`` node: the string lives in nested ``<tspan>``s (with
    the pixel ``font-size`` on the innermost one), the baseline ``x, y`` on the
    outer ``<text>``. Emit one ``text`` primitive with the page-mapped baseline
    and em size — the host draws it in its own font (verbatim text, not SMuFL:
    volta numbers, tempo, lyrics, titles)."""
    s = "".join(node.itertext()).strip()
    if not s:
        return None
    x = float(node.get("x", 0.0))
    y = float(node.get("y", 0.0))
    px, py = _apply(xf, x, y)
    _, _, sx, _ = xf
    size = _text_font_size(node) * sx
    return {"k": "text", "s": s, "x": round(px, 1), "y": round(py, 1),
            "size": round(size, 1), "id": nid}


def _text_font_size(node):
    """The deepest ``font-size`` in ``px`` under ``node`` (verovio puts a real
    size on the innermost tspan and ``0px`` on the wrapper), defaulting to a
    readable size when none is stated."""
    best = 0.0
    for el in node.iter():
        fs = el.get("font-size", "")
        m = re.match(rf"({_NUM})px", fs)
        if m:
            best = max(best, float(m.group(1)))
    return best or 400.0


def _ellipse_to_path(node):
    """An ``<ellipse>`` (augmentation dots, etc.) as a closed path of four cubic
    beziers — the standard circle/ellipse approximation — in local coordinates,
    so it fills through the same tessellator as every other region (the host
    applies the transform)."""
    cx = float(node.get("cx", 0.0)); cy = float(node.get("cy", 0.0))
    rx = float(node.get("rx", 0.0)); ry = float(node.get("ry", 0.0))
    k = 0.5522847498  # 4/3 * (sqrt(2)-1): control-point offset for a quarter arc
    def pt(x, y):
        return f"{x:.1f} {y:.1f}"
    return (
        f"M{pt(cx + rx, cy)} "
        f"C{pt(cx + rx, cy + ry * k)} {pt(cx + rx * k, cy + ry)} {pt(cx, cy + ry)} "
        f"C{pt(cx - rx * k, cy + ry)} {pt(cx - rx, cy + ry * k)} {pt(cx - rx, cy)} "
        f"C{pt(cx - rx, cy - ry * k)} {pt(cx - rx * k, cy - ry)} {pt(cx, cy - ry)} "
        f"C{pt(cx + rx * k, cy - ry)} {pt(cx + rx, cy - ry * k)} {pt(cx + rx, cy)} Z"
    )


def _rect_to_path(node):
    x = float(node.get("x", 0)); y = float(node.get("y", 0))
    w = float(node.get("width", 0)); h = float(node.get("height", 0))
    pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)]
    parts = [f"{'M' if i == 0 else 'L'}{px:.1f} {py:.1f}"
             for i, (px, py) in enumerate(pts)]
    return " ".join(parts) + " Z"


def _find_definition_scale(root):
    for svg in root.iter(f"{_SVG}svg"):
        if svg.get("class") == "definition-scale":
            return svg
    return None


def _viewbox(node):
    vb = (node.get("viewBox") or "").split()
    if len(vb) == 4:
        return [float(vb[2]), float(vb[3])]
    return [0.0, 0.0]
