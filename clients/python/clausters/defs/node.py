"""Nodes (synths and groups) and client-side id allocation.

The server's node tree (`node`): the root group is id 0; clients allocate
positive ids. Add actions match the server: head/tail of a group, before/after
a node, or replace. `Synth` and `Group` are flat handles holding
an id; the `Server` does the OSC.
"""

from enum import IntEnum

from .. import _native

ROOT_NODE_ID = 0


class AddAction(IntEnum):
    HEAD = 0
    TAIL = 1
    BEFORE = 2
    AFTER = 3
    REPLACE = 4


class Node:
    def __init__(self, node_id: int, server=None):
        self.id = node_id
        #: the `Server` that created this handle (set by ``server.synth`` and
        #: friends), so `free` knows where to send without being told.
        self.server = server

    def free(self):
        """Free this node now (``/n_free``) — the way to cut something whose
        life is long (a `play`'d expression, a slow take). Sends through the
        server that created the handle, else the ambient one."""
        server = self.server
        if server is None:
            from ..base.main import main

            server = main.resolve_server(None)
        server.free(self)

    def __repr__(self):
        return f"{type(self).__name__}(id={self.id})"


class Synth(Node):
    def __init__(self, node_id: int, defname: str, server=None):
        super().__init__(node_id, server)
        self.defname = defname


class Group(Node):
    pass


class NodeIdAllocator:
    """The registry of the client's node-id range.

    Node ids name slots of a finite boot-time resource (the server's node
    table), so the allocator is an occupancy map, not a counter: every id
    handed out stays tracked until the server reports the node's death
    (``/n_end``, fed in through `free`), which makes it allocatable again —
    the space never exhausts while nodes keep dying. ``capacity=None`` builds
    the unbounded NRT/score variant (an offline score has no live ``/n_end``
    stream to recycle from).

    It carries no range of its own: the client range of the node-id space is
    a property of the server (the partition scales from ``--max-nodes``), so
    the `Server` sizes it from its ``ServerOptions`` via
    ``_native.node_id_partition``, the same formula the server applies."""

    def __init__(self, base: int, capacity: "int | None"):
        self._registry = _native.Registry(base, capacity)

    def alloc(self) -> int:
        """A free node id. Raises `RuntimeError` when the whole range is in
        flight — allocation never wraps into ids that may still be alive."""
        node_id = self._registry.alloc()
        if node_id is None:
            raise RuntimeError(
                "out of node ids: the client range is fully in flight "
                "(nodes are recycled when their /n_end arrives)")
        return node_id

    def free(self, node_id: int):
        """Returns ``node_id`` to the pool — called when its ``/n_end``
        arrives. Ids outside the client range (another owner's) and ids not
        currently allocated are ignored: every node death on the server is
        reported, not only those of nodes this client created."""
        if self._registry.contains(node_id):
            self._registry.release(node_id)

    @property
    def in_use(self) -> int:
        """How many ids are allocated (alive or in flight) right now."""
        return self._registry.in_use
