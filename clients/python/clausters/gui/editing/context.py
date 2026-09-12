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
node index and the id to mint next are the tree's and live with whoever holds a
tree. What is here is what is true
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
#: It is the same number as `clausters.document.FIRST_VERSION` and
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
        #: ``id(structure) -> (structure, identity)`` — what each structure was
        #: registered in the pile as. One identity per structure and not per
        #: view: two windows over one thing are one structure in the order, and
        #: minting a second identity for the second window would leave its undo
        #: walking legs that name somebody else. The object is held beside the
        #: number so its ``id`` cannot be reused by something else while the
        #: context is alive.
        self._structures: dict = {}
        self._views: list = []
        #: How deep the current turn is, and whether anything moved in it. One
        #: gesture can reach here twice — an editor routing an ``"undo"`` calls
        #: its own `undo`, which changes the data on its own — and the other
        #: windows want *one* redraw, not two.
        self._depth = 0
        #: The intents the turn being run projected onto the data. Carried to
        #: `adopt` for a view that can answer one as a **prop**; no view does
        #: today, and what they are still read for is the one bit `adopt` acts
        #: on — a turn that projected none is one nothing here can describe.
        self._intents: list = []
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

    def identity(self, structure, domain: str) -> int:
        """This structure's identity in the pile, minted on first ask.

        **Once per structure, not once per view.** Two windows over one thing
        are one structure in the undo order, so a second identity for the
        second window would leave its undo walking legs that name somebody
        else — which looks exactly like a dead button.
        """
        key = id(structure)
        found = self._structures.get(key)
        if found is None:
            found = (structure, self.history.register(domain))
            self._structures[key] = found
        return found[1]

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

    def step(self, direction: str) -> "list | None":
        """Take one step off the pile and give back what each structure must
        apply — ``None`` when there was nothing to take.

        The legs come **routed**: one entry per structure, its payloads in the
        order it must apply them. Which side of an entry a direction reads and
        which legs a structure owns are the crate's
        (`clausters._native.History.walk`), because every client was writing
        both for itself.
        """
        if self.history is None:
            return None
        walked = self.history.walk(direction)
        return None if walked is None else walked["legs"]

    def distribute(self, legs: list, walker) -> bool:
        """Hand a step's legs round **everything registered here** and say
        whether anything moved.

        One entry can name several structures — a stroke over a take and a bend
        of the curve above it are one order, and so is an edit to a page beside
        a lane — so the step is offered to every participant and each takes the
        legs naming the structure it holds. Whoever is walking is included
        whether or not it is in the list, since a structure with no window open
        still holds legs the step may name.

        It lives here rather than on the editor because a participant need not
        be one: what this asks of a thing is `project_legs`, and a
        `clausters.gui.notation.Score` answers it without drawing anything.
        """
        views = self.views()
        walkers = (views if any(view is walker for view in views)
                   else [walker, *views])
        stepped = False
        for view in walkers:
            stepped |= bool(view.project_legs(legs))
        return stepped

    def changed(self):
        """Say that the data changed in the turn being run.

        It is not the notification: a turn can reach here more than once, and
        what the other windows want is one answer at the end of the gesture
        rather than one per leg of it."""
        self._changed = True

    def moved(self, intent: dict):
        """One intent this turn wrote onto the data — a `changed` that says
        *what* changed.

        **No view answers one as a prop today**, and the docstring used to claim
        otherwise: adopting the placement or the length instead of redrawing is
        what this was collected for, and it stopped being needed when
        `clausters.gui.editing.Application.publish` stopped sending differences
        — the host reconciles a whole tree now and keeps the screen state that a
        redefine used to drop, so there is nothing left for a prop to save.

        What the list is still read for is its length, in `turn`: a turn that
        changed something and projected no intent is one nothing here can
        describe.
        """
        self._intents.append(intent)
        self._changed = True

    @contextmanager
    def turn(self, source):
        """One gesture, from whichever view made it.

        On the way out, every **other** view of this data is told what it is
        drawing has moved — which nothing else would do: an acknowledgement goes
        to the window whose gesture it answered, so a second window would go on
        drawing something that had changed under it. Nested turns collapse into
        one, because a gesture that reaches here twice is still one gesture.

        What each of them is told is `adopt`, and **every view answers it by
        redrawing what it holds**: the intents and the `whole` bit ride along
        for a view that could answer one more cheaply, and none does.
        """
        self._depth += 1
        try:
            yield self
        finally:
            self._depth -= 1
            if self._depth == 0:
                intents, changed = self._intents, self._changed
                self._intents, self._changed = [], False
                if changed:
                    # A turn that changed something and projected no intent is
                    # one nothing here can describe -- a trim, a patch cord, a
                    # gesture applied to the objects directly -- so the honest
                    # answer for the other windows is the whole picture.
                    whole = not intents
                    for view in self.views():
                        if view is not source:
                            view.adopt(intents, whole)
                    # ...and **every** view is told the data changed, the one
                    # that made the gesture included. That is a different
                    # question from bringing a window in step: a script driving
                    # something off the data — sounding a piece, writing a file
                    # — wants one answer per gesture whoever made it, and the
                    # window that made it is not exempt from having changed.
                    # The source is told whether or not it is **attached**:
                    # a view attaches when its window opens, and an editor
                    # driving something off the data is entitled to be told
                    # before it is on screen.
                    tell = list(self.views())
                    if source is not None and not any(v is source for v in tell):
                        tell.append(source)
                    for view in tell:
                        told = getattr(view, "data_changed", None)
                        if told is not None:
                            told()

    def close(self):
        """Release the crate's handles. What the data going away leaves behind;
        a view closing is not an event of a history."""
        self.history = None

    def __del__(self):
        try:
            self.close()
        except Exception:  # interpreter teardown: the library may be gone
            pass
