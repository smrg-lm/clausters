"""A timeline played on a **server transport**: the verbs are the transport's,
and the plan is stamped on the transport's clock.

The server here is a recorder: the transport commands are collected instead of
sent, and what the plan writes lands in the score interface as the
``/sched_atTransport`` messages it really sends. What is checked is the stamping
(which sample, on which axis), the re-cue rule around a locate, and what the
mode refuses.
"""

import pytest

from clausters.base import OscNrtInterface, Routine
from clausters.defs import Server
from clausters.seq import Event, Pbind, Timeline
from clausters.seq.timeline import OscItem

SR = 48_000.0
RATE = 2.0             # beats per second
BASE = 480_000         # the transport clock's sample when the test starts
LATENCY = 0.1


class TransportServer(Server):
    """A server whose transport answers and records instead of rolling."""

    def __init__(self, group=7):
        super().__init__(interface=OscNrtInterface())
        self.latency = LATENCY
        self.calls = []
        self.state = {"playing": False, "position_sample": 0,
                      "transport_sample": BASE, "group": group, "loop": None}

    def transport_state(self, timeout=None):
        return dict(self.state)

    def query_info(self, *_a, **_kw):
        class Info:
            nominal_sample_rate = SR
        return Info()

    def transport_play(self, position=None, timeout=None):
        self.calls.append(("play", position))
        self.state["playing"] = True

    def transport_stop(self, timeout=None):
        self.calls.append(("stop",))
        self.state["playing"] = False

    def transport_locate_sample(self, sample, timeout=None):
        self.calls.append(("locate", int(sample)))
        self.state["position_sample"] = int(sample)

    def sched_clear(self, axis=None):
        self.calls.append(("clear", axis))
        return self

    # ---- what the plan wrote ----

    def planned(self):
        """The ``(sample, addr)`` of every ``/sched_atTransport`` the plan sent,
        read back out of the recorded packets."""
        import struct

        out = []
        for _when, packet in self.interface.score.bundles:
            if b"/sched_atTransport" not in packet:
                continue
            # The transport id and the int64 target sit right after the
            # address and its type tag (",ihb" and its padding, 8 bytes).
            i = packet.index(b"/sched_atTransport")
            tags = packet.index(b",ihb", i)
            sample = struct.unpack_from(">q", packet, tags + 8 + 4)[0]
            addr = "/synth_new" if b"/synth_new" in packet else "/other"
            for known in (b"/a", b"/b"):
                if known + b"\0" in packet:
                    addr = known.decode()
            out.append((sample, addr))
        return out

    def onsets(self):
        """The plan's samples, in order, with the base and latency taken off:
        seconds of the transport from where the plan started."""
        base = BASE + LATENCY * SR
        return sorted(round((sample - base) / SR, 6) for sample, _a in self.planned())


def _timeline():
    return Timeline([(b, OscItem("/a")) for b in range(4)], tempo=RATE)


def test_a_transport_needs_a_governed_group():
    server = TransportServer(group=None)
    tl = _timeline()
    with pytest.raises(ValueError, match="transport_group"):
        tl.transport = server


def test_play_locates_rolls_and_plans_on_the_transports_clock():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=0.0, destination=server)

    assert server.calls == [("clear", "transport"), ("locate", 0), ("play", None)]
    # Four items, a beat apart at two beats a second, stamped `latency` ahead of
    # the transport's clock so nothing regenerated is late.
    assert server.onsets() == [0.0, 0.5, 1.0, 1.5]
    assert tl.playing


def test_the_offset_is_where_beat_0_falls_on_the_transport():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.transport_at = 4.0           # seconds of the transport's position
    tl.play(at=1.0, destination=server)

    # Beat 1 is half a second in, on top of the offset.
    assert ("locate", int((4.0 + 0.5) * SR)) in server.calls
    assert server.onsets() == [0.0, 0.5, 1.0]


