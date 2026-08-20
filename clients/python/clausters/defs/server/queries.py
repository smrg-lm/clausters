"""What a running server holds, asked over the wire.

Every method here is a blocking round trip: it sends and waits for the reply,
so none of it may be called from a routine (that would freeze the clock
thread). They report the **server's** state, not this handle's — the def store
survives restarts, and another client's nodes are in the same tree — which is
why asking beats assuming.
"""

from ...errors import CommandError
from ..info import (
    BufferInfo,
    DefInfo,
    Tree,
    UgenInfo,
    parse_buffer_list,
    parse_def_info,
    parse_query_tree,
    parse_ugen_info,
)
from ..node import Group, ROOT_NODE_ID
from .options import (
    DEFAULT_MAX_BUFFERS,
    DEFAULT_MAX_GRAPH_CHILDREN,
    DEFAULT_MAX_NODES,
    DEFAULT_MAX_UGEN_INPUTS,
    ServerInfo,
)


class ServerQueries:
    """The introspection half of `Server`; never instantiated on its own."""

    # ---- server introspection: what a running server actually holds ----

    def query_defs(self, *names, timeout: "float | None" = None) -> "list[DefInfo]":
        """The defs the server holds, each with its control surface
        (``/def_query``). With `names`, details exactly those — an unknown one
        comes back with an empty ``family`` (see `DefInfo.exists`) rather than
        raising; with no argument, every loaded def of every family.

        The def store persists across restarts, so a server may well hold defs
        this client never sent: this is how you find out. Blocking, RT only —
        never call it from a routine."""
        rows = self._request_batch("/def_query", *[str(n) for n in names],
                                   reply="/def_query.reply", timeout=timeout)
        return [parse_def_info(r) for r in rows]

    def query_buffers(self, timeout: "float | None" = None) -> "list[BufferInfo]":
        """Every **allocated** buffer with its shape (an argument-less
        ``/buffer_query``). Like `query_defs`, this reports what the server holds rather
        than what this client allocated. Blocking, RT only."""
        _, args = self.request("/buffer_query", timeout=timeout, expect=("/buffer_query.reply",))
        return parse_buffer_list(args)

    def query_ugens(self, *kinds, timeout: "float | None" = None) -> "list[UgenInfo]":
        """The server's UGen catalog (``/ugen_query``): every kind with its named
        inputs, defaults and rate rules, or just `kinds` if given.

        This is the catalog **this** server was built with, which is why it is
        worth asking instead of assuming: a build without the ``synth`` feature
        has no UGens at all and returns an empty list (its defs would all be
        FaustDefs, whose box vocabulary is Faust's own and lives client-side).
        Blocking, RT only."""
        rows = self._request_batch("/ugen_query", *[str(k) for k in kinds],
                                   reply="/ugen_query.reply", timeout=timeout)
        return [parse_ugen_info(r) for r in rows]

    def query_info(self, timeout: "float | None" = None) -> ServerInfo:
        """Asks the running server for its static configuration (RT only): bus
        counts, output/input channels, block size, sample rate and the
        boot-time pool sizes. Use it to size or check allocators against a
        server you did not launch; compare the result with `options`. The
        appended capacity fields degrade to the defaults against a server too
        old to report them."""
        _, args = self.request(
            "/server_query", timeout=timeout, expect=("/server_query.reply",)
        )

        def at(i, cast, default):
            return cast(args[i]) if i < len(args) else default

        return ServerInfo(
            audio_buses=int(args[0]),
            control_buses=int(args[1]),
            channels=int(args[2]),
            block_size=int(args[3]),
            nominal_sample_rate=float(args[4]),
            actual_sample_rate=float(args[5]),
            input_channels=at(6, int, 0),
            max_nodes=at(7, int, DEFAULT_MAX_NODES),
            max_buffers=at(8, int, DEFAULT_MAX_BUFFERS),
            max_graph_children=at(9, int, DEFAULT_MAX_GRAPH_CHILDREN),
            max_ugen_inputs=at(10, int, DEFAULT_MAX_UGEN_INPUTS),
            taps=at(11, int, 0),
            tap_frames=at(12, int, 0),
            max_frame=at(13, int, 65536),
            max_stream_buses=at(14, int, 128),
        )

    # ---- node tree introspection (RT only) ----

    def query_tree(self, group=ROOT_NODE_ID, timeout: "float | None" = None) -> Tree:
        """The node tree from `group` down (``/group_queryTree``) as a `Tree`:
        every entry is the same `NodeInfo` that `clausters.defs.Node.info`
        returns, so reading a subtree needs no follow-up query. This is the
        **structured** way to read the tree — never scrape the server's logs.

        ``print(tree)`` draws it indented. Blocking, RT only."""
        gid = group.id if hasattr(group, "id") else group
        addr, args = self.request("/group_queryTree", int(gid), 2,
                                  timeout=timeout, expect=("/group_queryTree.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/group_queryTree failed: {args}")
        return parse_query_tree(args)

    def group_at(self, path: str, timeout: "float | None" = None):
        """The group a path names (``/group_query``), as a
        `clausters.defs.Group` handle — or ``None`` when nothing answers to it.

        A path is the group names from the root down, ``/mixer/drums``; a group
        with no name contributes its id instead (``/1000/drums``), so every
        group is reachable whether it was labelled or not. Resolve once and keep
        the handle: the id is the identity, the path is how you found it, and a
        group that is renamed or freed leaves the handle pointing at the id it
        resolved to. Blocking, RT only."""
        addr, args = self.request("/group_query", str(path),
                                  timeout=timeout, expect=("/group_query.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/group_query failed: {args}")
        node_id = int(args[1])
        return Group.from_id(node_id, self) if node_id >= 0 else None

    def dump_graph(self, group=ROOT_NODE_ID, timeout: "float | None" = None) -> str:
        """The inferred bus graph of `group` as a human-readable string
        (``/group_dumpGraph``): what each child reads/writes and the current order.
        A debugging aid; for machine use prefer `query_tree`."""
        gid = group.id if hasattr(group, "id") else group
        addr, args = self.request("/group_dumpGraph", int(gid),
                                  timeout=timeout, expect=("/group_dumpGraph.reply", "/fail"))
        if addr == "/fail":
            raise CommandError(f"/group_dumpGraph failed: {args}")
        return str(args[1])
