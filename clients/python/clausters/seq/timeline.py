"""Timelines: a plan in logical time, played by itself (random-access sequencing).

The counterpart to the generative layer (`clausters.base.stream.Routine`,
`clausters.seq.pattern.Pbind`). A `Routine` is a forward-only generator: its
musical state lives in the generator's locals, so it cannot be *seeked*. A
`Timeline` is the opposite -- an **editable list of timed items kept sorted by
beat**, with its own tempo map and random access by time (`index_at`, `range`).
That is what makes DAW-style transport controls possible, and they are the
timeline's own: **play / pause / stop / locate / loop**, on a clock of its own
or on a server's transport (`Timeline.transport`).

An *item* is anything that can render itself on a destination -- it has a
`play(destination)` method. `clausters.seq.event.Event` already is one (it plays
a note on a `Server` or a `MidiServer` -- the same double dispatch the patterns
use), so a timeline of `Event`s renders to OSC *or* MIDI by which destination
the timeline plays on, exactly like the rest of the client. `OscItem` and
`MidiItem` wrap a raw OSC message or MIDI bytes, so a timeline can also be a
plain, editable OSC/MIDI score.

This layer is **client-side** while a timeline plays on its own clock: each has
its own local transport, and several clients phase-align through `quant`. On a
**server transport** (`Timeline.transport`) the verbs are the transport's own
commands and the plan rides the transport's clock, so one conductor's
play/stop/locate drives every timeline on it.
"""

import bisect
import math

from .. import _native
from ..base.main import main
from ..base.stream import Routine, StopStream


class _Entry:
    """One timed item on a timeline. A stable object so it can be `remove`d or
    `move`d by identity after other edits shift positions."""

    __slots__ = ("beat", "item")

    def __init__(self, beat, item):
        self.beat = beat
        self.item = item


class OscItem:
    """A raw OSC message ``(addr, *args)`` as a timeline item: rendering it sends
    the message at the timeline's current logical beat through a `Server`."""

    def __init__(self, addr, *args):
        self.addr = addr
        self.args = args

    def play(self, destination):
        destination.send_bundle((self.addr, *self.args))


class MidiItem:
    """Raw MIDI bytes as a timeline item: rendering it emits the message at the
    timeline's current logical beat through a `MidiServer`."""

    def __init__(self, message):
        self.message = bytes(message)

    def play(self, destination):
        destination.send_message(self.message)


#: The key that names a raw OSC message in an item's data, and the one that
#: names raw MIDI bytes. An `clausters.seq.event.Event` carries neither -- it is
#: its own parameters -- so what an item *is* is told apart by which of the two
#: keys is there, and by neither being there.
OSC_KEY = "osc"
MIDI_KEY = "midi"


def item_data(item) -> "dict | None":
    """One timeline item as plain, JSON-able data -- or ``None`` for an item this
    has no description of.

    **One description, because two seams need it.** A document writes a
    timeline's items as the configuration of a placed clang, and the editing
    domain hands them across the crate's ``events`` vocabulary as an event's
    opaque ``data``; the two are the same question -- *what is this item, written
    down* -- and answering it twice is how a marker comes back from one of them
    as a note.

    An `Event` is a `dict` and travels as itself. An `OscItem` and a `MidiItem`
    are not, and each names itself with its own key (`OSC_KEY`, `MIDI_KEY`),
    which is what a reader tells them apart by.
    """
    if isinstance(item, OscItem):
        return {OSC_KEY: str(item.addr), "args": list(item.args)}
    if isinstance(item, MidiItem):
        return {MIDI_KEY: list(item.message)}
    if isinstance(item, dict):
        return dict(item)
    return None


def item_from_data(data):
    """The item `item_data` wrote: an `OscItem`, a `MidiItem`, or the
    `clausters.seq.event.Event` anything else is."""
    from .event import Event

    data = dict(data or {})
    if OSC_KEY in data:
        return OscItem(str(data[OSC_KEY]), *(data.get("args") or ()))
    if MIDI_KEY in data:
        return MidiItem(bytes(int(b) & 0xFF for b in data[MIDI_KEY]))
    return Event(data)