def test_a_locate_clears_the_transport_queue_and_re_plans():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=0.0, destination=server)
    server.calls.clear()
    server.interface.score.bundles.clear()

    tl.locate(2.0)
    assert server.calls == [("clear", "transport"), ("locate", int(1.0 * SR))]
    # From beat 2: the two items left, the first of them now.
    assert server.onsets() == [0.0, 0.5]
    assert tl.position() == pytest.approx(2.0)


def test_a_resume_re_plans_nothing():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=0.0, destination=server)
    tl.pause()
    server.calls.clear()
    server.interface.score.bundles.clear()

    tl.play()                        # no `at`: the frozen queue carries on
    assert server.calls == [("play", None)]
    assert server.onsets() == []


def test_stop_halts_and_goes_back_to_the_mark():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=1.0, destination=server)
    server.calls.clear()

    tl.stop()
    assert server.calls[0] == ("stop",)
    assert ("locate", int(0.5 * SR)) in server.calls
    assert tl.position() == pytest.approx(1.0)


def test_what_the_mode_refuses():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server

    with pytest.raises(ValueError, match="quant"):
        tl.play(at=0.0, quant=4, destination=server)
    with pytest.raises(ValueError, match="loop"):
        tl.loop(0.0, 2.0)

    # Forward-only items cannot be planned from a position.
    forward = Timeline([(0.0, Routine(lambda: (yield 1)))], tempo=RATE)
    forward.transport = server
    with pytest.raises(ValueError, match="Routine"):
        forward.play(at=0.0, destination=server)

    generated = Timeline([(0.0, Pbind(degree=0))], tempo=RATE)
    generated.transport = server
    with pytest.raises(ValueError, match="Pbind"):
        generated.play(at=0.0, destination=server)


def test_a_child_is_planned_in_its_own_units():
    server = TransportServer()
    child = Timeline([(0, OscItem("/b")), (1, OscItem("/b"))], tempo=4.0)
    parent = Timeline([(0, OscItem("/a"))], tempo=RATE)
    parent.add(1, child)
    parent.transport = server
    parent.play(at=0.0, destination=server)

    # The parent's beat 1 is half a second in; the child's beat 1 a quarter
    # after that, at its own four beats a second.
    assert server.onsets() == [0.0, 0.5, 0.75]


def test_leaving_the_transport_goes_back_to_its_own_clock():
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=0.0, destination=server)
    tl.transport = None
    assert tl.transport is None
    assert not tl.playing
    assert server.calls[-1] == ("stop",)


def test_an_events_sustain_is_still_in_the_timelines_beats():
    server = TransportServer()
    tl = Timeline([(0, Event(instrument="default", dur=1.0, legato=1.0, target=7))],
                  tempo=RATE)
    tl.transport = server
    tl.play(at=0.0, destination=server)
    # One beat of sustain at two beats a second: the release is half a second
    # after the onset, on the transport's clock.
    assert server.onsets() == [0.0, 0.5]


def test_a_conductors_locate_re_plans_from_where_it_says():
    """The mode **is** the following: a broadcast is where a locate somebody
    else sent arrives, and the plan is written again from there. Fed straight to
    the handler the receiver would call, so no socket is involved."""
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=0.0, destination=server)
    server.interface.score.bundles.clear()

    # A conductor's locate: the engine moved, so the server says so too.
    server.state["position_sample"] = int(1.5 * SR)
    reply = ["/transport_query.reply", 0, 2.0, 1, 1, 0.0, 7, BASE, int(1.5 * SR), 0, 0,
             -1, -1, 0]
    player = tl._player
    player._broadcast(reply[0], reply[1:], 0.0, "test")     # a conductor's locate

    assert tl.position() == pytest.approx(3.0)              # beat 3 at two beats a second
    assert server.onsets() == [0.0]                         # the last item, re-planned
    assert ("clear", "transport") in server.calls[-3:], "the re-cue clears first"


def test_with_no_destination_the_items_go_to_the_transports_server():
    """A plan a broadcast writes runs on the receiver's thread, where no session
    is ambient: the items go to the server whose transport this is."""
    server = TransportServer()
    tl = _timeline()
    tl.transport = server
    tl.play(at=0.0)                                   # no destination named
    assert server.onsets() == [0.0, 0.5, 1.0, 1.5]
