"""A sequence played on the server: an event lane on a transport.

`clausters.play` of a `clausters.seq.EventSequence` -- and the notes editor's
own playback -- write the sequence as the data of an **event lane** on one of
the server's transports, and the server plays it by the transport's position:
a pause, a locate and a loop are the transport's, and a note sounding when it
stops is released and rings out. The steps are the shared crate's
(``NotesPlayback``); what is here is carrying them out on a server, and the
`clausters.defs.Transport` the lane is on, whose verbs then speak the
sequence's beats.

There is **one per server**, on one transport, so the sequences played on a
server share it: the one played last is the one that sounds, as on any
transport with one thing loaded.
"""

import weakref

from .. import _native
from .._steps import run_steps


class NotesPlayback:
    """**What plays sequences on one server**: the crate's playback, the steps
    it answers carried out, and the `clausters.defs.Transport` the lane is on.
    Reached through `of`, so a server has one."""

    _of = weakref.WeakKeyDictionary()
    _all = weakref.WeakSet()

    @classmethod
    def of(cls, server) -> "NotesPlayback":
        """The playback of ``server``, made on first ask."""
        found = cls._of.get(server)
        if found is None:
            found = cls._of[server] = cls(server)
            cls._all.add(found)
        return found

    def __init__(self, server):
        self.server = server
        self._native = _native.NotesPlayback()
        self._runner = _native.StepRunner()
        # Node ids come back on their `/node_end`, which only a registered
        # client hears.
        server._ensure_recycler()
        self._rate = None
        #: The sequence the lane holds, if any: the one played last.
        self.planned = None
        #: ``(sequence, version)`` the lane last took: everything over the
        #: sequence that hears of a change asks for it, and the lane takes it
        #: once.
        self._taken = None
        #: Where a pass starts, in the planned sequence's beats.
        self.cursor = 0.0
        #: Where a pass ends: ``None``, ``"contents"`` or a beat.
        self.end = None
        #: The time range a pass plays and a loop repeats, ``(start, end)`` in
        #: the planned sequence's beats, or ``None`` -- the same one a sweep
        #: leaves on a roll, and drawn there.
        self.span = None
        #: Whether the loop switch is on: the span, or every note with none.
        self.looping = False
        self._paused = False
        #: The transport it plays on -- the crate's word for it.
        self.transport_id = int(self._native.call(
            "state", _native.SequenceHandle(), server.ids)["transport"])
        #: That transport, as the object a script plays.
        self.transport = server.transport_at(self.transport_id)

    @property
    def rate(self) -> float:
        if self._rate is None:
            self._rate = float(self.server.query_info().nominal_sample_rate)
        return self._rate

    def state(self) -> dict:
        """The transport as the engine has it."""
        return self.transport.state()

    def call(self, verb: str, sequence, **args) -> dict:
        """One verb over ``sequence``, its steps carried out."""
        answer = self._native.call(verb, sequence._seq, self.server.ids,
                                   rate=self.rate, **args)
        steps = answer.get("steps")
        if steps:
            run_steps(self.server, self._runner, steps)
        return answer

    # ---- what the lane holds ----

    def load(self, sequence, at: float = 0.0, *, range=None, looping: bool = False,
             end=False) -> None:
        """**Play ``sequence`` from beat ``at``**: it becomes what the lane
        holds, and the transport's verbs are its own from now on. ``range`` --
        ``(start, end)`` in beats -- plays that span, going back to ``at``;
        ``looping`` loops it, or with no range every note; ``end`` is where a
        pass ends (see `clausters.gui.editing.NotesEditor.end`), kept from
        the last load when not given."""
        if end is not False:
            self.end = end
        self.call("end", sequence, end=self.end)
        self.call("play", sequence, **{"from": float(at)},
                  range=list(range) if range is not None else None,
                  loop=bool(looping))
        if self.planned is not sequence or tuple(range or ()) != tuple(self.span or ()):
            self.span = None if range is None else (float(range[0]), float(range[1]))
            self._show(sequence)
        self.looping = bool(looping)
        self.planned = sequence
        self.cursor = float(at)
        self._paused = False
        self.transport._driver = self

    def update(self, sequence, version=None) -> None:
        """**The lane takes ``sequence`` again**, when it is the one the lane
        holds and it has not taken it at this ``version`` of its context
        already: everything over a sequence that hears of a change -- the
        editor that made it, the others adopting it, the script's own write --
        asks, and the lane is one. ``None`` is a change no context counted,
        always taken."""
        if self.planned is not sequence:
            return
        if version is not None and self._taken == (id(sequence), version):
            return
        self._taken = None if version is None else (id(sequence), version)
        self.call("update", sequence)

    @classmethod
    def changed(cls, sequence, version=None) -> None:
        """``sequence`` changed: every playback holding it takes it again."""
        for playback in list(cls._all):
            playback.update(sequence, version)

    # ---- the transport's verbs, over what is loaded ----

    def playing(self) -> bool:
        playing = bool(self.state().get("playing"))
        if self.planned is not None:
            self.call("setRolling", self.planned, rolling=playing)
        return playing

    def play(self, at=None) -> None:
        if self.planned is None:
            return
        if at is None and self._paused:
            self.call("resume", self.planned)
            self._paused = False
            return
        self.load(self.planned, self.cursor if at is None else float(at),
                  range=self.span, looping=self.looping)

    def set_end(self, end) -> None:
        self.end = end
        if self.planned is not None:
            self.call("end", self.planned, end=end)

    def pause(self) -> None:
        if self.planned is not None:
            self.call("pause", self.planned)
            self._paused = True

    def stop(self) -> None:
        if self.planned is not None:
            self.call("stop", self.planned, back=self.cursor)
            self._paused = False

    def locate(self, at: float) -> None:
        if self.planned is None:
            return
        self.cursor = float(at)
        if self.playing():
            self.load(self.planned, self.cursor, range=self.span, looping=self.looping)
        else:
            self.call("cue", self.planned, at=self.cursor)

    def set_span(self, span, *, show: bool = True) -> None:
        """The time range: kept stopped or rolling, drawn on every roll over
        the planned sequence, and followed at once by a loop in progress."""
        self.span = None if span is None else (float(span[0]), float(span[1]))
        if self.planned is None:
            return
        if show:
            self._show(self.planned)
        if self.looping:
            self._loop()

    def set_looping(self, on: bool) -> None:
        """The loop switch, as `L` is: kept stopped, and followed at once by
        a pass in progress."""
        self.looping = bool(on)
        if self.planned is not None:
            self._loop()
            self._each_roll(self.planned, lambda roll: roll.show_looping(self.looping))

    def _loop(self) -> None:
        self.call("loop", self.planned,
                  range=list(self.span) if self.span is not None else None,
                  loop=self.looping)

    def _show(self, sequence) -> None:
        """Every roll over ``sequence`` draws the span."""
        self._each_roll(sequence, lambda roll: roll.show_span(self.span))

    @staticmethod
    def _each_roll(sequence, do) -> None:
        from ..history import ATTR

        context = getattr(sequence, ATTR, None)
        for view in [] if context is None else context.views():
            if getattr(view, "show_span", None) is not None and view.structure is sequence:
                do(view)


def play_sequence(sequence, at: float = 0.0, server=None):
    """`clausters.play` of a sequence: loaded on the server's notes transport
    from beat ``at``, its pass ending where its contents do, and the
    `clausters.defs.Transport` it is on answered -- booting a server for the
    default session when there is none."""
    from ..base.main import main

    playback = NotesPlayback.of(main.server_or_boot(server))
    playback.load(sequence, at, end="contents")
    return playback.transport
