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
"""

from ... import _native
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


class NotesEditor(Editor):
    """An event sequence on a roll, edited note by note, in place.

    Args:
        sequence: the `clausters.seq.EventSequence` to edit. It is the edited
            one: read it after any gesture.
        sample_rate: the rate the roll's axis counts in.
        editable: ``False`` for a roll a hand may look at and not edit -- the
            notes of a rendering.
        title: the window's title.
    """

    def __init__(self, sequence, *, sample_rate: float, editable: bool = True,
                 title: str = "Notes", **options):
        domain = NotesDomain()
        super().__init__(sequence, sample_rate=sample_rate, domain=domain,
                         view=NotesView(), title=title, **options)
        self.editable = bool(editable)
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
        self.echo.send(outcome.get("answer"))
        return changed


def is_events(structure) -> bool:
    """Whether `edit` opens this in the notes editor: an event sequence, or a
    timeline, which it renders into one."""
    return isinstance(structure, (EventSequence, Timeline))


__all__ = ["NotesDomain", "NotesEditor", "NotesView", "is_events"]
