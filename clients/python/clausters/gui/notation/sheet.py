"""The score model: notation as data, and operations as data over it.

A **sheet** is a plain ``dict`` — two durational structures that do not contain
each other, the metric layout (``grid``) and the content (``staves`` of voices
of items), with every duration an exact rational written ``[numerator,
denominator]``. It is data all the way: this module holds no handle, nothing has
to be freed, and composing operations creates no intermediate anybody owns.

**The logic is not here.** Every operation — its arithmetic, its validation and
its refusals — is `clausters_core::notation`, reached through the C ABI, and
this module is a shell of names that assembles the operation and hands it over.
That is not only the non-divergence rule: a **standalone host has no client
language in the process at all**, and a score it opens has to be editable there,
which is only true while the whole vocabulary lives on the Rust side. An
operation that worked because a client computed something first would be one a
standalone could not perform.

The same rule draws the line inside this file. Naming an operation is this
shell's; *resolving* one is not. ``transpose(sheet, 2, span=measures(3, 10))``
builds a payload and sends it; turning "measures 3 to 10" into a stretch of time
is arithmetic against the grid, it changes the moment a meter changes or a bar
is irregular, and it is done once, in Rust, for every client.

Three doors in and two out: `from_voice` lifts the v1 slot stream (which is what
a client's own reduction of its `Event`s and `Timeline`s produces) into a sheet,
`from_mei` reads a document the other way, `to_mei` writes a sheet out as MEI
for the engraver, `to_notes` reads it back
into what it *sounds* (under an `interpretation`, which is data and replaceable),
and `ops` lists the verbs this core knows — the list each client is contrasted against, since operations
ride inside a payload and the binding table cannot see them.
"""

from __future__ import annotations

import json

from ... import _native
from ._abi import _MISSING, _text, _u8


def _unwrap(payload: str):
    """Read the envelope the sheet calls answer in.

    The C ABI answers ``{"ok": …}`` or ``{"error": "…"}`` because a refusal has
    to keep its reason; Python raises instead, which is the same behaviour in
    the shape a caller expects. A refused operation changed nothing — the sheet
    crossed by value, so the caller still holds what it sent.
    """
    envelope = json.loads(payload)
    if "error" in envelope:
        raise ValueError(envelope["error"])
    return envelope["ok"]


def _core():
    if not _native.has_notation():
        raise RuntimeError(_MISSING)
    return _native.lib()


def from_voice(voice: list, *, meter: str = "4/4", clef: str = "G2",
               key: str = "C") -> dict:
    """Lift a **voice** — the flat slot stream, ``{"midis": [60], "ticks": 8}``
    per note or chord and ``{"ticks": 8}`` per rest — into a sheet.

    The bridge a client crosses once. Reducing `clausters.seq` data to slots
    reads Python-native types and stays in this client; everything above the
    slot is the shared model. Ticks become exact durations and MIDI numbers
    become **spelled** pitches in the accidental world ``key`` implies — the
    only choice a bare number leaves, and the reason the key is asked for here
    rather than at the end.
    """
    raw = json.dumps(voice)
    return _unwrap(_text(_core().clausters_core_voice_to_sheet,
                         _u8(raw), len(raw.encode("utf-8")),
                         _u8(meter), len(meter.encode("utf-8")),
                         _u8(clef), len(clef.encode("utf-8")),
                         _u8(key), len(key.encode("utf-8"))))


def apply(sheet: dict, op: dict) -> dict:
    """Apply one operation to ``sheet``, returning the new sheet.

    ``op`` names the verb under ``"op"`` and carries its parameters beside it
    (``{"op": "transpose", "semitones": 2}``). The verb-shaped helpers below
    build these, and this is what they all call; reach for it directly to send
    an operation this shell has no helper for yet.

    Raises ``ValueError`` with the core's own sentence when the operation is
    refused — a measure range that runs backwards, a parameter that is not
    readable. Nothing changes on a refusal.
    """
    a, b = json.dumps(sheet), json.dumps(op)
    return _unwrap(_text(_core().clausters_core_sheet_apply,
                         _u8(a), len(a.encode("utf-8")),
                         _u8(b), len(b.encode("utf-8"))))


def to_mei(sheet: dict) -> str:
    """Write ``sheet`` out as MEI — what `engrave` and `Score` read.

    Raises ``ValueError`` with the emitter's reason when the model holds
    something MEI cannot be written for yet: a duration that is not an exact
    note value (a tuplet), an accidental past a double, or more than one voice.
    Each says which it is, so a caller knows whether it is wrong or early.
    """
    raw = json.dumps(sheet)
    return _unwrap(_text(_core().clausters_core_sheet_to_mei,
                         _u8(raw), len(raw.encode("utf-8"))))


