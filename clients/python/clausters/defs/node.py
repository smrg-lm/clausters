"""Nodes (synths and groups) and client-side id allocation.

The server's node tree (`node`): the root group is id 0; clients allocate
positive ids. Add actions match the server: head/tail of a group, before/after
a node, or replace. `Synth` and `Group` hold an id and the server it lives on,
and own the commands addressed to it: `Synth.new` / `Group.new` /
`Group.graph` create one, and `Node.set`, `Node.map`, `Node.run` and
`Node.free` drive it. The id pool itself belongs to the `Server`.
"""

from enum import IntEnum

from .. import _native
from .info import NodeInfo, parse_n_info
from ._wire import resolve as _resolve

ROOT_NODE_ID = 0


class AddAction(IntEnum):
    HEAD = 0
    TAIL = 1
    BEFORE = 2
    AFTER = 3
    REPLACE = 4


def _flatten_controls(controls) -> list:
    """Accepts a dict or a list of (name, value) pairs (so the reserved
    ``in``/``out`` controls, which are Python keywords, are expressible)."""
    if controls is None:
        return []
    items = controls.items() if isinstance(controls, dict) else controls
    flat = []
    for name, value in items:
        flat += [name, value]
    return flat


class Node:
    def __init__(self, node_id: int, server=None):
        self.id = node_id
        #: the `Server` that created this handle (set by `Synth.new` and
        #: friends), so its commands know where to go without being told.
        self.server = server

    def _server(self):
        """This node's server, or the ambient one — a handle built from a
        reported id (a responder, the GUI, the arrangement) carries none."""
        return _resolve(self.server)

    def set(self, controls):
        """Set controls by name (``/node_set``). On a GraphDef instance the names
        resolve against the graph's surface, not its private members."""
        self._server().send_msg("/node_set", self.id, *_flatten_controls(controls))

    def map(self, name, bus, *, audio=False):
        """Map a control to a bus (``/node_map``, or ``/node_mapAudio`` for an audio
        bus), so the control follows whatever the bus carries."""
        index = bus.index if hasattr(bus, "index") else bus
        self._server().send_msg("/node_mapAudio" if audio else "/node_map",
                                self.id, name, index)

    def info(self, timeout: float = 5.0) -> NodeInfo:
        """This node as the server holds it **right now** (``/node_query`` ->
        ``/node_query.reply``): where it sits in the tree, and for a synth its def, its
        controls, its ``/node_map`` bindings and the buses it reads and writes.

        A photograph, not a state: a running envelope or a mapped control moves
        under the record's feet, so nothing caches it. A node that is gone —
        freed, or ended by a ``done_action`` — comes back with ``exists``
        false rather than raising. Blocking, RT only."""
        _, args = self._server().request("/node_query", self.id, timeout=timeout,
                                         expect=("/node_query.reply",))
        return parse_n_info(args)

    def u_cmd(self, ugen_index: int, name: str, *args):
        """Sends a typed command to **one UGen instance** inside this synth
        (``/node_ugenCmd nodeID ugenIndex name args…``). The server hashes ``name`` to a
        stable selector and routes the numeric ``args`` to that UGen on the audio
        thread. The FFT chain uses it to swap a window live, e.g.
        ``synth.u_cmd(fft_index, "window", 4)`` for a Blackman window
        (a `clausters._native.Window` value); an unrecognized ``name`` is a
        no-op on the server."""
        self._server().send_msg("/node_ugenCmd", self.id, int(ugen_index), str(name),
                                *(float(a) for a in args))

    def free(self):
        """Free this node now (``/node_free``) — the way to cut something whose
        life is long (a `play`'d expression, a slow take). Frees a GraphDef
        instance too, private buses included.

        The id is **not** returned to the registry here: it stays tracked until
        the server confirms the death with ``/node_end`` — releasing at send time
        could re-hand an id whose node is still alive on the server."""
        self._server().send_msg("/node_free", self.id)

    def run(self, flag: bool = True):
        """Pauses (``flag=False``) or resumes (``flag=True``) this node — a
        synth or a whole group — with ``/node_run``. A paused node stays in the
        tree and keeps its state but is skipped (silent); this is what resumes
        a synth parked by ``DoneAction.PAUSE_SELF``."""
        self._server().send_msg("/node_run", self.id, 1 if flag else 0)

    def pause(self):
        """Pauses this node (``/node_run … 0``). See `run`."""
        self.run(False)

    def resume(self):
        """Resumes this node (``/node_run … 1``). See `run`."""
        self.run(True)

    def __repr__(self):
        return f"{type(self).__name__}(id={self.id})"


