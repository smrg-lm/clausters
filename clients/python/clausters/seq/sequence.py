"""Event sequences: events as concrete data, held by the document.

An `EventSequence` is what a notes editor edits and what a timeline renders
into. Where a `clausters.seq.Timeline` holds **playables** -- events, patterns,
routines, other timelines, each code that runs when it plays -- a sequence
holds **events**, each with an identity of its own, in beats, with the tempo
map that times them. The step from one to the other is a render, and it goes
one way: a timeline's generators, its nesting and its tempo curve become the
events they produced, and nothing rebuilds the timeline from them.

The sequence itself lives on the Rust side (the document's ``EventSequence``);
this object is a handle to it, so every client edits the same structure and a
notes editor opened on it edits it in place, with no copy to write back.
"""

import json
import weakref
from typing import overload

from .. import _native
from .event import Event


class EventSequence:
    """A sequence of events in beats, each with an identity of its own.

    Built from ``(beat, event)`` pairs, like a `clausters.seq.Timeline`; an
    event is an `Event` or anything a dict of its keys. Iterating yields
    ``(beat, Event)`` pairs in beat order, as copies. **What a script reads
    and writes is objects**: `events` is the events as `SeqEvent`s, and
    `automation` the curves over the whole sequence, each a view of what the
    sequence holds -- so a change is made through the object it changes
    (``event.at = 2.0``, ``event["midinote"] = 62``, ``event.remove()``),
    and no call takes or answers an id.

    Args:
        events: ``(beat, event)`` pairs.
        tempo_map: the `clausters.base.TempoMap` that times the beats, or
            ``None``.
    """

    def __init__(self, events=(), *, tempo_map=None):
        data = {"events": [{"at": float(beat), "data": _keys(event)}
                           for beat, event in events]}
        if tempo_map is not None:
            data["tempo_map"] = json.loads(tempo_map.dump())
        self._seq = _native.SequenceHandle(data)

    @classmethod
    def from_data(cls, data) -> "EventSequence":
        """The sequence `data` wrote (or a bare list of ``{"at", "data"}``)."""
        sequence = cls.__new__(cls)
        sequence._seq = _native.SequenceHandle(data)
        return sequence

    def data(self) -> dict:
        """The sequence as plain data: its events with their ids, its tempo map
        and its automation -- what a session stores and `from_data` reads."""
        return self._seq.call("state")

    # ---- its structures, as objects ----

    @property
    def _objects(self) -> "weakref.WeakValueDictionary":
        """The identity map: ``(kind, id) ->`` the one object that represents
        that event or curve, while anything holds it."""
        objects = self.__dict__.get("_identity")
        if objects is None:
            objects = self.__dict__["_identity"] = weakref.WeakValueDictionary()
        return objects

    def _event(self, id: int) -> "SeqEvent":
        """The object of the event with this id -- the same one every time."""
        found = self._objects.get(("event", id))
        if found is None:
            found = SeqEvent._of(self, id)
            self._objects[("event", id)] = found
        return found

    def _curve_view(self, event: "int | None", id: int):
        """The object of curve ``id`` -- of the sequence when ``event`` is
        ``None``, else of that event -- the same one every time."""
        from ..multitrack import Automation

        found = self._objects.get(("curve", id))
        if found is None:
            found = Automation._held_by(self, event, id)
            self._objects[("curve", id)] = found
        return found

    def _curves(self, event: "int | None") -> "list | None":
        """The curves as written -- the sequence's when ``event`` is ``None``,
        else that event's, and ``None`` when it holds no such event."""
        args = {} if event is None else {"id": int(event)}
        written = self._seq.call("automation", **args)
        return None if written is None else list(written.get("automation") or ())

    def _curve(self, event: "int | None", id: int) -> "dict | None":
        """Curve ``id`` as written, while the sequence -- or, with ``event``,
        that event -- holds it: the curve holder's read, as a multitrack's."""
        for written in self._curves(event) or ():
            if int(written.get("id", 0)) == int(id):
                return written
        return None

    def _remove_curve(self, event: "int | None", id: int) -> None:
        """Remove curve ``id`` from the sequence, or from ``event``."""
        if event is None:
            intent = {"intent": "removeautomation", "curve": int(id)}
        else:
            intent = {"intent": "removeeventautomation", "id": int(event), "curve": int(id)}
        self._edit(intent, "remove a curve")

    def _ids(self, **window) -> list:
        return [int(i) for i in self._seq.call("ids", **window)["ids"]]

    @property
    def events(self) -> "SeqEvents":
        """**The events, as objects**, in beat order: a live collection --
        iterate it, index it, ask it what is `SeqEvents.at` a beat or in a
        `SeqEvents.range` -- whose members are `SeqEvent`s, each a view of
        one event the sequence holds. The same event read twice is the same
        object."""
        return SeqEvents(self)

    @property
    def automation(self) -> "SeqAutomation":
        """**The curves over the whole sequence**: a live collection of
        `clausters.multitrack.Automation` views, their points on the
        sequence's beats. The notes editor draws each as a row under the
        roll."""
        return SeqAutomation(self, None)

    # ---- reading ----

    def __len__(self) -> int:
        return int(self._seq.call("len")["len"])

    def __iter__(self):
        """``(beat, Event)`` pairs in beat order: copies of the keys, as a
        `clausters.seq.Timeline` iterates -- `events` is the events
        themselves."""
        for written in self.data().get("events", []):
            yield float(written["at"]), Event(written.get("data") or {})

    def duration(self) -> float:
        """Where the last event stops sounding, in beats."""
        return float(self._seq.call("duration")["duration"])

    @property
    def tempo_map(self):
        """The `clausters.base.TempoMap` that times the beats, or ``None``.
        Setting one is an edit."""
        written = self.data().get("tempo_map")
        return None if written is None else _native.TempoMap.load(json.dumps(written))

    @tempo_map.setter
    def tempo_map(self, value):
        written = None if value is None else json.loads(value.dump())
        self._edit({"intent": "tempo", "tempo_map": written}, "change the tempo map")

    @property
    def midi(self) -> "str | None":
        """**Which MIDI specification the sequence is written for**:
        ``"1.0"``, ``"mpe"`` or ``"2.0"`` -- or ``None``, a sequence for the
        server, where any curve is legal. It decides which curves the
        sequence can hold: per note, MIDI 1.0 says only pressure, MPE bend,
        pressure and timbre, 2.0 those and per-note controllers; over a channel
        a MIDI spec says a CC, the bend, pressure and timbre, never a bare
        ``control``. A file read is MIDI 1.0. Change it with `set_midi`."""
        written = self.data().get("midi")
        if isinstance(written, dict):
            return next(iter(written), None)
        return written

    def set_midi(self, spec: "str | None", *, upper: bool = False, members: int = 15) -> None:
        """Write the sequence for ``spec`` -- ``"1.0"``, ``"mpe"``, ``"2.0"`` or
        ``None`` -- an edit. An MPE zone is the lower one (master channel 1)
        unless ``upper``, with ``members`` member channels. `ValueError` when a
        curve the sequence holds has no spelling in that spec."""
        if spec == "mpe":
            written = {"mpe": {"upper": bool(upper), "members": int(members)}}
        else:
            written = spec
        self._edit({"intent": "midi", "midi": written}, "write it for another MIDI spec")

    # ---- editing ----

    def _apply(self, intent: dict, *, inverse: bool = True) -> dict:
        """Apply one edit in the sequence's vocabulary and answer
        ``{"applied", "current"?, "id"?}`` -- ``current`` the edit that puts it
        back, read before this one landed, unless ``inverse`` is ``False``.
        `ValueError` when refused. The door the objects write through."""
        return self._seq.call("apply", intent=intent, inverse=bool(inverse))

    def _edit(self, intent: dict, label: str) -> dict:
        """**One change a script makes through an object**: applied, and
        answered as `_apply` answers. ``label`` is what an undo would call it.

        A sequence with a history -- one an editor is open on, or one a script
        asked for `history` -- takes the change as a turn of it: recorded, and
        every view over the sequence told. One with none just changes."""
        from ..history import ATTR
        from .playback import NotesPlayback

        context = getattr(self, ATTR, None)
        if context is None:
            answer = self._apply(intent, inverse=False)
        else:
            answer = context.script_edit(self, intent, label)
        if answer.get("applied"):
            # Heard where it plays: the lane holding it takes it again, once
            # however many views over it asked.
            NotesPlayback.changed(self, None if context is None else context.version)
        return answer

    # ---- what a history asks of it ----

    def _script_key(self) -> tuple:
        """The key and the domain it joins a history under -- a notes
        editor's, so the two are one structure."""
        return f"sequence:{id(self)}", _native.EVENTS

    def _restore(self) -> dict:
        """The edit that puts the sequence back as it is now."""
        return {"intent": "restore", "sequence": self.data()}

    def _forward(self, intent: dict, answer: dict) -> dict:
        """The edit a redo applies: ``intent`` with the identity it was given,
        so a redone add brings back the same event -- and the same object. An
        edit that mints identities of its own is redone as the state it
        left."""
        minted = answer.get("id")
        kind = intent.get("intent")
        if minted is None:
            return intent
        if kind == "add":
            return {**intent, "event": {**intent["event"], "id": int(minted)}}
        if kind in ("automation", "eventautomation"):
            return {**intent, "automation": {**intent["automation"], "id": int(minted)}}
        return self._restore()

    @property
    def history(self):
        """**The sequence's history** (`clausters.history.UndoHistory`): the undo
        order its editors share, made on first ask. From then on every change
        made through the sequence's objects is an entry of it, and a turn the
        windows over the sequence see; ``with seq.history("humanize"):`` makes
        everything inside it one entry."""
        from ..history import UndoHistory

        return UndoHistory(self)

    def _write_curve(self, event: "int | None", written: dict, label: str) -> int:
        """Write a curve whole -- the sequence's when ``event`` is ``None``, else
        that event's -- and answer its id: the one it had, or a new one for a
        curve with ``id`` 0."""
        if event is None:
            intent = {"intent": "automation", "automation": written}
        else:
            intent = {"intent": "eventautomation", "id": int(event), "automation": written}
        return int(self._edit(intent, label)["id"])

    # ---- playing it ----

    def play(self, at: float = 0.0, server=None):
        """**Play the sequence on the server**, from beat ``at``: its events
        become an event lane's data on the server's notes transport, which
        plays them by its position, and the pass ends where the last note
        does. Answers that `clausters.defs.Transport` -- its ``pause``,
        ``locate``, ``loop`` and ``stop`` speak this sequence's beats, and
        ``wait()`` returns when the pass ends. A change made through the
        sequence's objects while it plays is heard from where the position is.

        One transport per server: what was playing on it gives way. With no
        ``server`` and none anywhere, the default session boots one."""
        from .playback import play_sequence

        return play_sequence(self, at, server)

    # ---- MIDI files ----

    def midi_messages(self, ppq: int = 960) -> list:
        """The sequence as the MIDI messages a file of it holds -- the render
        `to_smf` writes -- as ``(beat, bytes)`` pairs, in order, at ``ppq``
        ticks per beat. What a MIDI destination plays."""
        written = self._seq.call("midi", ppq=int(ppq))
        return [(tick / ppq, bytes(message)) for tick, message in written["events"]]

    def to_smf(self, ppq: int = 480) -> bytes:
        """The sequence as a Standard MIDI File, at ``ppq`` ticks per beat:
        every event's MIDI messages -- a note as its on and off, a ``"midi"``
        event as its message -- its automation as its channels' messages and
        its notes' as theirs, as its `midi` spec says them (MIDI 1.0
        when it names none; a 2.0 sequence as MPE), and the tempo map as the
        file's tempo. An ``"osc"`` event has no MIDI spelling and is left out,
        as is a curve the spec cannot say; a ramp is sampled where the MIDI
        value changes, and a tempo ramp is written as the step at its
        breakpoint, since a file's tempo only steps."""
        from .. import _midi

        written = self._seq.call("midi", ppq=int(ppq))
        return _midi.write_smf_tempo(written["events"], ppq, written["tempo"])

    @classmethod
    def from_smf(cls, data: bytes) -> "EventSequence":
        """The sequence a Standard MIDI File holds: its notes -- each note-on
        with the note-off that closes it -- its streams as curves (a channel's
        CC, bend and pressure as the sequence's automation; poly pressure, and
        an MPE zone's member channels, as the notes'), its other messages as
        ``"midi"`` events, in beats, with the file's tempo as the tempo map
        (its default 120 quarter notes a minute when it states none). Its
        `midi` spec is MPE when the file declares a zone, else MIDI 1.0."""
        from .. import _midi

        read = _midi.read_smf(data)
        sequence = cls()
        sequence._seq.call("loadmidi", ppq=read["ppq"], events=read["events"],
                           tempo=read["tempo"])
        return sequence

    def to_clip(self, ppq: int = 480) -> bytes:
        """The sequence as a MIDI 2.0 Clip File (SMF2CLIP), at ``ppq`` ticks
        per beat: its notes at 16-bit velocity, its automation as 32-bit
        channel messages, its notes' as per-note ones -- per-note pitch
        bend, poly pressure, the registered per-note controller 74 for timbre
        and an assignable one for a CC -- and its tempo map as Set Tempo
        messages. What its `midi` spec cannot say of one note is left out."""
        from .. import _midi

        written = self._seq.call("ump", ppq=int(ppq))
        return _midi.write_clip_ump(written["events"], ppq)

    @classmethod
    def from_clip(cls, data: bytes) -> "EventSequence":
        """The sequence a MIDI 2.0 Clip File holds: its notes, its channels'
        messages as its automation and its per-note messages as the notes',
        its Set Tempo messages as the tempo map (120 quarter notes a minute
        when it has none), and ``"2.0"`` as its `midi` spec."""
        from .. import _midi

        read = _midi.read_clip(data)
        sequence = cls()
        sequence._seq.call("loadump", ppq=read["ppq"], events=read["events"])
        return sequence

    def __repr__(self):
        return f"EventSequence({len(self)} events)"


