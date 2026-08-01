"""What the server reports about its resources: the records and their parsers.

One rule holds for every resource the server owns — a node, a buffer, a def:

* an **Info** is a frozen-in-time record of **one** instance, identified by
  itself (``id``, ``bufnum``, ``name``), carrying no server and no commands,
* the **instance** asks about itself (`Node.info`, `Buffer.info`) and gets
  exactly that record,
* the **server** asks about every instance of a type (`Server.query_buffers`,
  `Server.query_defs`, `Server.query_tree`) and answers with a structure of
  those same records — a list, or a `Tree`,
* and a resource that is **not there** is a state, not an error: the record
  comes back with ``exists = False`` rather than raising, so one dead id never
  aborts a query about the others.

The records live here rather than next to one resource because both ends need
them: `Server` builds them from the catalog replies, and `Node`/`Buffer` from
their own. A bus has no record at all — the server does not model one (its
index and width are the client allocator's invention), which is why there is
no ``BusInfo``.
"""

from dataclasses import dataclass, field


def _control_key(key):
    """A control identifier in a reply is a name string, or an int index when
    the server could not resolve a name."""
    return key if isinstance(key, str) else int(key)


@dataclass
class ControlInfo:
    """One entry of a def's control surface, as `Server.query_defs` reports it.

    ``rate`` is the control type the def declared: ``"kr"`` (a plain control),
    ``"tr"`` (a one-block trigger) or ``"ir"`` (a scalar frozen at init) — a
    different vocabulary from the calculation rates `UgenInfo` reports, which
    also include ``"ar"`` and ``"dr"``. Neither of those can be a control: an
    audio-rate value is mapped in from a bus, and a demand value is pulled by a
    driver rather than set. A
    FaustDef's params also carry ``min``/``max``/``step``; they are ``None`` for
    the other families, which declare no range. On a GraphDef this describes a
    surface **port**, and ``targets`` lists the ``(member, control, mul, add)``
    it drives inside — the scaling included, so a patch can draw the port's
    real connections."""

    name: str
    default: float
    rate: str = "kr"
    min: "float | None" = None
    max: "float | None" = None
    step: "float | None" = None
    targets: tuple = ()


@dataclass
class DefInfo:
    """A def the server holds: its name, its ``family`` (``"synth"``,
    ``"faust"`` or ``"graph"``) and its control surface.

    A def the server does not hold comes back with ``exists`` false, an empty
    family and no controls, rather than raising — one unknown name never fails
    a batch."""

    name: str
    family: str
    controls: "list[ControlInfo]"
    exists: bool = True


@dataclass
class BufferInfo:
    """A buffer the server holds: its slot and its shape.

    ``sample_rate`` is 0.0 while unknown — a buffer this client allocated but
    has not asked about carries the shape it dictated and not the server's
    rate. A slot with nothing in it comes back with ``exists`` false."""

    bufnum: int
    frames: int
    channels: int
    sample_rate: float
    exists: bool = True


@dataclass
class UgenInput:
    """One named input slot of a UGen, in **wire order**.

    The wire is positional — a def lists input values, it never names them — so
    this is what a palette labels an inlet with, and ``default`` is what to
    offer when the user leaves the slot alone."""

    name: str
    default: float


@dataclass
class UgenInfo:
    """A UGen kind as `Server.query_ugens` reports it, straight from the
    server's catalog.

    This is a **type**, not an instantiated resource: there is no handle for it
    and so no ``exists``. ``arity`` is the input count, or ``-1`` for a variadic
    kind — whose ``inputs`` then name only the fixed head (``EnvGen``'s five
    before the envelope array). ``rates`` are the rates the kind may be
    instantiated at and ``default_rate`` the one a def gets by omitting
    ``rate``. ``exec``, ``bus``, ``op_family`` and ``spectral`` expose the
    compiler's own classification; the ones that do not apply are empty
    strings."""

    name: str
    arity: int
    default_rate: str
    rates: "tuple[str, ...]"
    exec: str
    bus: str
    needs_path: bool
    op_family: str
    spectral: str
    inputs: "list[UgenInput]"

    @property
    def variadic(self) -> bool:
        return self.arity < 0


@dataclass
class NodeMap:
    """One live ``/n_map``/``/n_mapa`` binding: the control follows the bus."""

    control: int
    bus: int
    audio: bool = False


