"""The two directions between the client's sequencing data and a score.

The third way into the engraver, beside typed score text and the SVG adapter:
turn the client's own `clausters.seq` data (an `Event` run, a `Timeline`, an
`EventSequence`) into a score, so a melody, a bounced timeline or a take
played from a keyboard is *seen* and edited as notation -- and back again,
`to_timeline` and `to_sequence`, which read a sheet into what it sounds.

**Both directions are the core's.** A score is rendered into events by
`clausters.gui.notation.render_events`, and events are read into a score by
`clausters.gui.notation.read_events`: what the events say of their page is
written as they say it, and what they do not -- when a note falls on the
page, in which voice, spelled how -- is decided there, once, for every client.
What this module adds is the client's own types on either side.
"""

from __future__ import annotations

from . import sheet

#: The keys of a transcription (`clausters.gui.notation.read_events`): how a
#: sequence is read where its events do not say.
TRANSCRIPTION_KEYS = ("meter", "key", "clef", "beat_unit", "division",
                      "tuplets", "voices", "dynamics")


def from_notes(notes, **how) -> str:
    """Engrave a **monophonic** run of events into an MEI string.

    ``notes`` is any iterable of `clausters.seq.event.Event` (a
    `clausters.seq.event.rest` is a silence); each occupies its written
    ``dur`` beats back to back, so this is the notation of a melody the way a
    ``Pbind``/``Routine`` sequence reads it. ``how`` is the transcription, as
    `sheet_from_events` takes it. Returns the MEI to hand to `engrave` or
    `Score`.
    """
    return sheet.to_mei(sheet_from_notes(notes, **how))


def from_timeline(timeline, **how) -> str:
    """Engrave a `clausters.seq.timeline.Timeline` -- its placed events, as
    `sheet_from_events` reads them -- into an MEI string."""
    return sheet.to_mei(sheet_from_events(timeline, **how))


# -- stopping at the model ----------------------------------------------------
# The same reductions, handing back the **sheet** rather than the MEI. What
# they are for is everything the model can do that a string cannot: operate on
# the score, and read it back into sound.


def sheet_from_events(sequence, *, interp: dict | None = None, **how) -> dict:
    """Read a `clausters.seq.EventSequence` -- or a
    `clausters.seq.timeline.Timeline`, or any ``(beat, event)`` pairs -- into
    a sheet: the way back from `to_sequence`.

    An event's notation keys (`clausters.seq.event.NOTATION_KEYS`) are
    written as they say, and a sequence a score was rendered into is read
    back as it was written. What the events do not say is decided by the
    transcription, the keyword arguments
    (`clausters.gui.notation.read_events` describes each): ``meter``,
    ``key``, ``clef``, ``beat_unit``, ``division`` (the smallest written
    value an onset is snapped to), ``tuplets``, ``voices`` and ``dynamics``.
    ``interp`` is the reading whose dynamics name a level.

    Events that carry no pitch (an ``"osc"`` or ``"midi"`` one) are skipped,
    and a rest is a silence. **The sequence is not changed** -- a take keeps
    the times it was played with, and is read again with another ``division``
    by calling this again.
    """
    return sheet.read_events(_data(sequence), _how(how), interp)["sheet"]


def sheet_from_notes(notes, *, interp: dict | None = None, **how) -> dict:
    """`from_notes`, stopping at the score model instead of the MEI: the run
    placed back to back, each event at the end of the one before it, and read
    as `sheet_from_events` reads a sequence -- in one voice, as a line is."""
    at, placed = 0.0, []
    for event in notes:
        placed.append((at, event))
        at += float(event["dur"])
    how.setdefault("voices", 1)
    return sheet.read_events(_data(placed), _how(how), interp)["sheet"]


def sheet_from_timeline(timeline, *, interp: dict | None = None, **how) -> dict:
    """`sheet_from_events`, under the name it had: a timeline's placed events
    read into a sheet."""
    return sheet_from_events(timeline, interp=interp, **how)