class Timeline:
    """A plan in logical time: ``(beat, item)`` kept sorted by beat, with random
    access by time, its own tempo map, and the verbs that play it.

    Items are kept in beat order (a stable insert preserves the order of items
    added at the same beat, e.g. a note-off before a re-trigger). Edit it
    freely -- `add`, `remove`, `move`, `clear` -- and read ranges of it by time --
    `index_at`, `range`, `at`. `add` returns a handle (an opaque entry) you pass
    back to `remove`/`move`, so edits stay correct as other inserts shift
    indices.

    **Its tempo is its own.** `map` is the timeline's `TempoMap`, the plan of
    how its beats fall on seconds, and it is data like the items: edited on the
    map, saved with the timeline. There is no `set_tempo` here, because no clock
    is handled: a timeline plays itself (`play`, `locate`, `pause`, `stop`,
    `loop`) on a clock of its own that is born on its beat 0, so a clock beat
    *is* a timeline beat.

    **An item is anything playable**: an `Event`, an `OscItem`/`MidiItem`, an
    `Automation`, an event pattern, a `Routine` -- and **another timeline**, which its
    parent plays when it reaches it. Each timeline keeps its own units: a
    child's beats go to seconds through its own map, so siblings at different
    tempi start together by construction. One tree plays on one engine, the
    root's; a child stays an object of its own, and played by itself it plays
    on its own clock.

    A timeline is stateful, so **one instance has at most one parent**: adding
    one that already has a parent is refused (`copy` makes an independent
    one), and so is adding an ancestor, which would be a cycle.

    Args:
        items: optional iterable of ``(beat, item)`` to seed the timeline.
        tempo: the constant tempo of a new map, in beats per second.
        tempo_map: a `TempoMap` to hold instead of building one.
    """

    def __init__(self, items=None, tempo: float = 1.0, tempo_map=None):
        self._entries = []
        #: the timeline this one is an item of, or ``None``.
        self.parent = None
        self._map = tempo_map if tempo_map is not None else _native.TempoMap(float(tempo))
        self._player = None
        self._transport = None
        #: **Where this timeline's beat 0 falls on the transport**, in seconds
        #: of the transport's position -- the transport's axis is physical, so
        #: the offset is too. Only read in transport mode.
        self.transport_at = 0.0
        if items is not None:
            for beat, item in items:
                self.add(beat, item)

    @property
    def map(self):
        """The timeline's tempo map: how its beats fall on seconds. Editable
        data -- write a tempo change on it (`TempoMap.push`, `ramp`, `env`) and
        what plays follows it."""
        return self._map

    @map.setter
    def map(self, tempo_map):
        self._map = tempo_map
        if self._player is not None:
            self._player.clock.map = tempo_map

    # ---- editing ----

    def add(self, beat, item):
        """Insert ``item`` at ``beat`` (kept sorted); returns an entry handle.

        A timeline as ``item`` becomes this one's child: refused if it already
        has a parent (its `copy` has none) or if it is this timeline or one of
        its ancestors. A value pattern is refused: it is the definition of a
        generator and does not play -- an `EventPattern` does."""
        from .pattern import EventPattern, Pattern

        if isinstance(item, Pattern) and not isinstance(item, EventPattern):
            raise TypeError(
                f"a {type(item).__name__} of values does not play, so it is not a "
                f"timeline item: a pattern plays when its values are events (a "
                f"Pbind, or a list pattern of event patterns only)"
            )
        if isinstance(item, Timeline):
            self._check_child(item)
            item.parent = self
        entry = _Entry(float(beat), item)
        bisect.insort(self._entries, entry, key=lambda e: e.beat)
        return entry

    def remove(self, entry):
        """Remove an entry returned by `add` (by identity)."""
        self._entries.remove(entry)
        if isinstance(entry.item, Timeline):
            entry.item.parent = None
        return self

    def move(self, entry, new_beat):
        """Move an entry to ``new_beat``, keeping the timeline sorted."""
        self._entries.remove(entry)
        entry.beat = float(new_beat)
        bisect.insort(self._entries, entry, key=lambda e: e.beat)
        return entry

    def clear(self):
        """Drop every item."""
        for e in self._entries:
            if isinstance(e.item, Timeline):
                e.item.parent = None
        self._entries.clear()
        return self

    def replace(self, items):
        """Replace the whole contents with ``items`` (``(beat, item)`` pairs),
        **in one step**.

        The step is what this is for. Clearing and re-adding leaves the timeline
        empty in between, which is invisible in a single-threaded script and
        very visible to anything reading it while an event loop applies an edit:
        a rebuild that outlasts CPython's switch interval was seen half-done in
        87.7% of reads at 4000 notes. Building the new order first and binding
        it in one assignment means a reader either sees the timeline before the
        edit or after it -- iteration binds the list once, so a read already in
        progress finishes on the order it started with.
        """
        entries = [_Entry(float(beat), item) for beat, item in items]
        children = [e.item for e in entries if isinstance(e.item, Timeline)]
        for child in children:
            self._check_child(child, replacing=True)
        entries.sort(key=lambda e: e.beat)
        for e in self._entries:
            if isinstance(e.item, Timeline):
                e.item.parent = None
        for child in children:
            child.parent = self
        self._entries = entries
        return self

    def _check_child(self, item, replacing=False):
        """Refuses a child that already has another parent, or that is this
        timeline or one of its ancestors."""
        if item.parent is not None and not (replacing and item.parent is self):
            raise ValueError(
                "this timeline already has a parent; a timeline is stateful "
                "and plays in one place -- add item.copy() instead")
        node = self
        while node is not None:
            if node is item:
                raise ValueError("a timeline cannot contain itself or an ancestor")
            node = node.parent

    def copy(self) -> "Timeline":
        """An independent timeline with the same plan: its own tempo map, its
        children copied (recursively), and the other items shared -- an event
        or a message is a value, and a routine item is played fresh on every
        pass anyway. It is stopped at beat 0 and has no parent."""
        new = Timeline(tempo_map=self._map.copy())
        new._entries = [
            _Entry(e.beat, e.item.copy() if isinstance(e.item, Timeline) else e.item)
            for e in self._entries
        ]
        for e in new._entries:
            if isinstance(e.item, Timeline):
                e.item.parent = new
        return new

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
        """The cursor (index) of the first item at or after ``beat`` -- the seek
        primitive `play(at=...)` and `locate` start from."""
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
        """The timeline's logical length, in its own beats: the beat of the last
        item (0.0 when empty), **extended by any child that lasts longer**.

        A parent is never shorter than what it holds. A child's length is in
        the child's beats, so it goes to seconds through the child's map and
        back to this timeline's beats through this one's, from the beat it is
        placed at. A looping child never ends."""
        end = 0.0
        for e in self._entries:
            if isinstance(e.item, Timeline):
                child = e.item
                if child._looping():
                    return math.inf
                secs = self._map.secs_at(e.beat) + child._map.secs_at(child.duration())
                end = max(end, self._map.beats_at(secs))
            else:
                end = max(end, e.beat)
        return end

    def _looping(self) -> bool:
        return self._player is not None and self._player.loop is not None

    def __len__(self):
        return len(self._entries)

    def __getitem__(self, i):
        e = self._entries[i]
        return (e.beat, e.item)

    def __iter__(self):
        return ((e.beat, e.item) for e in self._entries)

    # ---- playing ----

    @property
    def transport(self):
        """The server whose **transport** plays this timeline, or ``None`` --
        the ordinary case -- for its own clock.

        One mode per root, and the same verbs in both: `play`, `pause`, `stop`
        and `locate` are the transport's own commands here, exactly as the
        multitrack's playback uses them, and the timeline's items are planned
        onto the transport's clock (`/sched_atTransport`) from the position it
        is at. Setting it needs a **governed group** bound
        (`clausters.defs.Server.transport_group`), since that is what makes a
        transport own the nodes it plays -- and what the timeline's synths are
        placed under, so a pause freezes them with the sound.

        Assigning halts whatever was playing: a timeline plays in one place.
        """
        return self._transport

    @transport.setter
    def transport(self, server):
        if self._player is not None:
            self._player.halt()
            self._player.close()
            self._player = None
        if server is not None:
            state = server.transport_state()
            if state.get("group") is None:
                raise ValueError(
                    "a timeline on a transport needs a governed group: bind one "
                    "with server.transport_group(group) -- the transport owns "
                    "the nodes it plays")
        self._transport = server
        if server is not None:
            # The mode **is** the following: with the plan on the transport's
            # own clock, a roll and a freeze need nothing from here, and what a
            # locate needs is the re-cue -- so the broadcasts are listened to
            # from the moment the timeline is on the transport, whoever drives
            # it.
            self._player_for().follow()

    def play(self, at: "float | None" = None, quant=None, destination=None):
        """Play the timeline from beat ``at``, on a clock of its own.

        ``at=None`` resumes where `pause` left it (beat 0 the first time).
        ``quant`` starts it on the next multiple of ``quant`` beats of the
        **ambient** clock (the routine's, the session's), which is how it lands
        on another clock's bar. ``destination`` is where items play (a
        `Server`, a `MidiServer`); ``None`` resolves the ambient server when an
        item needs one.

        Playing a child on its own plays only it, on its own clock; its parent
        is not involved. Returns ``self``."""
        player = self._player_for()
        if destination is not None:
            player.destination = destination
        if at is None:
            player.resume(quant)
        else:
            player.mark = float(at)
            player.play(float(at), quant)
        return self

    def locate(self, beat: float):
        """Move to ``beat``. Playing, the timeline goes on from there: what is
        sounding keeps its own release, children are entered at the beat that
        corresponds, and an onset the new position has passed is not
        recovered. Stopped, it is where the next `play` starts. Returns
        ``self``."""
        self._player_for().locate(float(beat))
        return self

    def pause(self):
        """Halt, holding the position: `play` with no ``at`` resumes there.
        Returns ``self``."""
        if self._player is not None:
            self._player.halt()
        return self

    def stop(self):
        """Halt and go back to the mark: the beat of the last `play` given an
        ``at``, or of the last `locate` made while stopped (a resume does not
        move it). Returns ``self``."""
        if self._player is not None:
            self._player.halt()
            self._player.hold(self._player.mark)
        return self

    def loop(self, start: float, end: float):
        """Loop the half-open beat window ``[start, end)``: reaching ``end``
        goes on from ``start``, with physical time running on. A child longer
        than the window loops over its part that corresponds to it. Set before
        or during play. Returns ``self``."""
        self._player_for().loop = (float(start), float(end))
        return self

    def unloop(self):
        """Stop looping. Returns ``self``."""
        if self._player is not None:
            self._player.loop = None
        return self

    def position(self) -> float:
        """Where the timeline is, in its beats.

        On a server transport it is what this client last heard (a verb it
        sent, a broadcast, a `refresh`) rather than a round trip, the way
        `clausters.gui.PlayheadSync` reads the transport."""
        return 0.0 if self._player is None else self._player.position()

    def refresh(self):
        """Ask where the transport is and keep the answer; returns ``self``.

        Only a timeline **on a transport** has anything to ask: on its own
        clock the position is here. (In the web client this is a promise, for
        the reason every request there is one.)"""
        if self._player is not None:
            self._player.refresh()
        return self

    @property
    def playing(self) -> bool:
        """Whether it is playing. False after `pause`/`stop` and once the end
        is reached."""
        return self._player is not None and self._player.running

    @property
    def finished(self) -> bool:
        """Whether it stopped because it reached its end (a loop never does)."""
        return self._player is not None and self._player.finished

    def _player_for(self):
        if self._player is None:
            self._player = (_Player(self) if self._transport is None
                            else _TransportPlayer(self))
        return self._player