def from_mei(mei: str) -> dict:
    """Read an MEI **document** into a sheet.

    The other return path, and not the one `to_notes` is: that one turns a score
    into sound, this turns a *document* into a score. A page opened from typed
    text — ABC, MusicXML, a hand-written MEI — is a document and nothing else
    until this reads one, which is why none of the verbs above can touch it
    before that.

    There is one input format rather than four: the engraver normalizes whatever
    it loaded to MEI, so hand it ``Score.mei()`` and every importer verovio has
    is covered.

    What the model does not hold is **what the engraver recomputes when nobody
    chose it** — automatic beaming, the line breaks that merely fit, the staff
    geometry — so it is not read and is not loss. What a writer chose is held:
    the header, the barlines, the breaks, the beams. Ids written by this layer
    come back, so an item edited before a save is the same item after it; a
    document from anywhere else gets fresh ones, because an id means something
    only inside the model that minted it.

    Raises ``ValueError`` when the text is not readable XML or carries no score.
    """
    return _unwrap(_text(_core().clausters_core_mei_to_sheet,
                         _u8(mei), len(mei.encode("utf-8"))))


def interpretation() -> dict:
    """The default **reading** of a score: every number `to_notes` depends on.

    What a staccato does to a length, what ``mf`` is in amplitude, how far a
    crescendo travels, which positions in the bar are stressed. Read it, change
    what you disagree with, and pass it back to `to_notes` — that is the whole
    of overriding an interpretation, and nothing in the core is edited to play a
    score in another style.

    It comes from Rust rather than being written here for the same reason the
    operations do: two clients each holding their own copy of the dynamics table
    play the same score at two amplitudes, and nothing compares them. It is also
    the **parity surface** for the reading, since the interpretation rides inside
    a payload and the binding table cannot see its fields.
    """
    return json.loads(_text(_core().clausters_core_interpretation))


def to_notes(sheet: dict, interp: dict | None = None) -> list:
    """Read ``sheet`` into the notes it **sounds**, in time order.

    The path back out of the score, and the reason it is not a conversion: the
    symbols mean something. A staccato shortens the sound and moves no attack, a
    dynamic governs every note after it until the next one, a hairpin is a shape
    over a stretch of notes rather than a mark on any of them, and a tie is one
    sound of the summed length.

    Each note is a dict with **two lengths** — ``dur``, what is written, and
    ``sustain``, what is heard — in beats (a quarter is one beat by default,
    ``interp["beat_unit"]``); plus ``t``, ``pitch``, ``amp``, the ``staff`` and
    ``voice`` it was written on, and the model ``id`` it came from. The pair of
    lengths maps straight onto an `clausters.seq.event.Event`'s ``dur`` and
    ``sustain``, which is what `to_timeline` does.

    ``interp`` is the reading (`interpretation`); left out, the default. Any
    field left out of it keeps its default, so overriding one is a one-key dict.

    **The instrument is not in the notation** — a staff does not say what plays
    it — so the notes name their staff and the binding is made where the score
    is rendered.
    """
    a = json.dumps(sheet)
    b = json.dumps(interp if interp is not None else {})
    return _unwrap(_text(_core().clausters_core_sheet_perform,
                         _u8(a), len(a.encode("utf-8")),
                         _u8(b), len(b.encode("utf-8"))))


def ops() -> list:
    """Every operation the core knows, each naming its required and optional
    parameters.

    **The parity surface the binding table cannot provide.** Operations ride
    inside a payload through one symbol, so nothing fails when one client grows
    a verb the other lacks — the same structural blindness that let five builder
    divergences stand. Both clients are read against this list instead.
    """
    return json.loads(_text(_core().clausters_core_sheet_ops))


# -- spans, and the verbs -----------------------------------------------------


def measures(first: int, last: int) -> dict:
    """The span of measures ``first`` to ``last``, **1-based and inclusive** —
    the numbers a reader says out loud.

    This only *names* the span. What stretch of time it covers is resolved by
    the core against the grid, because that answer changes with a meter change
    or an irregular bar and two clients computing it separately would disagree
    about which notes an edit touches.
    """
    return {"measures": [first, last]}


