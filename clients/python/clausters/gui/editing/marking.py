"""Marking: what the hand marked, for the editors whose operations act on it.

`Marking` is a **capability, not a level of the hierarchy**. An editor takes it
when what a hand marks is what its operations act on -- a multitrack's held
regions, a roll's events moved, deleted, quantized and copied together, an
audio editor's range of samples -- and an editor with no such operation does
not: in the points editor a hand takes one segment, for its shape, so it marks
nothing and has none of these words.

The three words are here, once: `Marking.selected`, `Marking.select` and
`Marking.unselect`. What an editor says is what its marks **are** -- which
objects of its structure, and where they are kept -- through two hooks,
``_marked`` and ``_mark``.

**What is marked is the window's**, never the structure's: it enters no
history, and two windows over one structure each have their own. It is told
apart from the transport's ``span`` -- the time range a sweep leaves, which
says what sounds -- by what it is, not by which editor holds it.
"""

import json


class Marking:
    """The marking surface of an editor: `selected`, `select`, `unselect`.

    Composed into an editor beside `clausters.gui.editing.Editor`; never used
    alone. The editor answers two hooks: ``_marked()``, what is marked now in
    the structure's own type, and ``_mark(marked)``, marking that in place of
    what was marked, with ``None`` for nothing.

    Where the host keeps the marks -- a widget's ``selected`` prop, the names
    of what is held -- ``_marks`` reads them and ``_set_marks`` writes them,
    so an editor's hooks are only the mapping between a name and its object.
    """

    @property
    def selected(self):
        """**What the hand marked**, in the structure's own type -- the
        editor's class says which: a roll's events, a multitrack's regions,
        an audio editor's samples."""
        return self._marked()

    def select(self, marked) -> None:
        """Mark ``marked`` -- what `selected` answers -- in place of what was
        marked."""
        self._mark(marked)

    def unselect(self) -> None:
        """Mark nothing."""
        self._mark(None)

    # ---- what an editor answers ----

    def _marked(self):
        raise NotImplementedError

    def _mark(self, marked) -> None:
        raise NotImplementedError

    # ---- marks the host keeps ----

    def _marks(self, widget) -> list:
        """The names in ``widget``'s ``selected`` prop, as the host has them
        now. Empty with no window open."""
        if self._host is None or self._window is None or widget is None:
            return []
        return json.loads(self._host.query(widget).props.get("selected") or "[]")

    def _set_marks(self, widget, names) -> None:
        """Set ``widget``'s ``selected`` prop to ``names``. Nothing with no
        window open."""
        if self._host is None or self._window is None or widget is None:
            return
        self._host.set(widget, selected=json.dumps(list(names)))
