"""The curves of played events: what sends them.

An `clausters.seq.event.Event` carries its curves (``automation=[...]``), and a
channel's curve is a playable of its own
(`clausters.multitrack.Automation.play`). Neither builds anything on the
server: a note is the plain synth it always was, and the controls its curves
drive are **set**, one value every block, in timed bundles. `CurveEmitter` is
what sends them -- one per `clausters.defs.Server`, reached as
``server.curves``.

**What each value is is the shared core's** (`clausters._native.
editing_event_curves`): a note's own curve wins over its channel's on the same
control, a bend adds the two onto the frequency the note was started with, and
a channel's curve is glided as the server's event lane glides it. What is here
is what a language owns: the list of what sounds, the clock it sounds on, and
the wake that sends the next stretch.

**It stays a little ahead of the clock, and no further.** The server keeps a
bounded queue of timed bundles and drops what does not fit, and a curve is
hundreds of values a second, so a whole curve is never sent at once: the
emitter wakes every `WINDOW` and sends the instants up to two windows ahead,
one bundle per instant holding every curve that sounds.

**A curve reaches a note from its start to its off**; through its release the
note keeps the last value. The off is the last instant a client knows the node
is there, and a ``/node_set`` to a node that is gone is a ``/fail`` -- and,
offline, the end of the render.

**A channel's curve reaches the notes played on the clock it was played on.**
Each clock is an axis of its own here, since a curve is placed by the beat it
was played at and a beat belongs to one clock.
"""

import itertools
import math
import threading
import time

from .. import _native
from ..base.moment import Moment

#: How often an emitter wakes, in seconds. It sends up to two of these ahead
#: of its clock, so a wake that comes late by less than one is not heard.
WINDOW = 0.05

#: The sample rate a curve is stepped at when nothing says the server's.
_RATE = 48_000.0


class _Note:
    """A sounding note: where it starts and ends on its axis, and its own
    curves as written."""

    __slots__ = ("node", "start", "beat", "off", "channel", "freq", "curves")

    def __init__(self, node, start, beat, off, channel, freq, curves):
        self.node = node
        self.start = start
        #: The beat it started on -- its curves' zero. Seconds with no clock.
        self.beat = beat
        self.off = off
        self.channel = channel
        self.freq = freq
        self.curves = curves


class _Curve:
    """A channel's curve, playing: its zero, and the glide where the last
    window left it."""

    __slots__ = ("id", "start", "beat", "target", "points", "state")

    def __init__(self, id, start, beat, target, points):
        self.id = id
        self.start = start
        self.beat = beat
        self.target = target
        self.points = points
        self.state = None


def _messages(sets) -> list:
    """``[node, control, value]`` triples as ``/node_set`` messages, one per
    node, in the order the nodes first appear."""
    by_node: dict = {}
    for node, control, value in sets:
        by_node.setdefault(int(node), []).extend((str(control), float(value)))
    return [("/node_set", node, *pairs) for node, pairs in by_node.items()]


