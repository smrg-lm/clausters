"""Nodes (synths and groups) and client-side id allocation.

The server's node tree (`node`): the root group is id 0; clients allocate
positive ids. Add actions match the server: head/tail of a group, before/after
a node, or replace. `Synth` and `Group` hold an id and the server it lives on,
and own the commands addressed to it: **building one creates it** —
``Synth("beep")`` allocates an id and sends ``/synth_new``, ``Group()`` sends
``/group_new``, `Group.graph` instantiates a GraphDef — and `Node.set`,
`Node.map`, `Node.run` and `Node.free` drive it. To name a node that already
exists, from an id a responder or a query reported, use `Synth.from_id` /
`Group.from_id`, which send nothing. The id pool itself belongs to the
`Server`.
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


def _target_id(target) -> int:
    """A target is a `Node` or its raw id, so ``target=group`` and
    ``target=group.id`` are the same thing (the web client's ``nodeId``)."""
    return int(target.id if isinstance(target, Node) else target)


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
    """One entry in the server's node tree, and the commands addressed to it.

    A node is an integer id on a particular server, and that is all a client
    holds: the sound, the state and the position in the tree live over there.
    What this class adds is that the id knows where to go — `set`, `map`,
    `run`, `free` and `info` each send one command to the right server without
    being told which.

    The tree has two kinds of node and each has its own class: a `Synth` (a
    running def, making sound) and a `Group` (a container, and the thing you
    aim other nodes at). Build one of those; `Node` is what they share, and
    what you get back when something reports a bare id.

    Order is meaning here. The server processes the tree front to back, so a
    node's neighbours decide what it can hear: a reverb placed after the voices
    reads what they wrote, the same reverb placed before them reads last
    block's silence. That is what the `AddAction` on every constructor controls.

    The three structural pieces together — a def to play, a group to hold the
    voices, and the synths themselves:

    ```python
    from clausters import Group, Server, Synth, SynthDef
    from clausters.defs import control, out, sine

    s = Server.boot()
    d = SynthDef("beep", out(0, sine(control("freq", 440.0)) * 0.2))
    d.send(s)

    g = Group(server=s)                 # one place to hold the voices
    n = Synth("beep", {"freq": 220.0}, target=g, server=s)
    Synth("beep", {"freq": 330.0}, target=g, server=s)

    n.set({"freq": 440.0})              # one voice
    g.set({"freq": 110.0})              # every voice in the group
    g.free()                            # and its members with it
    ```

    Attributes:
        id: the node's id on the server. Allocated by the client (the
            `Server`'s `NodeIdAllocator`), so it is known the moment the
            command is sent, with no reply to wait for.
        server: the `Server` this node lives on, or `None` for a handle built
            from an id someone else reported — that one falls back to the
            ambient server, like `clausters.play`.
    """

    def __init__(self, node_id: int, server=None):
        """A handle on the node with this id. `Synth` and `Group` build one by
        *creating* the node; this is the bare form, for an id that arrived from
        somewhere else and whose kind is not known."""
        self.id = node_id
        #: the `Server` this node lives on (set when it was created), so its
        #: commands know where to go without being told.
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
    """One running instance of a def — a voice, sounding now.

    A def is a recipe and a synth is one performance of it: several synths of
    the same def run at once, each with its own controls and its own envelope.
    **Both def families instantiate the same way**, because the server names a
    def rather than a kind — a `SynthDef` (a UGen graph) and a `FaustDef`
    (JIT-compiled DSP) are peers, and a synth of either is the same node in the
    same tree, driven by the same `Node.set`. The def has to be installed
    first, so send it before naming it here.

    A synth's controls are its surface. Set them by name with `Node.set`, or
    hand one over to a `Bus` with `Node.map` so it follows whatever that bus
    carries — that is how one modulator drives many voices with the client out
    of the loop.

    How it ends is usually not your call. A def with an envelope frees its own
    synth when the envelope finishes (`clausters.defs.DoneAction.FREE_SELF`),
    which is what makes a note a note; `Node.free` is for the ones with no end
    of their own — a drone, a live effect, a take being cut short.

    A note that ends itself, and a drone that does not:

    ```python
    from clausters import Server, Synth, SynthDef
    from clausters.defs import DoneAction, Env, control, env_gen, out, sine

    s = Server.boot()
    SynthDef("note",
             out(0, sine(control("freq", 440.0)) * 0.2
                    * env_gen(Env.perc(),
                              done_action=DoneAction.FREE_SELF))).send(s)
    d = SynthDef("drone", out(0, sine(control("freq", 60.0)) * 0.1))
    d.send(s)

    Synth("note", {"freq": 660.0}, server=s)   # sounds, then frees itself
    n = Synth("drone", server=s)               # stays until told
    n.set({"freq": 55.0})
    n.free()
    ```

    Attributes:
        defname: the name of the def this synth is running.
        id: see `Node`.
        server: see `Node`.
    """

    def __init__(self, defname, controls=None, *, target=ROOT_NODE_ID,
                 action=AddAction.TAIL, server=None):
        """Starts a synth from a def already loaded on the server, by name
        (``/synth_new``). Building one *is* starting it: the id comes from the
        server's `NodeIdAllocator` and the command goes out here, so the synth
        is sounding by the time this returns.

        ```python
        g = Group(server=s)
        n = Synth("beep", {"freq": 440.0}, target=g, server=s)
        ```

        An unknown def name raises nothing here: the command is fire-and-forget,
        the server answers ``/fail`` on its own channel, and the handle you get
        back carries an id no node was ever created for — `Node.info` reports
        ``exists`` false for it.

        Args:
            defname: the name of a def already installed on the server, of
                either family. Sending a def is asynchronous, but
                ``d.send(s)`` waits for the server's ``/done`` by default, so
                the def is there by the time you name it.
            controls: the controls to override the def's defaults with — a
                dict of names to values, or a list of ``(name, value)`` pairs.
                Pairs are how you reach the reserved ``in`` and ``out``
                controls, which are Python keywords.
            target: the node this one is placed relative to — a `Group`, a
                `Node`, or a bare id. Defaults to the root group.
            action: where relative to ``target``, an `AddAction`. Defaults to
                the tail, i.e. after everything already in the target group,
                so a new voice is heard by whatever comes later.
            server: the `Server` to start it on; ``None`` takes the ambient
                one (the running session, else the default session).
        """
        srv = _resolve(server)
        node_id = srv._node_id()
        srv.send_msg("/synth_new", defname, node_id, int(action), _target_id(target),
                     *_flatten_controls(controls))
        super().__init__(node_id, srv)
        self.defname = defname

    @classmethod
    def from_id(cls, node_id: int, defname: str, server=None) -> "Synth":
        """A handle on a synth that is **already** on the server, named by the
        id something else reported (a responder, a node-tree query, the GUI).
        Sends nothing."""
        synth = cls.__new__(cls)
        Node.__init__(synth, node_id, server)
        synth.defname = defname
        return synth


class Group(Node):
    """A node that holds other nodes: an order, a handle and a boundary.

    A group makes no sound of its own. What it gives you is the three things a
    piece needs once it has more than one voice:

    - **A handle for many.** A group *is* a `Node`, so every command on `Node`
      applies to everything inside it at once — `Node.set` reaches all the
      members that have that control, `Node.run` pauses them together, and
      `Node.free` frees the group and its contents in one command. That is one
      message instead of one per voice.
    - **A place in the order.** The server processes the tree front to back,
      so a group is where you say *when* a stage runs: sources in one group,
      the effect that reads them in another after it. Aim a node at a group
      with ``target=``, and place it inside with an `AddAction`.
    - **A lifetime.** Freeing the group ends everything it holds, which is how
      a section, a take or a voice with several nodes is cut as a unit.

    The root group (id ``0``, `ROOT_NODE_ID`) is always there and is what a
    node with no ``target`` is added to. Two constructors build a group that is
    *not* empty: `graph` instantiates a GraphDef — a named configuration of
    several defs already wired to each other — and `voice` spawns one more
    voice inside a running instance of one.

    Sources and an effect, ordered by their groups rather than by luck:

    ```python
    from clausters import AddAction, Bus, Group, Server, Synth, SynthDef
    from clausters.defs import control, in_, out, sine

    s = Server.boot()
    SynthDef("voice",
             out(control("bus", 0.0), sine(control("freq", 440.0)) * 0.2)).send(s)
    SynthDef("wash",
             out(0, in_(control("bus", 0.0)) * control("amp", 0.5))).send(s)

    mix = Bus.audio(server=s)
    g = Group(server=s)                                   # the sources, first
    fx = Group(target=g, action=AddAction.AFTER, server=s)  # what reads them

    for freq in (220.0, 277.0, 330.0):
        Synth("voice", {"freq": freq, "bus": mix.index}, target=g, server=s)
    Synth("wash", {"bus": mix.index}, target=fx, server=s)

    g.set({"freq": 110.0})   # every voice at once; the wash has no freq
    g.free()                 # the three voices, one command
    fx.free()
    ```
    """

    def __init__(self, *, target=ROOT_NODE_ID, action=AddAction.TAIL,
                 server=None):
        """An empty group in the node tree (``/group_new``). Building one *is*
        creating it, as with `Synth`; to name a group that already exists, use
        `from_id`.

        ```python
        g = Group(server=s)                                    # runs first
        fx = Group(target=g, action=AddAction.AFTER, server=s)
        ```

        Args:
            target: the node this group is placed relative to — a `Node` or a
                bare id. Defaults to the root group.
            action: where relative to ``target``, an `AddAction`. Defaults to
                the tail, so a new group runs after everything already there.
            server: the `Server` to create it on; ``None`` takes the ambient
                one.
        """
        srv = _resolve(server)
        node_id = srv._node_id()
        srv.send_msg("/group_new", node_id, int(action), _target_id(target))
        super().__init__(node_id, srv)

    @classmethod
    def from_id(cls, node_id: int, server=None) -> "Group":
        """A handle on a group that is **already** on the server, named by the
        id something else reported. Sends nothing."""
        group = cls.__new__(cls)
        Node.__init__(group, node_id, server)
        return group

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
        srv.send_msg("/graph_new", defname, node_id, int(action), _target_id(target),
                     *_flatten_controls(ports))
        return cls.from_id(node_id, srv)

    def voice(self, ports=None) -> "Group":
        """Spawns a per-voice sub-graph (``/graph_newVoice``) inside this running
        GraphDef instance (a group from `graph`), wired to its shared private
        buses. ``ports`` overrides the voice-port defaults. The returned group
        is the voice: drive it through its surface with `set` and free it with
        `free`."""
        srv = self._server()
        node_id = srv._node_id()
        srv.send_msg("/graph_newVoice", self.id, node_id, *_flatten_controls(ports))
        return Group.from_id(node_id, srv)


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
