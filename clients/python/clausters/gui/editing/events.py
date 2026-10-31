"""Editing an event sequence on a roll: the notes editor.

What it opens is a `clausters.seq.EventSequence` -- events as concrete data,
each with an id. `clausters.gui.edit` opens a `clausters.seq.Timeline` by
rendering it first (`Timeline.render_events`): the roll edits the events the
timeline produced, never the timeline.

**The roll is an application of the GUI host's**, as the audio editor and the
multitrack are: the window, what each gesture does to the sequence, the entry it
leaves, and the playback all run in the host's process, and this is its handler
(`clausters.gui.editing.HostedEditor`). The script's sequence is the window's
while one is open, and when the last window over it goes it holds what that
window left it as.

**It sounds in the application**: the sequence is the data of an event lane on
a transport of the window's own, so playing it moves no multitrack and no other
window, and a pause, a stop and a locate are the transport's. An edit sends the
lane its new data, so a note moved ahead of the line is heard where it lands,
and what is sounding keeps its release. The space bar over the window plays and
pauses. A note names the def it plays with: the application's engine knows
``default``, and a def sent to a client's server is not there.
"""

from ...seq.sequence import EventSequence
from ...seq.timeline import Timeline
from .hosted import HostedEditor, HostedTransport, _Block, _Held, _RemoteHistory
from .marking import Marking


class _RemoteSequence:
    """**The sequence while a window has it**: each verb of the sequence's
    handle (`clausters._native.SequenceHandle`) asked of the application."""

    def __init__(self, held: _Held, local):
        self._held = held
        #: The handle it had, read again when the last window goes.
        self.local = local

    def call(self, verb: str, **args):
        """One verb: its answer. Raises for a refusal."""
        return self._held.live()._handle({"verb": verb, **args})


def _held_of(sequence) -> _Held:
    """Who has ``sequence``: made once, per sequence."""
    held = sequence.__dict__.get("_windows")
    if held is None:
        held = sequence.__dict__["_windows"] = _Held()
    return held


class SequenceHandler(HostedEditor):
    """**A window that shows a sequence**, a roll or a page over one: the
    sequence the window's from when the first opens, and the changes a script
    makes through its objects entries of the history the window walks."""

    #: The sequence the window shows, or ``None``.
    _shown = None

    def _held(self) -> _Held:
        return _held_of(self._shown)

    def _take(self) -> None:
        self._shown._seq = _RemoteSequence(self._held(), self._shown._seq)

    def _let_go(self, left: "dict | None") -> None:
        """The last window over the sequence went: it holds what that window
        left it as."""
        from ... import _native

        handle = self._shown._seq
        if not isinstance(handle, _RemoteSequence):
            return
        written = (left or {}).get("sequence")
        self._shown._seq = handle.local if written is None else _native.SequenceHandle(written)

    def _handle(self, call: dict, *, label: "str | None" = None):
        """One verb of the sequence's handle, asked of the window's."""
        args = {"call": call}
        if label is not None:
            args["label"] = label
        return self._do("handle", **args)

    def _script_edit(self, sequence, intent: dict, label: str) -> dict:
        """A change the script's sequence makes: an edit of the window's,
        under ``label``, or a leg of the ``with sequence.history(...)`` block
        open over it."""
        answer = self._handle({"verb": "apply", "intent": intent, "inverse": True})
        if answer.get("applied"):
            leg = {"forward": sequence._forward(intent, answer),
                   "backward": answer.get("current")}
            if self._legs is not None:
                self._legs.append(leg)
            else:
                self._handle({"verb": "record", "legs": [leg]}, label=label)
        return answer

    def _block(self, label: str) -> _Block:
        return _Block(self, label)

    @property
    def _history(self) -> _RemoteHistory:
        return _RemoteHistory(self)