class _Axis:
    """One clock's time, and what sounds on it. ``clock`` is ``None`` for the
    notes played outside any clock, whose time is the wall's."""

    def __init__(self, emitter, clock):
        self.emitter = emitter
        self.clock = clock
        self.notes: dict = {}
        #: The channels' curves, newest first: the first that names a control
        #: is the one a note reads.
        self.channels: list = []
        #: The second, on this axis, everything has been sent up to.
        self.until = None
        self.awake = False
        #: When the wake last ran, on the monotonic clock.
        self.woken = 0.0

    # ---- time ----

    def now(self) -> float:
        clock = self.clock
        if clock is None:
            return self.emitter.wall()
        return clock.beats2secs(clock.beats())

    def placed(self, moment, pin=None) -> "tuple[float, float]":
        """A moment as ``(seconds on this axis, the beat a curve played there
        counts from)``. With no clock both are the wall's seconds, from the
        instant ``pin`` the caller read it at."""
        if self.clock is None:
            secs = (self.emitter.wall() if pin is None else float(pin)) + moment.beat
            return secs, secs
        return moment.secs(), moment.beat

    def _position(self, beat: float, start: float, to: float) -> dict:
        """Where a curve whose zero is ``beat`` stands at ``start``, and how
        fast it advances up to ``to``: the tempo, read off the clock for this
        stretch."""
        clock = self.clock
        if clock is None:
            return {"at": start - beat, "rate": 1.0}
        at = clock.secs2beats(start)
        if to > start:
            rate = (clock.secs2beats(to) - at) / (to - start)
        else:
            rate = float(clock.tempo)
        return {"at": at - beat, "rate": rate}

    def _ask(self, start: float, to: float, notes, *, first=False, fresh=False) -> dict:
        """One window of the core's rule over ``notes`` and this axis's
        channels. ``fresh`` hands the channels' curves with no glide, for a
        stretch that is not the one after the last."""
        return _native.editing_event_curves({
            "sample_rate": self.emitter.sample_rate(self.clock),
            "from": start, "to": to, "start": first,
            "notes": [{"node": n.node, "start": n.start, "off": n.off,
                       "channel": n.channel, "freq": n.freq, "curves": n.curves,
                       **self._position(n.beat, start, to)} for n in notes],
            "channels": [{"id": c.id, "start": c.start, "target": c.target,
                          "points": c.points,
                          "state": None if fresh else c.state,
                          **self._position(c.beat, start, to)}
                         for c in self.channels],
        })

    def _send(self, bundles) -> None:
        server, clock = self.emitter.server, self.clock
        for secs, sets in bundles or ():
            if clock is None:
                # The wall's own second, not a delay from a now read again.
                server._send_at(Moment(None, 0.0), _messages(sets), keep=False,
                                pin=float(secs))
            else:
                server._send_at(Moment(clock, clock.secs2beats(float(secs))),
                                _messages(sets), keep=False)

    # ---- a note ----

    def begin(self, note: _Note) -> list:
        """Take ``note`` in, and answer what its controls are set to as it is
        made: its curves' first values, as messages for the bundle that makes
        it."""
        self.notes[note.node] = note
        if not (note.curves or self.channels):
            return []
        # The channels' curves where they stand at the note's own start, not
        # where the wake, which runs ahead, has left their glide.
        answer = self._ask(note.start, note.start, [note], first=True, fresh=True)
        bundles = answer.get("bundles") or ()
        return _messages(bundles[0][1]) if bundles else []

    def follow(self, note: _Note) -> None:
        """The note is made: send what was already sent for the others, and
        see that the wake runs."""
        if not (note.curves or self.channels):
            return
        if self.until is not None and self.until > note.start:
            self._send(self._ask(note.start, self.until, [note], fresh=True).get("bundles"))
        self.wake(note.start)

    # ---- the wake ----

    def wake(self, since: float) -> None:
        """See that the periodic wake runs, sending from ``since`` on when it
        was not."""
        stale = (self.clock is not None and not self.emitter.offline
                 and time.monotonic() - self.woken > 8 * WINDOW)
        if self.awake and not stale:
            return
        begin = min(since, self.now())
        if self.until is None or self.until < begin:
            # A stretch nothing was sent over: a glide left there is stale.
            self.until = begin
            for curve in self.channels:
                curve.state = None
        self.awake = True
        self.woken = time.monotonic()
        if self.clock is not None:
            self.clock.sched(0.0, self._on_clock)
        elif self.emitter.offline:
            # No clock and no wall: the score takes every value now.
            while self._window(self.until + 1.0):
                pass
        else:
            threading.Thread(target=self._on_thread, daemon=True,
                             name="clausters-curves").start()

    def _window(self, to: float) -> bool:
        """Send the instants up to ``to``; whether anything is left to send
        after them."""
        if not self.notes:
            self.awake = False
            return False
        if to > self.until:
            answer = self._ask(self.until, to, list(self.notes.values()))
            self._send(answer.get("bundles"))
            states = {int(key): float(y) for key, y in answer.get("states") or ()}
            for curve in self.channels:
                if curve.id in states:
                    curve.state = states[curve.id]
            for node in answer.get("over") or ():
                self.emitter._gone(int(node), self)
            self.until = to
            if answer.get("idle") or not self.notes:
                self.awake = False
                return False
        return True

    def _tick(self) -> bool:
        self.woken = time.monotonic()
        return self._window(self.now() + 2 * WINDOW)

    def _on_clock(self):
        """The wake as a clock's item: answers the beats to the next one, or
        nothing when there is nothing left to send."""
        with self.emitter._lock:
            if not self._tick():
                return None
            clock = self.clock
            beat = clock.beats()
            ahead = clock.secs2beats(clock.beats2secs(beat) + WINDOW) - beat
            return max(float(ahead), 1e-6)

    def _on_thread(self) -> None:
        while True:
            with self.emitter._lock:
                if not self._tick():
                    return
            time.sleep(WINDOW)


