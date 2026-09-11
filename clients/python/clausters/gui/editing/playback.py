"""What makes a piece **sound**, kept in step with what the editor draws.

The multitrack editor's other half. `clausters.gui.editing.MultitrackView` says
what a piece looks like and `clausters.gui.editing.MultitrackDomain` says what a
gesture makes of it; this says what it is heard as.

**It decides nothing.** What a track and a clip *are* on the server is the
shared core's (`mt.piece`, `mt.track`, `mt.clip`, `mt.reader` and the channel
strip under all of them), and which of them a given piece needs is the document
crate's -- `clausters._native.multitrack_plan` answers it, wired to which
buffer, at which frame, with which level. So this module is a **diff**: it
compares the plan against what is already sounding and sends the difference.
Both clients run the same two calls, which is why one piece sounds the same in
both of them.

# Why a diff and not a rebuild

A piece plays itself from the transport: every reader reads the engine's own
position, so a locate is no message at all and moving a box is one `set`. That
only holds if the nodes **stay**: rebuilding the tree on every edit would
restart everything that is sounding, and a hand dragging a box would hear its
own gesture as a stutter. So a track, a clip and a reader are each added once
and set thereafter, and only the one thing a set cannot express -- a clip whose
source changed *width*, which is a different wiring -- is torn down and made
again.
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
        #: The def names already sent. A piece asks for the widths it uses, and
        #: a take of another width arriving later asks for more.
        self._sent: set = set()
        #: The `mt.piece` instance: one group, and everything else a slot in it.
        self.piece = None
        #: track id -> its slot group.
        self.tracks: dict = {}
        #: track id -> ``(bus, channels)``: the control buses that track's
        #: meters write, a run of ``2 * channels`` -- the level first and the
        #: mark that waits after it. What the host reads every frame, and the
        #: reason a level that moves every block costs no message.
        self.meters: dict = {}
        #: How long a meter's mark waits, in seconds, as the core says.
        self._meter_hold = 0.0
        #: region id -> ``(group, slot, ports last sent, track id)``. The slot
        #: is kept because a source of another width is another clip def, which
        #: is the one change a `set` cannot express; the track is kept so a
        #: track that went away takes its clips out of the table with it.
        self.clips: dict = {}
        #: ``(region id, channel)`` -> ``(group, ports last sent)``.
        self.readers: dict = {}
        #: automation id -> ``(synth, buffer, bus, owner group, port, table)``.
        #: A curve is a node of this client's own rather than a member of the
        #: piece's graph: it writes a control bus and the port is **mapped** to
        #: it, which is what lets one curve drive a control three levels down
        #: without anybody learning the node behind it.
        self.curves: dict = {}
        #: The group the curve nodes live in, before the piece so a value is
        #: written in the block it is read.
        self.curve_group = None
        #: The name of the curve def, as the core gives it. Sent with all the
        #: others -- it is in the same list, because it is one of the defs a
        #: piece is played by.
        self._curve_def = ""
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

    def plan(self) -> dict:
        """What the piece is, as instances: the crate's answer, not this
        module's opinion of it."""
        bridge = self.editor.bridge
        return _native.multitrack_plan(
            self.editor.structure.write(), bridge.rate, bridge.bpm,
            self.editor.bridge.sources.table())

    def sync(self) -> None:
        """Make what sounds be what is drawn.

        The whole of it, and it runs on every edit whoever made it. Everything
        that is already right is left alone, which is what lets a hand drag a
        box without hearing the rest of the piece restart.
        """
        plan = self.plan()
        if not plan:
            return
        self._send_defs(plan)
        if self.piece is None:
            self.piece = Group.graph(plan["graph"], {"gain": self.gain},
                                     server=self.server)
            # **The piece's group is the transport's**: from here the engine
            # freezes that subtree on a stop and thaws it on a play, and every
            # reader's position is the engine's own rather than a number kept
            # in step here.
            self.server.transport_group(self.piece)
        if self.curve_group is None:
            # Before the piece: a control bus written after it is read is a
            # block late, every block, which on a fade is an audible lag.
            self.curve_group = Group(target=self.piece,
                                     action=AddAction.BEFORE,
                                     server=self.server)
        self._sync_tracks(plan["tracks"])
        self._reap_curves(plan)

    def _send_defs(self, plan: dict) -> None:
        """Send the defs this piece's widths need, and only the ones not sent.

        In the order the core gives them: a graph never names one that has not
        been sent, and getting that wrong fails in another process at
        instantiation with nothing to point at.
        """
        defs = _native.mixer_defs(
            [tuple(pair) for pair in plan.get("widths", [])], plan["channels"])
        self._curve_def = defs.get("curve", "")
        self._meter_hold = float(defs.get("meterHold", 0.0))
        for family, specs in (("synth", defs.get("synth", [])),
                              ("graph", defs.get("graph", []))):
            for spec in specs:
                if spec["name"] in self._sent:
                    continue
                self.server.send_msg("/def_send", family, json.dumps(spec))
                self._sent.add(spec["name"])

    def _sync_tracks(self, planned: list) -> None:
        seen = set()
        for track in planned:
            seen.add(track["track"])
            group = self.tracks.get(track["track"])
            if group is None:
                group = self.piece.add_slot("tracks")
                self.tracks[track["track"]] = group
                self._meter(track["track"], group, track["channels"])
            group.set({"gain": track["gain"], "mute": track["mute"]})
            self._sync_curves(group, track["curves"])
            self._sync_clips(track["track"], group, track["clips"])
        for id in [id for id in self.tracks if id not in seen]:
            self._free_track(id)

    def _meter(self, id, track, channels: int) -> None:
        """Put this track's meters on it, and remember where they write.

        **Two of them**, which is one def twice: with no hold it is the level,
        with the core's hold it is the mark that stays up long enough to be
        read. Both are slot instances, so a piece nobody meters holds none -- and
        the buses are allocated here because it is this client that allocates
        buses, and told to the meter as a port because it is the host that reads
        them.
        """
        channels = max(1, int(channels))
        bus = Bus.control(2 * channels, server=self.server)
        for run, hold in ((0, 0.0), (channels, self._meter_hold)):
            ports = {"meter/out0": float(bus.index + run), "meter/hold": hold}
            if channels > 1:
                ports["meter/out1"] = float(bus.index + run + 1)
            track.add_slot("meters", ports)
        self.meters[id] = (bus, channels)

    def _sync_clips(self, id, track, planned: list) -> None:
        seen = set()
        for clip in planned:
            seen.add(clip["region"])
            ports = {"gain": clip["gain"], "mute": clip["mute"]}
            held = self.clips.get(clip["region"])
            # A source of another width is another clip def -- a mono take is
            # panned into the track and a stereo one is balanced -- so it is the
            # one change that cannot be a set.
            if held is not None and held[1] != clip["slot"]:
                self._free_clip(clip["region"])
                held = None
            if held is None:
                group = track.add_slot(clip["slot"], ports)
            else:
                group, _slot, sent, _track = held
                moved = {k: v for k, v in ports.items() if v != sent.get(k)}
                if moved:
                    group.set(moved)
            self.clips[clip["region"]] = (group, clip["slot"], ports, id)
            self._sync_curves(group, clip["curves"])
            self._sync_readers(clip["region"], group, clip["readers"])
        mine = [r for r, held in self.clips.items() if held[3] == id]
        for region in [r for r in mine if r not in seen]:
            self._free_clip(region)

    def _sync_readers(self, region, clip, planned: list) -> None:
        seen = set()
        for reader in planned:
            key = (region, reader["channel"])
            seen.add(key)
            ports = {"buf": float(reader["buffer"]),
                     "chan": float(reader["channel"]),
                     "at": reader["at"],
                     "span": reader["span"],
                     "start": reader["start"],
                     "loop": 1.0 if reader["looping"] else 0.0}
            held = self.readers.get(key)
            if held is None:
                self.readers[key] = (clip.add_slot("source", ports), ports)
                continue
            group, sent = held
            # Every one of these is an ordinary control, `buf` included, so a
            # box that was re-cut over a different buffer keeps sounding.
            moved = {k: v for k, v in ports.items() if v != sent.get(k)}
            if moved:
                group.set(moved)
            self.readers[key] = (group, ports)
        for key in [k for k in self.readers if k[0] == region and k not in seen]:
            self.readers.pop(key)[0].free()

    # ---- the curves ----

    def _sync_curves(self, owner, planned: list) -> None:
        """Put each curve's table on the server and map the port to it.

        A curve is **not** a member of the piece's graph, and that is the point:
        it writes a control bus, the port is mapped to that bus, and the port's
        own member ids stay private. The table is read at the transport's own
        position, so a locate costs no message at all -- which is the whole
        reason a curve is a table and not a stream of sets.
        """
        if not planned:
            return
        for curve in planned:
            held = self.curves.get(curve["id"])
            table = curve["table"]
            if held is None:
                buffer = Buffer.from_samples(table, server=self.server)
                bus = Bus.control(1, server=self.server)
                node = Synth(self._curve_def,
                             {"out": float(bus.index), "buf": float(buffer.bufnum),
                              "at": curve["at"], "step": curve["step"]},
                             target=self.curve_group, server=self.server)
                self.server.send_msg("/graph_map", owner.id, curve["port"],
                                     int(bus.index))
                self.curves[curve["id"]] = (node, buffer, bus, owner,
                                            curve["port"], table)
                continue
            node, buffer, bus, _owner, port, sent = held
            if table != sent:
                # A curve whose points moved is a new table, and a table is
                # replaced rather than written into -- its length changes with
                # its first and last point. `buf` is an ordinary control, so
                # the reader follows without stopping.
                fresh = Buffer.from_samples(table, server=self.server)
                node.set({"buf": float(fresh.bufnum), "at": curve["at"],
                          "step": curve["step"]})
                buffer.free()
                buffer = fresh
            else:
                node.set({"at": curve["at"], "step": curve["step"]})
            self.curves[curve["id"]] = (node, buffer, bus, owner, port, table)

    def _reap_curves(self, plan: dict) -> None:
        """Free the curves the piece no longer has, and give their ports back.

        **Unmapping is not optional**: a port left mapped to a bus nobody writes
        holds whatever was in it, so a curve that was deleted would go on
        driving the control it drove, at the last value it happened to say.
        """
        alive = {c["id"] for track in plan["tracks"]
                 for c in track["curves"]}
        alive |= {c["id"] for track in plan["tracks"]
                  for clip in track["clips"] for c in clip["curves"]}
        for id in [id for id in self.curves if id not in alive]:
            node, buffer, bus, owner, port, _table = self.curves.pop(id)
            self.server.send_msg("/graph_map", owner.id, port, -1)
            node.free()
            buffer.free()
            bus.free()

    def _free_clip(self, region, *, freeing: bool = True) -> None:
        for key in [k for k in self.readers if k[0] == region]:
            self.readers.pop(key)
        group = self.clips.pop(region)[0]
        if freeing:
            group.free()

    def _free_track(self, id) -> None:
        # Freeing the group frees everything inside it, so the clips and the
        # meters only have to leave the table -- which is what the track id in
        # them is for. The buses are not the group's, so they are given back.
        for region in [r for r, held in self.clips.items() if held[3] == id]:
            self._free_clip(region, freeing=False)
        held = self.meters.pop(id, None)
        if held is not None:
            held[0].free()
        self.tracks.pop(id).free()

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
        """Free the piece's instance. The piece itself is untouched: what a
        playback holds is nodes, and nodes are not the composition."""
        for _node, buffer, bus, *_rest in self.curves.values():
            buffer.free()
            bus.free()
        self.curves.clear()
        if self.curve_group is not None:
            self.curve_group.free()
            self.curve_group = None
        for bus, _channels in self.meters.values():
            bus.free()
        self.meters.clear()
        self.readers.clear()
        self.clips.clear()
        self.tracks.clear()
        if self.piece is not None:
            self.piece.free()
            self.piece = None
