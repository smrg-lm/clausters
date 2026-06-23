"""Static timelines and a playhead (random-access sequencing).

The counterpart to the generative layer (`clausters.base.stream.Routine`,
`clausters.seq.pattern.Pbind`). A `Routine` is a forward-only generator: its
musical state lives in the generator's locals, so it cannot be *seeked*. A
`Timeline` is the opposite — a **static, editable list of timed items kept
sorted by beat**, with random access by time (`index_at`, `range`). That is what
makes DAW-style transport controls possible: a `Playhead` scans the timeline
forward as the clock advances, and **play / stop / locate / loop** re-seek the
cursor by time at the boundaries.

An *item* is anything that can realize itself on a destination — it has a
`play(destination)` method. `clausters.seq.event.Event` already is one (it plays
a note on a `Server` or a `MidiServer` — the same double dispatch the patterns
use), so a timeline of `Event`s renders to OSC *or* MIDI by which destination
the playhead holds, exactly like the rest of the client. `OscEvent` and
`MidiEvent` wrap a raw OSC message or MIDI bytes, so a timeline can also be a
plain, editable OSC/MIDI score.

This layer is **client-side**: each playhead has its own local transport over
its own timeline, and several clients phase-align through `quant` and the shared
`/transport` grid (see the timing docs). A server-broadcast transport (one
conductor's play/stop/locate driving every client) can layer on top later
without changing this.
"""

import bisect

from ..base.main import main
from ..base.stream import Routine


class _Entry:
    """One timed item on a timeline. A stable object so it can be `remove`d or
    `move`d by identity after other edits shift positions."""

    __slots__ = ("beat", "item")

    def __init__(self, beat, item):
        self.beat = beat
        self.item = item


class OscEvent:
    """A raw OSC message ``(addr, *args)`` as a timeline item: realizing it sends
    the message at the playhead's current logical beat through a `Server`."""

    def __init__(self, addr, *args):
        self.addr = addr
        self.args = args

    def play(self, destination):
        destination.send_bundle((self.addr, *self.args))


class MidiEvent:
    """Raw MIDI bytes as a timeline item: realizing it emits the message at the
    playhead's current logical beat through a `MidiServer`."""

    def __init__(self, message):
        self.message = bytes(message)

    def play(self, destination):
        destination.send_message(self.message)


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
    def from_pattern(cls, pattern, dur=None, tempo: float = 1.0) -> "Timeline":
        """Bounce an event pattern (a `Pbind`) into a static timeline by running
        it offline and recording each event at its logical beat. ``dur`` bounds
        an open-ended pattern (beats); ``None`` drains a finite one fully."""
        from ..base.clock import TempoClock

        timeline = cls()
        recorder = _Recorder(timeline)
        clock = TempoClock(tempo)
        pattern.play(clock, recorder)
        clock.render(until_beat=dur)
        return timeline


class _Recorder:
    """A capture destination: `play_event` appends the event to a timeline at the
    running routine's logical beat instead of sending it (used by
    `Timeline.from_pattern`)."""

    def __init__(self, timeline: Timeline):
        self.timeline = timeline

    def play_event(self, event):
        from .event import Event

        beat = getattr(main.current_tt, "_logical_beat", 0.0) or 0.0
        self.timeline.add(beat, Event(event))
        return None


class Playhead:
    """A transport over a `Timeline`: play / stop / locate / loop, and a song
    `position`.

    The playhead scans the timeline forward as a `clock` advances, realizing each
    item on a `destination` (a `Server` for OSC, a `MidiServer` for MIDI — the
    same seam as the rest of the client). The forward scan is what `play` runs;
    the random access lives at the boundaries — `play(at=…)` and `locate(beat)`
    re-seek the cursor by time, which a forward-only routine could never do.

    Timing rides the clock's logical time like everything else, so a playhead
    inherits `quant` (start on a bar), `lock_to` (sample-exact) and
    `join_transport` (the shared grid) for free.

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
        self._epoch = 0            # invalidates an in-flight feeder on stop/locate
        self._routine = None
        self._loop = None          # (start, end) in beats, or None
        self._start_beat = 0.0     # the beat the current run started from
        self._pos_beat = 0.0       # timeline beat at the last wake
        self._pos_clock = None     # clock beat at the last wake (for interpolation)

    # ---- transport ----

    def play(self, at: float = 0.0, quant=None):
        """Start (or restart) playback from beat ``at``, snapping the start to a
        ``quant`` beat boundary of the clock's grid (a bar). Re-seeks the cursor
        to ``at`` by time, so it works as a locate-and-play."""
        self._start_beat = float(at)
        self._pos_beat = float(at)
        self._pos_clock = None
        self._running = True
        self._epoch += 1
        epoch = self._epoch
        if self._routine is not None:
            self.clock.unsched(self._routine)
        self._routine = Routine(lambda: self._feed(epoch))
        self.clock.play(self._routine, quant)
        return self

    def stop(self):
        """Halt the playhead. Items already realized keep sounding (their
        releases are scheduled); no further items are played."""
        self._running = False
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
        return self._running

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
