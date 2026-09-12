"""What makes a piece **sound**, kept in step with what the editor draws.

The multitrack editor's other half. `clausters.gui.editing.MultitrackView` says
what a piece looks like and `clausters.gui.editing.MultitrackDomain` says what a
gesture makes of it; this says what it is heard as.

**It decides nothing.** What a track and a clip *are* on the server is the
shared core's (`mt.piece`, `mt.track`, `mt.clip`, `mt.reader` and the channel
strip under all of them); which of them a given piece needs is the document
crate's; and the **difference** between that and what is already sounding is
`clausters._native.Instance`, in the shared crate. So what is left here is three
things a language genuinely owns: a socket, an allocator, and one table from the
crate's handles to the objects this client made.

# Why a diff and not a rebuild

A piece plays itself from the transport: every reader reads the engine's own
position, so a locate is no message at all and moving a box is one `set`. That
only holds if the nodes **stay**: rebuilding the tree on every edit would
restart everything that is sounding, and a hand dragging a box would hear its
own gesture as a stutter. So a track, a clip and a reader are each added once
and set thereafter, and only what a set cannot express is torn down and made
again -- which is the reconciler's rule and is written down there.

# Handles: the crate names what it cannot make

An operation never carries a node id, a bus index or a buffer number, because
the crate allocates none of them. It carries a **handle** -- a string it mints
from the document's own ids -- and `Playback` keeps the one table from handle to
whatever it made. A port that has to name a resource names it the same way, and
`_value` is where that is resolved.
"""

import json

from ... import _native
from ...defs.buffer import Buffer
from ...defs.bus import Bus
from ...defs.node import AddAction, Group, Synth
from ..transport import Transport

__all__ = ["Playback"]