class _ClockView:
    """A timeline's clock as what it plays sees it, while the root's clock
    wakes the tree.

    An item of a child measures in the **child's** beats -- an event's sustain,
    an automation's length, a routine's yields, a pattern's durations -- but
    only the root's clock runs. So the node hands each item this view: its
    beats and conversions are the child's, placed on the root's time by the
    node's origin, and what it schedules goes onto the root's clock at the beat
    that corresponds. Timetags and sessions are the root clock's."""

    def __init__(self, node):
        self._node = node

    @property
    def _root(self):
        return self._node.player.clock

    # the child's beats on the root's axis of seconds
    def beats2secs(self, beats):
        return self._node.origin + self._node.timeline._map.secs_at(beats)

    def secs2beats(self, secs):
        return self._node.timeline._map.beats_at(secs - self._node.origin)

    def root_beat(self, beats):
        if self._node.is_root:
            return beats
        return self._node.player.root_beat(self.beats2secs(beats))

    def local_beat(self, root_beat):
        if self._node.is_root:
            return root_beat
        return self.secs2beats(self._node.player.root_secs(root_beat))

    def beats(self):
        routine = main.current_routine
        if getattr(routine, "clock", None) is self:
            return routine._logical_beat
        return self.local_beat(self._root.beats())

    @property
    def sched_axis(self):
        """The function a `clausters.defs.Server` stamps a bundle with when the
        timeline plays on a **server transport**: seconds of this node's axis ->
        a sample of the transport's clock. ``None`` on a timeline playing on its
        own clock, where the ordinary timetag or ``/sched_at`` path applies."""
        return self._node.player.sched_axis

    @property
    def tempo(self):
        return self._node.timeline._map.tempo_at(self.beats())

    @property
    def map(self):
        return self._node.timeline._map

    def sched(self, delay_beats, item):
        self._schedule(self.beats() + float(delay_beats), item)

    def sched_abs(self, beat, item):
        self._schedule(float(beat), item)

    def play(self, routine, quant=None):
        self._schedule(self.beats(), routine)
        return routine

    def unsched(self, item):
        wrapper = self._node.player.wrappers.pop(id(item), None)
        if wrapper is not None:
            self._root.unsched(wrapper)

    def _schedule(self, beat, item):
        player = self._node.player
        wrapper = Routine(_translated(item, self))
        player.wrappers[id(item)] = wrapper
        player.owned.append((self._node, wrapper))
        self._root.sched_abs(self.root_beat(beat), wrapper)

    # what a Server and a session read, from the root clock
    def __getattr__(self, name):
        if name in ("timebase", "pacing_origin", "start_time", "session", "name",
                    "rolling", "frozen"):
            # ``None`` where there is no clock behind the view: a timeline on a
            # server transport has none, and what it needs instead is the axis
            # above.
            return getattr(self._root, name, None)
        raise AttributeError(name)