class Synth(Node):
    def __init__(self, node_id: int, defname: str, server=None):
        super().__init__(node_id, server)
        self.defname = defname

    @classmethod
    def new(cls, defname, controls=None, *, target=ROOT_NODE_ID,
            action=AddAction.TAIL, server=None) -> "Synth":
        """Starts a synth from a def already loaded on the server, by name
        (``/synth_new``), with ``controls`` (a ``{name: value}`` dict, or pairs)
        overriding the def defaults."""
        srv = _resolve(server)
        node_id = srv._node_id()
        srv.send_msg("/synth_new", defname, node_id, int(action), int(target),
                     *_flatten_controls(controls))
        return cls(node_id, defname, srv)


class Group(Node):
    @classmethod
    def new(cls, *, target=ROOT_NODE_ID, action=AddAction.TAIL,
            server=None) -> "Group":
        """An empty group in the node tree (``/group_new``)."""
        srv = _resolve(server)
        node_id = srv._node_id()
        srv.send_msg("/group_new", node_id, int(action), int(target))
        return cls(node_id, srv)

    @classmethod
    def graph(cls, defname, ports=None, *, target=ROOT_NODE_ID,
              action=AddAction.TAIL, server=None) -> "Group":
        """Instantiates a GraphDef already loaded on the server, by name
        (``/graph_new``), as a wired group, with ``ports`` (a
        ``{name: value}`` dict) overriding the def defaults. The returned
        `Group` is the instance: drive it through the surface with `set`
        (``/node_set`` resolves names against the surface, not the private
        members) and tear it down with `free` (which also reclaims its private
        buses)."""
        srv = _resolve(server)
        node_id = srv._node_id()
        srv.send_msg("/graph_new", defname, node_id, int(action), int(target),
                     *_flatten_controls(ports))
        return cls(node_id, srv)

    def voice(self, ports=None) -> "Group":
        """Spawns a per-voice sub-graph (``/graph_newVoice``) inside this running
        GraphDef instance (a group from `graph`), wired to its shared private
        buses. ``ports`` overrides the voice-port defaults. The returned group
        is the voice: drive it through its surface with `set` and free it with
        `free`."""
        srv = self._server()
        node_id = srv._node_id()
        srv.send_msg("/graph_newVoice", self.id, node_id, *_flatten_controls(ports))
        return Group(node_id, srv)


class NodeIdAllocator:
    """The registry of the client's node-id range.

    Node ids name slots of a finite boot-time resource (the server's node
    table), so the allocator is an occupancy map, not a counter: every id
    handed out stays tracked until the server reports the node's death
    (``/node_end``, fed in through `free`), which makes it allocatable again —
    the space never exhausts while nodes keep dying. ``capacity=None`` builds
    the unbounded NRT/score variant (an offline score has no live ``/node_end``
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
                "(nodes are recycled when their /node_end arrives)")
        return node_id

    def free(self, node_id: int):
        """Returns ``node_id`` to the pool — called when its ``/node_end``
        arrives. Ids outside the client range (another owner's) and ids not
        currently allocated are ignored: every node death on the server is
        reported, not only those of nodes this client created."""
        if self._registry.contains(node_id):
            self._registry.release(node_id)

    @property
    def in_use(self) -> int:
        """How many ids are allocated (alive or in flight) right now."""
        return self._registry.in_use
