"""The editing context of one structure — **whose history it is**.

An undo stack belongs to the data, not to the view. Two windows over one
composition share a history, and an undo in either updates both; a stack minted
per editor sees only the gestures *that* editor made, so stepping one of them
reverts across the other's edits and writes a state nobody was ever in. The
crate placed its pile beside the data for exactly that reason, and this is the
same argument one level up: an editor asks the *data* for its editing context
instead of building one of its own.

**The context is the shared crate's** (`clausters._native.EditingCore`): the
history, the version, and its **members** — the multitrack and samples editors
opened in it, whose turns and steps it takes, and the structures the crate does
not apply (a curve, a timeline, a score), which join as external members and
get their legs back. No application holds a history of its own: one opened
alone is a context of one, and two opened in the same one walk one order. What
is here is what a language owns — the objects each member edits, what puts a
step back onto them, and the windows to tell.

What stays a view's is what a view can see — its selection, its zoom, which
layer the hand is on. Those never enter a history either, which is the same line
drawn twice.

The context is reached through `Editing.of`, which caches it **on the
structure**: what is being edited is loose Python objects, so the object itself
is the only thing two editors are guaranteed to have in common. It lives as long
as the data does and dies with it, which is what the crate's own rule asks for —
a history is session state, never serialized, and it goes when the data goes.
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
#: version is, this one is what an editing context counts from.
FIRST_VERSION = 1


class Editing:
    """One editing context: its history, its members, and the views drawing
    them.

    Not built directly for an editor: `Editing.of` is the door, so two editors
    over one thing cannot end up with two. A context built by hand is one a
    caller hands several editors (``context=``) so they share an order.
    """

    def __init__(self):
        #: The context itself, in the shared crate.
        self.core = _native.EditingCore()
        #: The version — the counter a view reports to its host and the host
        #: names back on its next gesture. The crate's: it is read back from
        #: every turn, step and record, and never moved here.
        self.version = FIRST_VERSION
        #: ``id(structure) -> (structure, member, identity)`` — the member a
        #: structure first joined as, and the identity the crate named it by.
        #: The object is held beside the number so its ``id`` cannot be reused
        #: by something else while the context is alive.
        self._structures: dict = {}
        #: ``member -> (structure, handler)`` — what carries a step out for each
        #: member. An editor hands its `clausters.gui.editing.Domain`, and a
        #: `clausters.gui.notation.Score` hands itself. It is kept **here**
        #: rather than on a window, because the order is the context's: a step
        #: must reach a structure whether or not a window over it is open.
        self._handlers: dict = {}
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

    # ---- members ----

    def open(self, verb: str, key: str, request: dict, structure,
             handler) -> tuple:
        """**Open an editor in this context** — ``verb`` is ``"openMultitrack"``
        or ``"openSamples"`` — as the structure ``key`` names, and answer its
        ``(member, identity)``.

        ``handler`` is what carries a step out for it: the editor's domain.

        Raises:
            ValueError: the crate refused the request, with its reason.
        """
        answer = self.core.call(verb, key=str(key), **request)
        if "error" in answer or "member" not in answer:
            raise ValueError(answer.get("error", "the context opened nothing"))
        member, identity = int(answer["member"]), int(answer["structure"])
        self._handlers[member] = (structure, handler)
        self._structures.setdefault(id(structure), (structure, member, identity))
        return member, identity

    def identity(self, structure, domain: str, applier=None) -> int:
        """This structure's identity in the order, joining it as an **external
        member** on first ask, with **what can put an edit back onto it**.

        **Once per structure, not once per view.** Two windows over one thing
        are one structure in the undo order, so a second identity for the
        second window would leave its undo walking legs that name somebody
        else — which looks exactly like a dead button.

        ``applier`` is anything answering ``project(structure, payload)``. A
        structure an editor opened in the crate already has its identity, and
        this answers it.
        """
        found = self._structures.get(id(structure))
        if found is None:
            _, identity = self.open("external", f"object:{id(structure)}",
                                    {"domain": str(domain)}, structure, applier)
            return identity
        _, member, identity = found
        if applier is not None and self._handlers.get(member, (None, None))[1] is None:
            # Joined by something that could not apply (a caller that only
            # wanted the number); the first that can, wins the slot.
            self._handlers[member] = (structure, applier)
        return identity

    def member(self, member: int, verb: str, **args):
        """One verb of a member's own door, with the version filled in by the
        context."""
        if self.core is None:
            return {}
        return self.core.call("member", member=int(member), call={"verb": verb, **args})

    def _member_of(self, identity: int) -> "int | None":
        for _, member, mine in self._structures.values():
            if mine == identity:
                return member
        return None

    # ---- the order ----

    def record(self, legs: list, *, label: str = "edit", coalesce: bool = False) -> bool:
        """**Record an entry** an external member applied itself: its legs over
        one structure, each ``{"structure", "forward", "backward", "key"}``.
        Answers whether the history took it; the version moves when it did.

        Raises:
            ValueError: the legs name more than one structure — an entry over
                several is a turn of an application, which records its own.
        """
        if self.core is None or not legs:
            return False
        structures = {int(leg["structure"]) for leg in legs}
        if len(structures) != 1:
            raise ValueError("an entry recorded here is over one structure")
        member = self._member_of(structures.pop())
        if member is None:
            return False
        answer = self.core.call(
            "record", member=member, label=str(label), coalesce=bool(coalesce),
            legs=[{"forward": leg["forward"], "backward": leg["backward"],
                   "key": leg.get("key") or ""} for leg in legs])
        self.version = int(answer.get("version", self.version))
        return bool(answer.get("recorded"))

    def moved(self) -> int:
        """An edit that leaves no entry — one with no inverse to record — still
        moves the version. Answers it."""
        if self.core is not None:
            self.version = int(self.core.call("moved").get("version", self.version))
        return self.version

    def event(self, member: int, addr: str, args: list) -> dict:
        """**One message to a member**, read, recorded and answered by the
        crate: ``{"outcome", "stepped"?, "corrections", "version"}``."""
        if self.core is None:
            return {}
        turned = self.core.call("event", member=int(member), addr=str(addr), args=args)
        if turned:
            self.version = int(turned.get("version", self.version))
        return turned or {}

    def step(self, direction: str) -> dict:
        """**Take one step of the order**: ``{"stepped", "reason"?, "effects",
        "corrections", "version"}``. The step is already taken in the crate;
        `carry` is what puts it back onto the objects here."""
        if self.core is None:
            return {"stepped": False}
        stepped = self.core.call("step", direction=str(direction))
        self.version = int(stepped.get("version", self.version))
        return stepped

    def carry(self, stepped: dict) -> None:
        """**Carry a step's effects out** on the structures they name: a piece
        written back, a take's writes projected, an external member's payloads
        applied."""
        for effect in stepped.get("effects") or ():
            structure, handler = self._handlers.get(int(effect.get("member", -1)),
                                                    (None, None))
            if handler is None:
                continue
            if effect.get("kind") == "multitrack":
                handler.stepped(structure, effect.get("applied") or {})
                continue
            for payload in effect.get("payloads") or ():
                if isinstance(payload, dict):
                    handler.project(structure, payload)

    def _state(self) -> dict:
        return {} if self.core is None else self.core.call("state")

    @property
    def can_undo(self) -> bool:
        """Whether there is an edit to step back over."""
        return bool(self._state().get("canUndo"))

    @property
    def can_redo(self) -> bool:
        """Whether there is an undone edit to step forward into."""
        return bool(self._state().get("canRedo"))

    @property
    def undo_label(self) -> "str | None":
        """What an undo would be called, for a menu item."""
        return self._state().get("undoLabel")

    @property
    def redo_label(self) -> "str | None":
        """What a redo would be called, for a menu item."""
        return self._state().get("redoLabel")

    # ---- the views ----

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

    @contextmanager
    def turn(self, source):
        """One gesture, from whichever view made it.

        On the way out, every **other** view of this data is told what it is
        drawing has moved — which nothing else would do: an acknowledgement goes
        to the window whose gesture it answered, so a second window would go on
        drawing something that had changed under it. Nested turns collapse into
        one, because a gesture that reaches here twice is still one gesture.

        What each of them is told is `adopt`, and it carries nothing: a view
        answers it by correcting every widget it holds.
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
                    # that made the gesture included: a script driving something
                    # off the data -- sounding a piece, writing a file -- wants
                    # one answer per gesture whoever made it. The source is told
                    # whether or not it is **attached**, since an editor driving
                    # something off the data is entitled to be told before it is
                    # on screen.
                    tell = list(self.views())
                    if source is not None and not any(v is source for v in tell):
                        tell.append(source)
                    for view in tell:
                        told = getattr(view, "data_changed", None)
                        if told is not None:
                            told()

    def close(self):
        """Release the crate's context, with every editor opened in it. What the
        data going away leaves behind; a view closing is not an event of a
        history."""
        core, self.core = getattr(self, "core", None), None
        if core is not None:
            core.free()

    def __del__(self):
        try:
            self.close()
        except Exception:  # interpreter teardown: the library may be gone
            pass