def _how(how: dict) -> dict:
    """The transcription a caller's keywords say, refusing a word that is
    none of its keys."""
    unknown = [key for key in how if key not in TRANSCRIPTION_KEYS]
    if unknown:
        raise TypeError(
            f"a sequence is not read by {unknown[0]!r}: a transcription has "
            f"{', '.join(TRANSCRIPTION_KEYS)}")
    return {key: value for key, value in how.items() if value is not None}


def _data(sequence) -> dict:
    """What `read_events` takes: a sequence's data, or the events of
    ``(beat, event)`` pairs as one."""
    from ...seq.sequence import EventSequence, _keys

    if isinstance(sequence, EventSequence):
        return sequence.data()
    return {"events": [{"at": float(beat), "data": _keys(event)}
                       for beat, event in sequence
                       if hasattr(event, "midinote") or isinstance(event, dict)]}


def to_timeline(score, *, instruments=None, interp: dict | None = None,
                **event_keys):
    """Read a sheet into a `clausters.seq.timeline.Timeline` that plays it.

    The return trip, and the one `clausters.gui.notation.to_notes` does the
    thinking for: each sounding note becomes an `clausters.seq.event.Event` at
    its onset, carrying the **written** value as ``dur`` and the **heard** one as
    ``sustain`` -- which is the pair the page keeps apart and the reason a
    staccato quarter is still a quarter.

    ``instruments`` binds a staff to what plays it, since the notation does not
    say: a def name for every staff, or a mapping from staff index (0 is the top
    one) to def name. Left out, events take the client's default instrument.
    Anything in ``event_keys`` is merged into every event, for the parameters a
    score has no symbol for at all (``pan``, a control the def reads).

    ``interp`` is the reading (`clausters.gui.notation.interpretation`); left
    out, the default.

    **What is on the page comes with it.** Each event also carries what the
    note is on the page (`clausters.seq.event.NOTATION_KEYS`) -- the pitch as
    it is written, its written value, its staff and voice, and its marks
    verbatim, not the ``sustain`` they produced -- so a timeline read from a
    score and written back with `sheet_from_events` engraves the same notes.
    What does not survive that trip is everything that is not one note's: a
    slur, a hairpin, the meter and the barlines, the title -- none of them can
    ride an event. A timeline holds events alone; `to_sequence` keeps the
    rest, in the sequence's ``notation`` section, and the dynamics as curves.

    The render is the core's (`clausters.gui.notation.render_events`), the
    same one in every client.
    """
    from ...seq.event import Event
    from ...seq.timeline import Timeline

    out = Timeline()
    for event in _rendered(score, instruments, interp, event_keys)["events"]:
        out.add(event["at"], Event(event["data"]))
    return out


def to_sequence(score, *, instruments=None, interp: dict | None = None,
                **event_keys):
    """Render a sheet into a `clausters.seq.EventSequence`, one way
    (`clausters.gui.notation.render_events`): its events as concrete data a
    notes editor edits -- each with an id, in beats, on its voice's channel --
    the staves' dynamics as curves of those channels, and what is no note's
    in the sequence's ``notation`` section. ``instruments`` and ``event_keys``
    are as `to_timeline` takes them."""
    from ...seq.sequence import EventSequence

    return EventSequence.from_data(_rendered(score, instruments, interp, event_keys))


def _rendered(score, instruments, interp, event_keys) -> dict:
    """The sheet rendered (`render_events`), each event given what plays its
    staff and the keys a score has no symbol for."""
    data = sheet.render_events(score, interp)
    for event in data["events"]:
        keys = dict(event_keys)
        keys.update(event["data"])
        instrument = _instrument(instruments, keys["staff"])
        if instrument is not None:
            keys["instrument"] = instrument
        event["data"] = keys
    return data


def _instrument(instruments, staff: int):
    """What plays ``staff``: one name for every staff, or a mapping."""
    if instruments is None:
        return None
    if isinstance(instruments, str):
        return instruments
    return instruments.get(staff)