class SeqEvent:
    """**One event of a sequence**, as an object: a live view of the event the
    sequence holds under its id, never a copy. Reading it asks the sequence,
    so after a hand moves the note in the roll the object reads where it now
    is.

    It is made by the sequence -- `EventSequence.events` hands them out, one
    object per event -- and is not built directly. Its keys read as a mapping
    (``event["midinote"]``); `event` answers a free `clausters.seq.Event`
    with the same keys, to play or to copy.

    An event the sequence no longer holds is **detached**: `sequence` is
    ``None`` and reading it raises `ValueError`. An undo that brings the event
    back brings this same object back with it.
    """

    __slots__ = ("_sequence", "_id", "__weakref__")

    @classmethod
    def _of(cls, sequence, id: int) -> "SeqEvent":
        event = cls.__new__(cls)
        event._sequence = sequence
        event._id = int(id)
        return event

    def _written(self) -> dict:
        written = self._sequence._seq.call("event", id=self._id)
        if written is None:
            raise ValueError("the sequence no longer holds this event")
        return written

    @property
    def sequence(self) -> "EventSequence | None":
        """The sequence that holds the event, or ``None`` once it holds it no
        more."""
        if self._sequence._seq.call("event", id=self._id) is None:
            return None
        return self._sequence

    def _held(self) -> "EventSequence":
        """The sequence, for a write -- which a detached event refuses."""
        if self.sequence is None:
            raise ValueError("the sequence no longer holds this event")
        return self._sequence

    @property
    def at(self) -> float:
        """Where the event sits, in the sequence's beats. Setting it moves the
        event, and it keeps its place among the events at the beat it goes
        to."""
        return float(self._written()["at"])

    @at.setter
    def at(self, beat: float) -> None:
        self._held()._edit({"intent": "move", "id": self._id, "at": float(beat)},
                           "move an event")

    def __getitem__(self, key: str):
        return (self._written().get("data") or {})[key]

    def __setitem__(self, key: str, value) -> None:
        """Write one key, with its family's coherence: a moved ``midinote``
        moves the ``freq`` and the ``degree`` the event holds."""
        self._held()._edit({"intent": "set", "id": self._id, "key": str(key),
                            "value": value}, f"set {key}")

    def remove(self) -> None:
        """Remove the event from its sequence. This object is left detached,
        and an undo that brings the event back brings it back too."""
        self._held()._edit({"intent": "remove", "id": self._id}, "remove an event")

    def get(self, key: str, default=None):
        """One key of the event, or ``default`` when it has none."""
        return (self._written().get("data") or {}).get(key, default)

    def __contains__(self, key) -> bool:
        return key in (self._written().get("data") or {})

    def __iter__(self):
        return iter(list(self._written().get("data") or {}))

    @property
    def event(self) -> Event:
        """The event's keys as a free `clausters.seq.Event` -- a copy, to play
        or to add elsewhere; changing it changes nothing here."""
        return Event(self._written().get("data") or {})

    @property
    def automation(self) -> "SeqAutomation":
        """**The curves over this event alone** -- its own automation, as MPE
        gives a note its bend, pressure and timbre: a live collection of
        `clausters.multitrack.Automation` views, each point's beat counted
        from the event's start and free to run past the note's end into its
        release."""
        return SeqAutomation(self._sequence, self._id)

    def __repr__(self) -> str:
        try:
            written = self._written()
        except ValueError:
            return "<SeqEvent, detached>"
        return f"<SeqEvent at {float(written['at']):g} {written.get('data') or {}}>"


