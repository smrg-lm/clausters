"""What makes a piece **sound**, kept in step with what the editor draws.

The multitrack editor's other half. `MultitrackView` says what a piece looks
like and `MultitrackDomain` says what a gesture makes of it; this says what it
is heard as, and it is the same object either way — a piece is a statement, so
putting the readers where it says is one verb whether it is the first time or
the hundredth.

**A box is a reader.** Each one is a single resident node reading its buffer at
the transport's position — no queue, nothing scheduled, nothing re-cued. Moving
a box while it plays is one ``/node_set`` on a node that is already running, so
it is heard where it was dropped with nothing that is sounding cut, and the
seeking, the pausing and the looping are the server transport's.

**The time is the server's**, which settles everything under it: play, pause and
stop are `clausters.defs.server.Server.transport_play`,
`clausters.defs.server.Server.transport_stop` and
`clausters.defs.server.Server.transport_locate_sample`; the readers are one
**governed** group, so a pause freezes them with every node's state intact and
playing again continues rather than starting over; and the host draws the line
from the engine's own position, with nothing sent per frame.

**What is not here yet is the chain.** A track's level, its mute and its solo
reach the readers; the curves drawn on a track and inside a box do not, because
a curve's ``gain`` and the knob's ``gain`` have to name one parameter of one node
before either can drive it, and that is the synthesis node system's design rather
than this module's. Until it exists a curve is drawn, edited and kept by the
piece, and heard by nothing.
"""

from ...defs import SynthDef, out
from ...defs.node import Group, Synth
from ...defs.ugens import buf_rd, control, transport_pos
from ..transport import Transport

__all__ = ["READER", "Playback", "box_args", "reader", "track_level"]

#: The controls a box cannot change without being started again: what it reads
#: and whether it wraps are read at the rate the reader is built with.
FIXED = ("buf", "loop")

#: The def every box is read by. One name for the whole client, because it is
#: one def: a box is a window onto a source and they differ by their controls.
READER = "clausters.box"


def reader(name: str = READER) -> SynthDef:
    """The def a box sounds through: a buffer read at the transport's position.

    No position of its own — seeking, looping and pausing are the transport's,
    and moving a box is one ``/node_set`` of ``at``. `transport_pos` is the
    transport's position minus where the box starts, so the reader is at frame 0
    when the transport reaches it, and the gate is the box's length (`buf_rd`
    clamps past the end instead of going quiet).

    **A box is a window onto a source, and it reads from where the window
    opens.** ``start`` is that frame, so trimming the left edge or splitting a
    box makes the piece play what the picture shows: without it every box would
    read from frame zero and both halves of a split would play the beginning.
    """
    buf = control("buf", 0.0, "ir")
    at = control("at", 0.0)            # where it starts, in frames of the transport
    span = control("span", 0.0)        # how long it lasts, in frames
    start = control("start", 0.0)      # the frame of the source its own zero reads
    wrap = control("loop", 0.0, "ir")  # whether it wraps past the source's end
    amp = control("amp", 0.5, lag=0.02)
    pos = transport_pos(at)
    live = (pos >= 0.0) * (pos < span)
    sig = buf_rd(buf, 0.0, pos + start, wrap) * live * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


