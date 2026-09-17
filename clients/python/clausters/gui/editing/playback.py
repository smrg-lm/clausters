"""What makes a piece **sound**, kept in step with what the editor draws.

The multitrack editor's other half. `clausters.gui.editing.MultitrackView` says
what a piece looks like and `clausters.gui.editing.MultitrackDomain` says what a
gesture makes of it; this says what it is heard as.

**It decides nothing.** What a track and a clip *are* on the server is the
shared core's; which of them a given piece needs, the difference between that
and what is already sounding, the messages that carry it out and **how the piece
is played** -- the tempo a piece that states none is read at, the sample a beat
is when the transport is located, what play, pause, stop and cue send, and that a
paused meter is zeroed -- are `clausters._native.MultitrackPlayback`, in the shared
crate. The GUI host playing a session with no script behind it holds the same
object, so the two are one program. What is left here is what a language
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

A verb answers **steps**: a message to send, a ``/done`` the rest waits for, a
barrier. A buffer's fill waiting for its allocation is one of those steps,
stated once, and every endpoint carries out the same list.
"""

from ... import _native
from ..._steps import run_steps
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
        #: The piece as it is playing -- the crate's, as the host's is.
        self._piece = _native.MultitrackPlayback(chunk=server._bulk_chunk())
        #: The steps not carried out yet -- the crate's walk, as the page's and
        #: the GUI host's are.
        self._runner = _native.StepRunner()
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        server._ensure_recycler()
        bridge = editor.bridge
        #: The piece's transport, as the engine last reported it, and the line
        #: the host draws from its position. ``head_clock="piece"`` says it
        #: once: the host draws the line from the engine's own position instead
        #: of an anchor kept in step here.
        self.transport = Transport(
            editor._host,
            lambda: [] if editor.piece_widget is None else [editor.piece_widget],
            head_clock="piece", governed=True,
            tempo_map=bridge.tempo, sample_rate=bridge.rate,
            extent=lambda: editor.structure.end)
        self.transport.server = server
        self.sync()
        self.locate(editor.cursor or 0.0)

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
        self.locate(self.transport.position)

    # ---- the instance ----

    @property
    def meters(self) -> dict:
        """track id -> ``(bus, channels)``: the control buses that track's
        meters write, a run of ``2 * channels`` -- the level first and the mark
        that waits after it.

        What the host reads every frame, and the reason a level that moves every
        block costs no message.
        """
        return {row["track"]: (int(row["bus"]), int(row["channels"]))
                for row in self._piece.meters()}

    def sync(self) -> None:
        """Make what sounds be what is drawn.

        The whole of it, and it runs on every edit whoever made it. Everything
        that is already right is left alone, which is what lets a hand drag a
        box without hearing the rest of the piece restart.
        """
        bridge = self.editor.bridge
        self._run(self._piece.sync(
            self.editor.structure.write(), bridge.rate,
            bridge.sources.table(), self.gain, self.server.ids))

    def _run(self, steps: list) -> None:
        """Carry the steps out through the crate's runner.

        What may go out is sent; where something is awaited, the runner puts
        the message it waits on last, and that one is sent as a request -- so
        the reply cannot arrive before anyone is listening for it -- and its
        reply is handed back, which releases the rest. Which reply releases
        what is the runner's, as it is the page's and the GUI host's.
        """
        run_steps(self.server, self._runner, steps)

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
        self._run(self._piece.play())
        self.transport.reported(playing=True)
        return self

    def pause(self):
        """Freeze the piece where it stands, with every node's state intact,
        and its meters at zero."""
        self._run(self._piece.pause())
        self.transport.reported(playing=False)
        return self

    def stop(self):
        """Halt and go back to **the mark**, not to the top.

        The playhead is never placed: stopped, it stands where the position
        cursor is, so the next play starts from the mark the reader put down
        rather than from wherever the last pass happened to end.
        """
        mark = self.editor.cursor or 0.0
        self._run(self._piece.stop(mark))
        self.transport.reported(playing=False,
                                position_sample=self._piece.beats_to_samples(mark))
        return self

    def locate(self, beat: float):
        """Seek to ``beat``. The readers seek in the engine, so nothing is
        re-cued and what is sounding carries on from there."""
        self._run(self._piece.locate(beat))
        self.transport.reported(position_sample=self._piece.beats_to_samples(beat))
        return self

    def cue(self, beat: float):
        """The **position cursor** moved: cue a stopped transport there, and
        leave a rolling one alone.

        Two cursors, and only one of them is placed. A click that landed on
        nothing puts the position cursor down, which is where the next play
        starts; moving the mark mid-pass must not move the music.
        """
        self._piece.set_rolling(self.playing)
        steps = self._piece.cue(beat)
        if steps:
            self._run(steps)
            self.transport.reported(position_sample=self._piece.beats_to_samples(beat))
        return self

    def close(self):
        """Free the piece's instance. The piece itself is untouched: what a
        playback holds is nodes, and nodes are not the composition."""
        self._run(self._piece.close(self.server.ids))