class SeqEvents:
    """**A sequence's events, as a live collection** of `SeqEvent`s in beat
    order (`EventSequence.events`). It reads the sequence each time it is
    asked, so it is never out of date and never needs refreshing."""

    __slots__ = ("_sequence",)

    def __init__(self, sequence):
        self._sequence = sequence

    def _of(self, ids) -> "list[SeqEvent]":
        return [self._sequence._event(i) for i in ids]

    def __len__(self) -> int:
        return len(self._sequence)

    def __iter__(self):
        return iter(self._of(self._sequence._ids()))

    @overload
    def __getitem__(self, i: int) -> SeqEvent: ...
    @overload
    def __getitem__(self, i: slice) -> "list[SeqEvent]": ...

    def __getitem__(self, i):
        ids = self._sequence._ids()
        if isinstance(i, slice):
            return self._of(ids[i])
        return self._sequence._event(ids[i])

    def add(self, beat: float, event) -> SeqEvent:
        """**Add an event at** ``beat`` -- an `Event`, a dict of its keys, or
        another `SeqEvent`, whose keys are copied -- and answer the `SeqEvent`
        that is it. It goes after every event already at that beat."""
        keys = event.event if isinstance(event, SeqEvent) else event
        answer = self._sequence._edit(
            {"intent": "add", "event": {"at": float(beat), "data": _keys(keys)}},
            "add an event")
        return self._sequence._event(int(answer["id"]))

    def at(self, beat: float) -> "list[SeqEvent]":
        """The events exactly at ``beat``, in the order they were placed."""
        return self._of(self._sequence._ids(at=float(beat)))

    def range(self, t0: float, t1: float) -> "list[SeqEvent]":
        """The events in the half-open beat window ``[t0, t1)``."""
        return self._of(self._sequence._ids(**{"from": float(t0), "to": float(t1)}))

    def __repr__(self) -> str:
        return f"<{len(self)} events>"