class Playback:
    """The readers of one piece, and the transport that moves them.

    Built by `clausters.gui.editing.MultitrackEditor` when it is given a server,
    and reachable as its ``playback``. Nothing here is subscribed to anything:
    the editor tells it the piece changed, whoever changed it — this window's
    gesture, a second window over the same piece, or a step of the history — and
    `sync` puts the readers where the piece now says they are.

    Args:
        editor: the `clausters.gui.editing.MultitrackEditor` whose piece this
            sounds. Its `clausters.gui.editing.Sources` is where a source's
            buffer is found, since which buffer a source was read into is the
            one fact about a piece that is not in the piece.
        server: the server the readers live on.
        group: the group they live in, and the one the server's transport
            governs. A group of its own by default and never the root, which
            would freeze every sound the session has.
        amp: the level a box at full track level is read at.
    """

    def __init__(self, editor, *, server, group=None, amp: float = 0.5):
        self.editor = editor
        self.server = server
        self.amp = float(amp)
        #: region id -> ``(node, what it was last set with)``. The controls are
        #: kept beside the node because two of them are **initial-rate** — what a
        #: box reads and whether it wraps are fixed when the reader is built — so
        #: telling a change of those from a move means knowing what was sent.
        self.nodes: dict = {}
        reader().send(server)
        #: **The group the transport governs**, and the call this rests on: from
        #: here the engine freezes that subtree on a stop and thaws it on a play,
        #: with every node's state intact.
        self.group = Group(server=server) if group is None else group
        server.transport_group(self.group)
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
        self.transport.locate(editor.cursor or 0.0)
        self.sync()

    def attach(self, host) -> None:
        """The piece went on screen: draw the line from the engine's own
        position.

        A playback is built before the window is — a piece can be played by a
        script that never draws it — so this is where the two meet, and it is one
        statement: `clausters.gui.host.GuiHost.head_clock` and the transport's
        own are the same decision, and letting them disagree draws a line nobody
        put there.
        """
        if host is None:
            return
        self.transport.host = host
        host.head_clock("piece")
        self.transport.locate(self.transport.position)

    # ---- the readers ----

    def sync(self) -> None:
        """Make what is drawn be what sounds.

        The whole of it, and it runs on every edit whoever made it. A box that
        went away takes its node with it; one that moved is a set on the node
        that is already sounding.
        """
        seen = set()
        for track in self.editor.structure.tracks:
            gain = self.amp * track_level(self.editor.structure, track)
            lane = track.active_lane
            for region in (lane.regions if lane is not None else ()):
                args = box_args(self.editor.bridge, region, gain)
                if args is None:
                    continue
                seen.add(region.id)
                held = self.nodes.get(region.id)
                if held is None:
                    self.nodes[region.id] = (self._start(args), args)
                    continue
                node, sent = held
                # ``buf`` and ``loop`` are **initial-rate**: what a box reads and
                # whether it wraps are fixed when the reader is built, so a
                # change of either is a new reader and everything else is a set
                # on the one that is already sounding.
                if any(args[k] != sent[k] for k in FIXED):
                    node.free()
                    self.nodes[region.id] = (self._start(args), args)
                else:
                    moved = {k: v for k, v in args.items()
                             if k not in FIXED and v != sent[k]}
                    if moved:
                        node.set(moved)
                    self.nodes[region.id] = (node, args)
        for id in [id for id in self.nodes if id not in seen]:
            self.nodes.pop(id)[0].free()

    def _start(self, args: dict) -> Synth:
        """One reader, in the governed group."""
        return Synth(READER, args, target=self.group, server=self.server)

    # ---- the transport ----

    @property
    def playing(self) -> bool:
        """Whether the piece is rolling, as the engine last answered."""
        return self.transport.refresh().playing

    @property
    def position(self) -> float:
        """Where the piece is, in beats — one round trip, because the position
        is the engine's."""
        return self.transport.refresh().position

    def play(self):
        """Play, or continue a paused pass: the engine keeps where it stopped,
        so resuming is the same verb as starting and nothing is re-cued."""
        self.transport.play(self.server)
        return self

    def pause(self):
        """Freeze the readers where they stand, with every node's state
        intact."""
        self.transport.pause()
        return self

    def stop(self):
        """Halt and go back to **the mark**, not to the top.

        The playhead is never placed: stopped, it stands where the position
        cursor is, so the next play starts from the mark the reader put down
        rather than from wherever the last pass happened to end.
        """
        self.transport.pause()
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
        """Free the readers and the group they live in. The piece is untouched:
        what a playback holds is nodes, and nodes are not the composition."""
        for node, _sent in self.nodes.values():
            node.free()
        self.nodes.clear()
        if self.group is not None:
            self.group.free()
            self.group = None


def box_args(bridge, region, gain: float) -> "dict | None":
    """What one box is read with, or ``None`` for a box that cannot be read —
    one whose source nobody loaded, or one that is a window onto something that
    is not a source at all.

    The whole crossing from the piece to the readers, and it is where the two
    axes meet: a box is placed in **beats** and read in **frames**, and the
    conversion is the editor's bridge rather than a ratio written here.
    """
    window = _window_of(region)
    if window is None:
        return None
    source = (window.get("source") or {}).get("source")
    bufnum = bridge.sources.bufnum(source) if source is not None else -1
    if bufnum < 0:
        return None
    return {"buf": float(bufnum),
            "loop": 1.0 if region.content.looping else 0.0,
            "at": bridge.frame_at(region.position),
            "span": bridge.frames_over(region.position, region.length),
            # **Where the window opens**, in the source's own frames: a trim of
            # the left edge and a split both move it, and a reader that ignored
            # it would play the beginning twice.
            "start": float(window.get("start", 0.0)) * bridge.rate,
            "amp": 0.0 if region.muted else gain}


def track_level(piece, track) -> float:
    """What a track contributes: nothing when it is muted, nothing when another
    is soloed, its level otherwise.

    The mixer's rules, and they are the client's because the **document** holds
    the flags and never reads them — a level is not a fact about the piece, it is
    what somebody set the knob to.
    """
    soloing = any(t.soloed for t in piece.tracks)
    if track.muted or (soloing and not track.soloed):
        return 0.0
    return float((track.config or {}).get("level", 1.0))


def _window_of(region) -> "dict | None":
    """The window a region is, or ``None`` for a box that is a window onto
    something else (a composite)."""
    content = (region.content.write() if hasattr(region.content, "write")
               else region.content)
    if not isinstance(content, dict):
        return None
    window = content.get("window")
    return window if isinstance(window, dict) else None
