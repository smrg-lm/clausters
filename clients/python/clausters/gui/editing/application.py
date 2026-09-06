"""The application: what a window set owns, as against what one structure owns.

An `clausters.gui.editing.Editor` edits **one structure** — a buffer's samples,
a break-point curve, a timeline of events. Almost nothing an editor does is
about that structure, though: resolving a host, handing out widget ids,
answering the acknowledgement, draining the socket, walking the undo order. All
of it is true of the **session on screen** rather than of the data, and an
editor that owns it is an editor that has to be copied whole the moment a second
one wants to share a window set.

So this is the other half of the split the subpackage already makes between an
editor and the `clausters.gui.editing.Editing` context. The context is what the
**data** owns — its history, its version, the views to tell. An application is
what the **screen** owns — the host, the id space, the echo, the loop. What is
left in between is what an editor genuinely is: a structure bound to a
`clausters.gui.editing.Domain` and a `clausters.gui.editing.View`.

Two consequences worth stating, because they are why this exists rather than
being a tidier arrangement of the same code:

- **Several editors can share one.** One host, one id space, one socket drain
  and one undo order across a bundle of subviews over structures that have
  nothing composed behind them. That is an application in the ordinary sense,
  and until now the only thing shaped like one was the multitrack — which got
  there by being a subclass of the editor rather than a peer of it.
- **An editor with no window is not a special case.** An application with no
  host resolves nothing, hands out ids from its own counter and answers the
  acknowledgement by doing nothing, which is exactly what inspecting `draw()`
  in a test needs and what an editor used to carry as a branch per method.

Every editor holds one, and makes its own when it is not handed one, so nothing
a script writes changes.
"""

import weakref

from ... import _native
from ..ids import CAPACITY, GuiIdAllocator
from .context import FIRST_VERSION, Editing
from .echo import Echo

#: The base a host-less draw counts widget ids from. Above the hand-picked range
#: and above `clausters.gui.ids.BASE_ID`, so a tree drawn with no host does not
#: collide with one drawn on a host that is allocating.
BASE_ID = 10_000


class _Anyone:
    """The drawer of a call that named none — `new_id()` asked of the
    application itself rather than by an editor.

    A real object rather than ``None`` so that it can be a weak key like every
    other drawer. One is enough for the whole module: the tables it keys live on
    an application, so two applications sharing this key still get their own.
    """


_ANYONE = _Anyone()


