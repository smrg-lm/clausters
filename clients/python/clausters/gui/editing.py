"""The editing context of one arrangement — **whose history it is**.

An undo stack belongs to the data, not to the view. Two windows over one
composition share a history, and an undo in either updates both; a stack minted
per editor sees only the gestures *that* editor made, so stepping one of them
reverts across the other's edits and writes a state nobody was ever in. The
crate placed its pile beside the data for exactly that reason, and this is the
same argument one level up: an editor asks the *arrangement* for its editing
context instead of building one of its own.

What a context owns is everything that is true of the composition rather than of
a window: the held `Document`, the `History` over it, the index from node id to
what an intent writes to, the next id to mint, and the version. What stays a
view's is what a view can see — its selection, its zoom, which layer the hand is
on. Those never enter a history either, which is the same line drawn twice.

The context is reached through `Editing.of`, which caches it **on the element**:
the arrangement is loose Python objects, so the element is the only thing two
editors are guaranteed to have in common. It lives as long as the composition
does and dies with it, which is what the crate's own rule asks for — a history
is session state, never serialized, and it goes when the data goes.
"""

import weakref
from contextlib import contextmanager

from .. import _native
from ..form.aggregate import Aggregate
from ..form.document import FIRST_VERSION, ID_ATTR, next_node_id, to_document

#: Where a composition's context is cached on its root element.
ATTR = "_clausters_editing"


class Editing:
    """One arrangement's held document, its history, and the index between them.

    Not built directly: `Editing.of` is the door, so two editors over one
    composition cannot end up with two.
    """

    def __init__(self):
        #: The pile: one editing context, one ordered order over whatever is
        #: registered in it. A dedicated roll or a standalone curve opened over
        #: this composition registers itself **here**, which is what makes one
        #: undo walk one order across all of them.
        self.history = _native.History()
        #: The arrangement's face of that pile.
        self.log = _native.Log(history=self.history)
        #: The crate's held document — opened once and kept, so a gesture costs
        #: the edit rather than the composition.
        self.document = None
        #: Whether the held document has to be derived from the arrangement
        #: again before the next edit. Set wherever the tree moves by a route
        #: that is not an intent.
        self.rederive = False
        #: node id -> ``(owner, member, element)``: the arrangement object an
        #: intent naming that node writes to. Built with the document, since
        #: `to_document` is what stamps the ids.
        self.by_node: dict = {}
        #: The next node id to mint for a node a gesture creates.
        self.next_node = None
        #: The composition's version — the document half of the two counters.
        self.version = FIRST_VERSION
        #: The views drawing this composition, weakly: an editor that goes away
        #: takes its window with it, and a context does not keep one alive.
        self._views: list = []
        #: How deep the current turn is, and whether anything moved in it. One
        #: gesture can reach here twice — `Editor.apply` routing an ``"undo"``
        #: calls `Editor.undo`, which changes the composition on its own — and
        #: the other windows want *one* redraw, not two.
        self._depth = 0
        #: What the turn being run did: the intents it projected, and whether it
        #: changed **which widgets exist**. The two are answered differently by
        #: the other windows, which is the whole reason they are collected
        #: rather than reduced to a bit.
        self._intents: list = []
        self._structural = False
        self._changed = False

    @classmethod
    def of(cls, element) -> "Editing":
        """The context of this composition, made on first ask.

        Cached on the element, so every editor over it gets the same one — the
        whole point, and the reason this is a classmethod rather than a
        constructor.
        """
        context = getattr(element, ATTR, None)
        if context is None:
            context = cls()
            setattr(element, ATTR, context)
        return context

    def held(self, element) -> tuple:
        """The log and the document, deriving the document if that is what it
        takes.

        The document is opened once and kept: rebuilding it per gesture handed
        back the whole of what holding the tree in the crate had won (36 ms and
        71 ms on a 10240-event composition, against 0.014 ms for the edit
        itself). What a rebuild was quietly doing is explicit here —
        `to_document` stamps each element with the id it keeps, so a
        re-derivation names the same nodes and the history keeps its footing.
        """
        if self.document is None or self.rederive:
            document = to_document(element, version=self.version)
            if self.document is not None:
                self.document.close()
            self.document = _native.Document(document)
            # The index is added to rather than rebuilt, and that is deliberate:
            # an element a gesture took out of the tree keeps its entry, which is
            # what lets the inverse of a cut put the placement back. A
            # re-derivation names the same nodes, so nothing here goes stale --
            # it only keeps what the tree no longer reaches.
            self.index(element)
            self.next_node = next_node_id(element)
            self.rederive = False
        return self.log, self.document

    def index(self, element, owner=None, member=None):
        """Walk the arrangement collecting node id -> what an intent writes to.

        A `place` needs the owning aggregate and the member handle (a placement
        is the aggregate's, not the element's); everything else needs the
        element. The walk mirrors `clausters.form.document`'s own, which is what
        keeps the two agreeing about what has an id.
        """
        # The id belongs to the **placement** when there is one: a clip is a
        # window onto samples, so what an intent names is the window.
        node = getattr(member if member is not None else element, ID_ATTR, None)
        if node is not None:
            self.by_node[int(node)] = (owner, member, element)
        # A view opened over a *part* of this composition -- a dedicated roll of
        # one track -- must reach this context and not mint a second one, so the
        # walk stamps what it passes. Only where there is none: a part that
        # already had a context of its own was being edited on its own terms,
        # and taking its history away without being asked is not this walk's
        # to do.
        if getattr(element, ATTR, None) is None:
            setattr(element, ATTR, self)
        if isinstance(element, Aggregate):
            for handle in element.handles:
                self.index(handle.element, element, handle)

    def mint(self, element) -> int:
        """The next id for a node a gesture creates — a note added in a roll."""
        if self.next_node is None:
            self.next_node = next_node_id(element)
        node, self.next_node = self.next_node, self.next_node + 1
        return node

    def attach(self, view):
        """Take a view into this composition's list, so an edit made in one
        window can reach the others."""
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
        """Say that the composition changed in the turn being run.

        It is not the notification: a turn can reach here more than once, and
        what the other windows want is one answer at the end of the gesture
        rather than one per leg of it."""
        self._changed = True

    def moved(self, intent: dict):
        """One intent this turn wrote onto the arrangement.

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

        On the way out, every **other** view of this composition is told the
        data it is drawing has moved — which nothing else would do: an
        acknowledgement goes to the window whose gesture it answered, so a
        second window would go on drawing a piece that had changed under it.
        Nested turns collapse into one, because a gesture that reaches here
        twice is still one gesture.
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
        """Release the crate's handles. What the composition going away leaves
        behind; a view closing is not an event of a history."""
        if self.log is not None:
            self.log.close()
            self.log = None
        if self.document is not None:
            self.document.close()
            self.document = None
        self.history = None

    def __del__(self):
        try:
            self.close()
        except Exception:  # interpreter teardown: the library may be gone
            pass