def transpose(sheet: dict, semitones: int, *, steps: int | None = None,
              span: dict | None = None) -> dict:
    """Move every note by an interval, keeping the spelling the interval
    implies: a major third up from C is E, not F-flat.

    ``semitones`` is the chromatic size, positive upward. ``steps`` is the
    diatonic size — how many places the notehead moves on the staff — and left
    out it is the ordinary reading of that many semitones (4 semitones is a
    major third, so 2 steps). Pass it to ask for the interval nobody's shorthand
    means, a diminished third over a major second.

    ``span`` limits what moves (`measures`); left out, everything moves.
    """
    op = {"op": "transpose", "semitones": semitones}
    if steps is not None:
        op["steps"] = steps
    if span is not None:
        op["span"] = span
    return apply(sheet, op)


def pitch(step: str, octave: int, alter: int = 0) -> dict:
    """A written pitch: the letter its notehead sits on, its scientific octave
    (``4`` is the octave of middle C) and how many semitones it is altered by.

    Naming a pitch, not deriving one. Spelling a MIDI number is a *rule* — `F#`
    and `Gb` are one number and two notes — so it happens in the core, on the
    way in through `from_voice`; a caller writing a note into a score names the
    note it means.
    """
    return {"step": step, "octave": octave, "alter": alter}


# -- the operators: what rearranges a whole score -----------------------------


def concat(sheet: dict, other: dict) -> dict:
    """``other`` after ``sheet``.

    Each voice continues the voice in the same position, with a rest filling any
    that ran short. The **grid is the first score's, continued**: when it ends on
    a barline, the second score's meters follow it, so a 4/4 section before a 3/4
    one is exactly that. When it ends mid-measure there is no barline for the
    second grid to start at, and a second score with a metric layout of its own
    is refused rather than silently re-barred.
    """
    return apply(sheet, {"op": "concat", "sheet": other})


def stack(sheet: dict, other: dict, *, as_staff: bool = False) -> dict:
    """``other`` at the same time as ``sheet``.

    ``as_staff=False`` writes its voices on the same staves — counterpoint on one
    staff; ``as_staff=True`` appends staves below — a second hand or instrument.
    Both are superposition; the difference is where the notes are written.

    Refused when the two grids differ: two scores cannot share a moment while
    disagreeing about where the barlines are.
    """
    return apply(sheet, {"op": "stack", "sheet": other, "as_staff": as_staff})


def repeat(sheet: dict, count: int, *, span: dict | None = None) -> dict:
    """A stretch played ``count`` times in a row — ``2`` is one repeat, ``1``
    changes nothing.

    The copies go where the original is, pushing what follows later, and the grid
    grows by as many measures as the stretch spans. ``count=0`` is refused: that
    is a deletion, and it has its own verb.
    """
    return apply(sheet, _span_op({"op": "repeat", "count": count}, span))


def retrograde(sheet: dict, *, span: dict | None = None) -> dict:
    """The span's items in reverse order, voice by voice.

    The durations come back mirrored, so the stretch lasts exactly as long as it
    did and the grid is untouched. A tie travels with the pair it joined.
    """
    return apply(sheet, _span_op({"op": "retrograde"}, span))


def invert(sheet: dict, *, axis: dict | None = None,
           span: dict | None = None) -> dict:
    """Mirror the span's pitches about ``axis`` (`pitch`).

    Exact in both dimensions the model keeps apart: the notehead reflects across
    the axis on the staff and the sound reflects across it in semitones, with the
    accidental taking up what is left — which is what an inversion written by
    hand looks like. Without an axis, the line turns about its own first note.
    """
    return apply(sheet, _span_op({"op": "invert", **({"axis": axis} if axis else {})}, span))


def stretch(sheet: dict, factor, *, span: dict | None = None) -> dict:
    """Multiply the span's written values. Augmentation is ``(2, 1)``, diminution
    ``(1, 2)``, and anything else is the same operation at another ratio.

    **The grid does not move**, which is the point: the phrase is re-barred
    against the barlines it already had, tying across them where a value now
    overruns one.
    """
    num, den = factor if isinstance(factor, (tuple, list)) else (factor, 1)
    return apply(sheet, _span_op({"op": "stretch", "factor": [num, den]}, span))


# -- the operators: what rearranges the metric layout --------------------------


def set_meter(sheet: dict, measure: int, count: int, unit: int) -> dict:
    """Put ``count``/``unit`` in force from ``measure`` (counting from 1).

    The grid alone changes: the same notes fall in different measures afterwards,
    which is what changing the meter of a piece means.
    """
    return apply(sheet, {"op": "set_meter", "measure": measure,
                         "count": count, "unit": unit})


