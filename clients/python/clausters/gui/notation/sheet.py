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

Two doors in and one out: `from_voice` lifts the v1 slot stream (which is what a
client's own reduction of its `Event`s and `Timeline`s produces) into a sheet,
`to_mei` writes a sheet out as MEI for the engraver, and `ops` lists the verbs
this core knows — the list each client is contrasted against, since operations
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
