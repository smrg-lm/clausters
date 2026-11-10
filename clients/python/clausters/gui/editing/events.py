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
        """Join what is selected -- the window's ``join`` (`verb`): notes of
        one pitch that touch become one note, as a split's two halves do, and
        any other selection of a line is joined, each note going on into the
        next without a new attack (`clausters.seq.EventSequence.join`)."""
        return self.verb("join")

    def unjoin(self) -> bool:
        """Take the joins among what is selected away -- the window's
        ``unjoin`` (`verb`, Shift+J), `clausters.seq.EventSequence.unjoin`
        over the roll's selection."""
        return self.verb("unjoin")

    def step_up(self) -> bool:
        """Move what is selected up -- every note the roll takes when nothing
        is: a semitone in MIDI mode, a step of the key in notation mode, the
        letter after with the sign the key writes on it. The window's
        ``step_up`` (`verb`, Up); one entry of the history."""
        return self.verb("step_up")

    def step_down(self) -> bool:
        """Move what is selected down, as `step_up` moves it up -- the
        window's ``step_down`` (`verb`, Down)."""
        return self.verb("step_down")

    def octave_up(self) -> bool:
        """Move what is selected up an octave, every note the roll takes when
        nothing is, its letters kept -- the window's ``octave_up`` (`verb`,
        Ctrl+Up)."""
        return self.verb("octave_up")

    def octave_down(self) -> bool:
        """Move what is selected down an octave -- the window's
        ``octave_down`` (`verb`, Ctrl+Down)."""
        return self.verb("octave_down")

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

    @property
    def monophonic(self) -> bool:
        """**Whether the roll edits one note at a time in a line** -- off
        until it is turned on. On, a gesture never leaves two notes of one
        line (a staff and voice, else a channel) overlapping: a note lengthened
        or moved over the next shortens it from its start, removes it where it
        covers it whole, and shortens the one before it to its onset, all in
        the gesture's one edit. A length that reaches the next note does not
        join them. The window's own, like its grid: M in the window and the
        Edit menu flip it, and setting it is no edit:
        ``editor.monophonic = True``."""
        return bool(self._call("editing").get("monophonic", False))

    @monophonic.setter
    def monophonic(self, on: bool) -> None:
        self._set(monophonic=bool(on))

    # ---- the roll's two modes ----

    @property
    def mode(self) -> str:
        """**The roll's mode**: ``"midi"``, the events as MIDI says them --
        notes on and off, velocities, channels, curves, joins -- or
        ``"notation"``, a view of the page they are written as: each note on a
        staff and a voice, as long as it is written with what it sounds
        inside it, a velocity shown and edited as a dynamic, and what it
        sounds derived from what is written. The window's, not the
        sequence's: setting it is no edit, but for **entering notation over a
        sequence written as no notation**, which converts it in place first
        with the reading it stores (`to_notation`, one entry of the history)
        -- where a hand entering notation in the window is asked how first.
        Going to MIDI converts nothing."""
        return str(self._call("editing").get("mode", "midi"))

    @mode.setter
    def mode(self, mode: str) -> None:
        if mode not in ("midi", "notation"):
            raise ValueError(f"a roll's mode is 'midi' or 'notation', not {mode!r}")
        # the window's verb asks a hand how to convert; a script has said
        if mode == "notation" and not self._call("editing").get("written", True):
            self.structure.to_notation()
        self.verb(f"{mode}_mode")

    def to_notation(self, *, meter: "str | None" = None, division: "int | None" = None,
                    writing: "str | None" = None, copy: bool = False):
        """**Write the sequence as notation and edit it so**: read with
        ``meter`` (``"3/4"``) and ``division`` (``16`` for a sixteenth) over
        its stored reading, each part written in ``"voices"`` or with
        ``"ties"`` -- `clausters.seq.EventSequence.to_notation` -- and the roll
        put in notation mode. Returns the sequence. With ``copy=True`` the
        sequence is left as it is and a converted copy is returned
        (`clausters.seq.EventSequence.notated`), which an editor of its own
        opens."""
        reading = {k: v for k, v in (("meter", meter), ("division", division)) if v is not None}
        if copy:
            return self.structure.notated(reading or None, writing)
        self.structure.to_notation(reading or None, writing)
        self.mode = "notation"
        return self.structure

    def to_midi(self) -> None:
        """The roll in MIDI mode: nothing is converted, since the events
        already hold what they sound."""
        self.mode = "midi"

    @property
    def scope(self):
        """**What the roll shows in notation mode**: ``"system"``, every
        staff; ``("part", n)``, the staves of part ``n``; or ``("staff",
        n)``. The window's, like its zoom: setting it is no edit. The View's
        Notation menu has it as Show the system, the part or the staff."""
        said = self._call("editing").get("scope") or {"show": "system"}
        if said.get("show", "system") == "system":
            return "system"
        return (said["show"], int(said.get("index", 0)))

    @scope.setter
    def scope(self, scope) -> None:
        if scope == "system":
            said = {"show": "system"}
        else:
            kind, index = scope
            if kind not in ("part", "staff"):
                raise ValueError(f"a roll shows the 'system', a 'part' or a 'staff', not {kind!r}")
            said = {"show": kind, "index": int(index)}
        self._set(scope=said)

    @property
    def voice(self) -> tuple:
        """**The voice the roll edits in notation mode**, ``(staff,
        voice)`` counted from 0: what a note drawn is written in, and the one
        voice a press takes -- the others are drawn faint. Ctrl+Alt+1 to 4
        choose the voice, and the Notation menu the staff. No edit."""
        staff, voice = self._call("editing").get("voice", [0, 0])
        return (int(staff), int(voice))

    @voice.setter
    def voice(self, voice) -> None:
        self._set(voice=[int(voice[0]), int(voice[1])])

    @property
    def writing(self) -> str:
        """**How the sequence's parts write their polyphony**: ``"voices"``
        or ``"ties"`` -- the same music, written two ways. Setting it writes
        every part so, one edit of the parts that changes no event and nothing
        heard."""
        parts = self.structure.parts
        return str(parts[0].get("writing", {}).get("polyphony", "voices")) if parts else "voices"

    @writing.setter
    def writing(self, writing: str) -> None:
        if writing not in ("voices", "ties"):
            raise ValueError(f"a writing is 'voices' or 'ties', not {writing!r}")
        self.verb(f"write_{writing}")

    def slur(self) -> bool:
        """A slur over what is selected, from its first note to its last --
        the window's ``slur`` (S), in notation mode; joined where the roll
        edits monophonically, attacked otherwise. Again over the same notes,
        it is taken away."""
        return self.verb("slur")

    def slur_join(self) -> bool:
        """Turn the slurs on what is selected from joined to attacked, or
        back -- the window's ``slur_join`` (Shift+S)."""
        return self.verb("slur_join")

    def tie(self) -> bool:
        """Tie the selected notes of one pitch that touch into one -- the
        window's ``tie`` (T): a tied note is one event, which the page writes
        with ties."""
        return self.verb("tie")

    def staccato(self) -> bool:
        """A staccato on each selected note, or none where all have one --
        the window's ``staccato`` (.)."""
        return self.verb("staccato")

    def pedal(self) -> bool:
        """The pedal down from the first selected note to the last -- the
        window's ``pedal`` (P)."""
        return self.verb("pedal")

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
