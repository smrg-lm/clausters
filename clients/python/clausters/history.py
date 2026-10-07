"""The editing context of one structure -- **whose history it is** -- and the
`UndoHistory` a script reads it through.

An undo stack belongs to the data, not to the view. Two windows over one
structure share a history, and an undo in either updates both; a stack minted
per editor sees only the gestures *that* editor made, so stepping one of them
reverts across the other's edits and writes a state nobody was ever in. The
crate placed its pile beside the data for exactly that reason, and this is the
same argument one level up: an editor asks the *data* for its editing context
instead of building one of its own.

**The context is the shared crate's** (`clausters._native.EditingCore`): the
history, the version, and its **members** -- the multitrack and audio editors
opened in it, whose turns and steps it takes, and the structures the crate does
not apply (a curve, a timeline, a score), which join as external members and
get their legs back. No application holds a history of its own: one opened
alone is a context of one, and two opened in the same one walk one order. What
is here is what a language owns -- the objects each member edits, what puts a
step back onto them, and the windows to tell.

What stays a view's is what a view can see -- its selection, its zoom, which
layer the hand is on. Those never enter a history either, which is the same line
drawn twice.

**It needs no window, so it is not the GUI's.** A script's change to a
structure that has a history -- a sequence an editor is open on, or one a
script asked for (``seq.history``) -- is recorded in it as a turn, the same
one a gesture is: the windows over it are brought in step, the playback
reading it is synced, and an undo in either a window or the script walks one
order. A structure nobody asked a history of changes freely and records
nothing, so a script that writes ten thousand notes keeps no ten thousand
inverses.

The context is reached through `Editing.of`, which caches it **on the
structure**: what is being edited is loose Python objects, so the object itself
is the only thing two editors are guaranteed to have in common. It lives as long
as the data does and dies with it, which is what the crate's own rule asks for --
a history is session state, never serialized, and it goes when the data goes.
"""

import weakref
from contextlib import contextmanager

from . import _native

#: Where a structure's context is cached on it.
ATTR = "_clausters_editing"

