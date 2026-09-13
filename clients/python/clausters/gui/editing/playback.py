"""What makes a piece **sound**, kept in step with what the editor draws.

The multitrack editor's other half. `clausters.gui.editing.MultitrackView` says
what a piece looks like and `clausters.gui.editing.MultitrackDomain` says what a
gesture makes of it; this says what it is heard as.

**It decides nothing.** What a track and a clip *are* on the server is the
shared core's (`mt.piece`, `mt.track`, `mt.clip`, `mt.reader` and the channel
strip under all of them); which of them a given piece needs is the document
crate's; and the **difference** between that and what is already sounding is
`clausters._native.Instance`, in the shared crate; and the messages that carry
it out are the crate's applier. So what is left here is what a language
genuinely owns: a socket, and waiting on it.

# Why a diff and not a rebuild

A piece plays itself from the transport: every reader reads the engine's own
position, so a locate is no message at all and moving a box is one `set`. That
only holds if the nodes **stay**: rebuilding the tree on every edit would
restart everything that is sounding, and a hand dragging a box would hear its
own gesture as a stutter. So a track, a clip and a reader are each added once
and set thereafter, and only what a set cannot express is torn down and made
again -- which is the reconciler's rule and is written down there.

# Steps: the crate says what waits

An operation never carries a node id, a bus index or a buffer number. The
crate's applier (`clausters._native.Applier`) keeps the table from each
operation's handle to what it became, allocates those from the server's id
spaces, and answers **steps**: a message to send, a ``/done`` the rest waits
for, a barrier. A buffer's fill waiting for its allocation is one of those
steps, stated once, and every endpoint -- this client, the page and the GUI
host playing a piece on its own -- carries out the same list.
"""

from array import array

from ... import _native
from ...errors import CommandError
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
        #: The operations as steps, and the table from handle to what each
        #: became -- the crate's, as the instance is.
        self._applier = _native.Applier(chunk=server._bulk_chunk())
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        server._ensure_recycler()
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
            bus = self._applier.bus(row["bus"])
            if bus is not None:
                out[row["track"]] = (bus[0], int(row["channels"]))
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

        **The order is the answer, and so are the waits.** A def before the
        graph that names it, a buffer's fill after its allocation answered, a
        node freed before the one that replaces it is made -- all of that is the
        crate's, stated as steps, and this only sends them and waits where a
        step says to.
        """
        self._run(self._applier.apply(ops, self.server.ids))

    def _run(self, steps: list) -> None:
        """Send each step, waiting where one says to."""
        index = 0
        while index < len(steps):
            step = steps[index]
            if "send" in step:
                addr = step["send"]["addr"]
                args = [_arg(a) for a in step["send"]["args"]]
                after = steps[index + 1] if index + 1 < len(steps) else {}
                if "await" in after:
                    # Sent and waited for as one request, so the `/done` cannot
                    # arrive before anyone is listening for it.
                    raddr, rargs = self.server.request(
                        addr, *args, expect=("/done", "/fail"))
                    if raddr == "/fail":
                        raise CommandError(f"{addr} failed: {rargs}")
                    index += 2
                    continue
                self.server.send_msg(addr, *args)
            elif "sync" in step:
                self.server.sync()
            index += 1

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
            self.server.send_msg("/bus_fill", bus, 2 * channels, 0.0)

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


def _arg(arg: dict):
    """One step argument as the value `send_msg` encodes to its tag."""
    if "i" in arg:
        return int(arg["i"])
    if "f" in arg:
        return float(arg["f"])
    if "b" in arg:
        return array("f", arg["b"]).tobytes()
    return str(arg["s"])
