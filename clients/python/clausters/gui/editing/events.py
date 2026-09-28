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
sequence planned onto the notes editor's own transport, so playing it never
moves a multitrack, and planned again after every edit while it sounds -- a
note moved ahead of the line is heard where it lands, and what is sounding
keeps its release. The space bar over the window plays and pauses.
"""

import weakref

from ... import _native
from ..._steps import run_steps
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
        wid = self.widget(editor, "notes", editor.structure)
        editor._sync_core()
        tree = editor._call("window", widget=wid)
        # **A script's own widgets are its objects**, so they are appended here
        # rather than composed in the crate.
        tree["children"] = [*tree.get("children", ()), *editor.extra]
        return tree

    def props(self, editor, widget_id: int) -> dict:
        return editor._call("props", widget=int(widget_id))


class _NotesPlayback:
    """**What sounds the notes editors of one server** -- the crate's playback
    and the steps it answers, carried out on that server. One per server, since
    the editors on it share one transport: the one played last is the one that
    sounds."""

    _of = weakref.WeakKeyDictionary()

    @classmethod
    def of(cls, server) -> "_NotesPlayback":
        found = cls._of.get(server)
        if found is None:
            found = cls._of[server] = cls(server)
        return found

    def __init__(self, server):
        self.server = server
        self._native = _native.NotesPlayback()
        self._runner = _native.StepRunner()
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        server._ensure_recycler()
        self._rate = None
        #: The sequence the plan on the transport is of, if any.
        self.planned = None
        #: The transport it plays on -- the crate's word for it.
        self.transport_id = int(self._native.call(
            "state", _native.SequenceHandle(), server.ids)["transport"])

    @property
    def rate(self) -> float:
        if self._rate is None:
            self._rate = float(self.server.query_info().nominal_sample_rate)
        return self._rate

    def state(self) -> dict:
        """The transport as the engine has it."""
        return self.server.transport_at(self.transport_id).transport_state()

    def call(self, verb: str, sequence, **args) -> dict:
        """One verb over ``sequence``, its steps carried out."""
        answer = self._native.call(verb, sequence._seq, self.server.ids,
                                   rate=self.rate,
                                   latency=float(getattr(self.server, "latency", 0.1)),
                                   **args)
        steps = answer.get("steps")
        if steps:
            run_steps(self.server, self._runner, steps)
        return answer


class NotesEditor(Editor):
    """An event sequence on a roll, edited note by note, in place.

    Args:
        sequence: the `clausters.seq.EventSequence` to edit. It is the edited
            one: read it after any gesture.
        sample_rate: the rate the roll's axis counts in.
        editable: ``False`` for a roll a hand may look at and not edit -- the
            notes of a rendering.
        title: the window's title.
        server: the `clausters.defs.Server` it plays on; ``None`` resolves the
            ambient one when it first plays.
    """

    def __init__(self, sequence, *, sample_rate: float, editable: bool = True,
                 title: str = "Notes", server=None, **options):
        domain = NotesDomain()
        super().__init__(sequence, sample_rate=sample_rate, domain=domain,
                         view=NotesView(), title=title, **options)
        self.editable = bool(editable)
        self._server = server
        #: The timeline a play to a destination of its own (a MIDI port) runs.
        self._elsewhere = None
        self._member, self._structure_id = self._editing.open_notes(
            f"sequence:{id(sequence)}", sequence,
            {"rate": self.sample_rate, "editable": self.editable,
             "title": self.title, "w": int(self.size[0]), "h": int(self.size[1])},
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

    # ---- playing it ----

    @property
    def _playback(self) -> _NotesPlayback:
        if self._server is None:
            from ...base.main import main

            self._server = main.resolve_server()
        return _NotesPlayback.of(self._server)

    def play(self, beat: "float | None" = None, destination=None) -> "NotesEditor":
        """**Play the sequence** from ``beat`` -- or from the position cursor,
        or the start -- on the notes editor's own transport. Returns ``self``.

        ``destination`` is for a MIDI port (a `clausters.base.MidiServer`): the
        server has no MIDI output, so the events are played on this client's
        clock to that destination instead, each as the MIDI messages the core
        renders it to, and an edit is heard from the next play."""
        start = float(beat if beat is not None else (self.cursor or 0.0))
        if destination is not None:
            from ...seq.timeline import Timeline

            played = Timeline(list(self.structure))
            if self.structure.tempo_map is not None:
                played.map = self.structure.tempo_map
            played.play(at=start, destination=destination)
            self._elsewhere = played
            return self
        playback = self._playback
        clock = int(playback.state()["transport_sample"])
        playback.call("play", self.structure, **{"from": start, "clock": clock})
        playback.planned = self.structure
        return self

    def pause(self) -> "NotesEditor":
        """Pause where it stands: a `resume` carries the notes and the plan on."""
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
        playback.planned = None
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
        """A history step landed: the window is corrected, and a sequence that
        is sounding is planned again, so the undo is heard."""
        super().reflect_step()
        self._replan()

    def _replan(self) -> None:
        """The sequence changed while its plan is on the transport: write it
        again from where the transport stands, so the edit is heard now."""
        if self._server is None:
            return
        playback = self._playback
        if playback.planned is not self.structure:
            return
        state = playback.state()
        playback.call("replan", self.structure,
                      position=int(state["position_sample"]),
                      clock=int(state["transport_sample"]))

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
            self._replan()
        if outcome.get("play") is not None:
            # The space bar: a sounding sequence pauses, a silent one plays.
            if self.playing:
                self.pause()
            else:
                self.play()
        self.echo.send(outcome.get("answer"))
        return changed


def is_events(structure) -> bool:
    """Whether `edit` opens this in the notes editor: an event sequence, or a
    timeline, which it renders into one."""
    return isinstance(structure, (EventSequence, Timeline))


__all__ = ["NotesDomain", "NotesEditor", "NotesView", "is_events"]
