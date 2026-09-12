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
        #: ``id(structure) -> (structure, identity, applier)`` — what each
        #: structure was registered in the pile as, and what can put an edit
        #: back onto it. One identity per structure and not per view: two
        #: windows over one thing are one structure in the order, and minting a
        #: second identity for the second window would leave its undo walking
        #: legs that name somebody else. The object is held beside the number so
        #: its ``id`` cannot be reused by something else while the context is
        #: alive — and the **applier** is held because the pile's scope is this
        #: context while a window's is a window: see `distribute`.
        self._structures: dict = {}
        self._views: list = []
        #: How deep the current turn is, and whether anything moved in it. One
        #: gesture can reach here twice — an editor routing an ``"undo"`` calls
        #: its own `undo`, which changes the data on its own — and the other
        #: windows want *one* redraw, not two.
        self._depth = 0
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

    def identity(self, structure, domain: str, applier=None) -> int:
        """This structure's identity in the pile, minted on first ask, with
        **what can put an edit back onto it**.

        **Once per structure, not once per view.** Two windows over one thing
        are one structure in the undo order, so a second identity for the
        second window would leave its undo walking legs that name somebody
        else — which looks exactly like a dead button.

        ``applier`` is anything answering ``project(structure, payload)`` — an
        editor hands its `clausters.gui.editing.Domain`, and a
        `clausters.gui.notation.Score` hands itself. It is kept **here**, beside
        the identity, because that is the scope the pile has: an entry names a
        structure and the order over entries is global, so an entry that only
        *some of the time* has somebody to apply it is an entry that blocks
        every entry behind it. Registered once and kept, applying an edit stops
        depending on whether a window happens to be open.
        """
        key = id(structure)
        found = self._structures.get(key)
        if found is None:
            found = (structure, self.history.register(domain), applier)
            self._structures[key] = found
        elif found[2] is None and applier is not None:
            # Registered by something that could not apply (a caller that only
            # wanted the number); the first that can, wins the slot.
            found = (found[0], found[1], applier)
            self._structures[key] = found
        return found[1]

    def applier(self, identity: int):
        """What puts an edit back onto the structure this identity names, or
        ``None`` for one nothing registered."""
        for _, mine, applier in self._structures.values():
            if mine == identity:
                return applier
        return None

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

    def distribute(self, legs: list, walker=None) -> bool:
        """Put a step's legs back onto the structures they name, and say whether
        anything moved.

        One entry can name several structures — a stroke over a take and a bend
        of the curve above it are one order, and so is an edit to a page beside
        a lane — so the legs come routed and each goes to whatever was
        registered for that identity.

        **It asks the structures, not the windows**, and that is the whole of
        why it is written this way. A pile is ordered and global to this
        context, while a window comes and goes: when the applier was a *view*, a
        box entered from a piece and then closed left an entry nobody could
        apply, the step was refused, and — since a refused step puts the cursor
        back — every edit behind it became unreachable too. The pile was not
        missing one step, it was **blocked**. What can put an edit back is a
        structure and its vocabulary, neither of which is on screen, so that is
        what `identity` registers and this is what asks.

        ``walker`` is kept for callers that pass it and is not read: who drew the
        gesture matters to the redraw, which is the turn's, not to this.
        """
        stepped = False
        for leg in legs:
            applier = self.applier(int(leg.get("structure", -1)))
            if applier is None:
                continue
            structure = self.structure_of(int(leg.get("structure", -1)))
            for payload in leg.get("payloads", ()):
                if isinstance(payload, dict):
                    stepped |= bool(applier.project(structure, payload))
        return stepped

    def structure_of(self, identity: int):
        """The structure this identity names, or ``None``."""
        for structure, mine, _ in self._structures.values():
            if mine == identity:
                return structure
        return None

    def changed(self):
        """Say that the data changed in the turn being run.

        It is not the notification: a turn can reach here more than once, and
        what the other windows want is one answer at the end of the gesture
        rather than one per leg of it."""
        self._changed = True

    @contextmanager
    def turn(self, source):
        """One gesture, from whichever view made it.

        On the way out, every **other** view of this data is told what it is
        drawing has moved — which nothing else would do: an acknowledgement goes
        to the window whose gesture it answered, so a second window would go on
        drawing something that had changed under it. Nested turns collapse into
        one, because a gesture that reaches here twice is still one gesture.

        What each of them is told is `adopt`, and it carries nothing: a view
        answers it by correcting every widget it holds. It used to carry the
        turn's intents, so that a view could adopt a placement or a length as a
        **prop** instead of redrawing — but what `adopt` does is already props
        (`_resync` per widget, then one acknowledgement), never a redefine, so
        the intents would only have narrowed *which* widgets. They were passed
        for months and read by nobody.
        """
        self._depth += 1
        try:
            yield self
        finally:
            self._depth -= 1
            if self._depth == 0:
                changed, self._changed = self._changed, False
                if changed:
                    for view in self.views():
                        if view is not source:
                            view.adopt()
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
