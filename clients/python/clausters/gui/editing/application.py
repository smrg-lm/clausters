"""The application: what a window set owns, as against what one structure owns.

An `clausters.gui.editing.Editor` edits **one structure** -- a buffer's samples,
a break-point curve, a timeline of events. Almost nothing an editor does is
about that structure, though: resolving a host, handing out widget ids,
answering the acknowledgement, draining the socket, walking the undo order. All
of it is true of the **session on screen** rather than of the data, and an
editor that owns it is an editor that has to be copied whole the moment a second
one wants to share a window set.

So this is the other half of the split the subpackage already makes between an
editor and the `clausters.gui.editing.Editing` context. The context is what the
**data** owns -- its history, its version, the views to tell. An application is
what the **screen** owns -- the host, the id space, the loop. What is left in
between is what an editor genuinely is: a structure bound to a
`clausters.gui.editing.Domain` and a `clausters.gui.editing.View`, with its own
end of the conversation.

**The acknowledgement is not here, and that was a defect for a while.** An
`clausters.gui.editing.Echo` held the floor and the stamp on the application,
on the reasoning that it answers a host and there is one host -- which confuses
*who you talk to* with *what state the conversation has*. The crate is explicit
that a `Conversation` is **one view's** end: the floor rises when the version
moved and no event of **this view** moved it, so two windows over one structure
sharing a floor would each silence the other's staleness check, and a gesture
made against a picture a neighbouring window had already changed would be
accepted instead of refused. The echo is the editor's; what is shared here is
the host it answers to.

Two consequences worth stating, because they are why this exists rather than
being a tidier arrangement of the same code:

- **Several editors can share one.** One host, one id space, one socket drain
  and one undo order across a bundle of subviews. That is an application in the
  ordinary sense, and the multitrack is the one shaped like it: a multitrack plus the
  boxes a hand entered out of it are one window set, and
  `clausters.gui.editing.MultitrackEditor.enter` hands each of them this.
- **An editor with no window is not a special case.** An application with no
  host resolves nothing, hands out ids from its own counter and answers the
  acknowledgement by doing nothing, which is exactly what inspecting `draw()`
  in a test needs and what an editor used to carry as a branch per method.

Every editor holds one, and makes its own when it is not handed one, so nothing
a script writes changes.
"""

import weakref

from ..ids import CAPACITY, GuiIdAllocator
from .context import FIRST_VERSION, Editing
from .trace import log

#: The base a host-less draw counts widget ids from. Above the hand-picked range
#: and above `clausters.gui.ids.BASE_ID`, so a tree drawn with no host does not
#: collide with one drawn on a host that is allocating.
BASE_ID = 10_000


class _Anyone:
    """The drawer of a call that named none -- `new_id()` asked of the
    application itself rather than by an editor.

    A real object rather than ``None`` so that it can be a weak key like every
    other drawer. One is enough for the whole module: the tables it keys live on
    an application, so two applications sharing this key still get their own.
    """


_ANYONE = _Anyone()


def _resolve_host(host):
    """The host an `open` acts on: the one named, else the ambient one -- the
    same resolution `clausters.gui.guidef.View.open`, `clausters.plot` and
    `clausters.scope` share, so an editor is not the one resource that has to be
    handed a host."""
    if host is not None:
        return host
    from ...plot import _ambient_host

    return _ambient_host()


