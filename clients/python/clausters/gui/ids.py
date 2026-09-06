"""Client-side allocation of GUI widget ids.

Widget ids name nodes of the host's one widget namespace, exactly as node ids
name slots of the audio server's node table — so this allocator is the GUI
sibling of `clausters.defs.node.NodeIdAllocator`. It is built on the core's
`clausters._native.WidgetIds`, which is that same occupancy map **plus a second
door**, and the two doors are the thing to understand here:

- **A lease** (`alloc`) is what a hand-built GuiDef takes: an id for a widget
  nothing names, handed out in order and returned by `free`.
- **A name** (`id_for`) is what a view takes: an id asked for by saying what it
  draws — the structure's identity in the history, the role the widget plays,
  and which one it is — which gives back the **same number** for as long as that
  name keeps being drawn. A leased id changes on every redraw, so anything in
  flight across one lands on the wrong widget: an edit-back the owner has not
  answered yet, a correction travelling the other way, the screen state of a
  widget that no longer exists under that number. A named id has no such gap.

Both doors take from one occupancy map, which is what makes them impossible to
collide — and an anonymous `free` deliberately cannot take back a named id, so
a redefine freeing a subtree widget by widget does not hand a live name's number
to somebody else.

Three more things are worth spelling out:

- **Bounded, so the ids recycle.** Like the RT node allocator (and unlike the
  unbounded NRT one), the table has a capacity: ids come out of a fixed window
  ``[base, base + capacity)`` and, once the high-water mark reaches the top,
  reuse the ones that were returned. The capacity is generous (`CAPACITY`):
  exhaustion means that many widgets are live *at once*, a client bug, and is
  raised loudly.
- **The client drives the recycle.** A node id returns to the pool when the
  server reports the node's death (``/node_end``); a widget id has no such
  side-channel, so a leased one returns when the client frees the widget
  (`GuiHost.free`/`close`, and a redraw re-defining a window), and a named one
  returns when a draw stops asking for it (`retire`).
- **A draw names its drawer.** One table serves a whole host and a host carries
  more than one drawer — two editors opened on the ambient host are two of them
  — so `begin`/`retire` take an `owner` handed out by `owner`. Without it either
  drawer would retire the other's widgets simply by redrawing.

The base is 1000, preserving the long-standing contract that hand-picked ids
below 1000 never collide with assigned ones.
"""

from .. import _native
from ..base.ids import share_of

#: The first id the allocator hands out. Hand-picked ids below this never
#: collide with assigned ones (the documented `/gui_def` id convention).
BASE_ID = 1000
#: The size of the id window. Far beyond any real count of simultaneously live
#: widgets, so the space recycles (ids stay in ``[BASE_ID, BASE_ID + CAPACITY)``)
#: without ever exhausting in practice.
CAPACITY = 1 << 20


class GuiIdAllocator:
    """The registry of a host client's widget-id space.

    An occupancy map, not a counter: every id handed out by `alloc` stays
    tracked until `free` returns it, which makes it allocatable again — so a
    long session that opens and closes many windows (or an Editor that redraws
    repeatedly) recycles ids within a fixed window instead of climbing without
    bound.
    """

    def __init__(self, base: int = BASE_ID, capacity: int = CAPACITY, share=None):
        #: A ``share`` takes one slice of the window instead of all of it, for
        #: a host with more than one client naming widgets on it — the same
        #: arithmetic as the audio server's (`clausters.base.IdShare`).
        self._registry = _native.WidgetIds(*share_of(base, capacity, share))

    def owner(self) -> int:
        """A fresh drawer, for the `begin`/`retire` cycle below.

        One table serves the whole host, so a drawer is a number the table hands
        out rather than one a caller invents: two of them inventing their own
        would eventually pick the same one, and each would then take back the
        other's widgets by redrawing."""
        return self._registry.owner()

    def alloc(self) -> int:
        """A fresh id, unique across everything this allocator names. Raises
        `RuntimeError` if the whole window is live at once (a client bug —
        `CAPACITY` widgets never coexist in practice)."""
        wid = self._registry.alloc()
        if wid is None:
            raise RuntimeError(
                "out of gui widget ids: the id window is fully in use "
                "(freed widgets recycle their ids — this many live at once "
                "is a leak)")
        return wid

    def free(self, wid: int):
        """Return ``wid`` to the pool. Ids outside this allocator's window (a
        hand-picked id below the base) and ids not currently allocated are
        ignored, so freeing is always safe — mirrors `NodeIdAllocator.free`."""
        if self._registry.contains(wid):
            self._registry.release(wid)

    def id_for(self, owner: int, structure: int, role: str, key: str) -> int:
        """The id ``owner`` draws ``(structure, role, key)`` with — the **same**
        one for as long as that name keeps being drawn.

        Raises `RuntimeError` on exhaustion, as `alloc` does and for the same
        reason: a name that cannot be given a number is a client bug, not a
        value to be handled."""
        wid = self._registry.id_for(owner, structure, role, key)
        if wid is None:
            raise RuntimeError(
                "out of gui widget ids: the id window is fully in use "
                "(freed widgets recycle their ids — this many live at once "
                "is a leak)")
        return wid

    def id_of(self, structure: int, role: str, key: str) -> "int | None":
        """The id that draws ``(structure, role, key)`` **if it already has
        one** — no minting, and no effect on any draw. The inverse a view asks
        when it needs to know what is drawing something."""
        return self._registry.id_of(structure, role, key)

    def forget(self, structure: int, role: str, key: str) -> "int | None":
        """Give one name's id back, outside any draw. Answers the id released."""
        return self._registry.forget(structure, role, key)

    def begin(self, owner: int):
        """Start ``owner``'s draw: every name it asks for until `retire` counts
        as still drawn. Another drawer's cycle is untouched."""
        self._registry.begin(owner)

    def retire(self, owner: int) -> list:
        """End ``owner``'s draw and take back every name of that owner's it did
        not ask for, answering the ids released, ascending."""
        return self._registry.retire(owner)

    def clear(self):
        """Drop every name and every id: the table as it was made.

        A client reset. Only an id space nothing outside it holds may be cleared
        — an editor's private one before it has a host, never a live host's."""
        self._registry.clear()

    @property
    def in_use(self) -> int:
        """How many ids are allocated right now, leased and named together."""
        return self._registry.in_use

    @property
    def named(self) -> int:
        """How many of them answer to a name."""
        return self._registry.named