#: The version an unedited context is at. One rather than zero, because zero is
#: what an edit means by *unstated* when it names the state it was made
#: against -- the same reservation the GUI host's sequence numbers make.
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
        #: The version -- the counter a view reports to its host and the host
        #: names back on its next gesture. The crate's: it is read back from
        #: every turn, step and record, and never moved here.
        self.version = FIRST_VERSION
        #: ``id(structure) -> (structure, member, identity)`` -- the member a
        #: structure first joined as, and the identity the crate named it by.
        #: The object is held beside the number so its ``id`` cannot be reused
        #: by something else while the context is alive.
        self._structures: dict = {}
        #: ``member -> (structure, handler)`` -- what carries a step out for each
        #: member. An editor hands its `clausters.gui.editing.Domain`, and a
        #: `clausters.gui.notation.Score` hands itself. It is kept **here**
        #: rather than on a window, because the order is the context's: a step
        #: must reach a structure whether or not a window over it is open.
        self._handlers: dict = {}
        self._views: list = []
        #: How deep the current turn is, and whether anything moved in it. One
        #: gesture can reach here twice -- an editor routing an ``"undo"`` calls
        #: its own `undo`, which changes the data on its own -- and the other
        #: windows want *one* redraw, not two.
        self._depth = 0
        self._changed = False
        #: ``id(structure) -> [depth, before, changed]`` -- the ``with
        #: history(label)`` blocks open over a structure: one entry, recorded
        #: when the outermost closes.
        self._blocks: dict = {}
        #: How deep an editor's or a step's own write is. A change it makes
        #: through a structure's objects -- a points editor over a held curve
        #: writes the curve into its sequence -- is part of the entry that
        #: editor or step records, not one of its own.
        self._applying = 0

    @classmethod
    def of(cls, structure) -> "Editing":
        """The context of this structure, made on first ask.

        Cached on the object, so every editor over it gets the same one -- the
        whole point, and the reason this is a classmethod rather than a
        constructor. A structure another one holds -- a curve a sequence holds
        -- names that one as its ``_history_owner``, and shares its context: a
        window over the curve and a roll over the sequence are one order.
        """
        owner = getattr(structure, "_history_owner", None)
        if owner is not None:
            return cls.of(owner)
        context = getattr(structure, ATTR, None)
        if context is None:
            context = cls()
            setattr(structure, ATTR, context)
        return context

    # ---- members ----

    def open(self, verb: str, key: str, request: dict, structure,
             handler) -> tuple:
        """**Open an editor in this context** -- ``verb`` is ``"openMultitrack"``,
        ``"openAudio"`` or ``"openPoints"`` -- as the structure ``key`` names, and answer its
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

    def open_notes(self, key: str, sequence, request: dict, handler) -> tuple:
        """**Open a notes editor over ``sequence``** -- a
        `clausters.seq.EventSequence`, which the editor then edits in place --
        as the structure ``key`` names, and answer its ``(member, identity)``.

        Raises:
            ValueError: the crate refused the request, with its reason.
        """
        answer = self.core.open_notes(sequence._seq, key=str(key), **request)
        if "error" in answer or "member" not in answer:
            raise ValueError(answer.get("error", "the context opened nothing"))
        member, identity = int(answer["member"]), int(answer["structure"])
        self._handlers[member] = (sequence, handler)
        self._structures.setdefault(id(sequence), (sequence, member, identity))
        self.claim(sequence)
        return member, identity

    def open_score(self, key: str, score, request: dict, handler) -> tuple:
        """**Open a score editor over ``score``** -- a
        `clausters.gui.notation.Score`, which the editor then edits in place --
        as the structure ``key`` names, and answer its ``(member, identity)``.
        The score is claimed: an edit a script makes through it is a turn of
        this context.

        Raises:
            ValueError: the crate refused the request, with its reason.
        """
        answer = self.core.open_score(score._h, key=str(key), **request)
        if "error" in answer or "member" not in answer:
            raise ValueError(answer.get("error", "the context opened nothing"))
        member, identity = int(answer["member"]), int(answer["structure"])
        self._handlers[member] = (score, handler)
        self._structures.setdefault(id(score), (score, member, identity))
        self.claim(score)
        return member, identity

    def open_score_over(self, key: str, score, sequence, request: dict, handler) -> tuple:
        """**Open a score editor over ``sequence``**, on the page it is read
        into -- ``score`` is the `clausters.gui.notation.Score` the page is
        drawn from, loaded by the crate with the sequence read -- as the
        structure ``key`` names, which is the sequence's: the editor's
        entries are the sequence's, one order with every roll over it. Answers
        its ``(member, identity)``, and the sequence is claimed.

        Raises:
            ValueError: the crate refused the request, with its reason.
        """
        answer = self.core.open_score_over(score._h, sequence._seq, key=str(key), **request)
        if "error" in answer or "member" not in answer:
            raise ValueError(answer.get("error", "the context opened nothing"))
        member, identity = int(answer["member"]), int(answer["structure"])
        self._handlers[member] = (sequence, handler)
        self._structures.setdefault(id(sequence), (sequence, member, identity))
        self.claim(sequence)
        return member, identity

    def act(self, member: int, call: dict) -> dict:
        """**One verb a client calls on a member** -- ``call`` as that
        member's verbs read it -- recorded and answered by the crate like a
        gesture: the turn, with the corrections it owes."""
        if self.core is None:
            return {}
        return self.core.call("act", member=int(member), call=call) or {}

    def open_multitrack(self, key: str, multitrack, request: dict, handler) -> tuple:
        """**Open a multitrack editor over ``multitrack``** -- a
        `clausters.multitrack.Multitrack`, which the editor then edits in place
        -- as the structure ``key`` names, and answer its ``(member, identity)``.

        Raises:
            ValueError: the crate refused the request, with its reason.
        """
        answer = self.core.open_multitrack(multitrack._mt, key=str(key), **request)
        if "error" in answer or "member" not in answer:
            raise ValueError(answer.get("error", "the context opened nothing"))
        member, identity = int(answer["member"]), int(answer["structure"])
        self._handlers[member] = (multitrack, handler)
        self._structures.setdefault(id(multitrack), (multitrack, member, identity))
        self.claim(multitrack)
        return member, identity

    def bind_sequence(self, member: int, source: int, sequence) -> dict:
        """**Bind a multitrack member's ``source`` to ``sequence``** -- a
        `clausters.seq.EventSequence` -- so a region over it draws the
        sequence's notes; answers the member's corrected picture. The sequence
        is claimed: a script's change to it is a turn of this context."""
        self.claim(sequence)
        return self.core.bind_sequence(sequence._seq, int(member), int(source))

    def claim(self, structure) -> None:
        """**Make this the structure's context**, so `Editing.of` and the
        structure's own `UndoHistory` answer it. An editor opened in a context
        the caller handed it claims what it edits: the windows are here, so a
        script's change has to be a turn here to reach them."""
        setattr(structure, ATTR, self)

    def _script_identity(self, structure) -> int:
        """The identity in the order of a structure a script changes -- an
        editor's when one is open on it, else an external member joined now,
        keyed as that editor's would be so the two are one structure."""
        found = self._structures.get(id(structure))
        if found is not None:
            return found[2]
        key, domain = structure._script_key()
        _, identity = self.open("external", key, {"domain": domain}, structure,
                                _ScriptSteps())
        return identity

    @contextmanager
    def applying(self):
        """An editor's or a step's own write: a change made through objects
        inside it is applied and told to the views, and recorded by nobody but
        the entry the editor or the step already stands for."""
        self._applying += 1
        try:
            yield
        finally:
            self._applying -= 1

    def script_edit(self, structure, intent: dict, label: str) -> dict:
        """**One change a script makes to a structure in this context** -- a
        sequence, a multitrack -- as a turn: applied, recorded -- as its own
        entry, or into the ``with history(label)`` block open over the
        structure -- and every view over it told. Answers what the
        structure's door answers."""
        block = self._blocks.get(id(structure))
        with self.turn(None):
            if self._applying:
                answer = structure._apply(intent, inverse=False)
                if answer.get("applied"):
                    self.changed()
                return answer
            if block is not None and block[3] is None:
                # A block over a structure that is restored whole: its state
                # before and after are the entry.
                answer = structure._apply(intent, inverse=False)
                if answer.get("applied"):
                    block[2] = True
                    self.changed()
                return answer
            answer = structure._apply(intent, inverse=True)
            if answer.get("applied"):
                leg = {"structure": self._script_identity(structure),
                       "forward": {"edit": structure._forward(intent, answer)},
                       "backward": answer["current"]}
                if block is not None:
                    block[2] = True
                    block[3].append(leg)
                else:
                    self.record([leg], label=label)
                self.changed()
            return answer

    @contextmanager
    def block(self, structure, label: str):
        """Everything a script changes in the structure inside it is **one**
        entry, called ``label``, and one turn. Blocks nest: an inner one is
        part of the outer.

        A structure that can be **restored whole** (a sequence) is recorded as
        its state before and after; one that cannot (a multitrack, whose
        vocabulary states its parts) as the edits made, in order, which an
        undo walks back in reverse."""
        key = id(structure)
        block = self._blocks.get(key)
        if block is not None:
            block[0] += 1
            try:
                yield
            finally:
                block[0] -= 1
            return
        restore = getattr(structure, "_restore", None)
        before = restore() if restore is not None else None
        self._blocks[key] = block = [1, before, False, None if restore is not None else []]
        # Recorded inside the turn, so the views are told the version the
        # entry moved to rather than the one before it.
        with self.turn(None):
            try:
                yield
            finally:
                del self._blocks[key]
                if block[2] and block[3] is None:
                    self.record([{"structure": self._script_identity(structure),
                                  "forward": {"edit": structure._restore()},
                                  "backward": block[1]}], label=label)
                elif block[2]:
                    self.record(block[3], label=label)

    def identity(self, structure, domain: str, applier=None) -> int:
        """This structure's identity in the order, joining it as an **external
        member** on first ask, with **what can put an edit back onto it**.

        **Once per structure, not once per view.** Two windows over one thing
        are one structure in the undo order, so a second identity for the
        second window would leave its undo walking legs that name somebody
        else -- which looks exactly like a dead button.

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
            ValueError: the legs name more than one structure -- an entry over
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
        """An edit that leaves no entry -- one with no inverse to record -- still
        moves the version. Answers it."""
        if self.core is not None:
            self.version = int(self.core.call("moved").get("version", self.version))
        return self.version

    def limit_bytes(self, bytes: "int | None") -> None:
        """**Limits the takes only the history holds** to ``bytes``, or lifts
        the limit with ``None``. Past it the oldest entries go first, and the
        takes they held come back to be freed."""
        if self.core is not None:
            self.core.call("bytes", bytes=None if bytes is None else int(bytes))

    def limit_resident(self, bytes: "int | None") -> None:
        """**Keeps at most ``bytes`` of the takes only the history holds in
        memory**, or lifts the limit with ``None``. Past it the oldest are
        written to disk and read back when a step needs them."""
        if self.core is not None:
            self.core.call("resident", bytes=None if bytes is None else int(bytes))

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
        """**Carry a step's effects out** on the structures they name: a multitrack
        written back, a take's writes projected, an audio editor's join
        stitched again, a curve's points written back, an external member's
        payloads applied -- and then the
        takes the step let go of freed."""
        with self.applying():
            self._carry(stepped)
        self.release(stepped.get("freed"), stepped.get("stored"))

    def _carry(self, stepped: dict) -> None:
        for effect in stepped.get("effects") or ():
            structure, handler = self._handlers.get(int(effect.get("member", -1)),
                                                    (None, None))
            if handler is None:
                continue
            if effect.get("kind") == "multitrack":
                handler.stepped(structure, effect.get("applied") or {})
                continue
            if effect.get("kind") == "audio":
                handler.run(structure, effect.get("steps") or [])
                continue
            if effect.get("kind") == "points":
                handler.write(structure, effect.get("points") or [])
                continue
            for payload in effect.get("payloads") or ():
                if isinstance(payload, dict):
                    handler.project(structure, payload)

    def release(self, freed, stored=None) -> None:
        """**Free the takes nothing reaches any more, and write to disk the
        ones past the resident budget** -- what a turn or a step hands back as
        ``freed`` and ``stored``, each under the member that made those takes,
        whose server they are on -- an audio editor's buffers, a multitrack's
        joins by source id. Called after the turn's own steps are
        carried out, so no join is still reading them."""
        for entry in freed or ():
            structure, handler = self._handlers.get(int(entry.get("member", -1)),
                                                    (None, None))
            if handler is None:
                continue
            if entry.get("buffers") and hasattr(handler, "free"):
                handler.free(structure, entry.get("buffers") or [],
                             entry.get("spilled") or [])
            if entry.get("sources") and hasattr(handler, "free_sources"):
                handler.free_sources(structure, entry["sources"])
        for entry in stored or ():
            member = int(entry.get("member", -1))
            structure, handler = self._handlers.get(member, (None, None))
            if handler is not None and hasattr(handler, "store"):
                handler.store(structure, int(entry["buffer"]), entry.get("steps") or [])

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
        drawing has moved -- which nothing else would do: an acknowledgement goes
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
                    # off the data -- sounding a multitrack, writing a file -- wants
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


class _ScriptSteps:
    """What puts a step back onto a structure a script joined -- a sequence,
    a multitrack: the payload, through the structure's own door."""

    def project(self, structure, payload: dict) -> bool:
        return bool(structure._apply(payload, inverse=False).get("applied"))