class Application:
    """One window set: its host, its widget ids, its acknowledgement, its loop.

    Args:
        context: the `clausters.gui.editing.Editing` this application's undo
            order runs in. ``None`` -- the ordinary case -- reads it off the
            editors registered here, so an application over one structure has
            that structure's context without being told.
        base_id: where the host-less id counter starts.
        version: a zero-argument callable answering the version to stamp an
            acknowledgement with. ``None`` reads it from `context`. It is a
            callable and not a number for the reason `clausters.gui.editing.Echo`
            states: the version belongs to the editing context and moves under
            this object.
    """

    def __init__(self, *, context=None, base_id: int = BASE_ID, version=None):
        #: The context when one was named; otherwise the editors' own, asked for
        #: on each use.
        self._context = context
        #: Where a host-less draw takes its ids from -- one table per drawer,
        #: built on the first ask and never used again once there is a host.
        #: **Per drawer** and not per application, because an unopened draw's
        #: ids reach nothing: two of them cannot collide with each other, and
        #: keeping them apart is what lets a draw with no window start its
        #: numbering over so that drawing one picture twice gives one tree.
        #:
        #: Keyed by the **drawer**, weakly, not by its address: `id()` is reused
        #: the moment an object is freed, so a table keyed by one would hand a
        #: new editor whatever the last one at that address had drawn. Weak keys
        #: also let a closed editor's table go with it.
        self._base_id = int(base_id)
        self._offline: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()
        #: Each drawer's owner in each table it has drawn on -- the drawer weakly,
        #: and the table under it, for the reason above.
        self._owners: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()
        #: How this application answers a version, when it was given a way.
        self._version_of = version
        #: The editors drawing in this window set, in the order they registered.
        #: Held strongly, the way the host holds an open editor: an application
        #: is what a script keeps, and its editors go when it does.
        self._editors: list = []
        #: The host this window set answers, or ``None`` before it is opened.
        #: The **acknowledgement is not here**: an `clausters.gui.editing.Echo`
        #: is one *view's* end of the conversation, and it is the editor's. See
        #: the module docstring.
        self._host = None

    # ---- who is in it ----

    def register(self, editor) -> "Application":
        """Take an editor into this application. Idempotent, so an editor that
        re-registers does not get drained twice."""
        if not any(held is editor for held in self._editors):
            self._editors.append(editor)
        return self

    def forget(self, editor) -> "Application":
        """Drop an editor from this application -- what `close` does, and what a
        composed view does when its window goes."""
        self._editors = [held for held in self._editors if held is not editor]
        return self

    @property
    def editors(self) -> list:
        """The editors registered here, in registration order."""
        return list(self._editors)

    # ---- the editing context, and the version ----

    @property
    def context(self) -> "Editing | None":
        """The editing context this application's undo order runs in.

        Named at construction, or read off the first editor that has one -- which
        is the ordinary case and is what keeps an application over one structure
        from having to be told what it is editing.
        """
        if self._context is not None:
            return self._context
        for editor in self._editors:
            found = getattr(editor, "_editing", None)
            if found is not None:
                return found
        return None

    @context.setter
    def context(self, context) -> None:
        self._context = context

    def _version(self) -> int:
        """The version an acknowledgement carries. A method rather than a
        property because it is what `Echo` was handed."""
        if self._version_of is not None:
            return int(self._version_of())
        context = self.context
        return FIRST_VERSION if context is None else int(context.version)

    # ---- the host ----

    @property
    def host(self):
        """The host this application answers, or ``None`` before it is
        opened."""
        return self._host

    @host.setter
    def host(self, host) -> None:
        self._host = host
        for editor in self._editors:
            echo = getattr(editor, "echo", None)
            if echo is not None:
                echo.host = host

    def resolve(self, host=None):
        """Adopt a host: the one named, else the ambient one. Answers the host
        adopted.

        **Only when it has none**, which is the rule the multitrack learned the
        hard way: an application already open answers *its* host, and overwriting
        that with the one a second window opened on sends every acknowledgement
        to the wrong place -- silently, since in the ordinary case the two are the
        same object.
        """
        if self.host is None:
            self.host = _resolve_host(host)
        return self.host

    # ---- the widget-id space, and its two doors ----

    def _ids(self, drawer=None) -> GuiIdAllocator:
        """The table ``drawer`` names widgets in.

        The **host's** once there is one, so that two applications on one host --
        two editors opened on the ambient host, say -- cannot hand out the same
        number. Before that, one private table per drawer: an unopened draw's
        ids reach nothing, so they need not be unique across drawers, and the
        privacy is what lets such a draw restart its numbering.

        Both doors come from whichever table it is -- a leased id and a named one
        are the same resource taken two ways, and splitting them across two
        tables is how two widgets end up with one number.
        """
        ids = getattr(self.host, "ids", None)
        if ids is not None:
            return ids
        drawer = _ANYONE if drawer is None else drawer
        table = self._offline.get(drawer)
        if table is None:
            table = GuiIdAllocator(base=self._base_id, capacity=CAPACITY)
            self._offline[drawer] = table
        return table

    def _owner(self, drawer, table: GuiIdAllocator) -> int:
        """``drawer``'s owner in ``table``, minted once per pair.

        Per pair rather than once, because a drawer that drew before it was
        opened has already named widgets in a table the host knows nothing
        about: each table hands out its own drawers.
        """
        drawer = _ANYONE if drawer is None else drawer
        mine = self._owners.get(drawer)
        if mine is None:
            mine = weakref.WeakKeyDictionary()
            self._owners[drawer] = mine
        owner = mine.get(table)
        if owner is None:
            owner = table.owner()
            mine[table] = owner
        return owner

    def new_id(self, drawer=None) -> int:
        """A **leased** widget id: one for a widget nothing names -- a hand-built
        tree, a decoration. It changes across redraws, which is why a view that
        draws a structure asks `id_for` instead."""
        return self._ids(drawer).alloc()

    def id_for(self, structure: int, role: str, key: str = "", drawer=None) -> int:
        """The id that draws ``(structure, role, key)`` -- the **same** number for
        as long as ``drawer`` keeps drawing that name.

        The door a view takes, and the whole of what makes a redraw safe: an
        edit-back in flight, a correction on its way out and a widget's screen
        state all name an id, and an id that changed under them lands on
        somebody else.
        """
        table = self._ids(drawer)
        return table.id_for(self._owner(drawer, table), int(structure), role, str(key))

    def reset_ids(self, drawer=None) -> None:
        """Start ``drawer``'s draw: from here, every name it asks for counts as
        drawn, and `retire_ids` takes back the rest.

        Every other drawer is untouched -- the cycle names whose it is, so two
        editors on one host redraw independently.
        """
        table = self._ids(drawer)
        if table is not getattr(self.host, "ids", None):
            # **Nothing outside this draw holds one of these ids.** With no host
            # there is no window, no pending gesture and no second drawer in this
            # table, so it starts over and two draws of one picture come out
            # identical -- the property a test that inspects a tree twice rests
            # on. On a host it would be wrong: the leases there belong to every
            # window the client has open, not to whoever is drawing.
            table.clear()
            mine = self._owners.get(_ANYONE if drawer is None else drawer)
            if mine is not None:
                mine.pop(table, None)
        table.begin(self._owner(drawer, table))

    def retire_ids(self, drawer=None) -> list:
        """End ``drawer``'s draw and take back every name it stopped drawing,
        answering the ids released. A draw that named nothing releases
        nothing."""
        table = self._ids(drawer)
        return table.retire(self._owner(drawer, table))

    # ---- the history walk ----

    #: What the **last** step could not reach, when a walk was refused because
    #: no participant held the structure the entry names -- the label of the edit
    #: that is waiting, for whoever wants to say why nothing happened. ``None``
    #: after a step that landed, and after one there was nothing to take.
    refusal: "str | None" = None

    def step(self, direction: str, walker) -> bool:
        """One step of the order, **taken by the context** and carried out here.

        The history and the members are the crate's (`clausters.gui.editing.
        Editing`): it walks the pile, hands each leg to the member that owns the
        structure, and puts the cursor back -- with the reason -- when nothing
        could apply it. What is left is the objects: a multitrack written back, a
        take's writes, a curve's payloads.

        ``walker`` is whoever asked, and it is the one that draws afterwards:
        every other window is told on the way out of the turn, the way it is told
        about any edit, so a step is one answer per window rather than two.
        """
        context = walker._editing
        if context is None:
            return False
        return self.stepped(context.step(direction), walker)

    def stepped(self, stepped: dict, walker) -> bool:
        """Carry out a step the context **already took** -- by `step`, or inside
        a turn whose message was an undo -- and say whether anything moved.

        A step nothing could apply is not a step: the crate put the cursor back,
        and `refusal` is why, which is what an acknowledgement says instead of
        a dead button.
        """
        self.refusal = None
        if not stepped.get("stepped"):
            self.refusal = stepped.get("reason")
            log.debug("step   nothing moved%s",
                      "" if self.refusal is None else f" ({self.refusal})")
            return False
        walker._editing.carry(stepped)
        log.debug("step   -> version %s", stepped.get("version"))
        walker.reflect_step()
        return True

    # ---- publishing a picture: the difference, when there is one ----

    def publish(self, widget_id: int, tree: dict, *blobs: bytes,
                window: "int | None" = None) -> None:
        """Make the host draw ``tree`` for ``widget_id`` -- **the whole tree,
        every time**.

        A definition used to mean *free this and build that*, so re-sending a
        window because one number moved took the screen state of every widget in
        it -- a scroll position, a zoom, a selection in flight -- and dropped
        everything the host had pending there. It no longer does: a ``/gui_def``
        over a tree the host is already drawing says *what to look like*, and
        the host **reconciles**, matching widget to widget by the id that names
        what it draws and keeping what is its own.

        **So this client holds no picture of the host's**, and that is the whole
        of the change. It used to keep the last tree per window and send the
        difference, which is only correct if that copy equals what the host
        holds -- and it cannot: the host mutates on its own (a drag writes an
        offset per frame, a wheel writes a window, a marquee writes a mark) and
        screen state is reported by nothing, correctly, because screen state is
        the host's. A difference against a picture nobody is drawing is a set to
        a widget that moved somewhere else.

        **What that leaves the caller is the granularity, and it is the caller's
        for a reason.** ``/gui_def`` names any widget, so publish the one your
        edit touched -- you know which, because an intent names a node and a
        widget id is derived from it. Measured over a drag, the subtree of the
        clip that moved is flat in the size of the multitrack; the window is not, and
        on a large one it is megabytes a second of JSON for a gesture that
        touched one rectangle. Name a ``window`` to publish a part of it: the
        names under the old subtree go and the rest of the window keeps the ones
        it had.

        It answers nothing. It used to say whether anything had been rebuilt, so
        a caller could pair a redefinition with an announcement; what a def
        costs is now the host's answer and no longer knowable here, so the
        announcement pairs with **every** publish rather than with a bool.
        """
        host = self.host
        if host is None:
            return
        widget_id = int(widget_id)
        log.debug("publish %s: %d widget(s)%s", widget_id, _widgets(tree),
                  "" if window is None else f" inside window {window}")
        if window is None:
            host.define(widget_id, tree, *blobs)
        else:
            host.redefine(widget_id, tree, *blobs, window=int(window))

    # ---- the loop ----

    def poll(self, timeout: float = 0.0) -> bool:
        """Drain the host's pending messages into every editor registered here
        **and on to the windows' own handlers**. Returns whether any data
        changed.

        One loop, because one socket: two loops over one host race each other for
        the same messages. Each editor is offered every message and answers only
        for the widgets it drew, which is what `Editor.apply` is written to do.

        Call it from the script's loop -- **never** from the clock thread, which a
        routine must never block. The second half is why a window may carry both:
        a panel beside an editor is the script's, addressed to widgets no editor
        drew, and its `clausters.gui.handle.WidgetHandle.on_event` callbacks run
        here because this is the loop that took the message off the socket.
        """
        host = self.host
        if host is None:
            raise RuntimeError("open(host) the editor first")
        if host.looping:
            # The loop drains this host and hands every message to `apply`
            # already. Answering `False` rather than raising is deliberate: a
            # script written around this call keeps running unchanged, it has
            # simply stopped being the thing that delivers.
            return False
        changed = False
        while (msg := host.poll(timeout)) is not None:
            for editor in list(self._editors):
                changed |= bool(editor.apply(*msg))
            host.dispatch(*msg)
            timeout = 0.0  # only the first wait blocks
        return changed

    def wait(self, until, timeout: "float | None" = None) -> bool:
        """Hold the calling thread while ``until()`` is true -- the drain the
        host's own `wait` is, so a script ends on one call.

        ``True`` when the condition cleared, ``False`` when ``timeout`` ran out
        first. With no host there is nothing to wait for, and the answer is
        whether the condition is already clear.
        """
        host = self.host
        if host is None:
            return not until()
        return host._wait_while(until, timeout)


def _widgets(tree: dict) -> int:
    """How many widgets a published tree holds -- the trace's measure of what a
    redraw cost, now that how much of it the host rebuilds is the host's."""
    return 1 + sum(_widgets(child) for child in tree.get("children") or ()
                   if isinstance(child, dict))