class SeqAutomation:
    """**Curves a sequence holds, as a live collection** of
    `clausters.multitrack.Automation` views: the sequence's own
    (`EventSequence.automation`) or one event's (`SeqEvent.automation`)."""

    __slots__ = ("_sequence", "_event")

    def __init__(self, sequence, event: "int | None"):
        self._sequence = sequence
        self._event = event

    def _written(self) -> list:
        written = self._sequence._curves(self._event)
        if written is None:
            raise ValueError("the sequence no longer holds this event")
        return written

    def _views(self) -> list:
        return [self._sequence._curve_view(self._event, int(c["id"]))
                for c in self._written()]

    def __len__(self) -> int:
        return len(self._written())

    def __iter__(self):
        return iter(self._views())

    def __getitem__(self, i: int):
        return self._views()[i]

    def add(self, target, points=(), name: "str | None" = None):
        """**Add a curve** and answer it, held: the `clausters.multitrack.
        Automation` that is now a view of what the sequence holds.

        ``target`` says what it moves -- ``{"cc": 74}`` (0 to 127), ``{"bend":
        True}`` (semitones), ``{"pressure": True}``, ``{"timbre": True}`` (0 to
        1) or ``{"control": "cutoff"}``, with ``min``/``max`` to override the
        range and, on the sequence's curves, ``channel`` for the one channel
        it acts on (counted from 0, as a note's; without it, every channel).
        ``points`` are ``(beat, value)`` pairs or the document's points;
        ``name`` labels it. Or ``target`` is a free `Automation`, which is
        added as it is and becomes the view.

        `ValueError` when the sequence's `EventSequence.midi` spec cannot say
        such a curve here."""
        from ..multitrack import Automation, _points

        if isinstance(target, Automation):
            curve = target
            if curve._holder is not None:
                raise ValueError("this curve is held already: add a copy of it "
                                 "(Automation.read(curve.write()))")
            written = {**curve.write(), "id": 0}
        else:
            curve = None
            written = {"id": 0, "target": dict(target), "points": _points(points)}
            if name is not None:
                written["name"] = str(name)
        if self._event is not None:
            self._written()
        id = self._sequence._write_curve(self._event, written, "add a curve")
        if curve is None:
            return self._sequence._curve_view(self._event, id)
        curve._bind(self._sequence, self._event, id)
        self._sequence._objects[("curve", id)] = curve
        return curve

    def to_events(self, curve) -> None:
        """**Give one of the sequence's curves to the notes it reaches**: each
        note on its channel (every note, for a curve that names none) takes
        the stretch of the curve its span covers as a curve of its own --
        sounding as it did, since a channel reaches a note from its on to its
        off -- and the sequence's goes, leaving ``curve`` detached. A note
        with its own curve over that control keeps it; over a bend, which
        adds, that is a `ValueError`, as is a curve the sequence's
        `EventSequence.midi` spec cannot say of one note."""
        if self._event is not None:
            raise ValueError("an event's curves are already its notes'")
        if getattr(curve, "_holder", None) != (self._sequence, None):
            raise ValueError("not one of this sequence's curves")
        self._sequence._edit({"intent": "automationtoevents", "curve": curve.id},
                             "give a curve to the notes")

    def from_events(self, target: dict, channel: "int | None" = None):
        """**Gather the notes' curves over** ``target`` **into one of the
        sequence's** -- of the notes on ``channel``, or of every note -- and
        answer it: each note's curve over its span, on the channel its notes
        share. The notes' curves go. It holds where the notes agree: two that
        sound at once with different curves are a `ValueError`, since one
        channel cannot say both -- a chord whose curve was given to its notes
        gives it back."""
        if self._event is not None:
            raise ValueError("an event's curves gather from nothing")
        intent = {"intent": "eventstoautomation", "target": dict(target)}
        if channel is not None:
            intent["channel"] = int(channel)
        answer = self._sequence._edit(intent, "gather the notes' curves")
        return self._sequence._curve_view(None, int(answer["id"]))

    def __repr__(self) -> str:
        return f"<{len(self)} curves>"


