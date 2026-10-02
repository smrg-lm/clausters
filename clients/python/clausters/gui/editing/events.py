"""Editing an event sequence on a roll: the notes editor.

What it opens is a `clausters.seq.EventSequence` -- events as concrete data,
each with an id -- and it edits that sequence **in place**: the editor in the
shared crate (the ``openNotes`` member of `clausters._native.EditingCore`) holds
the very sequence the script's handle names, so every edit is read back through
the handle and there is nothing to write back. `clausters.gui.edit` opens a
`clausters.seq.Timeline` by rendering it first (`Timeline.render_events`): the
roll edits the events the timeline produced, never the timeline.

**The editor is the crate's**: the window, the notes with their ids, what each
gesture does to the sequence, the entry it leaves and the corrections it
answers with. What is here is what a language owns -- the socket, and handing
the crate the window it is open in.

**It sounds through a playback of its own** (the crate's ``NotesPlayback``): the
sequence is the data of an event lane on the notes editor's own transport, so
playing it never moves a multitrack, and the server plays it by the
transport's position -- a pause, a stop and a locate are the transport's. An
edit sends the lane its new data, so a note moved ahead of the line is heard
where it lands, and what is sounding keeps its release. The space bar over the
window plays and pauses.
"""


from ... import _native
from ...seq.playback import NotesPlayback
from ...seq.sequence import EventSequence
from ...seq.timeline import Timeline
from .domain import Domain
from .editor import Editor
from .samples import _plain
from .view import View


class NotesDomain(Domain):
    """A sequence's vocabulary, the crate's ``events``. A step is applied by the
    crate to the sequence it shares, so there is nothing here to carry out."""

    name = _native.EVENTS
    ingested = True