def insert_measures(sheet: dict, at: int, count: int) -> dict:
    """Open ``count`` empty measures before measure ``at``.

    Time is added, so both structures move: a rest of the new measures' length is
    written in, and every meter after the cut slides along with the music.
    """
    return apply(sheet, {"op": "insert_measures", "at": at, "count": count})


def remove_measures(sheet: dict, first: int, last: int) -> dict:
    """Take measures ``first`` to ``last`` out, with whatever was written in
    them. The other half of `insert_measures`, and the same rule."""
    return apply(sheet, {"op": "remove_measures", "first": first, "last": last})


# -- the edit verbs: what a hand does to one item ------------------------------


def insert(sheet: dict, dur, *, after: int | None = None, pitches: list | None = None,
           position: int | None = None, staff: int = 0, voice: int = 0) -> dict:
    """Write a new note, chord or rest into a voice.

    ``after`` names the item it follows (by id) and puts it in that item's own
    voice; without one it goes first, on ``staff``/``voice``. No ``pitches`` and
    no ``position`` is a rest. Everything after it moves later by ``dur``:
    writing a note into finished music adds time.

    ``position`` is a place on the staff — whole diatonic steps from its **top
    line**, positive upward — which is what the page's own ``"insert"`` gesture
    reports, since a renderer can measure a place and not a pitch. Given, the
    pitch is worked out from that staff's clef and the key, so clicking the
    middle line in E flat writes a B flat and no client has to know how to read
    a C clef. ``pitches`` wins where both are given.
    """
    num, den = dur if isinstance(dur, (tuple, list)) else (dur, 1)
    op = {"op": "insert", "dur": [num, den], "pitches": pitches or [],
          "staff": staff, "voice": voice}
    if after is not None:
        op["after"] = after
    if position is not None:
        op["position"] = position
    return apply(sheet, op)


def delete(sheet: dict, id: int) -> dict:
    """Take an item out; everything after it moves earlier by its value.

    Not `silence` — that leaves a rest and nothing moves. Confusing the two is
    how a piece comes out shorter than it was with no obvious sign of where.
    """
    return apply(sheet, {"op": "delete", "id": id})


def silence(sheet: dict, id: int) -> dict:
    """Turn an item into a rest of the same length. Nothing moves, and it is
    still the same item, so an id kept for it still names it."""
    return apply(sheet, {"op": "silence", "id": id})


def set_dur(sheet: dict, id: int, dur) -> dict:
    """Give an item a different written value. What follows moves by the
    difference, and the measures it now falls across are worked out when the
    page is written."""
    num, den = dur if isinstance(dur, (tuple, list)) else (dur, 1)
    return apply(sheet, {"op": "set_dur", "id": id, "dur": [num, den]})


def set_pitches(sheet: dict, id: int, pitches: list) -> dict:
    """Give an item different pitches — one for a note, several for a chord, none
    to make it a rest. The value and the id are kept, so this is the same item
    newly spelled rather than a replacement."""
    return apply(sheet, {"op": "set_pitches", "id": id, "pitches": pitches})


def tie(sheet: dict, id: int, tied: bool = True) -> dict:
    """Tie an item into the one after it, or untie it.

    This is the tie you *write* — the note goes on sounding through the next
    item. The ties added where a value crosses a barline are made when the page
    is written and are never stored, so the two compose.
    """
    return apply(sheet, {"op": "tie", "id": id, "tied": tied})


def to_voice(sheet: dict, ids: list, voice: int) -> dict:
    """Move items to another voice on their staff, leaving rests where they were.

    How two lines written as one come apart: the items keep their ids and their
    place in time, and a rest holds each gap open, so nothing around either line
    moves. Refused when the items are not all in one voice.
    """
    return apply(sheet, {"op": "to_voice", "ids": ids, "voice": voice})


def _span_op(op: dict, span: dict | None) -> dict:
    if span is not None:
        op["span"] = span
    return op


