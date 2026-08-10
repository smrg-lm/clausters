"""Score generation: the client's sequencing data into MEI.

The third way into the engraver, beside typed score text and the SVG adapter:
turn the client's own `clausters.seq` data (Event, Timeline) into MEI — the
format `clausters.gui.notation.engrave` already reads — so a melody or a
bounced timeline is *seen* and edited as notation, the inverse of the
score->sound flow.

The **seam this module is** is worth naming, because it is where the
agnostic/shell line falls and it is what a richer encoding extends: the
reduction here is the client's half (it reads Python-native types and flattens
them into a *voice*, a monophonic-per-slot stream of ticks and MIDI pitches),
and laying that voice out into barred, tied measures is the shared half in
``clausters_core::notation``. Every client writes the same document from the
same voice.
"""

from __future__ import annotations

import json

from ... import _native
from ._abi import _MISSING, _text, _u8

# 32nd-note resolution: every duration snaps to an integer number of these, so
# the encoder's barline splitting and tie decomposition are exact integer
# arithmetic. Mirrors `clausters_core::notation`, which does that work.
_TPW = 32  # ticks per whole note

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