def _translated(item, view):
    """The body of the routine the root clock wakes for ``item``, an item
    scheduled on a child's view: each wake runs ``item`` at the child's beat
    that corresponds, and turns the child beats it yields into root beats."""
    def body():
        while True:
            me = main.current_routine
            root_beat = me._logical_beat
            local = view.local_beat(root_beat)
            saved = (me.clock, me._logical_beat)
            me.clock, me._logical_beat = view, local
            try:
                if hasattr(item, "next"):
                    item.clock = view
                    item._logical_beat = local
                    try:
                        delta = item.next(view)
                    except StopStream:
                        return
                else:
                    delta = item()
            finally:
                me.clock, me._logical_beat = saved
            if delta is None:
                return
            yield view.root_beat(local + float(delta)) - root_beat
    return body


class _Node:
    """One timeline of a playing tree: where its beat 0 falls on the root's
    axis of seconds, its cursor, and the children it has entered."""

    def __init__(self, player, timeline, origin, beat):
        self.player = player
        self.timeline = timeline
        self.origin = origin
        self.is_root = timeline is player.timeline
        self.view = _ClockView(self)
        self.children = []
        self.enter(beat)

    def enter(self, beat):
        """Place the cursor at ``beat``: the next onset at or after it, and the
        children the beat is inside, entered at the beat that corresponds."""
        tl = self.timeline
        self.cursor = tl.index_at(beat)
        self.children = []
        secs = self.origin + tl._map.secs_at(beat)
        for e in tl._entries[:self.cursor]:
            if not isinstance(e.item, Timeline):
                continue
            child = e.item
            origin = self.origin + tl._map.secs_at(e.beat)
            local = child._map.beats_at(secs - origin)
            if local < child.duration() or child._looping():
                self.children.append(_Node(self.player, child, origin, local))

    def next_due(self):
        """``(seconds, action, beat)`` of the next thing to do in this subtree,
        or ``None`` when it has ended. ``beat`` is the root's exact beat when
        the action is the root's own, and ``None`` when it has to be read
        through the maps."""
        tl = self.timeline
        best = None
        if self.cursor < len(tl._entries):
            e = tl._entries[self.cursor]
            best = (self.origin + tl._map.secs_at(e.beat), self._onset,
                    e.beat if self.is_root else None)
        loop = None if self.is_root or tl._player is None else tl._player.loop
        if loop is not None:
            end_secs = self.origin + tl._map.secs_at(loop[1])
            if best is None or best[0] >= end_secs:
                best = (end_secs, self._wrap, None)
        for child in self.children:
            due = child.next_due()
            if due is not None and (best is None or due[0] < best[0]):
                best = due
        return best

    def _wrap(self):
        start, end = self.timeline._player.loop
        tl = self.timeline
        self.player.release(self)
        self.origin += tl._map.secs_at(end) - tl._map.secs_at(start)
        self.enter(start)

    def _onset(self):
        tl = self.timeline
        e = tl._entries[self.cursor]
        self.cursor += 1
        item = e.item
        if isinstance(item, Timeline):
            origin = self.origin + tl._map.secs_at(e.beat)
            self.children.append(_Node(self.player, item, origin, 0.0))
            return
        self.player.render(self, e.beat, item)

    def prune(self):
        self.children = [c for c in self.children if c.next_due() is not None]
        for c in self.children:
            c.prune()