def marks(*, articulations: list | None = None, dynamic: str | None = None,
          ornament: str | None = None, grace: str | None = None,
          stem: str | None = None, sounding=None) -> dict:
    """What a note carries beyond its pitch and value.

    ``articulations`` are MEI's names (``"stacc"``, ``"acc"``, ``"ten"``,
    ``"marc"``); ``dynamic`` is written under the staff at this note
    (``"pp"``…``"ff"``); ``ornament`` is ``"trill"``, ``"mordent"``, ``"turn"``
    or ``"fermata"``; ``grace`` makes it a grace note (``"acc"`` for an
    acciaccatura, ``"unacc"`` for an appoggiatura); ``stem`` forces ``"up"`` or
    ``"down"``; ``sounding`` is how long it **sounds** when that is not how long
    it is written — a staccato quarter that sounds an eighth carries both, and
    the two are kept apart because a page that shortened the written value would
    be a different piece of music.

    Every one of them is a fact about the note, not an instruction to the
    engraver, which is what lets a player read them back.
    """
    out: dict = {}
    if articulations:
        out["articulations"] = articulations
    for key, value in (("dynamic", dynamic), ("ornament", ornament),
                       ("grace", grace), ("stem", stem)):
        if value is not None:
            out[key] = value
    if sounding is not None:
        num, den = sounding if isinstance(sounding, (tuple, list)) else (sounding, 1)
        out["sounding"] = [num, den]
    return out


def set_marks(sheet: dict, id: int, marks: dict) -> dict:
    """Give an item the marks it carries (`marks`).

    It **replaces** rather than merges: reading the marks, changing one and
    sending them back is two calls and no ambiguity, where a merge would leave
    no way to remove a mark at all. Refused on a rest, which has nothing to
    articulate.
    """
    return apply(sheet, {"op": "set_marks", "id": id, "marks": marks})


def add_spanner(sheet: dict, kind: str, from_id: int, to_id: int) -> dict:
    """Write something between two notes: ``"slur"``, ``"crescendo"`` or
    ``"diminuendo"``.

    It cannot go on an item because it has two ends, so it goes on the sheet
    beside the staves. Adding the same one twice changes nothing; naming an item
    that is not there is refused, because a hairpin that never appears with no
    reason given is worse than an error.
    """
    return apply(sheet, {"op": "add_spanner", "kind": kind,
                         "from": from_id, "to": to_id})


def remove_spanner(sheet: dict, kind: str, from_id: int, to_id: int) -> dict:
    """Take back what `add_spanner` wrote. Removing one that is not there
    changes nothing rather than refusing, since the state asked for holds."""
    return apply(sheet, {"op": "remove_spanner", "kind": kind,
                         "from": from_id, "to": to_id})


def header(*, title: str = "", subtitle: str = "", composer: str = "",
           lyricist: str = "") -> dict:
    """What is written above the music, for `set_header`.

    Every field is optional because most of them are most of the time: a score
    built by operating on a motif is untitled until somebody names it, and that
    is a state rather than something missing.
    """
    out = {}
    for key, value in (("title", title), ("subtitle", subtitle),
                       ("composer", composer), ("lyricist", lyricist)):
        if value:
            out[key] = value
    return out


def move_steps(sheet: dict, id: int, steps: int) -> dict:
    """Move an item along the staff by ``steps`` **diatonic** places, up when
    positive — what dragging a note on the page is.

    The arrival takes the **key signature's** alteration for the letter it lands
    on, which is what reading in a key means: dragging a note onto a B in E flat
    gives a B flat, and nobody has to say so. That is the difference between
    this and `transpose`, which moves by a named *interval* and keeps the
    alteration the arithmetic implies.

    A chord moves whole. Refused on a rest, which has no pitch to move.
    """
    return apply(sheet, {"op": "move_steps", "id": id, "steps": steps})


def set_header(sheet: dict, header: dict) -> dict:
    """Write what is above the music (`header`).

    It **replaces** rather than merges, as `set_marks` does: with a merge there
    would be no way to clear a field at all, since an omitted one and an emptied
    one look identical on the wire.
    """
    return apply(sheet, {"op": "set_header", "header": header})


def set_barline(sheet: dict, measure: int, kind: str) -> dict:
    """Give ``measure`` (1-based) a right barline: ``"end"``, ``"rptstart"``,
    ``"rptend"``, ``"rptboth"``, ``"dbl"``, ``"invis"`` — or ``"single"``, which
    takes the override back rather than storing one saying "ordinary".

    A repeat barline is **notation**: it is drawn, and it is not what makes a
    passage play twice. Repetition is written out (`repeat`), which is why the
    interpreter has nothing to expand.
    """
    return apply(sheet, {"op": "set_barline", "measure": measure, "kind": kind})


def set_break(sheet: dict, measure: int, kind: str) -> dict:
    """Break the ``"system"`` or the ``"page"`` before ``measure`` (1-based);
    ``"none"`` takes it back.

    This is layout, and it is an edit for the same reason a forced stem is one:
    the engraver breaks lines wherever they fit, and a break somebody *chose* is
    a statement about the page that no recomputation recovers.
    """
    return apply(sheet, {"op": "set_break", "measure": measure, "kind": kind})
