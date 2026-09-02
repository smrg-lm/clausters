"""Client-side allocation of GUI widget ids.

Widget ids name nodes of the host's one widget namespace, exactly as node ids
name slots of the audio server's node table — so this allocator is the GUI
sibling of `clausters.defs.node.NodeIdAllocator`, built on the same core
`clausters._native.Registry` occupancy map. Two things are worth spelling out:

- **Bounded, so the ids recycle.** Like the RT node allocator (and unlike the
  unbounded NRT one), the registry has a capacity: `alloc` hands ids out of a
  fixed window ``[base, base + capacity)`` and, once the high-water mark reaches
  the top, reuses the ones `free` has returned. So the numeric id space never
  climbs without bound over a live session — which matters because the multitrack
  `clausters.gui.editing.FormEditor` re-allocates its whole widget range on every
  redraw. The capacity is generous (`CAPACITY`): exhaustion means that many
  widgets are live *at once*, a client bug, and is raised loudly.
- **The client drives the recycle.** A node id returns to the pool when the
  server reports the node's death (``/node_end``); a widget id has no such
  side-channel, so it returns when the client frees the widget (`GuiHost.free`/
  `close`, and a redraw re-defining a window, which frees the old subtree first).

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
        self._registry = _native.Registry(*share_of(base, capacity, share))

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

    @property
    def in_use(self) -> int:
        """How many ids are allocated right now."""
        return self._registry.in_use