@dataclass
class NodeInfo:
    """A node the server holds — a synth or a group — at one moment.

    Unlike a buffer's, this record goes stale on its own: an envelope runs, a
    mapped control follows its bus, a ``done_action`` frees the node. It is a
    photograph, which is why no handle keeps one.

    A **group** carries ``head``/``tail`` (``-1`` when empty) and its children
    are the `Tree`'s business; a **synth** carries ``defname``, its
    ``controls`` by name, its ``maps`` and the ``reads``/``writes`` bus lists
    the server infers (``"-"`` when none). A node that is gone comes back with
    ``exists`` false and nothing else filled in."""

    id: int
    parent: int = -1
    prev: int = -1
    next: int = -1
    is_group: bool = False
    exists: bool = True
    head: int = -1
    tail: int = -1
    defname: str = ""
    controls: dict = field(default_factory=dict)
    maps: "list[NodeMap]" = field(default_factory=list)
    reads: str = "-"
    writes: str = "-"


@dataclass
class Tree:
    """The node tree from one group down: a `NodeInfo` plus its children.

    The structure is the only thing the tree adds — every entry is the same
    record `Node.info` returns, so reading a tree needs no follow-up query.
    The queried group is the root, and its own ``parent``/``prev``/``next`` are
    unknown (``-1``): the reply starts at it, so it has no siblings to report.

    Printing is two-faced, as usual in Python: ``repr`` identifies the tree in
    one line, ``str`` draws it indented, which is what `print` shows."""

    info: NodeInfo
    children: "list[Tree]" = field(default_factory=list)

    @property
    def id(self) -> int:
        """The node this subtree is rooted at."""
        return self.info.id

    def walk(self):
        """Yields every `NodeInfo` in the tree, depth-first, this one first."""
        yield self.info
        for child in self.children:
            yield from child.walk()

    def find(self, node) -> "Tree | None":
        """The subtree rooted at `node` (an id or a handle), or ``None``."""
        wanted = node.id if hasattr(node, "id") else int(node)
        for sub in self._subtrees():
            if sub.info.id == wanted:
                return sub
        return None

    def _subtrees(self):
        yield self
        for child in self.children:
            yield from child._subtrees()

    def __repr__(self) -> str:
        kind = "group" if self.info.is_group else self.info.defname
        return f"Tree({self.info.id} {kind}, {len(self.children)} children)"

    def __str__(self) -> str:
        return "\n".join(self._lines(0))

    def _lines(self, depth: int) -> "list[str]":
        pad = "  " * depth
        info = self.info
        if info.is_group:
            head = f"{pad}group {info.id}"
            if not self.children:
                head += " (empty)"
            out = [head]
            for child in self.children:
                out.extend(child._lines(depth + 1))
            return out
        mapped = {m.control: m for m in info.maps}
        parts = []
        for i, (name, value) in enumerate(info.controls.items()):
            m = mapped.get(i)
            if m is None:
                parts.append(f"{name}={value:g}")
            else:
                parts.append(f"{name}<-{'a' if m.audio else 'c'}{m.bus}")
        line = f"{pad}{info.id} {info.defname}"
        if parts:
            line += "  " + " ".join(parts)
        return [line]


def parse_def_info(args) -> DefInfo:
    """One ``/d_info`` reply: ``name, family, numControls`` then per control
    ``name, default, rate`` — plus ``min, max, step`` for a faust param, or
    ``numTargets`` and the target tuples for a graph port."""
    name, family, count = str(args[0]), str(args[1]), int(args[2])
    controls, i = [], 3
    for _ in range(count):
        c = ControlInfo(name=str(args[i]), default=float(args[i + 1]),
                        rate=str(args[i + 2]))
        i += 3
        if family == "faust":
            c.min, c.max, c.step = (float(args[i]), float(args[i + 1]),
                                    float(args[i + 2]))
            i += 3
        elif family == "graph":
            n_targets = int(args[i])
            i += 1
            targets = []
            for _ in range(n_targets):
                targets.append((int(args[i]), str(args[i + 1]),
                                float(args[i + 2]), float(args[i + 3])))
                i += 4
            c.targets = tuple(targets)
        controls.append(c)
    return DefInfo(name=name, family=family, controls=controls,
                   exists=bool(family))