class _Recorder:
    """A destination that keeps what plays instead of sounding it: each event
    at the beat it plays on, in the beats of the structure being rendered.

    It stands where a `clausters.defs.Server` would -- an event plays on it
    through ``play_event``, a raw OSC message through ``send_bundle`` or
    ``send_msg``, raw MIDI through ``send_message`` -- and records each one
    as the event it is, with no node, no latency and no server behind it."""

    def __init__(self):
        self.events = []

    @staticmethod
    def _now(delay: float = 0.0):
        """The beat it is, and the function that carries a beat of the clock it
        was stamped on to the rendered structure's own: a child timeline plays
        in its own beats, and a sequence is in its root's."""
        from ..base.moment import Moment

        moment = Moment.current()
        root = getattr(moment.clock, "root_beat", None)
        to_root = root if callable(root) else (lambda beat: beat)
        return to_root(moment.beat + delay), moment.beat + delay, to_root

    def play_event(self, event):
        at, local, to_root = self._now()
        keys = {k: v for k, v in event.keys_data().items() if k not in ("node", "server")}
        if keys.get("type", "note") == "note":
            # How long it sounds, in the root's beats as well.
            keys["sustain"] = to_root(local + event.sustain()) - at
        self.events.append((at, keys))
        return None

    def send_bundle(self, *messages, delay_beats: float = 0.0, clock=None, at=None):
        beat = self._now(delay_beats)[0]
        for message in messages:
            self.events.append((beat, {"type": "osc", "addr": str(message[0]),
                                       "args": list(message[1:])}))

    def send_msg(self, addr, *args):
        self.send_bundle((addr, *args))

    def send_message(self, message):
        self.events.append((self._now()[0], _native.event_of_midi(bytes(message))))