class UndoHistory:
    """**A structure's history, as a script reads it**: the undo order its
    editors share, which a script's own changes join.

    ``seq.history`` is the door; asking for it is what gives a sequence a
    history, so a script that never asks and opens no editor records nothing.
    From then on each change made through the sequence's objects is an entry,
    and a turn: every window over it redraws, the playback reading it is
    synced, and one Ctrl+Z in a window takes back what the script did.

    Calling it makes a block: ``with seq.history("humanize"):`` makes every
    change inside it **one** entry, labelled ``"humanize"`` -- the text an undo
    names -- and one turn, so the windows redraw once, at the end.
    """

    def __init__(self, structure):
        self._structure = structure
        self._context = Editing.of(structure)

    def __call__(self, label: str = "edit"):
        return self._context.block(self._structure, str(label))

    def undo(self) -> bool:
        """Step back over the last entry -- a script's or a window's -- and
        answer whether anything moved."""
        return self._step("undo")

    def redo(self) -> bool:
        """Step forward again after `undo`; whether anything moved."""
        return self._step("redo")

    def _step(self, direction: str) -> bool:
        context = self._context
        with context.turn(None):
            stepped = context.step(direction)
            if not stepped.get("stepped"):
                return False
            context.carry(stepped)
            context.changed()
        return True

    @property
    def can_undo(self) -> bool:
        """Whether there is an entry to step back over."""
        return self._context.can_undo

    @property
    def can_redo(self) -> bool:
        """Whether there is an undone entry to step forward into."""
        return self._context.can_redo

    @property
    def undo_label(self) -> "str | None":
        """What an undo would take back, as a menu names it."""
        return self._context.undo_label

    @property
    def redo_label(self) -> "str | None":
        """What a redo would bring back."""
        return self._context.redo_label

    def __repr__(self) -> str:
        return f"<UndoHistory, undo {self.undo_label!r}>"
