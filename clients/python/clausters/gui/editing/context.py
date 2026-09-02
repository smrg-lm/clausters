"""The editing context of one structure — **whose history it is**.

An undo stack belongs to the data, not to the view. Two windows over one
composition share a history, and an undo in either updates both; a stack minted
per editor sees only the gestures *that* editor made, so stepping one of them
reverts across the other's edits and writes a state nobody was ever in. The
crate placed its pile beside the data for exactly that reason, and this is the
same argument one level up: an editor asks the *data* for its editing context
instead of building one of its own.

What a context owns is everything that is true of the work rather than of a
window: the `History` over it, the version, and the list of views to tell when
one of them edits. What stays a view's is what a view can see — its selection,
its zoom, which layer the hand is on. Those never enter a history either, which
is the same line drawn twice.

The context is reached through `Editing.of`, which caches it **on the
structure**: what is being edited is loose Python objects, so the object itself
is the only thing two editors are guaranteed to have in common. It lives as long
as the data does and dies with it, which is what the crate's own rule asks for —
a history is session state, never serialized, and it goes when the data goes.

**The arrangement's context is a subclass**, not this one: a held `Document`, the
node index and the id to mint next are the tree's and live with `FormEditor`
(`clausters.gui.editing.formeditor.FormEditing`). What is here is what is true
of editing anything.
"""

import weakref
from contextlib import contextmanager

from ... import _native

#: Where a structure's context is cached on it.
ATTR = "_clausters_editing"

#: The version an unedited context is at. One rather than zero, because zero is
#: what an edit means by *unstated* when it names the state it was made
#: against — the same reservation the GUI host's sequence numbers make.
#:
#: It is the same number as `clausters.form.document.FIRST_VERSION` and
#: deliberately not the same symbol: that one is what a **file** says its
#: version is, this one is what an editing context counts from. They coincide
#: because an unedited document is version one, and a generic module importing
#: the arrangement's would be the dependency this subpackage exists to refuse.
FIRST_VERSION = 1


class Editing:
    """One structure's history, and the views drawing it.

    Not built directly: `Editing.of` is the door, so two editors over one thing
    cannot end up with two.
    """

    def __init__(self):
        #: The pile: one editing context, one ordered order over whatever is
        #: registered in it. A dedicated roll or a standalone curve opened over
        #: this data registers itself **here**, which is what makes one undo
        #: walk one order across all of them.
        self.history = _native.History()
        #: The version — the counter a view reports to its host and the host
        #: names back on its next gesture. It moves on every edit and on every
        #: redefine.
        self.version = FIRST_VERSION
        #: The views drawing this data, weakly: an editor that goes away takes
        #: its window with it, and a context does not keep one alive.
        self._views: list = []
        #: How deep the current turn is, and whether anything moved in it. One
        #: gesture can reach here twice — an editor routing an ``"undo"`` calls
        #: its own `undo`, which changes the data on its own — and the other
        #: windows want *one* redraw, not two.
        self._depth = 0
        #: What the turn being run did: the intents it projected, and whether it
        #: changed **which widgets exist**. The two are answered differently by
        #: the other windows, which is the whole reason they are collected
        #: rather than reduced to a bit.
        self._intents: list = []
        self._structural = False
        self._changed = False

    @classmethod
    def of(cls, structure) -> "Editing":
        """The context of this structure, made on first ask.

        Cached on the object, so every editor over it gets the same one — the
        whole point, and the reason this is a classmethod rather than a
        constructor.
        """
        context = getattr(structure, ATTR, None)
        if context is None:
            context = cls()
            setattr(structure, ATTR, context)
        return context

    def register(self, domain: str) -> int:
        """Take a structure into this history and get its identity — the
        crate's `History.register`, named here so an editor never reaches past
        its context for it."""
        return self.history.register(domain)

    def attach(self, view):
        """Take a view into this data's list, so an edit made in one window can
        reach the others."""
        if not any(held() is view for held in self._views):
            self._views.append(weakref.ref(view))

    def detach(self, view):
        """Drop a view whose window is gone."""
        self._views = [held for held in self._views
                       if held() is not None and held() is not view]

    def views(self) -> list:
        """The views still alive, dropping the ones that are not."""
        self._views = [held for held in self._views if held() is not None]
        return [held() for held in self._views]

    def changed(self):
        """Say that the data changed in the turn being run.

        It is not the notification: a turn can reach here more than once, and
        what the other windows want is one answer at the end of the gesture
        rather than one per leg of it."""
        self._changed = True

    def moved(self, intent: dict):
        """One intent this turn wrote onto the data.

        The other windows adopt these as **props** — the placement, the length,
        the notes — which is what keeps a foreign edit from costing a redefine.
        A redefine rebuilds every widget and drops what the host had in flight,
        so doing it per edit makes a window flicker under a hand that is not
        even in it."""
        self._intents.append(intent)
        self._changed = True

    def restructured(self):
        """Say that the turn changed **which widgets exist** — a cut, a split, a
        join, an undo of one.

        This is the case no prop can carry: a widget that was not there a moment
        ago is not a value, so the other windows have to be redrawn whole."""
        self._structural = True
        self._changed = True

    @contextmanager
    def turn(self, source):
        """One gesture, from whichever view made it.

        On the way out, every **other** view of this data is told what it is
        drawing has moved — which nothing else would do: an acknowledgement goes
        to the window whose gesture it answered, so a second window would go on
        drawing something that had changed under it. Nested turns collapse into
        one, because a gesture that reaches here twice is still one gesture.
        """
        self._depth += 1
        try:
            yield self
        finally:
            self._depth -= 1
            if self._depth == 0:
                intents, structural, changed = (
                    self._intents, self._structural, self._changed)
                self._intents, self._structural, self._changed = [], False, False
                if changed:
                    # A turn that changed something and projected no intent is
                    # one nothing here can describe -- a trim, a patch cord, a
                    # gesture applied to the objects directly -- so the honest
                    # answer for the other windows is the whole picture.
                    whole = structural or not intents
                    for view in self.views():
                        if view is not source:
                            view.adopt(intents, whole)

    def close(self):
        """Release the crate's handles. What the data going away leaves behind;
        a view closing is not an event of a history."""
        self.history = None

    def __del__(self):
        try:
            self.close()
        except Exception:  # interpreter teardown: the library may be gone
            pass
