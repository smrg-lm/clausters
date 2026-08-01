"""Timeline + Playhead: static, random-access sequencing with transport control.

The `Timeline` edits and time-queries are pure unit tests. The `Playhead` is
driven offline (`clock.render()`) into a recording destination, so play / locate
/ loop are checked deterministically by the logical beats each item lands on —
no server, no real time. `stop` is checked at the queue level (it unscheds the
feeder). `follow_transport` is checked by feeding a simulated `/transport_query.reply`
broadcast over loopback (no live server). The full multi-client lockstep is the
manual E2E in `clients/python/examples/transport_conductor.py`.
"""

import socket
import time
import types

from clausters.base import NetAddr, OscReceiver, TempoClock
from clausters.base import _osclib as osc
from clausters.base.main import main
from clausters.seq import (
    Event,
    MidiEvent,
    OscEvent,
    Pbind,
    Playhead,
    Pseq,
    Timeline,
)


class RecordDest:
    """A destination that records ``(logical_beat, item)`` instead of sending,
    for every rendering path an item can take."""

    def __init__(self):
        self.events = []

    def _beat(self):
        return getattr(main.current_tt, "_logical_beat", 0.0) or 0.0

    def play_event(self, event):
        self.events.append((self._beat(), event))
        return None

    def send_bundle(self, *messages):
        self.events.append((self._beat(), ("osc", messages)))

    def send_message(self, message):
        self.events.append((self._beat(), ("midi", bytes(message))))

    def beats(self):
        return [b for b, _ in self.events]


# ---- Timeline editing ----


def test_timeline_keeps_sorted_and_stable():
    tl = Timeline()
    tl.add(2.0, "c")
    tl.add(0.0, "a")
    tl.add(1.0, "b1")
    tl.add(1.0, "b2")           # same beat: stable, after b1
    assert [item for _, item in tl] == ["a", "b1", "b2", "c"]
    assert [b for b, _ in tl] == [0.0, 1.0, 1.0, 2.0]
    assert tl.duration() == 2.0


def test_timeline_remove_and_move_by_handle():
    tl = Timeline()
    a = tl.add(0.0, "a")
    b = tl.add(1.0, "b")
    tl.move(a, 5.0)             # a jumps past b, stays sorted
    assert [item for _, item in tl] == ["b", "a"]
    tl.remove(b)
    assert [item for _, item in tl] == ["a"]
    assert len(tl) == 1


def test_timeline_quantize_snaps_beats_and_keeps_order():
    tl = Timeline([(0.1, "a"), (0.9, "b"), (1.3, "c")])
    tl.quantize(0.5)
    assert list(tl) == [(0.0, "a"), (1.0, "b"), (1.5, "c")]
    tl.quantize(0.0)            # no grid: a no-op
    assert [b for b, _ in tl] == [0.0, 1.0, 1.5]


def test_timeline_random_access_by_time():
    tl = Timeline([(0.0, "a"), (1.0, "b"), (2.0, "c"), (3.0, "d")])
    assert tl.index_at(1.0) == 1          # first item at or after 1.0
    assert tl.index_at(1.5) == 2
    assert tl.range(1.0, 3.0) == [(1.0, "b"), (2.0, "c")]   # [t0, t1)
    assert tl.at(2.0) == ["c"]


# ---- Playhead, driven offline ----


def _arp():
    return Timeline([
        (0.0, Event(freq=440)),
        (1.0, Event(freq=550)),
        (2.0, Event(freq=660)),
    ])


def test_playhead_plays_in_order_at_the_right_beats():
    clock = TempoClock(1.0)
    dest = RecordDest()
    Playhead(_arp(), clock, dest).play(at=0.0)
    clock.render()
    assert dest.beats() == [0.0, 1.0, 2.0]
    assert [e["freq"] for _, e in dest.events] == [440, 550, 660]


def test_playhead_locate_skips_and_offsets():
    # Start at beat 1: the first item (beat 0) is skipped, and the item at 1
    # sounds now (clock beat 0), the next a beat later.
    clock = TempoClock(1.0)
    dest = RecordDest()
    Playhead(_arp(), clock, dest).play(at=1.0)
    clock.render()
    assert dest.beats() == [0.0, 1.0]
    assert [e["freq"] for _, e in dest.events] == [550, 660]