class NotesView(View):
    """The roll: one ``notes`` widget, composed by the crate."""

    def build(self, editor) -> dict:
        # The axis names which roll of the sequence this is: a roll in hertz is
        # another picture of it, and a window beside one in MIDI notes must not
        # draw on its widget.
        wid = self.widget(editor, "notes", editor.structure, editor.y_axis)
        editor._roll = wid
        editor._sync_core()
        tree = editor._call("window", widget=wid)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class NotesEditor(Editor):
    """An event sequence on a roll, edited note by note, in place.

    Args:
        sequence: the `clausters.seq.EventSequence` to edit. It is the edited
            one: read it after any gesture.
        sample_rate: the rate the roll's axis counts in.
        editable: ``False`` for a roll a hand may look at and not edit -- the
            notes of a rendering.
        y_axis: what the roll's vertical axis is: ``"midi"``, MIDI notes on
            the keys, snapped to semitones; or ``"hz"``, frequency on a log
            scale, ruled in hertz, where a note moves continuously and writes
            its ``freq``.
        title: the window's title.
        server: the `clausters.defs.Server` it plays on; ``None`` resolves the
            ambient one when it first plays.
    """

    def __init__(self, sequence, *, sample_rate: float, editable: bool = True,
                 y_axis: str = "midi", title: str = "Notes", server=None,
                 **options):
        domain = NotesDomain()
        super().__init__(sequence, sample_rate=sample_rate, domain=domain,
                         view=NotesView(), title=title, **options)
        self.editable = bool(editable)
        #: The roll's vertical axis, ``"midi"`` or ``"hz"``.
        self.y_axis = str(y_axis)
        self._server = server
        #: The timeline a play to a destination of its own (a MIDI port) runs.
        self._elsewhere = None
        #: Where a pass ends (`end`).
        self._end = None
        #: The roll's widget id, once drawn.
        self._roll = None
        self._member, self._structure_id = self._editing.open_notes(
            f"sequence:{id(sequence)}", sequence,
            {"rate": self.sample_rate, "editable": self.editable,
             "domain": self.y_axis, "title": self.title, "w": int(self.size[0]), "h": int(self.size[1])},
            domain)

    @property
    def sequence(self) -> EventSequence:
        """The sequence the roll edits -- the one the editor was opened over."""
        return self.structure

    def tempo_map(self):
        """The sequence's own tempo map, which the roll's axis is drawn
        through."""
        return self.structure.tempo_map

    def _call(self, verb: str, **args) -> dict:
        """One verb of this editor's member, through the context."""
        return self._editing.member(self._member, verb, **args)

    def _sync_core(self) -> None:
        """Hand the crate the window it is open in and the chrome."""
        self._call("sync", window=self._window, rate=self.sample_rate,
                   editable=self.editable, title=self.title,
                   w=int(self.size[0]), h=int(self.size[1]))

    def open(self, host=None, id: "int | None" = None):
        """Open the window, with its play cursor drawn from the transport.

        The roll anchors the play cursor at 0, and the counter that makes that
        the sequence's own sample is the position of the transport the notes
        editor plays on -- stopped or rolling, the line is where the lane is.
        With no server to play on there is no position, and no line."""
        window = super().open(host, id)
        if self._host is not None and window is not None:
            try:
                transport = self._playback.transport_id
            except RuntimeError:
                return window
            self._host.head_clock(window, "transport", transport)
        return window

    def locate(self, at: float) -> None:
        """The position cursor was placed at beat ``at``: a stopped transport
        is cued there, so the play cursor goes with it and the next play starts
        from the mark; a rolling pass is left alone."""
        if self._server is None or self._elsewhere is not None:
            return
        self._playback.call("cue", self.structure, at=float(at))

    # ---- playing it ----

    @property
    def _playback(self) -> NotesPlayback:
        if self._server is None:
            from ...base.main import main

            self._server = main.resolve_server()
        return NotesPlayback.of(self._server)

    @property
    def transport(self):
        """**The transport the sequence plays on**, as the object a script
        plays: the `clausters.defs.Transport` `clausters.play` of a sequence
        answers, its verbs (``play``, ``pause``, ``stop``, ``locate``,
        ``loop``, ``wait``) and ``span`` about this editor's sequence and in
        its beats -- the span is the band an Alt+drag sweeps on the roll, the
        loop switch is `L`, and each side reads what the other set. Every
        sequence on a server shares it: asking for it makes this one the
        sequence it plays, from the position cursor."""
        playback = self._playback
        playback.hold(self.structure, at=float(self.cursor or 0.0), end=self._end)
        return playback.transport

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
        if self._server is not None:
            self._playback.call("end", self.structure, end=end)

    def play(self, beat: "float | None" = None, destination=None, *,
             range=None, looping: bool = False) -> "NotesEditor":
        """**Play the sequence** from ``beat`` -- or from the position cursor,
        or the start -- on the notes editor's own transport. Returns ``self``.

        It is the audio editor's pass: ``range`` -- ``(start, end)`` in beats,
        a time range a sweep left -- plays from its start to its end, going
        back to ``beat``; ``looping`` loops the range, or with none every note;
        with neither the pass ends where `end` says.

        ``destination`` is for a MIDI port (a `clausters.base.MidiServer`): the
        server has no MIDI output, so the sequence is played on this client's
        clock to that destination instead, as the MIDI messages a file of it
        holds (`clausters.seq.EventSequence.midi_messages`) -- its automation
        and its notes' included -- and an edit is heard from the next
        play."""
        start = float(beat if beat is not None else (self.cursor or 0.0))
        if destination is not None:
            from ...seq.event import MidiItem
            from ...seq.timeline import Timeline

            # The render a file of it holds: its notes, its automation and
            # its notes', as its MIDI spec says them.
            played = Timeline([(beat, MidiItem(message))
                               for beat, message in self.structure.midi_messages()])
            if self.structure.tempo_map is not None:
                played.map = self.structure.tempo_map
            played.play(at=start, destination=destination)
            self._elsewhere = played
            return self
        self._playback.load(self.structure, start, range=range, looping=looping,
                            end=self._end)
        return self

    def pause(self) -> "NotesEditor":
        """Pause where it stands: a `resume` carries the notes on."""
        if self._elsewhere is not None:
            self._elsewhere.pause()
            return self
        self._playback.call("pause", self.structure)
        return self

    def resume(self) -> "NotesEditor":
        """Roll again from where it paused."""
        if self._elsewhere is not None:
            self._elsewhere.play()
            return self
        self._playback.call("resume", self.structure)
        return self

    def stop(self) -> "NotesEditor":
        """Stop, free what sounds, and go back to where it started."""
        if self._elsewhere is not None:
            self._elsewhere.stop()
            self._elsewhere = None
            return self
        playback = self._playback
        playback.call("stop", self.structure, back=float(self.cursor or 0.0))
        return self

    @property
    def playing(self) -> bool:
        """Whether the sequence is sounding, as the engine answers -- a pass
        that ended on its mark stopped without anybody here saying so."""
        if self._server is None:
            return False
        playing = bool(self._playback.state().get("playing"))
        self._playback.call("setRolling", self.structure, rolling=playing)
        return playing

    def reflect_step(self) -> None:
        """A history step landed: the window is corrected, and the lane takes
        the sequence again, so the undo is heard."""
        super().reflect_step()
        self._update()

    # ---- the roll's two selections ----

    def show_span(self, span) -> None:
        """Draw ``span`` -- ``(start, end)`` in beats, or ``None`` -- as the
        roll's time range, the band a sweep leaves, and play it on the space
        bar. What `clausters.defs.Transport.span` sets on every roll over the
        sequence it plays."""
        self._call("span", span=None if span is None else [float(span[0]), float(span[1])])
        super().adopt()

    def show_looping(self, on: bool) -> None:
        """The window's loop switch, as `L` leaves it: what
        `clausters.defs.Transport.loop` turns on every roll over the
        sequence it plays."""
        if self._host is not None and self._window is not None and self._roll is not None:
            self._host.set(self._roll, looping=1 if on else 0)

    @property
    def selected(self) -> list:
        """**The events marked on the roll** -- by a click, Shift+click or a
        marquee -- as the `clausters.seq.SeqEvent` objects they are, in beat
        order. The picture's, not the sequence's: it enters no history and
        each window has its own. Empty with no window open. (The time range a
        sweep leaves is the transport's `clausters.defs.Transport.span`.)"""
        import json

        if self._host is None or self._window is None:
            return []
        marked = self._host.query(self._roll).props.get("selected") or "[]"
        events = self.structure.events
        held = {e._id: e for e in events}
        return [held[i] for i in json.loads(marked) if i in held]

    def select(self, events) -> None:
        """Mark ``events`` -- `clausters.seq.SeqEvent` objects of this sequence
        -- on the roll, in place of what was marked."""
        import json

        if self._host is None or self._window is None:
            return
        ids = [e._id for e in events if getattr(e, "_sequence", None) is self.structure]
        self._host.set(self._roll, selected=json.dumps(ids))

    def unselect(self) -> None:
        """Mark nothing on the roll."""
        self.select([])

    def adopt(self) -> None:
        """The sequence changed by another route -- another window, a script's
        change, a step a script took: the window is corrected, and the lane
        takes the sequence again, so the change is heard."""
        super().adopt()
        self._update()

    def _update(self) -> None:
        """The sequence changed: when it is what the lane holds, the lane takes
        it again, and the server plays it on from where the position is."""
        if self._server is None:
            return
        self._playback.update(self.structure, self._editing.version)

    # ---- the crate's turns ----

    def _deliver(self, addr: str, args) -> bool:
        self._sync_core()
        turned = self._editing.event(self._member, str(addr), _plain(list(args)))
        outcome = turned.get("outcome") or {}
        if outcome.get("turn") == "closed":
            return self._closed()
        if outcome.get("turn") == "step":
            stepped = self.app.stepped(turned.get("stepped") or {}, self)
            self.echo.send(outcome.get("answer"))
            return stepped
        return self._take(outcome)

    def _route(self, args) -> bool:
        """One ``/gui_event`` payload, with the stamp already taken off."""
        self._sync_core()
        wid, tag, values = args[0], args[1], list(args[2:])
        turned = self._editing.event(self._member, "/gui_event",
                                     _plain([wid, 0, 0, tag, *values]))
        return self._take(turned.get("outcome") or {})

    def _take(self, outcome: dict) -> bool:
        """Answer the host with what a turn came to; whether the sequence
        changed."""
        if outcome.get("turn") in (None, "nothing"):
            return False
        changed = bool(outcome.get("changed"))
        if changed:
            self.dirty = True
            self._editing.changed()
            self._update()
        if outcome.get("locate") is not None:
            self.cursor = float(outcome["locate"])
            # The roll plays on a transport of its own, so the mark is its own
            # too: a multitrack it was opened from keeps its cursor.
            self.locate(self.cursor)
            if callable(self.on_locate):
                self.on_locate(self.cursor)
        if outcome.get("play") is not None:
            # The space bar is play/stop: a stop goes back to the position
            # cursor, so the play cursor lands where the reader left the mark.
            if self.playing:
                self.stop()
            else:
                pass_ = outcome["play"]
                self.play(range=pass_.get("range"), looping=bool(pass_.get("looping")))
        if "span" in outcome and self._server is not None \
                and self._playback.planned is self.structure:
            # A sweep moved the time range: it is the playback's span, so a
            # script reads it, and the other rolls over the sequence draw it.
            self._playback.set_span(outcome["span"])
        if outcome.get("loop") is not None and self._server is not None:
            # `L`: the loop switch -- followed at once by a pass in progress,
            # read by a stopped playback on its next play.
            pass_ = outcome["loop"]
            playback = self._playback
            if playback.planned is self.structure:
                playback.set_span(pass_.get("range"), show=False)
                playback.set_looping(bool(pass_.get("looping")))
            else:
                playback.call("loop", self.structure, range=pass_.get("range"),
                              loop=bool(pass_.get("looping")))
        self.echo.send(outcome.get("answer"))
        return changed


def is_events(structure) -> bool:
    """Whether `edit` opens this in the notes editor: an event sequence, or a
    timeline, which it renders into one."""
    return isinstance(structure, (EventSequence, Timeline))


__all__ = ["NotesDomain", "NotesEditor", "NotesView", "is_events"]
