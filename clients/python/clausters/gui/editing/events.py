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

#: What the ``pianoroll`` widget sends and takes per note.
QUINTUPLE = 5

#: And per OSC marker: the time and the label.
PAIR = 2


def quintuples(flat) -> list:
    """A flat ``notes`` payload as ``(start, dur, pitch, velocity, channel)``
    tuples, dropping a trailing partial group rather than guessing at it."""
    values = list(flat)
    return [tuple(values[i:i + QUINTUPLE])
            for i in range(0, len(values) - (QUINTUPLE - 1), QUINTUPLE)]


def pairs(flat) -> list:
    """A flat ``osc`` payload as ``(time, label)`` tuples, dropping a trailing
    odd value the same way `quintuples` drops a partial group."""
    values = list(flat)
    return [(float(values[i]), str(values[i + 1]))
            for i in range(0, len(values) - (PAIR - 1), PAIR)]


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

    def __init__(self, *, tempo: float = 1.0, editable: bool = True):
        #: What a beat is worth on the view's axis. The roll draws in timeline
        #: samples and a timeline is in beats, so the crossing happens here —
        #: the editor's bridge is what supplies it.
        self.units_per_beat = 1.0
        self.tempo = float(tempo)
        #: Whether a note may be written back onto this timeline. A roll over
        #: what a **generator** produced is a rendering of an algorithm, so
        #: there is nothing to write it onto — the view says so with the
        #: widget's own ``notes_editable`` and this is the second half of it,
        #: for a host that does not read the prop.
        self.editable = bool(editable)
        #: What the last payload was a gesture *of*. Both lanes state the same
        #: whole-list intent, so the payload alone cannot say which hand made
        #: it, and an undo menu that called a dragged marker "edit the notes"
        #: would be naming the wrong lane.
        self._verb = "edit the notes"

    def payload(self, structure, tag: str, values) -> "dict | None":
        if not self.editable:
            return None
        if tag == "notes":
            self._verb = "edit the notes"
            return {"intent": "setevents",
                    "events": self._notes_now(structure, values)}
        if tag == "osc":
            markers = self._markers_now(structure, values)
            if markers is None:
                return None      # an unnamed marker — see `refusal`
            self._verb = "edit the markers"
            return {"intent": "setevents", "events": markers}
        return None

    def refusal(self, structure, tag: str, values) -> "str | None":
        """Why a marker gesture this domain understands cannot be written.

        A marker *is* a message, and the address is the whole of what it sends;
        the roll has no way to type one, so a marker added there has nothing to
        become. Saying so is the point — a picture that springs back with
        nothing attached teaches "sometimes it does not work" rather than "not
        here".
        """
        if tag == "osc" and self.editable and \
                self._markers_now(structure, values) is None:
            return ("a marker is the message it sends, and a roll cannot say "
                    "which: add it with timeline.add(beat, OscItem(addr, ...)) "
                    "and drag it here")
        return None

    def _notes_now(self, structure, values) -> list:
        """The whole timeline after a ``notes`` gesture: the drawn notes, with
        every marker left exactly where it is."""
        held = [item for _beat, item in structure if isinstance(item, SeqEvent)]
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
        return events + self._kept(structure, lambda item: isinstance(item, SeqEvent))

    def _markers_now(self, structure, values) -> "list | None":
        """The whole timeline after an ``osc`` gesture — the notes untouched and
        the markers as the lane now holds them — or ``None`` when the gesture
        added one that has no message to send.

        **A marker is matched by its label**, which is its address, and only
        then by order among the ones that share it. The payload carries the
        label the lane drew, so the message a marker sends survives being
        dragged and — unlike the notes one lane up, where order is the only
        identity there is — survives a *neighbour* being removed as well.
        """
        held = [(beat, item) for beat, item in structure
                if _label_of(item) is not None]
        taken = set()
        markers = []
        for time, label in pairs(values):
            was = next((i for i, (_b, item) in enumerate(held)
                        if i not in taken and _label_of(item) == label), None)
            if was is None:
                return None
            taken.add(was)
            markers.append({"at": float(time) / self.units_per_beat,
                            "data": _plain(item_data(held[was][1]))})
        return self._kept(structure,
                          lambda item: _label_of(item) is not None) + markers

    @staticmethod
    def _kept(structure, drawn) -> list:
        """The items the gesture did **not** draw, as the crate holds them —
        what keeps the lane nobody touched out of the edit that rebuilt the
        other one, and out of the inverse that puts it back."""
        return [{"at": float(beat), "data": _plain(item_data(item))}
                for beat, item in structure
                if not drawn(item) and item_data(item) is not None]

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
        structure.clear()
        for event in edited["state"]:
            data = event.get("data") or {}
            was = next((h for h in held if h[1] is not None and h[0] == data), None)
            if was is not None:
                item, was[1] = was[1], None
            else:
                item = item_from_data(data)
            structure.add(float(event.get("at", 0.0)), item)
        for beat, item in others:
            structure.add(beat, item)
        return True

    def label(self, payload: dict) -> str:
        return self._verb


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
        # **Say it before the hand tries.** A roll over what a generator
        # produced has nothing to write onto, so the widget refuses the press
        # instead of offering a drag it will unwind.
        if not getattr(editor.domain, "editable", True):
            body["notes_editable"] = False
        osc = _osc(editor)
        return window(pianoroll(id=wid, notes=notes or None, osc=osc or None,
                                ruler="beats", tempo=editor.tempo,
                                sample_rate=editor.sample_rate, **body),
                      *editor.extra,
                      title=editor.title, w=editor.size[0], h=editor.size[1],
                      layout="col")

    def props(self, editor, widget_id: int) -> dict:
        from ..guidef import _flat_notes, _flat_osc

        # **Both lanes**: a correction is what the widget should be drawing, and
        # a refused marker is answered by the markers as they still are.
        return {"notes": _flat_notes(_notes(editor)),
                "osc": _flat_osc(_osc(editor))}


class NotesEditor(Editor):
    """A timeline on screen, editable back into the `clausters.seq.Timeline`
    the caller already holds."""

    def __init__(self, timeline, *, sample_rate: float, tempo: float = 1.0,
                 title: str = "Notes", editable: bool = True, **options):
        domain = NotesDomain(tempo=tempo, editable=editable)
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


def _osc(editor) -> list:
    """The timeline's OSC (and raw MIDI) items as ``(time_units, label)`` pairs
    — the roll's OSC lane. An `OscItem` labels with its address, a `MidiItem`
    with a short tag.

    The label is the whole of what the lane can say — the message's arguments
    are not drawn — which is why a marker moved or removed there is matched
    back to its item **by label** (`NotesDomain._markers_now`) and one added
    there is refused: the address is what a marker sends, and the lane has no
    way to type one.
    """
    out = []
    for beat, item in editor.structure:
        if isinstance(item, OscItem):
            out.append((editor.beats_to_units(float(beat)), str(item.addr)))
        elif isinstance(item, MidiItem):
            out.append((editor.beats_to_units(float(beat)), "midi"))
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
