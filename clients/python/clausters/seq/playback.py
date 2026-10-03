"""A sequence played on the server: an event lane on a transport.

`clausters.play` of a `clausters.seq.EventSequence` -- and the notes editor's
own playback -- write the sequence as the data of an **event lane** on one of
the server's transports, and the server plays it by the transport's position:
a pause, a locate and a loop are the transport's, and a note sounding when it
stops is released and rings out. The steps are the shared crate's
(``NotesPlayback``); what is here is carrying them out on a server, and the
`clausters.defs.Transport` the lane is on, whose verbs then speak the
sequence's beats.

There is **one per sequence** on a server, each on a transport of its own, so
two sequences sound together and a roll's play cursor is its own sequence's.
A server has a fixed number of transports (``--transports``): a playback takes
one when it is made, which fails when none is left, and `NotesPlayback.free`
-- `clausters.defs.Transport.free` -- gives it back.
"""

import weakref

from .. import _native
from .._steps import run_steps


class NotesPlayback:
    """**What plays one sequence on one server**: the crate's playback, the
    steps it answers carried out, and the `clausters.defs.Transport` the lane
    is on. Reached through `of`, so a sequence has one per server."""

    _all = weakref.WeakSet()

    @classmethod
    def of(cls, server, sequence) -> "NotesPlayback":
        """The playback of ``sequence`` on ``server``, made on first ask.

        Raises:
            RuntimeError: when it has to be made and the server has no
                transport left for it.
        """
        held = server.__dict__.setdefault("_sequence_playbacks", {})
        found = held.get(id(sequence))
        if found is None:
            found = held[id(sequence)] = cls(server, sequence)
            cls._all.add(found)
        return found

    @classmethod
    def held(cls, server, sequence) -> "NotesPlayback | None":
        """The playback of ``sequence`` on ``server`` when it has one, without
        making it."""
        if server is None:
            return None
        return server.__dict__.get("_sequence_playbacks", {}).get(id(sequence))

    @classmethod
    def forget_all(cls, server) -> None:
        """Forget every playback on ``server`` -- its close: the bookkeeping
        goes, and the server is told nothing, as for anything else this handle
        made on it."""
        for playback in list(server.__dict__.get("_sequence_playbacks", {}).values()):
            playback._forget()

    def __init__(self, server, sequence):
        self.server = server
        #: The sequence it plays, held for as long as the playback is.
        self.sequence = sequence
        self._native = _native.NotesPlayback()
        self._runner = _native.StepRunner()
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        server._ensure_recycler()
        self._rate = None
        #: The version of the sequence's context the lane last took:
        #: everything over the sequence that hears of a change asks for it,
        #: and the lane takes it once.
        self._taken = None
        #: Where a pass starts, in the sequence's beats.
        self.cursor = 0.0
        #: Where a pass ends: ``None``, ``"contents"`` or a beat.
        self.end = None
        #: The time range a pass plays and a loop repeats, ``(start, end)`` in
        #: the sequence's beats, or ``None`` -- the same one a sweep leaves on
        #: a roll, and drawn there.
        self.span = None
        #: Whether the loop switch is on: the span, or every note with none.
        self.looping = False
        self._paused = False
        #: Whether a script was handed the transport: a playback only rolls
        #: opened is freed with the last of them, one a script holds is the
        #: script's to free.
        self.kept = False
        self._freed = False
        try:
            taken = self._native.call("open", sequence._seq, server.ids)
        except ValueError as refused:
            raise RuntimeError(str(refused)) from None
        #: The transport it plays on, taken from the server's.
        self.transport_id = int(taken["transport"])
        #: That transport, as the object a script plays.
        self.transport = server.transport_at(self.transport_id)
        self.transport._driver = self

    @property
    def rate(self) -> float:
        if self._rate is None:
            self._rate = float(self.server.query_info().nominal_sample_rate)
        return self._rate

    def state(self) -> dict:
        """The transport as the engine has it."""
        return self.transport.state()

    def call(self, verb: str, **args) -> dict:
        """One verb over the sequence, its steps carried out."""
        answer = self._native.call(verb, self.sequence._seq, self.server.ids,
                                   rate=self.rate, **args)
        steps = answer.get("steps")
        if steps:
            run_steps(self.server, self._runner, steps)
        return answer

    def free(self) -> None:
        """**Free the playback**: what sounds is released, the lane and its
        groups are freed, and the transport goes back to the server's, for
        another sequence to take. The `clausters.defs.Transport` it answered
        is then a transport with nothing loaded."""
        if self._freed:
            return
        self._freed = True
        try:
            self.call("close")
        finally:
            self._forget()

    def _forget(self) -> None:
        """The bookkeeping goes, and the server is told nothing."""
        self._freed = True
        if self.transport._driver is self:
            self.transport._driver = None
        self.server.__dict__.get("_sequence_playbacks", {}).pop(id(self.sequence), None)
        self._native.free()

    # ---- what the lane holds ----

    def load(self, at: float = 0.0, *, range=None, looping: bool = False,
             end=False) -> None:
        """**Play the sequence from beat ``at``.** ``range`` -- ``(start,
        end)`` in beats -- plays that span, going back to ``at``; ``looping``
        loops it, or with no range every note; ``end`` is where a pass ends
        (see `clausters.gui.editing.NotesEditor.end`), kept from the last
        load when not given."""
        if end is not False:
            self.end = end
        self.call("end", end=self.end)
        self.call("play", **{"from": float(at)},
                  range=list(range) if range is not None else None,
                  loop=bool(looping))
        if tuple(range or ()) != tuple(self.span or ()):
            self.span = None if range is None else (float(range[0]), float(range[1]))
            self._show()
        self.looping = bool(looping)
        self.cursor = float(at)
        self._paused = False

    def update(self, version=None) -> None:
        """**The lane takes the sequence again**, when it has not taken it at
        this ``version`` of its context already: everything over a sequence
        that hears of a change -- the editor that made it, the others adopting
        it, the script's own write -- asks, and the lane is one. ``None`` is a
        change no context counted, always taken."""
        if version is not None and self._taken == version:
            return
        self._taken = version
        self.call("update")

    @classmethod
    def changed(cls, sequence, version=None) -> None:
        """``sequence`` changed: every playback of it takes it again."""
        for playback in list(cls._all):
            if playback.sequence is sequence and not playback._freed:
                playback.update(version)

    def cue(self, at: float) -> None:
        """The position cursor was placed at beat ``at``: where the next pass
        starts, and a stopped transport is located there."""
        self.cursor = float(at)
        self.call("cue", at=self.cursor)

    # ---- the transport's verbs, over what is loaded ----

    def playing(self) -> bool:
        playing = bool(self.state().get("playing"))
        self.call("setRolling", rolling=playing)
        return playing

    def play(self, at=None) -> None:
        if at is None and self._paused:
            self.call("resume")
            self._paused = False
            return
        self.load(self.cursor if at is None else float(at),
                  range=self.span, looping=self.looping)

    def set_end(self, end) -> None:
        self.end = end
        self.call("end", end=end)

    def pause(self) -> None:
        self.call("pause")
        self._paused = True

    def stop(self) -> None:
        self.call("stop", back=self.cursor)
        self._paused = False

    def locate(self, at: float) -> None:
        self.cursor = float(at)
        if self.playing():
            self.load(self.cursor, range=self.span, looping=self.looping)
        else:
            self.call("cue", at=self.cursor)

    def set_span(self, span, *, show: bool = True) -> None:
        """The time range: kept stopped or rolling, drawn on every roll over
        the sequence, and followed at once by a loop in progress."""
        self.span = None if span is None else (float(span[0]), float(span[1]))
        if show:
            self._show()
        if self.looping:
            self._loop()

    def set_looping(self, on: bool) -> None:
        """The loop switch, as `L` is: kept stopped, and followed at once by
        a pass in progress."""
        self.looping = bool(on)
        self._loop()
        for roll in self.rolls():
            roll.show_looping(self.looping)

    def _loop(self) -> None:
        self.call("loop",
                  range=list(self.span) if self.span is not None else None,
                  loop=self.looping)

    def _show(self) -> None:
        """Every roll over the sequence draws the span."""
        for roll in self.rolls():
            roll.show_span(self.span)

    def rolls(self) -> list:
        """The rolls open over the sequence: the views of its history that
        draw a span."""
        from ..history import ATTR

        context = getattr(self.sequence, ATTR, None)
        return [view for view in ([] if context is None else context.views())
                if getattr(view, "show_span", None) is not None
                and view.structure is self.sequence]


def play_sequence(sequence, at: float = 0.0, server=None):
    """`clausters.play` of a sequence: loaded on a transport of its own from
    beat ``at``, its pass ending where its contents do, and the
    `clausters.defs.Transport` it is on answered -- booting a server for the
    default session when there is none. The transport is the sequence's until
    it is freed (`clausters.defs.Transport.free`)."""
    from ..base.main import main

    playback = NotesPlayback.of(main.server_or_boot(server), sequence)
    playback.kept = True
    playback.load(at, end="contents")
    return playback.transport
