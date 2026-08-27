"""Static timelines and a playhead (random-access sequencing).

The counterpart to the generative layer (`clausters.base.stream.Routine`,
`clausters.seq.pattern.Pbind`). A `Routine` is a forward-only generator: its
musical state lives in the generator's locals, so it cannot be *seeked*. A
`Timeline` is the opposite — a **static, editable list of timed items kept
sorted by beat**, with random access by time (`index_at`, `range`). That is what
makes DAW-style transport controls possible: a `Playhead` scans the timeline
forward as the clock advances, and **play / stop / locate / loop** re-seek the
cursor by time at the boundaries.

An *item* is anything that can render itself on a destination — it has a
`play(destination)` method. `clausters.seq.event.Event` already is one (it plays
a note on a `Server` or a `MidiServer` — the same double dispatch the patterns
use), so a timeline of `Event`s renders to OSC *or* MIDI by which destination
the playhead holds, exactly like the rest of the client. `OscEvent` and
`MidiEvent` wrap a raw OSC message or MIDI bytes, so a timeline can also be a
plain, editable OSC/MIDI score.

This layer is **client-side**: each playhead has its own local transport over
its own timeline, and several clients phase-align through `quant` and the shared
`/transport_set` grid (see the timing docs). A server-broadcast transport (one
conductor's play/stop/locate driving every client) can layer on top later
without changing this.
"""

import bisect

from ..base.moment import Moment
from ..base.stream import Routine


class _Entry:
    """One timed item on a timeline. A stable object so it can be `remove`d or
    `move`d by identity after other edits shift positions."""

    __slots__ = ("beat", "item")

    def __init__(self, beat, item):
        self.beat = beat
        self.item = item


class OscEvent:
    """A raw OSC message ``(addr, *args)`` as a timeline item: rendering it sends
    the message at the playhead's current logical beat through a `Server`."""

    def __init__(self, addr, *args):
        self.addr = addr
        self.args = args

    def play(self, destination):
        destination.send_bundle((self.addr, *self.args))


class MidiEvent:
    """Raw MIDI bytes as a timeline item: rendering it emits the message at the
    playhead's current logical beat through a `MidiServer`."""

    def __init__(self, message):
        self.message = bytes(message)

    def play(self, destination):
        destination.send_message(self.message)


#: How many events a bounce records before it decides the pattern is endless
#: (`Timeline.from_pattern`'s ``max_events`` default).
#:
#: A bounce holds every event in memory, so a million is already past any real
#: piece and nowhere near a legitimate one — which is what makes the cap honest
#: *here* and wrong inside `clausters.base.TempoClock.render`, where a long
#: offline render of a real score is exactly the thing that runs for a very
#: long time on purpose.
MAX_BOUNCED_EVENTS = 1_000_000


