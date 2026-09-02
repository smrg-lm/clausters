"""Editing a **timeline of events**: the roll, with no composition under it.

A `clausters.seq.Timeline` a script filled is edited by the same gesture that
edits a track's notes in the multitrack, and until now the only way to write one
back was an aggregate's `SetMembers` — which needs a tree to be a member *of*.
This is that gesture over the timeline itself: the crate's ``events``
vocabulary, one `clausters.gui.guidef.pianoroll`, and the object the caller
already holds written in place.

**What an event is stays the client's.** The crate carries an event's ``data``
and never reads it, so a `clausters.seq.Event` travels whole and comes back
whole — the pitch, the length, the instrument and whatever else the author put
on it. What the roll can say about a note is five numbers; what the note *is* is
more than that, and an edit that rebuilt one from the five would drop the rest.
"""

from ... import _native
from ...seq.event import Event as SeqEvent
from ...seq.timeline import Timeline
from .domain import Domain
from .editor import Editor
from .view import View

#: What the ``pianoroll`` widget sends and takes per note.
QUINTUPLE = 5


def quintuples(flat) -> list:
    """A flat ``notes`` payload as ``(start, dur, pitch, velocity, channel)``
    tuples, dropping a trailing partial group rather than guessing at it."""
    values = list(flat)
    return [tuple(values[i:i + QUINTUPLE])
            for i in range(0, len(values) - (QUINTUPLE - 1), QUINTUPLE)]


class NotesDomain(Domain):
    """A timeline's vocabulary: the crate's ``events``, with each event's own
    parameters carried in its ``data``."""

    name = _native.EVENTS

    def __init__(self, *, tempo: float = 1.0):
        #: What a beat is worth on the view's axis. The roll draws in timeline
        #: samples and a timeline is in beats, so the crossing happens here —
        #: the editor's bridge is what supplies it.
        self.units_per_beat = 1.0
        self.tempo = float(tempo)

    def payload(self, structure, tag: str, values) -> "dict | None":
        if tag != "notes":
            return None
        held = [event for _beat, event in structure if isinstance(event, SeqEvent)]
        events = []
        for i, (start, dur, pitch, velocity, channel) in enumerate(quintuples(values)):
            was = held[i] if i < len(held) else None
            length = float(dur) / self.units_per_beat
            if was is not None:
                # **An edit updates the note it names; it does not rebuild it.**
                # Order is the only identity the payload carries, so the i-th
                # note's own event is copied and the drawn fields written over
                # it — which keeps the instrument and everything else the
                # author put there.
                params = dict(was)
                params.update(midinote=int(pitch), sustain=length)
                if int(velocity) != _velocity(was):
                    params.update(velocity=int(velocity),
                                  amp=max(0.0, min(1.0, int(velocity) / 127.0)))
            else:
                params = dict(midinote=int(pitch), dur=length, legato=1.0,
                              amp=max(0.0, min(1.0, int(velocity) / 127.0)),
                              velocity=int(velocity))
            if int(channel):
                params["channel"] = int(channel)
            events.append({"at": float(start) / self.units_per_beat,
                           "data": _plain(params)})
        return {"intent": "setevents", "events": events}

    def state(self, structure) -> list:
        """The timeline as the crate holds it."""
        return [{"at": float(beat), "data": _plain(dict(event))}
                for beat, event in structure if isinstance(event, SeqEvent)]

    def current(self, structure, payload: dict) -> "dict | None":
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        return None if edited is None else edited.get("current")

    def project(self, structure, payload: dict) -> bool:
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        if edited is None or not edited.get("applied"):
            return False
        # **What is not a note is kept.** A timeline holds OSC and MIDI items
        # too, and a roll draws none of them; rebuilding from the events alone
        # would silently drop what the view could not see.
        others = [(beat, item) for beat, item in structure
                  if not isinstance(item, SeqEvent)]
        structure.clear()
        for event in edited["state"]:
            structure.add(float(event.get("at", 0.0)),
                          SeqEvent(dict(event.get("data") or {})))
        for beat, item in others:
            structure.add(beat, item)
        return True

    def label(self, payload: dict) -> str:
        return "edit the notes"


class NotesView(View):
    """One `clausters.gui.guidef.pianoroll`: the timeline's notes on the beat
    grid."""

    #: The pitch window a roll falls back to when the timeline is empty.
    DEFAULT_PITCH = (48, 84)
    PAD = 4

    def build(self, editor) -> dict:
        from ..guidef import pianoroll, window

        wid = self.register(editor._new_id(), editor.structure)
        notes = _notes(editor)
        body: dict = {}
        if notes:
            pitches = [n[2] for n in notes]
            body["min"] = min(min(pitches) - self.PAD, self.DEFAULT_PITCH[1])
            body["max"] = max(max(pitches) + self.PAD, self.DEFAULT_PITCH[0])
        return window(pianoroll(id=wid, notes=notes or None,
                                ruler="beats", tempo=editor.tempo,
                                sample_rate=editor.sample_rate, **body),
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def props(self, editor, widget_id: int) -> dict:
        from ..guidef import _flat_notes

        return {"notes": _flat_notes(_notes(editor))}


class NotesEditor(Editor):
    """A timeline on screen, editable back into the `clausters.seq.Timeline`
    the caller already holds."""

    def __init__(self, timeline, *, sample_rate: float, tempo: float = 1.0,
                 title: str = "Notes", **options):
        domain = NotesDomain(tempo=tempo)
        super().__init__(timeline, sample_rate=sample_rate, tempo=tempo,
                         domain=domain, view=NotesView(), title=title,
                         **options)
        # The bridge is the editor's, so the domain reads it from here rather
        # than keeping a second one.
        domain.units_per_beat = self.units_per_beat


def _notes(editor) -> list:
    """The timeline's notes as the roll draws them: ``(start, dur, pitch,
    velocity, channel)`` in timeline samples."""
    out = []
    for beat, event in editor.structure:
        pitch = _pitch(event)
        if pitch is None:
            continue
        out.append((editor.beats_to_units(float(beat)),
                    editor.beats_to_units(float(beat) + _length(event))
                    - editor.beats_to_units(float(beat)),
                    pitch, _velocity(event), int(event.get("channel") or 0)))
    return out


def _length(event) -> float:
    """How long a note sounds, in beats: what it was drawn at (`sustain`) when
    something set one, else its own length."""
    for key in ("sustain", "dur"):
        value = event.get(key)
        if value is not None:
            return float(value)
    return 1.0


def _pitch(event):
    """The MIDI pitch of a timeline item, or ``None`` when it carries none — an
    OSC marker, a rest, anything that is not an event."""
    if not isinstance(event, SeqEvent) or event.get("type") == "rest":
        return None
    try:
        return float(event.midinote())
    except (KeyError, TypeError, ValueError):
        return None


def _velocity(event) -> int:
    """The MIDI velocity of a note: an explicit ``velocity``, else the linear
    ``amp`` mapped onto the velocity range, else the default."""
    vel = event.get("velocity")
    if vel is not None:
        return max(0, min(127, int(vel)))
    amp = event.get("amp")
    if amp is not None:
        return max(1, min(127, round(float(amp) * 127)))
    return 100


def _plain(value):
    """An event's parameters as plain JSON-able data — what is not, travels as
    the name that answers for it, which is the rule `to_document` already
    follows for a clang's configuration."""
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    name = getattr(value, "name", None)
    return name if isinstance(name, str) and name else None


def is_events(structure) -> bool:
    """Whether `edit` should open this as a roll."""
    return isinstance(structure, Timeline)