class _TransportPlayer:
    """A timeline played on a **server transport**: the same verbs, carried out
    as the transport's own commands, and the tree planned onto the transport's
    clock instead of woken on a clock of its own.

    The transport is state in physical time -- frozen nodes, a locate and a
    loop exact in the engine -- and a timeline is a plan of discrete events in
    logical time. So nothing here drives time: `play`, `pause`, `stop` and
    `locate` are `/transport_play`, `/transport_stop` and
    `/transport_locateSample`, and what this adds is the **plan**: every item
    from a position, stamped on the transport's clock through the timeline's own
    map, so a pause holds the queue with the sound. A locate clears the
    transport queue (`sched_clear("transport")`) and re-plans from the new
    position, `latency` ahead so nothing regenerated is late.
    """

    #: What cannot be planned from a position: a routine and a pattern are
    #: forward-only, so there is nothing to re-generate a pass from.
    _unplayable = "a routine or a pattern cannot be planned from a position"

    def __init__(self, timeline):
        self.timeline = timeline
        self.destination = None
        self.mark = 0.0
        self.finished = False
        self.root = None
        self.owned = []
        self.wrappers = {}
        self.clock = None
        #: ``(base_sample, base_secs, rate)`` while a plan is being written:
        #: what turns a node's seconds into a sample of the transport's clock.
        self._stamp = None
        self._held = 0.0
        #: what the last broadcast said, so a conductor's own play and locate
        #: are told from this client's.
        #: the position sample this client itself cued, so its own locate's
        #: broadcast is not read as somebody else's.
        self._cued = None
        self._follow = None
        #: The transport as this client last heard it -- from a verb it sent, a
        #: broadcast, or `refresh`. Read rather than asked for, the way
        #: `clausters.gui.PlayheadSync` reads the transport: asking is a round trip
        #: and reading a position is not.
        self._reported = {"playing": False, "position_sample": 0}

    # the root's axis, as the clock player's
    def root_secs(self, beat):
        return self.timeline._map.secs_at(beat)

    def root_beat(self, secs):
        return self.timeline._map.beats_at(secs)

    @property
    def loop(self):
        return None

    @loop.setter
    def loop(self, span):
        raise ValueError(
            "a loop on a transport is the engine's, and a timeline's events "
            "would have to be re-cued on every wrap: loop it on its own clock "
            "(timeline.transport = None) or loop the transport itself")

    @property
    def sched_axis(self):
        if self._stamp is None:
            return None
        base_sample, base_secs, rate = self._stamp
        return lambda secs: int(round(base_sample + (secs - base_secs) * rate))

    @property
    def server(self):
        return self.timeline._transport

    def _state(self):
        """Ask the server where the transport is, and remember it."""
        self._reported = self.server.transport_state()
        return self._reported

    def refresh(self):
        """Ask the server for the transport's state and keep it; returns
        ``self``. What `position` and `playing` answer from."""
        self._state()
        return self

    def _rate(self):
        return float(self.server.query_info().nominal_sample_rate)

    def _timeline_secs(self, state):
        """Where the transport is, in seconds of **this timeline's** axis."""
        return state.get("position_sample", 0) / self._rate() - self.timeline.transport_at

    def position(self):
        """Where the transport is, in this timeline's beats, as this client last
        heard it -- a wrap inside the transport's loop and a locate some other
        client sent are both where it says, since both are broadcast. `refresh`
        asks again."""
        return self.root_beat(max(self._timeline_secs(self._reported), 0.0))

    @property
    def running(self):
        return bool(self._reported.get("playing"))

    def hold(self, beat):
        self.locate(beat)
        return self

    def resume(self, quant=None):  # noqa: D401
        """Roll again with **nothing re-planned**: a pause froze the transport's
        queue with the transport, so what was queued is still queued in its exact
        relative place -- which is the whole difference between a resume and a
        play here."""
        self._refuse_quant(quant)
        self.server.transport_play()
        self._reported["playing"] = True
        return self

    def play(self, at, quant=None):
        self._refuse_quant(quant)
        self.locate(at, cue=False)
        self.server.transport_play()
        self._reported["playing"] = True
        self._plan(at)
        return self

    @staticmethod
    def _refuse_quant(quant):
        if quant:
            raise ValueError(
                "quant is the client clock's: on a transport the start is the "
                "transport's own, so locate where you want it and roll")

    def locate(self, beat, cue: bool = True):
        beat = float(beat)
        self._held = beat
        server = self.server
        rate = self._rate()
        server.sched_clear("transport")
        sample = int(round((self.timeline.transport_at + self.root_secs(beat)) * rate))
        server.transport_locate_sample(sample)
        self._reported["position_sample"] = sample
        self._cued = sample
        if cue and self._reported.get("playing"):
            self._plan(beat)
        return self

    def halt(self):
        self._held = self.position()
        self.server.transport_stop()
        self._reported["playing"] = False
        return self

    # ---- following whoever drives the transport ----

    def follow(self):
        """Listen to the transport's broadcasts, so a **conductor** drives this
        timeline too: whoever calls the transport's verbs -- this client, a
        second one, the multitrack editor next door -- makes it roll, freeze and
        re-plan.

        That is the whole of following now: the plan rides the transport's own
        clock, so a roll and a freeze need nothing from here; what a locate
        needs is the re-cue, and this is where a locate somebody else sent
        arrives. Registered once per transport (see `close`)."""
        if self._follow is not None:
            return self
        from ..base._oscinterface import OscReceiver

        recv = OscReceiver().start()
        recv.add(self._broadcast)
        recv.send(self.server.target, "/server_notify", 1)
        self._follow = recv
        return self

    def close(self):
        """Give up the broadcast listener, if any."""
        if self._follow is not None:
            self._follow.close()
            self._follow = None
        return self

    def _broadcast(self, addr, args, when, src):
        if addr != "/transport_query.reply" or len(args) < 8:
            return
        playing, position = bool(int(args[3])), int(args[7])
        self._reported = {**self._reported, "playing": playing,
                          "position_sample": position,
                          "transport_sample": int(args[6])}
        if not playing:
            return
        rate = self._rate()
        # What this client cued itself is not news: its own locate broadcasts
        # too, and re-planning on the echo would write the plan twice.
        if self._cued is not None and abs(position - self._cued) < 0.05 * rate:
            return
        self._cued = position
        # Somebody else drove it -- a conductor's play or locate, a loop's wrap.
        # The engine does not clear the queue on a locate (a client's own does),
        # so the re-cue is the pair: clear what was queued for where we were,
        # and plan again from where it says.
        self.server.sched_clear("transport")
        self._plan(self.root_beat(max(position / rate - self.timeline.transport_at, 0.0)))

    def release(self, node):
        """Nothing is owned here: a plan holds no routines, and what is queued
        is the transport's (cleared by a locate)."""
        self.owned = []

    def render(self, node, beat, item):
        from .pattern import Pattern

        if isinstance(item, (Routine, Pattern)):
            raise ValueError(f"{type(item).__name__} at beat {beat}: {self._unplayable}")
        # The transport's own server when nothing else was named: a plan a
        # broadcast writes runs on the receiver's thread, where no session is
        # ambient, and the server whose transport this is is the one place the
        # items can be meant for.
        destination = self.destination if self.destination is not None else self.server
        me = main.current_routine
        saved = (me.clock, me._logical_beat)
        me.clock, me._logical_beat = node.view, beat
        try:
            item.play(destination)
        finally:
            me.clock, me._logical_beat = saved

    def _plan(self, at):
        """Write the whole tree from ``at`` onto the transport's clock.

        The walk is the clock player's -- the same nodes, the same entry rule,
        the same units -- with the waiting taken out: there is no time to pass
        here, since every item names a sample of a clock the engine is running.
        """
        state = self._state()
        rate = self._rate()
        base = state["transport_sample"] / rate
        self._stamp = (base * rate, self.root_secs(at), rate)
        stub = _PlanMoment()
        previous, main.current_routine = main.current_routine, stub
        try:
            self.root = _Node(self, self.timeline, 0.0, float(at))
            self.finished = False
            while True:
                due = self.root.next_due()
                if due is None:
                    break
                stub._logical_beat = due[2] if due[2] is not None else self.root_beat(due[0])
                due[1]()
                self.root.prune()
        finally:
            main.current_routine = previous
            self._stamp = None
        return self