class Timeline:
    """A static, editable sequence of ``(beat, item)`` kept sorted by beat, with
    random access by time.

    The structure a `Playhead` plays and a transport seeks. Items are kept in
    beat order (a stable insert preserves the order of items added at the same
    beat, e.g. a note-off before a re-trigger). Edit it freely — `add`,
    `remove`, `move`, `clear` — and read ranges of it by time — `index_at`,
    `range`, `at`.

    `add` returns a handle (an opaque entry) you pass back to `remove`/`move`, so
    edits stay correct as other inserts shift indices.

    Args:
        items: optional iterable of ``(beat, item)`` to seed the timeline.
    """

    def __init__(self, items=None):
        self._entries = []
        if items is not None:
            for beat, item in items:
                self.add(beat, item)

    # ---- editing ----

    def add(self, beat, item):
        """Insert ``item`` at ``beat`` (kept sorted); returns an entry handle."""
        entry = _Entry(float(beat), item)
        bisect.insort(self._entries, entry, key=lambda e: e.beat)
        return entry

    def remove(self, entry):
        """Remove an entry returned by `add` (by identity)."""
        self._entries.remove(entry)
        return self

    def move(self, entry, new_beat):
        """Move an entry to ``new_beat``, keeping the timeline sorted."""
        self._entries.remove(entry)
        entry.beat = float(new_beat)
        bisect.insort(self._entries, entry, key=lambda e: e.beat)
        return entry

    def clear(self):
        """Drop every item."""
        self._entries.clear()
        return self

    def quantize(self, grid):
        """Snap every placement to the nearest multiple of ``grid`` (beats):
        each entry's beat moves to the grid line, durations untouched. The
        data-side counterpart of the piano-roll's `q` gesture (which quantizes
        in the view when the GUI runs standalone). A zero/negative grid is a
        no-op. Returns the timeline."""
        g = float(grid)
        if g <= 0.0:
            return self
        for e in self._entries:
            e.beat = max(0.0, round(e.beat / g) * g)
        self._entries.sort(key=lambda e: e.beat)
        return self

    # ---- random access by time ----

    def index_at(self, beat) -> int:
        """The cursor (index) of the first item at or after ``beat`` — the seek
        primitive a playhead uses to start or locate."""
        return bisect.bisect_left(self._entries, float(beat), key=lambda e: e.beat)

    def range(self, t0, t1) -> list:
        """The ``(beat, item)`` pairs in the half-open beat window ``[t0, t1)``."""
        i = self.index_at(t0)
        j = bisect.bisect_left(self._entries, float(t1), key=lambda e: e.beat)
        return [(e.beat, e.item) for e in self._entries[i:j]]

    def at(self, beat) -> list:
        """The items exactly at ``beat``."""
        b = float(beat)
        return [e.item for e in self._entries if e.beat == b]

    def duration(self) -> float:
        """The beat of the last item (0.0 when empty) — the timeline's length."""
        return self._entries[-1].beat if self._entries else 0.0

    def __len__(self):
        return len(self._entries)

    def __getitem__(self, i):
        e = self._entries[i]
        return (e.beat, e.item)

    def __iter__(self):
        return ((e.beat, e.item) for e in self._entries)

    # ---- capture a pattern into a timeline ----

    @classmethod
    def from_pattern(
        cls,
        pattern,
        dur=None,
        tempo: float = 1.0,
        max_events: int = MAX_BOUNCED_EVENTS,
    ) -> "Timeline":
        """Bounce an event pattern (a `Pbind`) into a static timeline by running
        it offline and recording each event at its logical beat. ``dur`` bounds
        an open-ended pattern (beats); ``None`` drains a finite one fully.

        Without a ``dur``, an endless pattern is **caught rather than run
        forever**: the bounce raises once it has recorded ``max_events``
        (`MAX_BOUNCED_EVENTS` by default). That guard is this call's and not the
        clock's — a long offline `clausters.base.TempoClock.render` of a real
        score is meant to run for a long time, where a bounce with no bound is a
        mistake."""
        from ..base.clock import TempoClock

        timeline = cls()
        recorder = _Recorder(timeline)
        clock = TempoClock(tempo)
        pattern.play(clock, recorder)
        try:
            clock.render(until_beat=dur, max_steps=None if dur is not None else max_events)
        except RuntimeError as e:
            raise RuntimeError(
                f"Timeline.from_pattern: the pattern did not end after "
                f"{max_events} events — pass dur= to bound an endless one"
            ) from e
        return timeline


class _Recorder:
    """A capture destination: `play_event` appends the event to a timeline at the
    running routine's logical beat instead of sending it (used by
    `Timeline.from_pattern`)."""

    def __init__(self, timeline: Timeline):
        self.timeline = timeline

    def play_event(self, event):
        from .event import Event

        beat = Moment.current().beat
        self.timeline.add(beat, Event(event))
        return None


