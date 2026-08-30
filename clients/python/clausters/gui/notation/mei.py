"""The two directions between the client's sequencing data and a score.

The third way into the engraver, beside typed score text and the SVG adapter:
turn the client's own `clausters.seq` data (Event, Timeline) into MEI — the
format `clausters.gui.notation.engrave` already reads — so a melody or a
bounced timeline is *seen* and edited as notation, the inverse of the
score->sound flow.

**And back again.** `to_timeline` is the return trip: a sheet read into what it
sounds (`clausters.gui.notation.to_notes`, which is where the symbols are
honoured) and placed on a `clausters.seq.timeline.Timeline` of `Event`s. It is
here rather than beside the model because it is the same seam in the other
direction — building `Event`s reads Python-native types and stays in this
client, while what a staccato *means* is one implementation in Rust.

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

from ...seq.event import NOTATION_KEYS
from . import sheet

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

    An event may also say what the note is **on a page**
    (`clausters.seq.event.NOTATION_KEYS`): ``articulations``, ``dynamic``,
    ``ornament``, ``grace``, ``stem``, ``spelling``, ``accidental`` and ``tie``
    reach the score under their own names, and an explicit ``sustain`` becomes
    how long the note is *held* -- but only where no articulation already says
    so, since a staccato that was also written as a short length would be
    shortened twice on the way back.

    Returns the MEI to hand to `engrave` (a one-shot display list), `Score` (to
    edit and redraw) or `Score.from_notes` (the two in one). ``meter`` (``"4/4"``)
    sets the barring, ``clef`` (``"G2"``/``"F4"``/``"C3"``) the staff, ``key``
    the key signature and sharp-vs-flat spelling, and ``beat_unit`` what one beat
    is worth (``4`` = a quarter, matching ``TEMPO``/``L:1/4``).

    A duration that is not a single note value is written as **tied** notes (a
    dotted value when exact, e.g. ``1.5`` beats -> a dotted quarter), and a note
    that overruns a barline is split and tied across it. Off-grid durations
    (finer than a 32nd, e.g. a triplet) snap to the grid here, on the way in:
    the model itself holds an exact rational, so a tuplet is representable the
    moment a caller can express one — writing it is the emission milestone.
    """
    return sheet.to_mei(sheet_from_notes(notes, meter=meter, clef=clef,
                                         key=key, beat_unit=beat_unit))


def from_timeline(timeline, *, meter: str = "4/4", clef: str = "G2",
                  key: str = "C", beat_unit: int = 4) -> str:
    """Engrave a `clausters.seq.timeline.Timeline` into an MEI string.

    The timeline's placements become the score's rhythm: events **sharing a
    beat** are written as one chord, a gap between a group's written end and the
    next onset becomes a rest, and a gap before the first onset is a leading
    rest. Non-`Event` items (`OscEvent`/`MidiEvent`, which carry no pitch) are
    skipped, as are rest events (they read as silence, i.e. a gap).

    Each group is written for its **shortest** ``dur`` (one layer, so it is
    clamped never to overrun the next onset — the model holds several voices
    already, and writing them is the emission milestone). Options and the tie/barline behaviour are
    as `from_notes`; returns the MEI for `engrave`/`Score`/`Score.from_timeline`.
    """
    return sheet.to_mei(sheet_from_timeline(timeline, meter=meter, clef=clef,
                                            key=key, beat_unit=beat_unit))


# -- stopping at the model ----------------------------------------------------
# The same two reductions, handing back the **sheet** rather than the MEI. What
# they are for is everything the model can do that a string cannot: operate on
# the score, and read it back into sound.


def sheet_from_notes(notes, *, meter: str = "4/4", clef: str = "G2",
                     key: str = "C", beat_unit: int = 4) -> dict:
    """`from_notes`, stopping at the score model instead of the MEI.

    The sheet is what `clausters.gui.notation.to_mei` writes and what
    `clausters.gui.notation.to_notes` reads back, so a caller that wants to
    operate on the score — or hear it as the page says rather than as the events
    said — starts here.
    """
    return sheet.from_voice(_voice_from_notes(notes, beat_unit),
                            meter=meter, clef=clef, key=key)


def sheet_from_timeline(timeline, *, meter: str = "4/4", clef: str = "G2",
                        key: str = "C", beat_unit: int = 4) -> dict:
    """`from_timeline`, stopping at the score model instead of the MEI."""
    return sheet.from_voice(_voice_from_timeline(timeline, beat_unit),
                            meter=meter, clef=clef, key=key)


