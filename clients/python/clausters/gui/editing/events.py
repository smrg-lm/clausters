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
from ...seq.timeline import (MidiItem, OscItem, Timeline, item_data,
                             item_from_data)
from .domain import Domain
from .editor import Editor
from .view import View

def _label_of(item) -> "str | None":
    """The label the roll's OSC lane draws for an item, or ``None`` when the
    item is not one of that lane's — an `OscItem` labels with its address, a
    `MidiItem` with a short tag."""
    if isinstance(item, OscItem):
        return str(item.addr)
    if isinstance(item, MidiItem):
        return "midi"
    return None


class NotesDomain(Domain):
    """A timeline's vocabulary: the crate's ``events``, with each item's own
    parameters carried in its ``data``.

    **Every item is an event here, not only the notes.** A timeline holds OSC
    markers and raw MIDI beside its notes, the roll draws them in a lane of
    their own, and the crate is explicit that an event's ``data`` is the
    client's and that a lane of markers is one of the things this domain is for.
    So the state is the whole timeline and the two lanes are two *gestures* over
    it — which is what makes a marker dragged in the roll an edit with an
    inverse, instead of a picture that quietly stops agreeing with the data.
    """

    name = _native.EVENTS
    ingested = True

    def __init__(self, *, editable: bool = True):
        super().__init__()
        #: What a beat is worth on the view's axis. The roll draws in timeline
        #: samples and a timeline is in beats, so the crossing happens in the
        #: reading — the editor's bridge is what supplies this, from the
        #: timeline's own map.
        self.units_per_beat = 1.0
        #: Whether a note may be written back onto this timeline. A roll over
        #: what a **generator** produced is a rendering of an algorithm, so
        #: there is nothing to write it onto — the view says so with the
        #: widget's own ``notes_editable`` and this is the second half of it,
        #: for a host that does not read the prop.
        self.editable = bool(editable)

    def request(self, structure, tag: str, values) -> dict:
        """The report, the timeline it is over, and the axis it was drawn on.

        **The whole timeline travels, not the lane the gesture drew.** Both
        lanes state a whole-list intent, so a payload that named only the notes
        would be an edit that deletes every marker — and the reading needs the
        untouched lane in hand to carry it through.
        """
        return {"values": list(values), "state": self.state(structure),
                "unitsPerBeat": float(self.units_per_beat or 1.0),
                "editable": bool(self.editable)}

    def state(self, structure) -> list:
        """The timeline as the crate holds it — every item, notes and markers
        alike, since both are edited through this vocabulary."""
        return [{"at": float(beat), "data": _plain(item_data(item))}
                for beat, item in structure if item_data(item) is not None]

    def current(self, structure, payload: dict) -> "dict | None":
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        return None if edited is None else edited.get("current")

    def project(self, structure, payload: dict) -> bool:
        edited = _native.domain_edit(self.name, self.state(structure), payload)
        if edited is None or not edited.get("applied"):
            return False
        # **What this build cannot describe is kept.** An item that is neither
        # an event nor a marker never entered the state, so it is held aside
        # and put back rather than rebuilt from a description nobody wrote.
        others = [(beat, item) for beat, item in structure
                  if item_data(item) is None]
        # **An item the edit did not change is the same object**, matched by
        # what it says rather than by where it sits — so a marker the notes
        # gesture never touched, and a note that only moved, come out the other
        # side as themselves, keeping whatever the JSON seam cannot carry (a
        # message's arguments, an event's resolved server). Only what the
        # gesture actually rewrote is built from its description.
        held = [[_plain(item_data(item)), item] for _beat, item in structure
                if item_data(item) is not None]
        rebuilt = []
        for event in edited["state"]:
            data = event.get("data") or {}
            was = next((h for h in held if h[1] is not None and h[0] == data), None)
            if was is not None:
                item, was[1] = was[1], None
            else:
                item = item_from_data(data)
            rebuilt.append((float(event.get("at", 0.0)), item))
        # **One step, not a clear and a rebuild.** The projection runs on the
        # event loop's thread while the script may be reading the same
        # timeline, and a timeline emptied for the length of a rebuild is a
        # timeline somebody reads as empty -- see `Timeline.replace`.
        structure.replace(rebuilt + others)
        return True


class NotesView(View):
    """One `clausters.gui.guidef.pianoroll`: the timeline's notes on the beat
    grid."""

    def build(self, editor) -> dict:
        from ..guidef import _flat_notes, _flat_osc, _tempo_map, window

        # The pitch window the roll fits to its notes is the crate's, and so is
        # saying **before the hand tries** that a roll over what a generator
        # produced has nothing to write onto — the widget refuses the press
        # instead of offering a drag it will unwind.
        picture = self.catalogue(editor, "pianoroll", "roll", editor.structure, {
            "notes": _flat_notes(_notes(editor)),
            "osc": _flat_osc(_osc(editor)),
            "ruler": "beats",
            # The ruler draws the timeline's beats through the timeline's map:
            # configuration of the ruler, read from the data it shows.
            "tempo_map": _tempo_map(editor.structure.map),
            "sample_rate": editor.sample_rate,
            "editable": bool(getattr(editor.domain, "editable", True)),
        })
        return window(picture, *editor.extra,
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def props(self, editor, widget_id: int) -> dict:
        from ..guidef import _flat_notes, _flat_osc, _tempo_map

        # **Both lanes**: a correction is what the widget should be drawing, and
        # a refused marker is answered by the markers as they still are. The
        # ruler's map goes with them, so a tempo edited on the timeline redraws.
        return {"notes": _flat_notes(_notes(editor)),
                "osc": _flat_osc(_osc(editor)),
                "tempo_map": _tempo_map(editor.structure.map)}


class NotesEditor(Editor):
    """A timeline on screen, editable back into the `clausters.seq.Timeline`
    the caller already holds."""

    def __init__(self, timeline, *, sample_rate: float,
                 title: str = "Notes", editable: bool = True, **options):
        domain = NotesDomain(editable=editable)
        super().__init__(timeline, sample_rate=sample_rate,
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


def _osc(editor) -> list:
    """The timeline's OSC (and raw MIDI) items as ``(time_units, label)`` pairs
    — the roll's OSC lane. An `OscItem` labels with its address, a `MidiItem`
    with a short tag.

    The label is the whole of what the lane can say — the message's arguments
    are not drawn — which is why a marker moved or removed there is matched
    back to its item **by label**, in the crate's reading, and one added
    there is refused: the address is what a marker sends, and the lane has no
    way to type one.
    """
    out = []
    for beat, item in editor.structure:
        label = _label_of(item)
        if label is not None:
            out.append((editor.beats_to_units(float(beat)), label))
    return out


def _length(event) -> float:
    """How long a note **sounds**, in beats — `clausters.seq.Event.sustain`,
    which is ``dur * legato`` when nothing set one outright.

    That is what a roll draws and what a drag on a note's edge sets, so reading
    the explicit key alone would draw an articulated note at its grid length
    and hand the edit-back a number the hand never saw.
    """
    try:
        return float(event.sustain())
    except (KeyError, TypeError, ValueError):
        value = event.get("dur")
        return 1.0 if value is None else float(value)


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
    the name that answers for it, which is the rule the document already
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