class Playhead:
    """A transport over a `Timeline`: play / stop / locate / loop, and a song
    `position`.

    The playhead scans the timeline forward as a `clock` advances, rendering each
    item on a `destination` (a `Server` for OSC, a `MidiServer` for MIDI — the
    same seam as the rest of the client). The forward scan is what `play` runs;
    the random access lives at the boundaries — `play(at=…)` and `locate(beat)`
    re-seek the cursor by time, which a forward-only routine could never do.

    Timing rides the clock's logical time like everything else, so a playhead
    inherits `quant` (start on a bar), `lock_to` (sample-exact) and
    `join_transport` (the shared grid) for free.

    A pass ends on its own when the scan reaches the end of the timeline:
    `playing` goes False and `finished` says the end is why, so a transport
    reads the end off the playhead instead of timing it.

    Args:
        timeline: the `Timeline` to play.
        clock: the `TempoClock` that drives it (start it for live playback;
            `render` it for offline).
        destination: where items go — a `Server` or a `MidiServer`.
    """

    def __init__(self, timeline: Timeline, clock, destination):
        self.timeline = timeline
        self.clock = clock
        self.destination = destination
        self._running = False
        self._finished = False     # the scan ran off the end (set by the feeder)
        self._epoch = 0            # invalidates an in-flight feeder on stop/locate
        self._routine = None
        self._loop = None          # (start, end) in beats, or None
        self._start_beat = 0.0     # the beat the current run started from
        self._pos_beat = 0.0       # timeline beat at the last wake
        self._pos_clock = None     # clock beat at the last wake (for interpolation)
        self._follow = None        # (OscFunc, OscReceiver | None) when following a transport

    # ---- transport ----

    def play(self, at: float = 0.0, quant=None):
        """Start (or restart) playback from beat ``at``, snapping the start to a
        ``quant`` beat boundary of the clock's grid (a bar). Re-seeks the cursor
        to ``at`` by time, so it works as a locate-and-play."""
        self._start_beat = float(at)
        self._pos_beat = float(at)
        self._pos_clock = None
        self._running = True
        self._finished = False
        self._epoch += 1
        epoch = self._epoch
        if self._routine is not None:
            self.clock.unsched(self._routine)
        self._routine = Routine(lambda: self._feed(epoch))
        self.clock.play(self._routine, quant)
        return self

    def stop(self):
        """Halt the playhead. Items already rendered keep sounding (their
        releases are scheduled); no further items are played."""
        self._running = False
        self._finished = False     # halted by hand, not ended
        self._epoch += 1
        if self._routine is not None:
            self.clock.unsched(self._routine)
            self._routine = None
        return self

    def locate(self, beat: float):
        """Seek the playhead to ``beat``. While playing, restarts the scan from
        there (random access); while stopped, just sets where the next `play`
        begins."""
        if self._running:
            self.play(at=beat)
        else:
            self._start_beat = float(beat)
            self._pos_beat = float(beat)
            self._finished = False   # seeking away from the end leaves it behind
        return self

    def loop(self, start: float, end: float):
        """Loop the half-open beat window ``[start, end)``: when the scan reaches
        ``end`` it wraps back to ``start``. Set before or during play."""
        self._loop = (float(start), float(end))
        return self

    def unloop(self):
        """Stop looping; the scan plays through to the end."""
        self._loop = None
        return self

    @property
    def scanned_at(self):
        """The **clock** beat the scan last woke on — the origin `position`
        interpolates from, and, once the scan has drained, the beat at which its
        last item was rendered. ``None`` before the first wake.

        A transport reads it to keep a cursor moving after the scan is over: the
        piece ends where the last item does, which is a stretch of time later.
        """
        return self._pos_clock

    def position(self) -> float:
        """The current song position, in beats. Interpolated from the clock
        between items while playing; the start/last-seek beat while stopped."""
        if not self._running or self._pos_clock is None:
            return self._pos_beat
        pos = self._pos_beat + (self.clock.beats() - self._pos_clock)
        if self._loop is not None:
            start, end = self._loop
            span = end - start
            if span > 0 and pos >= end:
                pos = start + (pos - start) % span
        return pos

    @property
    def playing(self) -> bool:
        """Whether the scan is running. It goes False on `stop` **and** when the
        scan reaches the end of the timeline, so a transport can poll this one
        flag instead of comparing `position` against a length it computed
        itself."""
        return self._running

    @property
    def finished(self) -> bool:
        """Whether the scan ran off the end of the timeline, as opposed to being
        halted by hand (`stop`) or still playing. It is the *scan* that ended: a
        `loop` never ends, and the last item keeps sounding for its own length —
        the playhead schedules items, it does not wait for them."""
        return self._finished

    # ---- follow a server's shared transport (DAW conductor) ----

    def follow_transport(self, server, recv=None, quant=None):
        """Make this playhead obey a ``server``'s shared transport: when a
        conductor calls `transport_play` / `transport_stop` /
        `transport_locate` on the server, the server broadcasts the new state and
        this playhead rolls / halts / seeks to match — so several clients run in
        lockstep on the shared grid.

        It registers ``/server_notify`` (so the server's `/transport_query.reply` pushes
        arrive) and an `clausters.responders.OscFunc` on ``/transport_query.reply``
        that drives this playhead, then applies the current state once. Pass a
        started `clausters.base.OscReceiver` as ``recv`` (it must be subscribable
        on its own socket); one is created if omitted. ``quant`` snaps each
        rolling start to a beat boundary of the shared grid, so all followers
        land together. Release with `unfollow_transport`. Returns ``self``.

        Beat-aligned in plain wall-clock mode; sample-exact when the clock is
        also `lock_to` the server (see the timing docs)."""
        from ..base import OscReceiver
        from ..responders import OscFunc

        owns_recv = recv is None
        if recv is None:
            recv = OscReceiver().start()
        recv.send(server.target, "/server_notify", 1)

        def on_transport(msg, time, src):
            # msg == ["/transport_query.reply", origin, tempo, defined, playing, position]
            if len(msg) < 6 or not int(msg[3]):
                return
            playing, position = int(msg[4]), float(msg[5])
            if playing:
                self.play(at=position, quant=quant)
            else:
                self.stop()
                self.locate(position)

        func = OscFunc(on_transport, "/transport_query.reply", recv=recv)
        self._follow = (func, recv if owns_recv else None)

        state = server.transport_state()
        # Gated on the **grid**, not on the state: the state is always there
        # now, but a playhead runs on beats, and `position` is 0 until a grid
        # says what a beat is. Applying that would locate to 0 on a server
        # whose transport is being driven in samples.
        if state["tempo"] is not None:
            if state["playing"]:
                self.play(at=state["position"], quant=quant)
            else:
                self.locate(state["position"])
        return self

    def unfollow_transport(self):
        """Stop following a server transport (see `follow_transport`): frees the
        responder and closes the receiver it created. Returns ``self``."""
        if self._follow is not None:
            func, owned_recv = self._follow
            func.free()
            if owned_recv is not None:
                owned_recv.close()
            self._follow = None
        return self

    # ---- the feeder: a cursor walk fed to the clock ----

    def _feed(self, epoch):
        tl = self.timeline
        cursor = tl.index_at(self._start_beat)
        prev = self._start_beat
        while self._running and epoch == self._epoch:
            self._pos_beat = prev
            self._pos_clock = self.clock.beats()
            if self._loop is not None:
                start, end = self._loop
                if cursor >= len(tl) or tl[cursor][0] >= end:
                    tail = end - prev
                    if tail > 0:
                        yield tail
                    cursor = tl.index_at(start)
                    prev = start
                    continue
            if cursor >= len(tl):
                # Drained: the pass is over, and the transport driving it has to
                # know without polling a length of its own. The feeder runs on
                # the clock thread, so it records the end rather than announcing
                # it -- `playing` goes False, `position` freezes on the last item.
                self._running = False
                self._finished = True
                return
            beat, item = tl[cursor]
            wait = beat - prev
            if wait > 0:
                yield wait
                if not (self._running and epoch == self._epoch):
                    return
                prev = beat
                self._pos_beat = prev
                self._pos_clock = self.clock.beats()
            item.play(self.destination)
            cursor += 1