class Playback:
    """The instance of one piece, and the transport that moves it.

    Built by `clausters.gui.editing.MultitrackEditor` when it is given a server,
    and reachable as its ``playback``. Nothing here is subscribed to anything:
    the editor tells it the piece changed, whoever changed it -- this window's
    gesture, a second window over the same piece, or a step of the history --
    and `sync` makes what sounds be what is drawn.

    Args:
        editor: the `clausters.gui.editing.MultitrackEditor` whose piece this
            sounds. Its `clausters.gui.editing.Sources` is where a source's
            buffer is found, since which buffer a source was read into is the
            one fact about a piece that is not in the piece.
        server: the server the piece lives on.
        gain: the master's own level.
    """

    def __init__(self, editor, *, server, gain: float = 0.5):
        self.editor = editor
        self.server = server
        self.gain = float(gain)
        #: What is sounding, as the crate holds it. It answers the difference
        #: between that and the piece; nothing here decides what a difference
        #: is.
        self._instance = _native.Instance()
        #: handle -> the node this client made for it.
        self._nodes: dict = {}
        #: handle -> the control bus.
        self._buses: dict = {}
        #: handle -> the buffer.
        self._buffers: dict = {}
        bridge = editor.bridge
        #: The piece's transport. ``head_clock="piece"`` says it once: the verbs
        #: become the server's and the host draws the line from the engine's own
        #: position instead of an anchor kept in step here.
        self.transport = Transport(
            editor._host,
            lambda: [] if editor.piece_widget is None else [editor.piece_widget],
            head_clock="piece", governed=True,
            tempo_map=bridge.tempo, sample_rate=bridge.rate,
            extent=lambda: editor.structure.end)
        self.transport.server = server
        self.sync()
        self.transport.locate(editor.cursor or 0.0)

    def attach(self, host) -> None:
        """The piece went on screen: draw the line from the engine's own
        position.

        A playback is built before the window is -- a piece can be played by a
        script that never draws it -- so this is where the two meet, and it is
        one statement: `clausters.gui.host.GuiHost.head_clock` and the
        transport's own are the same decision, and letting them disagree draws a
        line nobody put there.
        """
        if host is None:
            return
        self.transport.host = host
        host.head_clock("piece")
        self.transport.locate(self.transport.position)

    # ---- the instance ----

    @property
    def meters(self) -> dict:
        """track id -> ``(bus, channels)``: the control buses that track's
        meters write, a run of ``2 * channels`` -- the level first and the mark
        that waits after it.

        What the host reads every frame, and the reason a level that moves every
        block costs no message. The crate says which run belongs to which track;
        the buses are this client's, because it is this client that allocates
        them.
        """
        out = {}
        for row in self._instance.meters():
            bus = self._buses.get(row["bus"])
            if bus is not None:
                out[row["track"]] = (bus, int(row["channels"]))
        return out

    def sync(self) -> None:
        """Make what sounds be what is drawn.

        The whole of it, and it runs on every edit whoever made it. Everything
        that is already right is left alone, which is what lets a hand drag a
        box without hearing the rest of the piece restart.
        """
        bridge = self.editor.bridge
        self.apply(self._instance.reconcile(
            self.editor.structure.write(), bridge.rate, bridge.bpm,
            bridge.sources.table(), self.gain))

    def apply(self, ops: list) -> None:
        """Do what the reconciler says, in order.

        **The order is the answer.** A def before the graph that names it, a
        buffer before the reader pointed at it, a node freed before the one that
        replaces it is made -- all of that is decided in the crate and this only
        carries it out, which is why a second client cannot carry it out
        differently.
        """
        for op in ops:
            getattr(self, "_op_" + str(op["op"]).lower())(op)

    def _value(self, port):
        """One port's value: a number as itself, and a **handle** resolved out
        of the table this filled when it made the thing."""
        if isinstance(port, dict):
            if "bus" in port:
                return float(self._buses[port["bus"]].index
                             + int(port.get("offset", 0)))
            return float(self._buffers[port["buffer"]].bufnum)
        return float(port)

    def _ports(self, op) -> dict:
        return {name: self._value(value)
                for name, value in (op.get("ports") or {}).items()}

    # ---- one method per operation ----

    def _op_def(self, op) -> None:
        self.server.send_msg("/def_send", op["family"], json.dumps(op["spec"]))

    def _op_barrier(self, op) -> None:
        # **The batch is closed before anything else is sent.** A def send is
        # asynchronous and answers `/done`, so a `/done` left in flight is one
        # the next command that waits for one takes as its own -- and a buffer
        # alloc that returns before it ran is written into before it exists.
        self.server.sync()

    def _op_graph(self, op) -> None:
        self._nodes[op["handle"]] = Group.graph(op["graph"], self._ports(op),
                                                server=self.server)

    def _op_transport(self, op) -> None:
        self.server.transport_group(self._nodes[op["handle"]])

    def _op_group(self, op) -> None:
        self._nodes[op["handle"]] = Group(target=self._nodes[op["before"]],
                                          action=AddAction.BEFORE,
                                          server=self.server)

    def _op_slot(self, op) -> None:
        target = self._nodes[op["target"]]
        self._nodes[op["handle"]] = target.add_slot(op["slot"], self._ports(op))

    def _op_synth(self, op) -> None:
        self._nodes[op["handle"]] = Synth(op["def"], self._ports(op),
                                          target=self._nodes[op["target"]],
                                          server=self.server)

    def _op_bus(self, op) -> None:
        self._buses[op["handle"]] = Bus.control(int(op["channels"]),
                                                server=self.server)

    def _op_buffer(self, op) -> None:
        self._buffers[op["handle"]] = Buffer.from_samples(op["samples"],
                                                          server=self.server)

    def _op_set(self, op) -> None:
        node = self._nodes.get(op["handle"])
        if node is not None:
            node.set(self._ports(op))

    def _op_map(self, op) -> None:
        node, bus = self._nodes.get(op["handle"]), self._buses.get(op["bus"])
        if node is not None and bus is not None:
            self.server.send_msg("/graph_map", node.id, op["port"],
                                 int(bus.index))

    def _op_unmap(self, op) -> None:
        # **A handle with nothing behind it is a node that is already gone**,
        # and a message naming one would reach whatever holds that id next. The
        # reconciler does not emit these -- freeing a node takes its map with
        # it, and there is a crate test saying so -- and this is the second
        # half of that: the two clients answer an impossible handle the same
        # way, instead of one raising and the other addressing node 0.
        node = self._nodes.get(op["handle"])
        if node is not None:
            self.server.send_msg("/graph_map", node.id, op["port"], -1)

    def _op_free(self, op) -> None:
        node = self._nodes.pop(op["handle"], None)
        if node is not None:
            node.free()
        # Freeing a group frees what is inside it, so these only leave the
        # table: a second free would name a node that is already gone.
        for handle in op.get("forget") or ():
            self._nodes.pop(handle, None)

    def _op_freebus(self, op) -> None:
        bus = self._buses.pop(op["handle"], None)
        if bus is not None:
            bus.free()

    def _op_freebuffer(self, op) -> None:
        buffer = self._buffers.pop(op["handle"], None)
        if buffer is not None:
            buffer.free()

    # ---- the transport ----

    @property
    def playing(self) -> bool:
        """Whether the piece is rolling, as the engine last answered."""
        return self.transport.refresh().playing

    @property
    def position(self) -> float:
        """Where the piece is, in beats -- one round trip, because the position
        is the engine's."""
        return self.transport.refresh().position

    def play(self):
        """Play, or continue a paused pass: the engine keeps where it stopped,
        so resuming is the same verb as starting and nothing is re-cued."""
        self.transport.play(self.server)
        return self

    def pause(self):
        """Freeze the piece where it stands, with every node's state intact."""
        self.transport.pause()
        self._silence_meters()
        return self

    def _silence_meters(self) -> None:
        """**A frozen meter must not go on claiming a level.**

        The transport freezes the piece's whole subtree, so a paused meter is
        starved of time and its bus keeps the last value it wrote -- forever.
        A picture of what the piece *was* doing then reads as what it is doing,
        which is the one thing a meter may never say. Nothing on the server can
        move it (a frozen node gets no time and a fall is time), so whoever
        stopped it says so: the buses go to zero and the strip falls empty,
        which is what is true of a piece that is not sounding.

        The mark goes with the level. A held peak is "the loudest thing lately"
        and lately ended when the transport did.
        """
        for bus, channels in self.meters.values():
            self.server.send_msg("/bus_fill", bus.index, 2 * channels, 0.0)

    def stop(self):
        """Halt and go back to **the mark**, not to the top.

        The playhead is never placed: stopped, it stands where the position
        cursor is, so the next play starts from the mark the reader put down
        rather than from wherever the last pass happened to end.
        """
        self.transport.pause()
        self._silence_meters()
        return self.locate(self.editor.cursor or 0.0)

    def locate(self, beat: float):
        """Seek to ``beat``. The readers seek in the engine, so nothing is
        re-cued and what is sounding carries on from there."""
        self.transport.locate(beat)
        return self

    def cue(self, beat: float):
        """The **position cursor** moved: cue a stopped transport there, and
        leave a rolling one alone.

        Two cursors, and only one of them is placed. A click that landed on
        nothing puts the position cursor down, which is where the next play
        starts; moving the mark mid-pass must not move the music.
        """
        if not self.playing:
            self.locate(beat)
        return self

    def close(self):
        """Free the piece's instance. The piece itself is untouched: what a
        playback holds is nodes, and nodes are not the composition."""
        self.apply(self._instance.teardown())