def _rendered(start, until, tempo_map) -> EventSequence:
    """Plays ``start(recorder, clock)`` on an offline session's clock and
    answers what played as a sequence. ``until`` bounds it, in the clock's
    beats; with none it runs until nothing is due, and an endless source is
    refused rather than run forever."""
    from ..render import MAX_BOUNCED_EVENTS
    from ..session import Session

    session = Session.nrt()
    recorder = _Recorder()
    with session._active():
        stop = start(recorder, session.clock)
        try:
            session.clock.render(until, max_steps=None if until is not None else MAX_BOUNCED_EVENTS)
        except RuntimeError as exc:
            if until is not None:
                raise
            raise RuntimeError(f"render_events: it did not end after {MAX_BOUNCED_EVENTS} "
                               f"events -- pass until= to bound it") from exc
        finally:
            if stop is not None:
                stop()
    data = {"events": [{"at": float(at), "data": keys} for at, keys in recorder.events]}
    if tempo_map is not None:
        data["tempo_map"] = json.loads(tempo_map.dump())
    return EventSequence.from_data(data)


def render_timeline(timeline, until=None) -> EventSequence:
    """`clausters.seq.Timeline.render_events`."""
    if timeline.transport is not None:
        raise ValueError("render_events plays a timeline on its own clock: take it off the "
                         "transport (timeline.transport = None) first")

    def start(recorder, clock):
        timeline.play(at=0.0, destination=recorder)
        return timeline.stop

    # The session's clock runs at one beat a second, so the timeline's beats
    # reach it as the seconds its own map makes of them.
    bound = None if until is None else timeline._map.secs_at(float(until))
    return _rendered(start, bound, timeline._map)


def render_pattern(pattern, until=None) -> EventSequence:
    """`clausters.seq.EventPattern.render_events`."""

    def start(recorder, clock):
        player = pattern.play(clock, recorder)
        return getattr(player, "stop", None)

    return _rendered(start, None if until is None else float(until), None)


def _keys(event) -> dict:
    """An event's keys as the document stores them."""
    if isinstance(event, Event):
        return event.keys_data()
    return Event(event).keys_data() if isinstance(event, dict) else dict(event)