def test_playhead_loop_wraps():
    clock = TempoClock(1.0)
    dest = RecordDest()
    ph = Playhead(_arp(), clock, dest)
    ph.loop(0.0, 2.0).play(at=0.0)        # window [0, 2): items at 0 and 1
    clock.render(until_beat=5.0)          # bound the otherwise-endless loop
    assert dest.beats() == [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]
    assert [e["freq"] for _, e in dest.events] == [440, 550, 440, 550, 440, 550]


def test_playhead_renders_raw_osc_and_midi_items():
    tl = Timeline([(0.0, OscEvent("/foo", 1)), (1.0, MidiEvent(b"\x90\x3c\x64"))])
    clock = TempoClock(1.0)
    dest = RecordDest()
    Playhead(tl, clock, dest).play()
    clock.render()
    assert dest.events[0][1] == ("osc", (("/foo", 1),))
    assert dest.events[1][1] == ("midi", b"\x90\x3c\x64")


def test_playhead_stop_unscheds_the_feeder():
    clock = TempoClock(1.0)
    ph = Playhead(_arp(), clock, RecordDest())
    ph.play(at=0.0)
    assert ph.playing and len(clock._queue) == 1
    ph.stop()
    assert not ph.playing and len(clock._queue) == 0


def test_playhead_ends_when_the_scan_drains():
    clock = TempoClock(1.0)
    ph = Playhead(_arp(), clock, RecordDest())
    ph.play(at=0.0)
    clock.render()
    assert not ph.playing, "a drained scan is not playing"
    assert ph.finished, "and it ended rather than being stopped"
    assert ph.position() == 2.0, "the position freezes on the last item"


def test_playhead_stopped_by_hand_did_not_finish():
    clock = TempoClock(1.0)
    ph = Playhead(_arp(), clock, RecordDest())
    ph.play(at=0.0)
    ph.stop()
    assert not ph.playing and not ph.finished


def test_playhead_finish_clears_on_replay_and_locate():
    clock = TempoClock(1.0)
    ph = Playhead(_arp(), clock, RecordDest())
    ph.play(at=0.0)
    clock.render()
    assert ph.finished
    ph.locate(0.0)
    assert not ph.finished, "seeking away from the end leaves it behind"
    ph.play(at=0.0)
    assert not ph.finished, "and so does a fresh pass"


def test_playhead_loop_never_finishes():
    clock = TempoClock(1.0)
    ph = Playhead(_arp(), clock, RecordDest())
    ph.loop(0.0, 2.0).play(at=0.0)
    clock.render(until_beat=5.0)
    assert ph.playing and not ph.finished


def test_playhead_position_when_stopped():
    ph = Playhead(_arp(), TempoClock(1.0), RecordDest())
    assert ph.position() == 0.0
    ph.locate(3.0)
    assert ph.position() == 3.0


# ---- capture a pattern into a timeline ----


def test_timeline_from_pattern_records_beats():
    tl = Timeline.from_pattern(Pbind(freq=Pseq([440, 550, 660]), dur=0.5))
    assert len(tl) == 3
    assert [b for b, _ in tl] == [0.0, 0.5, 1.0]
    assert [e["freq"] for _, e in tl] == [440, 550, 660]


# ---- following a server transport (simulated broadcast, no live server) ----


def _wait(predicate, timeout=2.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return False


def _feed_transport(recv, *, defined=1, playing=0, position=0.0):
    """Send a simulated /transport_query.reply broadcast to a receiver's port."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.sendto(
        osc.message("/transport_query.reply", osc.Int64(0), 2.0, int(defined),
                    int(playing), float(position)),
        ("127.0.0.1", recv.port),
    )
    sock.close()


def test_playhead_follows_transport_broadcast():
    recv = OscReceiver().start()
    # A fake server: follow_transport only needs target.addr() (where /server_notify
    # goes -- a discard port here) and transport_state() for the initial apply.
    server = types.SimpleNamespace(
        target=NetAddr("127.0.0.1", 57199),
        transport_state=lambda: None,
    )
    try:
        ph = Playhead(_arp(), TempoClock(1.0), RecordDest())
        ph.follow_transport(server, recv=recv)

        # A "play" broadcast rolls the playhead.
        _feed_transport(recv, playing=1, position=0.0)
        assert _wait(lambda: ph.playing)

        # A "stop" broadcast halts it.
        _feed_transport(recv, playing=0, position=4.0)
        assert _wait(lambda: not ph.playing)
        assert ph.position() == 4.0       # located to the broadcast position
    finally:
        recv.stop()