class NotesEditor(Marking, SequenceHandler):
    """**An event sequence on a roll, in the GUI host's application, and this
    script its handler.**

    **What is marked** (`selected`, `select`, `unselect`) is the events on
    the roll -- by a click, Shift+click or a marquee -- as the
    `clausters.seq.SeqEvent` objects they are, in beat order. The window's,
    not the sequence's: it enters no history and each window has its own, and
    it is empty with no window open. (The time range a sweep leaves is the
    transport's `HostedTransport.span`.)

    Args:
        sequence: the `clausters.seq.EventSequence` to edit. It is the edited
            one: read it after any gesture.
        editable: ``False`` for a roll a hand may look at and not edit -- the
            notes of a rendering.
        y_axis: what the roll's vertical axis is: ``"midi"``, MIDI notes on
            the keys, snapped to semitones; or ``"hz"``, frequency on a log
            scale, ruled in hertz, where a note moves continuously and writes
            its ``freq``.
        title: the window's title.
        width, height: the window's size; left out, the application's.
        chrome: ``False`` opens the roll with no menu bar and no toolbar, the
            keys kept.
        midi_in: ``True`` for a roll that **paints what is played**: the host
            opens its MIDI input (a port named ``clausters-gui`` to route a
            keyboard into) and writes each incoming note into the roll -- at
            the play cursor while the transport rolls, held as long as the
            key, or on the step cursor while it is stopped. Each one is an
            edit of the sequence, like a note a hand drew: recorded, and
            undone with the rest.
    """

    _app = "notes"

    def __init__(self, sequence, *, editable: bool = True, y_axis: str = "midi",
                 title: str = "Notes", width: "int | None" = None,
                 height: "int | None" = None, chrome: bool = True,
                 midi_in: bool = False):
        super().__init__(title=title, width=width, height=height, chrome=chrome)
        #: The sequence the roll edits, the one the editor was opened over.
        self.structure = sequence
        self._shown = sequence
        self.editable = bool(editable)
        #: Whether the roll paints incoming MIDI into the sequence.
        self.midi_in = bool(midi_in)
        #: The roll's vertical axis, ``"midi"`` or ``"hz"``.
        self.y_axis = str(y_axis)
        #: The timeline a play to a destination of its own (a MIDI port) runs.
        self._elsewhere = None
        #: Where a pass ends (`end`).
        self._end = None

    def _options(self, beside) -> dict:
        options = {"domain": self.y_axis, "editable": self.editable,
                   "midi_in": self.midi_in}
        if beside is None:
            options["sequence"] = self.structure.data()
        return options

    def _settle(self, left: "dict | None", last: bool) -> None:
        if last:
            self._let_go(left)

    @property
    def sequence(self) -> EventSequence:
        """The sequence the roll edits -- the one the editor was opened over."""
        return self.structure

    def tempo_map(self):
        """The sequence's own tempo map, which the roll's axis is drawn
        through."""
        return self.structure.tempo_map

    # ---- playing it ----

    @property
    def transport(self) -> HostedTransport:
        """**The transport the window plays on** (`HostedTransport`), as the
        object a script plays, its verbs about this window's pass and in the
        sequence's beats -- the span is the band an Alt+drag sweeps on the
        roll, the loop switch is `L`, and each side reads what the other set.
        Each window over the sequence has its own, given back with it."""
        return HostedTransport(self)

    @property
    def end(self):
        """**Where a pass ends**, as on a multitrack's transport: ``None`` by
        default -- the transport rolls on past the last note until it is
        stopped -- or ``"contents"``, where the last note ends (its onset and
        its length), or a beat, an **end marker**; either of the last two goes
        back to the position cursor. A note's release rings out past the end,
        since a stop releases the notes rather than freezing them."""
        return self._end

    @end.setter
    def end(self, end) -> None:
        self._end = end
        self._do("end", end=end)

    def play(self, beat: "float | None" = None, destination=None, *,
             range=None, looping: bool = False) -> "NotesEditor":
        """**Play the sequence** from ``beat`` -- or from the position cursor,
        or the start -- on its window's transport. Returns ``self``.

        It is the audio editor's pass: ``range`` -- ``(start, end)`` in beats,
        a time range a sweep left -- plays from its start to its end, going
        back to ``beat``; ``looping`` loops the range, or with none every note;
        with neither the pass ends where `end` says.

        ``destination`` is for a MIDI port (a `clausters.base.MidiServer`): the
        application has no MIDI output, so the sequence is played on this
        client's clock to that destination instead, as the MIDI messages a file
        of it holds (`clausters.seq.EventSequence.midi_messages`) -- its
        automation and its notes' included -- and an edit is heard from the
        next play."""
        if destination is not None:
            from ...seq.event import MidiItem

            played = Timeline([(at, MidiItem(message))
                               for at, message in self.structure.midi_messages()])
            if self.structure.tempo_map is not None:
                played.map = self.structure.tempo_map
            played.play(at=float(beat if beat is not None else 0.0), destination=destination)
            self._elsewhere = played
            return self
        args = {"looping": bool(looping)}
        if beat is not None:
            args["beat"] = float(beat)
        if range is not None:
            args["range"] = [float(range[0]), float(range[1])]
        self._do("play", **args)
        return self

    def pause(self) -> "NotesEditor":
        """Pause where it stands: a `resume` carries the notes on."""
        if self._elsewhere is not None:
            self._elsewhere.pause()
            return self
        self._do("pause")
        return self

    def resume(self) -> "NotesEditor":
        """Roll again from where it paused."""
        if self._elsewhere is not None:
            self._elsewhere.play()
            return self
        self._do("resume")
        return self

    def stop(self) -> "NotesEditor":
        """Stop, free what sounds, and go back to where it started."""
        if self._elsewhere is not None:
            self._elsewhere.stop()
            self._elsewhere = None
            return self
        self._do("stop")
        return self

    @property
    def playing(self) -> bool:
        """Whether the sequence is sounding, as the engine answers -- a pass
        that ended on its mark stopped without anybody here saying so."""
        if self.closed:
            return False
        return bool(self._do("playing"))

    def locate(self, at: float) -> None:
        """The position cursor to beat ``at``: a stopped transport is cued
        there, so the play cursor goes with it and the next play starts from
        the mark; a rolling pass moves with it."""
        self._do("locate", at=float(at))

    def split(self) -> bool:
        """Split what is held at the cursor -- the window's ``split`` (`verb`)."""
        return self.verb("split")

    def join(self) -> bool:
        """Join a touching run of what is held -- the window's ``join`` (`verb`)."""
        return self.verb("join")

    def quantize(self) -> bool:
        """Quantize what is selected to the grid -- the window's ``quantize`` (`verb`)."""
        return self.verb("quantize")

    @property
    def grid(self) -> float:
        """**The grid a note lands on, in beats** -- ``0.25`` for a
        sixteenth, its default, and ``0`` for none. A grid is a length in
        beats, read through the tempo map, so its lines move with the tempo:
        what a drag snaps to and `quantize` writes into the sequence. The
        window's own, like its zoom: setting it is no edit. Set it to choose
        another: ``editor.grid = 0.5``."""
        return float(self._call("grid").get("grid", 0.0))

    @grid.setter
    def grid(self, beats: float) -> None:
        self._set(grid=float(beats))

    @property
    def snap_to_grid(self) -> bool:
        """**Whether a hand snaps to the grid** -- the snap-to-grid switch,
        on until it is turned off. Off, a drag, a paste and a split land where
        the hand puts them; `quantize` still moves onto the grid. The window's
        own, like its grid: G in the window and the View menu flip it, and
        setting it is no edit: ``editor.snap_to_grid = False``."""
        return bool(self._call("grid").get("snap", True))

    @snap_to_grid.setter
    def snap_to_grid(self, on: bool) -> None:
        self._set(snap=bool(on))

    # ---- the roll's two selections ----

    def show_span(self, span) -> None:
        """Draw ``span`` -- ``(start, end)`` in beats, or ``None`` -- as the
        roll's time range, the band a sweep leaves, and play it on the space
        bar."""
        written = None if span is None else [float(span[0]), float(span[1])]
        self._do("set", call={"verb": "span", "span": written})

    def _marked(self) -> list:
        """The events marked on the roll, by the ids the host holds."""
        if self.closed:
            return []
        marked = self._query("selected").get("selected") or []
        held = {event._id: event for event in self.structure.events}
        return [held[int(i)] for i in marked if int(i) in held]

    def _mark(self, events) -> None:
        """Mark ``events`` -- those of this sequence -- on the roll."""
        if self.closed:
            return
        self._do("select", regions=[
            int(event._id) for event in events or ()
            if getattr(event, "_sequence", None) is self.structure])


def is_events(structure) -> bool:
    """Whether `edit` opens this in the notes editor: an event sequence, or a
    timeline, which it renders into one."""
    return isinstance(structure, (EventSequence, Timeline))


__all__ = ["NotesEditor", "SequenceHandler", "is_events"]
