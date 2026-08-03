"""The replay journal: what a freshly mounted canvas has to be told.

A notebook cell's front end does not exist while the cell runs. The output area
is rendered *after* the code finishes, so the first ``/gui_def`` a `plot` sends
is addressed to nobody, and so is every packet of a cell that draws and edits in
one go. The same hole reopens later for a reason that has nothing to do with
timing: a reloaded page, a reopened notebook and a moved output all rebuild the
front end against a kernel that has long since sent the tree.

So the carrier does not send to the front end, it sends to a `Journal` that
both forwards and remembers. On every mount the journal replays, and the host
in the page rebuilds the state the kernel believes it has. This is also why the
notebook needs no handshake: "the front end is ready" stops being a question
anyone has to ask.

**What is worth remembering is the tree, not its history.** Replaying a live
session verbatim would replay a slider's thousand ``/gui_set``s to arrive at the
value the last one already carries, so the journal keeps a *state*, not a log:

- ``/gui_def <id>`` starts (or replaces) the entry for that root.
- ``/gui_set <id>`` is folded into the entry it belongs to, one packet per
  property set, so a repeated edit of the same property replaces its
  predecessor instead of stacking.
- ``/gui_bind`` / ``/gui_free`` supersede what they undo: a bind replaces the
  previous binding for its widget, and freeing a root drops the whole entry.

The result is bounded by the size of what is on screen rather than by how long
the session has run, and replaying it produces the same picture as having been
there from the start.

**What is deliberately not journalled:** ``/gui_query``, which is a question
whose answer has nowhere to go on replay, and anything on the audio channel —
an engine's state is not reconstructible by replaying commands at it (a
``/synth_new`` replayed on mount would start a second voice), so a reloaded
page rejoins a running server rather than re-running its history.
"""

from clausters.base import _osclib

__all__ = ["Journal"]

#: Addresses that mutate the widget tree, in the order a rebuild needs them.
_DEF = "/gui_def"
_SET = "/gui_set"
_BIND = "/gui_bind"
_FREE = "/gui_free"


class _Entry:
    """One ``window``-rooted tree: its defining packet and the edits on top."""

    __slots__ = ("definition", "sets", "binds", "height")

    def __init__(self, definition: bytes, height=None):
        self.definition = definition
        #: The window's own ``h``, if it declared one — the cell's canvas is
        #: sized from it, so a `clausters.scope` and a tall multitrack window
        #: do not both come out at the default. ``None`` when the tree names no
        #: height.
        self.height = height
        #: (widget id, property name) -> the packet that last set it.
        self.sets: dict = {}
        #: widget id -> the packet that last bound (or unbound) it.
        self.binds: dict = {}

    def packets(self) -> list:
        return [self.definition, *self.sets.values(), *self.binds.values()]


