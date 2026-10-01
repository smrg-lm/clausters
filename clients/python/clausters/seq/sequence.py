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

from .. import _native
from .event import Event


class EventSequence:
    """A sequence of events in beats, each with an id.

    Built from ``(beat, event)`` pairs, like a `clausters.seq.Timeline`; an
    event is an `Event` or anything a dict of its keys. Iterating yields
    ``(beat, Event)`` pairs in beat order; `entries` adds each one's id, which is
    what the edits name an event by.

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

    # ---- reading ----

    def __len__(self) -> int:
        return int(self._seq.call("len")["len"])

    def __iter__(self):
        for _id, beat, event in self.entries():
            yield beat, event

    def entries(self) -> list:
        """Every event as ``(id, beat, Event)``, in beat order."""
        return [(e["id"], float(e["at"]), Event(e.get("data") or {}))
                for e in self.data().get("events", [])]

    def get(self, id: int) -> tuple:
        """The event with this id as ``(beat, Event)``. `KeyError` when there is
        none."""
        event = self._seq.call("event", id=int(id))
        if event is None:
            raise KeyError(id)
        return float(event["at"]), Event(event.get("data") or {})

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
        self.apply({"intent": "tempo", "tempo_map": written})

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
        self.apply({"intent": "midi", "midi": written})

    # ---- editing ----

    def apply(self, intent: dict) -> dict:
        """Apply one edit in the sequence's vocabulary (``add``, ``remove``,
        ``move``, ``set``, ``keys``, ``setevents``, ``tempo``,
        ``automation``, ``removeautomation``, ``eventautomation``,
        ``removeeventautomation``, ``automationtoevents``,
        ``eventstoautomation``, ``midi``, ``restore``) and
        answer ``{"applied", "current"}`` -- ``current`` the edit that puts it
        back, read before this one landed -- with ``"id"`` for an add, a curve
        of the sequence or of an event, or a curve gathered from the notes.
        `ValueError` when refused."""
        return self._seq.call("apply", intent=intent)

    def add(self, beat: float, event) -> int:
        """Add an event at ``beat``; its new id."""
        answer = self.apply({"intent": "add",
                             "event": {"at": float(beat), "data": _keys(event)}})
        return int(answer["id"])

    def remove(self, id: int) -> None:
        """Remove the event with this id."""
        self.apply({"intent": "remove", "id": int(id)})

    def move(self, id: int, beat: float) -> None:
        """Move the event with this id to ``beat``."""
        self.apply({"intent": "move", "id": int(id), "at": float(beat)})

    def set(self, id: int, key: str, value) -> None:
        """Write one key of an event, with its family's coherence: a moved
        ``midinote`` moves the ``freq`` and the ``degree`` the event holds."""
        self.apply({"intent": "set", "id": int(id), "key": key, "value": value})

    # ---- curves ----

    def add_automation(self, target: dict, points=(), name: str | None = None) -> int:
        """Add a curve over the whole sequence -- its automation -- and answer
        its id. ``target`` says what it moves: ``{"cc": 74}`` (0 to 127),
        ``{"bend": True}`` (semitones), ``{"pressure": True}``, ``{"timbre":
        True}`` (0 to 1) or ``{"control": "cutoff"}``, with ``min``/``max`` to
        override the range and ``channel`` for the one channel it acts on
        (counted from 0, as a note's; without it, every channel). ``points``
        are ``(beat, value)`` pairs; ``name`` labels it. The notes editor draws
        it as a row under the roll."""
        return self._curve({"intent": "automation"}, target, points, name)

    def add_event_automation(self, id: int, target: dict, points=(),
                             name: str | None = None) -> int:
        """Add a curve over the event with this id -- its own automation, as
        MPE gives a note its bend, pressure and timbre -- and answer its id.
        ``target`` as for `add_automation`; ``points`` are ``(beat, value)``
        pairs, each beat counted from the event's start, and free to run past
        the note's end into its release. The notes editor draws it inside the
        note, and a bend in the plane over the pitches it spans."""
        return self._curve({"intent": "eventautomation", "id": int(id)}, target, points, name)

    def automation_to_events(self, curve: int) -> None:
        """**Give the sequence's curve ``curve`` to the notes it reaches**:
        each note on its channel (every note, for a curve that names none)
        takes the stretch of the curve its span covers as a curve of its own --
        sounding as it did, since a channel reaches a note from its on to its
        off -- and the sequence's goes. A note with its own curve over that
        control keeps it; over a bend, which adds, that is a `ValueError`, as
        is a curve the sequence's `midi` spec cannot say of one note."""
        self.apply({"intent": "automationtoevents", "curve": int(curve)})

    def events_to_automation(self, target: dict, channel: "int | None" = None) -> int:
        """**Gather the notes' curves over** ``target`` **into one of the
        sequence's** -- of the notes on ``channel``, or of every note -- and
        answer its id: each note's curve over its span, on the channel its
        notes share. The notes' curves go. It holds where the notes agree: two
        that sound at once with different curves are a `ValueError`, since one
        channel cannot say both -- a chord whose curve was given to its notes
        gives it back."""
        intent = {"intent": "eventstoautomation", "target": dict(target)}
        if channel is not None:
            intent["channel"] = int(channel)
        return int(self.apply(intent)["id"])

    def remove_automation(self, curve: int) -> None:
        """Remove the sequence's curve with this id."""
        self.apply({"intent": "removeautomation", "curve": int(curve)})

    def remove_event_automation(self, id: int, curve: int) -> None:
        """Remove curve ``curve`` from the event with this id."""
        self.apply({"intent": "removeeventautomation", "id": int(id), "curve": int(curve)})

    def _curve(self, intent: dict, target: dict, points, name) -> int:
        automation = {"id": 0, "target": dict(target),
                      "points": [{"at": float(at), "value": float(v)} for at, v in points]}
        if name is not None:
            automation["name"] = str(name)
        return int(self.apply({**intent, "automation": automation})["id"])

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