def to_timeline(score, *, instruments=None, interp: dict | None = None,
                **event_keys):
    """Read a sheet into a `clausters.seq.timeline.Timeline` that plays it.

    The return trip, and the one `clausters.gui.notation.to_notes` does the
    thinking for: each sounding note becomes an `clausters.seq.event.Event` at
    its onset, carrying the **written** value as ``dur`` and the **heard** one as
    ``sustain`` — which is the pair the page keeps apart and the reason a
    staccato quarter is still a quarter.

    ``instruments`` binds a staff to what plays it, since the notation does not
    say: a def name for every staff, or a mapping from staff index (0 is the top
    one) to def name. Left out, events take the client's default instrument.
    Anything in ``event_keys`` is merged into every event, for the parameters a
    score has no symbol for at all (``pan``, a control the def reads).

    ``interp`` is the reading (`clausters.gui.notation.interpretation`); left
    out, the default.

    **What is on the page comes with it.** Each event also carries the marks the
    note was written with (`clausters.seq.event.NOTATION_KEYS`) -- its
    articulations verbatim, not the ``sustain`` they produced -- so a timeline
    read from a score and written back with `sheet_from_timeline` engraves the
    same page. What does not survive that trip is everything that is not one
    note's: a slur, a hairpin, a tuplet, the meter and the barlines, the title
    -- none of them can ride an event, and they are the reason a score is a
    score rather than a list of notes.
    """
    from ...seq.event import Event
    from ...seq.timeline import Timeline

    out = Timeline()
    for note in sheet.to_notes(score, interp):
        event = dict(event_keys)
        event.update(midinote=note["pitch"], dur=note["dur"],
                     sustain=note["sustain"], amp=note["amp"])
        event.update(note.get("marks", {}))
        event.pop("sounding", None)  # `sustain` already holds it, in beats
        for key in ("spelling", "accidental"):
            if note.get(key) is not None:
                event[key] = note[key]
        instrument = _instrument(instruments, note["staff"])
        if instrument is not None:
            event["instrument"] = instrument
        out.add(note["t"], Event(event))
    return out


def _instrument(instruments, staff: int):
    """What plays ``staff``: one name for every staff, or a mapping."""
    if instruments is None:
        return None
    if isinstance(instruments, str):
        return instruments
    return instruments.get(staff)


# -- the intermediate voice: back-to-back slots -----------------------------
# One flat, monophonic-per-slot stream both entry points reduce to; a note slot
# carries one midi, a chord slot several, a rest none. It crosses to the shared
# encoder as JSON, one object per slot, which lays it out into barred, tied
# measures and emits the XML.


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
            slot = {"midis": [round(ev.midinote())], "ticks": ticks}
            _write_marks(slot, [ev], ticks, beat_unit)
            voice.append(slot)
    return voice


def _write_marks(slot: dict, events, ticks: int, beat_unit: int) -> None:
    """Put what ``events`` say about the *page* onto ``slot``.

    Every key is carried under its own name (`clausters.seq.event.Event` and
    the slot agree on the vocabulary, which is what keeps the two directions
    one thing), except the length in the air, which is the one place the two
    do not line up:

    **A ``sustain`` reaches the page only when nothing on the page already
    says it.** An event that is both staccato and short is not two facts: the
    staccato is the fact, and the short length is what an interpretation makes
    of it. Written as both, the next reading would shorten an already
    shortened note. So ``sounding`` is what the sustain says that no symbol
    said -- and it is left out entirely when the note is held for its written
    value, where it says nothing at all.

    A chord is **one** slot and the model puts one set of marks on it, so the
    events sharing a beat are read together and the first to say something
    wins that key. Which is right rather than a compromise: what is written is
    written on the chord, so a staccato any of its notes carries is the
    chord's. A slot cannot hold two notes marked differently, and that is the
    documented loss.
    """
    for key in NOTATION_KEYS:
        for ev in events:
            value = ev.get(key)
            if value is not None:
                slot[key] = value
                break
    stated = next((ev for ev in events if ev.get("sustain") is not None), None)
    if stated is None or slot.get("articulations"):
        return
    held = _dur_ticks(stated.sustain(), beat_unit)
    if held != ticks:
        slot["sounding"] = held


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
        slot = {"midis": [round(ev.midinote()) for ev in groups[beat]],
                "ticks": ticks}
        _write_marks(slot, groups[beat], ticks, beat_unit)
        voice.append(slot)
        end = onset + ticks
    return voice