class Journal:
    """The state of the widget tree as packets, replayable onto a fresh host.

    `record` is told every outbound GUI packet and returns whether it was kept
    (``/gui_query`` is not). `replay` returns the packets that rebuild the
    current tree, definitions first.

    The journal tracks *roots* — the ids passed to ``/gui_def`` — and attributes
    every later packet to the root whose subtree owns the widget, which it
    learns from the ids written into the tree. An id it has never seen is
    attributed to the most recent root, which is the right guess for the
    ordinary case (define a window, then edit it) and harmless otherwise: a
    packet in the wrong entry replays a moment early or late, never wrongly.
    """

    def __init__(self):
        #: root id -> `_Entry`, in definition order (dicts keep insertion order,
        #: and a tree defined later may reference one defined earlier).
        self._roots: dict = {}
        #: widget id -> the root id whose subtree it belongs to.
        self._owner: dict = {}
        self._last_root = None

    def record(self, packet: bytes) -> bool:
        """Fold one outbound GUI packet into the journal. Returns ``False`` for
        a packet that is not worth replaying (it is still sent, just not kept).
        """
        try:
            addr, args = _osclib.decode(packet)
        except Exception:
            # Undecodable outbound bytes are not this class's problem to
            # diagnose: forward them, keep nothing.
            return False
        if not args or not isinstance(args[0], int):
            return False
        wid = args[0]
        if addr == _DEF:
            self._define(wid, packet, args)
            return True
        if addr == _FREE:
            self._free(wid)
            return True
        entry = self._entry_for(wid)
        if entry is None:
            return False
        if addr == _SET:
            for name in args[1::2]:
                if isinstance(name, str):
                    entry.sets[(wid, name)] = packet
            return True
        if addr == _BIND:
            entry.binds[wid] = packet
            return True
        return False

    def replay(self) -> list:
        """The packets that rebuild the current tree on a fresh host."""
        packets = []
        for entry in self._roots.values():
            packets.extend(entry.packets())
        return packets

    def replay_root(self, root: int) -> list:
        """The packets that rebuild one window, for a front end that shows only
        that one (`clausters_jupyter.bridge.Bridge`)."""
        entry = self._roots.get(root)
        return entry.packets() if entry is not None else []

    def root_of(self, packet: bytes):
        """The window a packet belongs to, or ``None`` if it belongs to none.

        The routing question, answered from the ownership the journal already
        tracks — which is why routing lives here rather than in a parser of its
        own. A `/gui_def` names its own root; anything else names a widget
        whose root was learned when that tree was defined.
        """
        try:
            addr, args = _osclib.decode(packet)
        except Exception:
            return None
        if not args or not isinstance(args[0], int):
            return None
        wid = args[0]
        if addr == _DEF or wid in self._roots:
            return wid
        return self._owner.get(wid)

    def forget(self):
        """Drop everything — the tree is gone (the host was replaced)."""
        self._roots.clear()
        self._owner.clear()
        self._last_root = None

    def __len__(self) -> int:
        """The number of packets a replay would send."""
        return sum(len(e.sets) + len(e.binds) + 1 for e in self._roots.values())

    # ---- internals ----

    def _define(self, root: int, packet: bytes, args: list):
        # Redefining a root replaces it outright, exactly as the host does.
        self._drop_owned(root)
        tree = _tree_in(args)
        self._roots[root] = _Entry(packet, _height_of(tree))
        self._owner[root] = root
        self._last_root = root
        for wid in _ids_in(tree):
            self._owner[wid] = root

    def _free(self, wid: int):
        if wid in self._roots:
            self._drop_owned(wid)
            del self._roots[wid]
            if self._last_root == wid:
                self._last_root = next(reversed(self._roots), None)
            return
        # Freeing a widget inside a tree: forget its edits, keep the tree. The
        # definition still contains it, so this is imperfect — the subtree
        # reappears on replay. Recording the free itself would be worse (it
        # would replay before the tree that it frees from).
        entry = self._entry_for(wid)
        if entry is not None:
            entry.sets = {k: v for k, v in entry.sets.items() if k[0] != wid}
            entry.binds.pop(wid, None)

    def _drop_owned(self, root: int):
        self._owner = {w: r for w, r in self._owner.items() if r != root}

    def _entry_for(self, wid: int):
        root = self._owner.get(wid, self._last_root)
        return self._roots.get(root) if root is not None else None


    def height_of(self, root: int):
        """The window's declared ``h``, or ``None``. The cell's canvas is sized
        from it (`clausters_jupyter.bridge.Bridge.widget_for`)."""
        entry = self._roots.get(root)
        return entry.height if entry is not None else None


def _tree_in(args: list):
    """A ``/gui_def``'s JSON payload, parsed, or ``None``.

    The tree arrives as the JSON string argument; the ids were written into it
    by `clausters.gui.host.GuiHost.define` before it was encoded, so parsing it
    back is how the journal learns which widgets belong to this root — and how
    tall the window asked to be — without the host telling it.
    """
    import json

    for arg in args[1:]:
        if not isinstance(arg, str):
            continue
        try:
            return json.loads(arg)
        except ValueError:
            continue
    return None


def _height_of(tree):
    """The root node's ``h``, as an int, or ``None``."""
    if isinstance(tree, dict):
        height = tree.get("h")
        if isinstance(height, (int, float)):
            return int(height)
    return None


def _ids_in(tree) -> list:
    """Every widget id in a parsed ``/gui_def`` tree."""
    found: list = []
    _walk_ids(tree, found)
    return found


def _walk_ids(node, out: list):
    if isinstance(node, dict):
        wid = node.get("id")
        if isinstance(wid, int):
            out.append(wid)
        for value in node.values():
            _walk_ids(value, out)
    elif isinstance(node, list):
        for item in node:
            _walk_ids(item, out)