def _resolve_host(host):
    """The host an `open` acts on: the one named, else the ambient one — the
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
            order runs in. ``None`` — the ordinary case — reads it off the
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
        #: Where a host-less draw takes its ids from — one table per drawer,
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
        #: Each drawer's owner in each table it has drawn on — the drawer weakly,
        #: and the table under it, for the reason above.
        self._owners: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()
        #: How this application answers a version, when it was given a way.
        self._version_of = version
        #: The last tree sent for each window, so the next one can be sent as
        #: the **difference**. A window absent here has never been published and
        #: is defined whole.
        self._published: dict = {}
        #: The editors drawing in this window set, in the order they registered.
        #: Held strongly, the way the host holds an open editor: an application
        #: is what a script keeps, and its editors go when it does.
        self._editors: list = []
        #: The end of the acknowledgement protocol — the stamp, the floor, the
        #: corrections and the reason. **One per application, not one per
        #: editor**: it answers a host, and there is one host.
        self.echo = Echo(host=None, version=self._version)

    # ---- who is in it ----

    def register(self, editor) -> "Application":
        """Take an editor into this application. Idempotent, so an editor that
        re-registers does not get drained twice."""
        if not any(held is editor for held in self._editors):
            self._editors.append(editor)
        return self

    def forget(self, editor) -> "Application":
        """Drop an editor from this application — what `close` does, and what a
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

        Named at construction, or read off the first editor that has one — which
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
        return self.echo.host

    @host.setter
    def host(self, host) -> None:
        self.echo.host = host

    def resolve(self, host=None):
        """Adopt a host: the one named, else the ambient one. Answers the host
        adopted.

        **Only when it has none**, which is the rule the multitrack learned the
        hard way: an application already open answers *its* host, and overwriting
        that with the one a second window opened on sends every acknowledgement
        to the wrong place — silently, since in the ordinary case the two are the
        same object.
        """
        if self.host is None:
            self.host = _resolve_host(host)
        return self.host

    # ---- the widget-id space, and its two doors ----

    def _ids(self, drawer=None) -> GuiIdAllocator:
        """The table ``drawer`` names widgets in.

        The **host's** once there is one, so that two applications on one host —
        two editors opened on the ambient host, say — cannot hand out the same
        number. Before that, one private table per drawer: an unopened draw's
        ids reach nothing, so they need not be unique across drawers, and the
        privacy is what lets such a draw restart its numbering.

        Both doors come from whichever table it is — a leased id and a named one
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
        """A **leased** widget id: one for a widget nothing names — a hand-built
        tree, a decoration. It changes across redraws, which is why a view that
        draws a structure asks `id_for` instead."""
        return self._ids(drawer).alloc()

    def id_for(self, structure: int, role: str, key: str = "", drawer=None) -> int:
        """The id that draws ``(structure, role, key)`` — the **same** number for
        as long as ``drawer`` keeps drawing that name.

        The door a view takes, and the whole of what makes a redraw safe: an
        edit-back in flight, a correction on its way out and a widget's screen
        state all name an id, and an id that changed under them lands on
        somebody else.
        """
        table = self._ids(drawer)
        return table.id_for(self._owner(drawer, table), int(structure), role, str(key))

    def id_of(self, structure: int, role: str, key: str = "",
              drawer=None) -> "int | None":
        """The id already drawing that name, or ``None`` — a lookup, which mints
        nothing and does not count as drawing it."""
        return self._ids(drawer).id_of(int(structure), role, str(key))

    def reset_ids(self, drawer=None) -> None:
        """Start ``drawer``'s draw: from here, every name it asks for counts as
        drawn, and `retire_ids` takes back the rest.

        Every other drawer is untouched — the cycle names whose it is, so two
        editors on one host redraw independently.
        """
        table = self._ids(drawer)
        if table is not getattr(self.host, "ids", None):
            # **Nothing outside this draw holds one of these ids.** With no host
            # there is no window, no pending gesture and no second drawer in this
            # table, so it starts over and two draws of one picture come out
            # identical — the property a test that inspects a tree twice rests
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

    # ---- the acknowledgement, which is the echo's ----

    @property
    def corrections(self) -> list:
        return self.echo.corrections

    @corrections.setter
    def corrections(self, value) -> None:
        self.echo.corrections = list(value)

    @property
    def floor(self) -> int:
        return self.echo.floor

    @floor.setter
    def floor(self, value) -> None:
        self.echo.floor = int(value)

    @property
    def reason(self) -> "str | None":
        return self.echo.reason

    @reason.setter
    def reason(self, value) -> None:
        self.echo.reason = value

    def announce(self) -> None:
        """Tell the host which version it is drawing, before any edit."""
        self.echo.announce()

    def stale(self, against: int) -> bool:
        """Whether an edit made against version ``against`` has been
        overtaken."""
        return self.echo.stale(against)

    def correct(self, widget_id: int, **props) -> None:
        """What the host should be drawing instead of what it drew."""
        self.echo.correct(widget_id, **props)

    def acknowledge(self, seq: int, reason: "str | None" = None) -> None:
        """Answer the host for everything up to ``seq``."""
        self.echo.acknowledge(seq, reason)

    # ---- the history walk ----

    def step(self, direction: str, walker) -> bool:
        """One step of the pile, **handed round the context**.

        The history holds structures the crate cannot reach, so it applies
        nothing: what comes back is an ordered list of legs, each naming the
        structure it belongs to. One entry can name several — a stroke over a
        take and a bend of the curve over it are one order — so the step is
        offered to **every editor in the context**, and each projects the legs it
        owns. An editor that walked only its own legs would step the cursor over
        somebody else's edit and undo nothing, which looks exactly like a dead
        button.

        ``walker`` is whoever asked, and it is the one that draws afterwards:
        every other window is told on the way out of the turn, the way it is told
        about any edit, so a step is one answer per window rather than two.
        """
        context = self.context
        if context is None:
            return False
        legs = context.step(direction)
        if legs is None or not context.distribute(legs, walker):
            return False
        # **Once for the walk, not once per window.** The version is the
        # context's, and every view reports the same one.
        context.version += 1
        walker.reflect_step()
        return True

    # ---- publishing a picture: the difference, when there is one ----

    def published(self, window_id: int, tree: dict) -> None:
        """Record ``tree`` as what the host is now drawing for ``window_id``.

        Called by whoever sent it, so that the *next* picture can be sent as a
        difference. `open` goes through here rather than through `publish`: the
        first tree is a definition by nature.
        """
        self._published[int(window_id)] = tree

    def forget_window(self, window_id: int) -> None:
        """Drop what is remembered about a window that closed, so a window that
        opens again is defined whole rather than diffed against a picture
        nobody is drawing."""
        self._published.pop(int(window_id), None)

    def publish(self, window_id: int, tree: dict, *blobs: bytes) -> bool:
        """Make the host draw ``tree`` for ``window_id``, **as a difference when
        it can be**. Answers whether it was a redefine.

        A redefine is expensive in a way that has nothing to do with bytes: the
        host frees the old subtree and builds a new one, so every widget's
        screen state goes with it — a scroll position, a zoom, a selection in
        flight — and everything the host had pending is dropped. Doing that
        because one number changed is what makes a window flicker under a hand
        that is not even in it.

        So when the two pictures have the **same shape** — the same widgets, in
        the same places, with the same names — what goes out is one `/gui_set`
        per widget whose props moved, and nothing is freed or built. Only a
        change of shape redefines, and that is also what `open` does.

        **What counts as a shape change is the core's** (`gui_difference`), not
        this module's: a redefine costs every widget's screen state, and two
        clients deciding differently when that is unavoidable is two clients
        redrawing differently.
        """
        host = self.host
        if host is None:
            return False
        window_id = int(window_id)
        previous = self._published.get(window_id)
        # A tree with blobs goes whole: a blob is referenced by index from a
        # `/gui_def`'s trailing arguments, and a `/gui_set` has no such index.
        sets = (None if blobs or previous is None
                else _native.gui_difference(previous, tree, window_id))
        if sets is None:
            host.define(window_id, tree, *blobs)
            self._published[window_id] = tree
            return True
        for wid, props in sets:
            host.set(wid, **props)
        self._published[window_id] = tree
        return False

    # ---- the loop ----

    def poll(self, timeout: float = 0.0) -> bool:
        """Drain the host's pending messages into every editor registered here
        **and on to the windows' own handlers**. Returns whether any data
        changed.

        One loop, because one socket: two loops over one host race each other for
        the same messages. Each editor is offered every message and answers only
        for the widgets it drew, which is what `Editor.apply` is written to do.

        Call it from the script's loop — **never** from the clock thread, which a
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
        """Hold the calling thread while ``until()`` is true — the drain the
        host's own `wait` is, so a script ends on one call.

        ``True`` when the condition cleared, ``False`` when ``timeout`` ran out
        first. With no host there is nothing to wait for, and the answer is
        whether the condition is already clear.
        """
        host = self.host
        if host is None:
            return not until()
        return host._wait_while(until, timeout)