class _PlanMoment:
    """The stand-in for a routine while a plan is written: what a `Moment` reads
    to stamp an item, with no clock running behind it."""

    __slots__ = ("clock", "_logical_beat")

    def __init__(self):
        self.clock = None
        self._logical_beat = 0.0


class _Player:
    """The engine of a timeline played as a root: its hidden clock, born on
    the timeline's beat 0, and the routine that wakes the whole tree on it."""

    #: A timeline on its own clock stamps on the ordinary axis (a timetag, or
    #: `/sched_at` under a sample timebase): there is no transport to name.
    sched_axis = None

    def __init__(self, timeline):
        self.timeline = timeline
        self.clock = None
        self.destination = None
        self.loop = None
        self.mark = 0.0
        self.running = False
        self.finished = False
        self.root = None
        self.owned = []           # (node, routine) started by this pass
        self.wrappers = {}
        self._engine = None
        self._epoch = 0
        self._held = 0.0

    # the root's axis: its beats and their seconds, through its map
    def root_secs(self, beat):
        return self.timeline._map.secs_at(beat)

    def root_beat(self, secs):
        return self.timeline._map.beats_at(secs)

    def _clock_for(self):
        """The hidden clock, which belongs to the session the timeline sounds
        in: made there, on that session's timebase, the first time it plays in
        it -- and made again, at the position it stopped at, when it plays in
        another. A timeline sounding in one session is refused in another."""
        from ..base.clock import TempoClock

        session = main._ambient_session()
        if self.clock is not None and self.clock.session is not session:
            if self.running:
                raise RuntimeError(
                    "this timeline is sounding in another session; stop it there "
                    "before playing it in this one"
                )
            held = self.position()
            self.clock.stop()
            if self.clock.session is not None and self.clock is not self.clock.session.clock:
                self.clock.session.release(self.clock)
            self.clock = None
            self._held = held
        if self.clock is None:
            self.clock = TempoClock(tempo_map=self.timeline._map)
            self.clock.locate(self._held)
        return self.clock

    def position(self):
        if self.running and self.clock is not None:
            return self.clock.beats()
        return self._held

    def hold(self, beat):
        self._held = float(beat)
        if self.clock is not None:
            self.clock.locate(self._held)

    def resume(self, quant=None):
        """Play on from where `halt` left it: on a clock, that is a play from
        the held position."""
        return self.play(self.position(), quant)

    def play(self, at, quant=None):
        clock = self._clock_for()
        self.halt()
        self.finished = False
        delay = 0.0
        if quant:
            ambient = main.resolve_clock()
            if ambient is not None and ambient is not clock:
                now = ambient.beats()
                delay = (ambient.beats2secs(now + ambient._quant_delay(quant))
                         - ambient.beats2secs(now))
        clock.locate(self.root_beat(self.root_secs(at) - delay))
        self.running = True
        self._epoch += 1
        epoch = self._epoch
        self._engine = Routine(lambda: self._run(epoch, at))
        clock.sched_abs(clock.beats(), self._engine)
        if not clock._running:
            clock.start()

    def locate(self, beat):
        if self.running:
            self.play(beat)
        else:
            self.mark = beat
            self.hold(beat)
            self.finished = False

    def halt(self):
        if self.clock is None:
            return
        self._held = self.clock.beats()
        self.running = False
        self._epoch += 1
        if self._engine is not None:
            self.clock.unsched(self._engine)
            self._engine = None
        self.release(None)

    def refresh(self):
        """Nothing to ask: on its own clock the position is here."""
        return self

    def close(self):
        """Nothing to give up: a timeline on its own clock listens to nothing."""
        return self

    def release(self, node):
        """Unschedule the routines a pass started, in ``node``'s subtree (all of
        them for ``None``)."""
        keep = []
        for owner, routine in self.owned:
            if node is None or _within(owner, node):
                self.clock.unsched(routine)
            else:
                keep.append((owner, routine))
        self.owned = keep
        if node is None:
            self.wrappers.clear()

    def render(self, node, beat, item):
        """Play one item of ``node`` at its ``beat``, with the moment and the
        clock set to the node's: an item measures in its own timeline's beats."""
        destination = self.destination
        me = main.current_routine
        saved = (me.clock, me._logical_beat)
        me.clock, me._logical_beat = node.view, beat
        try:
            self._render_on(node.view, item, destination)
        finally:
            me.clock, me._logical_beat = saved

    def _render_on(self, view, item, destination):
        from .pattern import EventPattern

        if isinstance(item, Routine):
            # Fresh on every pass: the item may still be sounding from the last
            # one, or on another clock.
            view.play(Routine(item.func))
            return
        if isinstance(item, EventPattern):
            item.play(view, destination or main.resolve_server())
            return
        item.play(destination if destination is not None else main.resolve_server())

    def _run(self, epoch, at):
        self.root = _Node(self, self.timeline, 0.0, at)
        me = main.current_routine
        if me._logical_beat < at:
            yield at - me._logical_beat
        while self.running and epoch == self._epoch:
            loop = self.loop
            due = self.root.next_due()
            if loop is not None:
                end_secs = self.root_secs(loop[1])
                if due is None or due[0] >= end_secs:
                    wait = loop[1] - me._logical_beat
                    if wait > 0:
                        yield wait
                        if not (self.running and epoch == self._epoch):
                            return
                    self.release(None)
                    self.clock.locate(loop[0])
                    self.root = _Node(self, self.timeline, 0.0, loop[0])
                    continue
            if due is None:
                self._held = me._logical_beat
                self.running = False
                self.finished = True
                return
            beat = due[2] if due[2] is not None else self.root_beat(due[0])
            wait = beat - me._logical_beat
            if wait > 0:
                yield wait
                if not (self.running and epoch == self._epoch):
                    return
            due[1]()
            self.root.prune()


def _within(node, ancestor):
    if node is ancestor:
        return True
    return any(_within(node, child) for child in ancestor.children)