class CurveEmitter:
    """What sends the curves of the events a server plays: the list of what
    sounds, by the clock it sounds on, and the values of the next stretch.

    Made by its `clausters.defs.Server` (``server.curves``) and driven by it:
    `clausters.defs.Server.play_event` hands it every note, and
    `clausters.multitrack.Automation.play` a channel's curve. A script does
    not call it.

    Args:
        server: the `clausters.defs.Server` whose notes these are.
    """

    def __init__(self, server):
        self.server = server
        self._lock = threading.RLock()
        self._axes: dict = {}
        #: node -> the axis it sounds on.
        self._notes: dict = {}
        #: curve id -> the axis it plays on.
        self._curves: dict = {}
        self._ids = itertools.count(1)

    @property
    def offline(self) -> bool:
        """Whether the server is a score being written rather than one that
        sounds."""
        return getattr(self.server.interface, "time_mode", "unix") == "score"

    def wall(self) -> float:
        """Now, for what plays outside any clock: Unix seconds, and the
        score's zero offline."""
        return 0.0 if self.offline else time.time()

    def sample_rate(self, clock) -> float:
        """The rate a curve is stepped at: the clock's, when it is on the
        server's samples, else the one the server was asked for."""
        rate = getattr(getattr(clock, "timebase", None), "sample_rate", None)
        if not rate:
            rate = getattr(getattr(self.server, "options", None), "sample_rate", None)
        return float(rate or _RATE)

    def _axis(self, clock) -> _Axis:
        key = None if clock is None else id(clock)
        axis = self._axes.get(key)
        if axis is None:
            axis = self._axes[key] = _Axis(self, clock)
        return axis

    def _gone(self, node: int, axis: _Axis) -> None:
        axis.notes.pop(node, None)
        self._notes.pop(node, None)
        if not axis.notes and not axis.channels:
            self._axes.pop(None if axis.clock is None else id(axis.clock), None)

    # ---- notes ----

    def note(self, node: int, when: Moment, event, sustain: float, pin=None) -> list:
        """Take in the note ``event`` is about to be, on ``node``, played at
        ``when`` for ``sustain`` beats -- ``pin`` being the wall-clock instant
        a clockless ``when`` counts from. Answers the ``/node_set`` messages
        that go in the bundle that makes it: its curves' first values."""
        curves = event.get("automation") or ()
        if not isinstance(curves, (list, tuple)):
            curves = (curves,)
        written = [{"target": c.target, "points": list(c.write().get("points") or ())}
                   for c in curves if getattr(c, "enabled", True)]
        with self._lock:
            axis = self._axis(when.clock)
            if len(axis.notes) > 256:
                # Nothing woke to say so: the notes whose off has passed.
                now = axis.now()
                for old in [n for n in axis.notes.values()
                            if n.off is not None and n.off < now]:
                    self._gone(old.node, axis)
            start, beat = axis.placed(when, pin)
            off = (start + float(sustain) if when.clock is None
                   else when.at(sustain).secs())
            note = _Note(int(node), start, beat,
                         off if math.isfinite(off) else None,
                         int(event.get("channel", 0) or 0), float(event.freq()), written)
            self._notes[note.node] = axis
            return axis.begin(note)

    def follow(self, node: int) -> None:
        """The bundle that makes ``node`` is sent: its curves follow."""
        with self._lock:
            axis = self._notes.get(int(node))
            note = None if axis is None else axis.notes.get(int(node))
            if note is not None:
                axis.follow(note)

    def forget(self, node: int) -> None:
        """``node`` is gone -- freed, released by hand, or ended: nothing more
        is sent to it."""
        with self._lock:
            axis = self._notes.get(int(node))
            if axis is not None:
                self._gone(int(node), axis)

    # ---- a channel's curve ----

    def play(self, target, points, when: Moment) -> int:
        """Play a channel's curve from ``when`` on: it reaches the notes of
        its channel on that clock, sounding and to come, and holds its last
        value past its end. Answers what `stop` takes."""
        with self._lock:
            axis = self._axis(when.clock)
            start, beat = axis.placed(when)
            curve = _Curve(next(self._ids), start, beat, target, list(points))
            axis.channels.insert(0, curve)
            self._curves[curve.id] = axis
            if axis.notes:
                axis.wake(start)
            return curve.id

    def stop(self, curve: int) -> None:
        """Stop the channel's curve `play` answered ``curve`` for: the notes it
        reached keep the last value it set."""
        with self._lock:
            axis = self._curves.pop(int(curve), None)
            if axis is None:
                return
            axis.channels = [c for c in axis.channels if c.id != int(curve)]
            if not axis.notes and not axis.channels:
                self._axes.pop(None if axis.clock is None else id(axis.clock), None)