def parse_ugen_info(args) -> UgenInfo:
    """One ``/u_info`` reply: ten fixed fields then ``(name, default)`` per
    named input."""
    count = int(args[9])
    inputs = [UgenInput(name=str(args[10 + 2 * k]), default=float(args[11 + 2 * k]))
              for k in range(count)]
    rates = str(args[3])
    return UgenInfo(
        name=str(args[0]),
        arity=int(args[1]),
        default_rate=str(args[2]),
        rates=tuple(r for r in rates.split(",") if r),
        exec=str(args[4]),
        bus=str(args[5]),
        needs_path=bool(int(args[6])),
        op_family=str(args[7]),
        spectral=str(args[8]),
        inputs=inputs,
    )


def parse_buffer_list(args) -> "list[BufferInfo]":
    """A ``/b_info`` reply, four args per buffer. ``frames`` -1 marks a slot
    with nothing in it (the argument-less listing form never reports one)."""
    out = []
    for i in range(0, len(args) - 3, 4):
        frames = int(args[i + 1])
        out.append(BufferInfo(bufnum=int(args[i]), frames=max(frames, 0),
                              channels=int(args[i + 2]),
                              sample_rate=float(args[i + 3]),
                              exists=frames >= 0))
    return out


def _parse_controls(args, i):
    """``numControls`` then (name|index, value) pairs -> (dict, next index)."""
    count = int(args[i])
    i += 1
    controls = {}
    for _ in range(count):
        controls[_control_key(args[i])] = float(args[i + 1])
        i += 2
    return controls, i


def _parse_maps(args, i):
    """``numMaps`` then (control, bus, audio) triples -> (list, next index)."""
    count = int(args[i])
    i += 1
    maps = []
    for _ in range(count):
        maps.append(NodeMap(control=int(args[i]), bus=int(args[i + 1]),
                            audio=bool(args[i + 2])))
        i += 3
    return maps, i


def parse_n_info(args) -> NodeInfo:
    """``/n_info`` -> one `NodeInfo` (see ``CmdTranslator::node_info``).
    ``is_group`` -1 is how the server says the node is not there."""
    id_, parent = int(args[0]), int(args[1])
    prev, next_, kind = int(args[2]), int(args[3]), int(args[4])
    if kind < 0:
        return NodeInfo(id=id_, exists=False)
    if kind == 1:
        return NodeInfo(id=id_, parent=parent, prev=prev, next=next_,
                        is_group=True, head=int(args[5]), tail=int(args[6]))
    info = NodeInfo(id=id_, parent=parent, prev=prev, next=next_,
                    defname=str(args[5]))
    info.controls, i = _parse_controls(args, 6)
    info.maps, i = _parse_maps(args, i)
    info.reads, info.writes = str(args[i]), str(args[i + 1])
    return info


def _parse_tree_nodes(args, i, count, detail, parent):
    """Recursively parse `count` entries of a ``/g_queryTree.reply`` starting
    at index `i`; returns (subtrees, next_index). A synth has child-count -1.

    The wire gives the nesting; the siblings and a group's head/tail follow
    from it, so each entry comes out as complete as `Node.info` would."""
    out = []
    for _ in range(count):
        node_id, child_count = int(args[i]), int(args[i + 1])
        i += 2
        if child_count == -1:
            info = NodeInfo(id=node_id, parent=parent, defname=str(args[i]))
            i += 1
            if detail >= 1:
                info.controls, i = _parse_controls(args, i)
            if detail >= 2:
                info.maps, i = _parse_maps(args, i)
                info.reads, info.writes = str(args[i]), str(args[i + 1])
                i += 2
            out.append(Tree(info=info))
        else:
            children, i = _parse_tree_nodes(args, i, child_count, detail, node_id)
            info = NodeInfo(id=node_id, parent=parent, is_group=True,
                            head=children[0].info.id if children else -1,
                            tail=children[-1].info.id if children else -1)
            out.append(Tree(info=info, children=children))
    for pos, sub in enumerate(out):
        sub.info.prev = out[pos - 1].info.id if pos else -1
        sub.info.next = out[pos + 1].info.id if pos + 1 < len(out) else -1
    return out, i


def parse_query_tree(args) -> Tree:
    """``/g_queryTree.reply`` -> a `Tree` of `NodeInfo`. A standalone function
    so it can be unit-tested without a server."""
    detail = int(args[0])
    root_id = int(args[1])
    count = int(args[2])
    children, _ = _parse_tree_nodes(args, 3, count, detail, root_id)
    root = NodeInfo(id=root_id, is_group=True,
                    head=children[0].info.id if children else -1,
                    tail=children[-1].info.id if children else -1)
    return Tree(info=root, children=children)
