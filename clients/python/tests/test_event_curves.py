"""An event carries its curves, and a channel's curve is a playable.

Nothing is built on the server for either: the note is the plain synth it
always was, and the controls its curves drive are set, one value a block, in
timed bundles. What is checked here is the bundles -- read off an offline
score, where every one is kept with its time -- and that the same curves
survive `render_events`.
"""

import threading
import time

import pytest

from clausters import Session, play
from clausters.base import _osclib
from clausters.base.stream import Routine
from clausters.defs import Server
from clausters.multitrack import Automation
from clausters.seq import Timeline
from clausters.seq.curves import WINDOW
from clausters.seq.event import Event

#: Seconds between two values of a curve at the score's rate.
STEP = 64 / 48_000.0


@pytest.fixture(autouse=True)
def _needs_native():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def played(tune) -> list:
    """Render ``tune`` (a generator function) on an offline session and
    answer its score as ``(seconds, [(addr, args), ...])``, in time order."""
    session = Session.nrt().activate()
    try:
        play(Routine(tune))
        session.clock.render()
        bundles = sorted(session.server.interface.score.bundles, key=lambda b: b[0])
        return [(t, [(addr, args) for addr, args, _ in _osclib.decode_packet(packet)])
                for t, packet in bundles]
    finally:
        session.deactivate()


def sets(score, control: str, node: "int | None" = None) -> list:
    """Every ``(seconds, value)`` a ``/node_set`` wrote on ``control``."""
    out = []
    for t, messages in score:
        for addr, args in messages:
            if addr != "/node_set" or (node is not None and args[0] != node):
                continue
            pairs = dict(zip(args[1::2], args[2::2]))
            if control in pairs:
                out.append((t, pairs[control]))
    return out


def test_an_event_s_curve_sets_its_control_from_its_start_to_its_end():
    ramp = Automation({"control": "amp"}, [(0.0, 0.0), (0.25, 1.0)])

    def tune():
        Event(freq=440, amp=0.2, dur=1.0, legato=0.5, automation=[ramp]).play()
        yield 1.0

    score = played(tune)
    start = score[0][1]
    assert [addr for addr, _ in start] == ["/synth_new", "/node_set"]
    assert start[1][1] == [1000, "amp", 0.0], "the curve's first value, as it is made"
    values = sets(score, "amp")
    # One value a block, on the grid, and none past the curve's own end.
    assert [t for t, _ in values[1:4]] == pytest.approx([STEP, 2 * STEP, 3 * STEP])
    assert values[1][1] == pytest.approx(STEP / 0.25, rel=1e-4)
    assert values[-1][1] == 1.0 and values[-1][0] < 0.25 + 2 * STEP
    # The other controls are the plain values they always were.
    assert not sets(score, "freq") and not sets(score, "pan")


def test_a_curve_reaches_a_note_to_its_off_and_no_further():
    """A `/node_set` to a node that is gone fails an offline render, and the
    off is the last instant a client knows the node is there."""
    long = Automation({"control": "amp"}, [(0.0, 0.0), (4.0, 1.0)])

    def tune():
        Event(instrument="beep", freq=440, dur=1.0, legato=0.25, automation=[long]).play()
        yield 1.0

    score = played(tune)
    assert score[-1][1] == [("/node_free", [1000])] and score[-1][0] == pytest.approx(0.25)
    assert max(t for t, _ in sets(score, "amp")) < 0.25


def test_a_beat_of_a_curve_is_a_beat_of_the_clock():
    """A curve's points are in beats from the note's start: at two beats a
    second it is over in half the seconds."""
    ramp = Automation({"control": "amp"}, [(0.0, 0.0), (1.0, 1.0)])

    def tune():
        Event(freq=440, dur=4.0, automation=[ramp]).play()
        yield 4.0

    session = Session.nrt().activate()
    try:
        session.clock.set_tempo(2.0)
        play(Routine(tune))
        session.clock.render()
        bundles = sorted(session.server.interface.score.bundles, key=lambda b: b[0])
        score = [(t, [(a, args) for a, args, _ in _osclib.decode_packet(p)]) for t, p in bundles]
    finally:
        session.deactivate()
    values = sets(score, "amp")
    assert values[-1][1] == 1.0 and values[-1][0] == pytest.approx(0.5, abs=2 * STEP)


def test_a_bend_multiplies_the_frequency_the_note_started_with():
    octave = Automation({"bend": True}, [(0.0, 12.0), (0.1, 0.0)])

    def tune():
        Event(freq=220, dur=1.0, automation=[octave]).play()
        yield 1.0

    score = played(tune)
    freqs = sets(score, "freq")
    assert freqs[0] == (0.0, pytest.approx(440.0))
    assert freqs[-1][1] == pytest.approx(220.0)


def test_a_channel_s_curve_reaches_the_notes_of_its_channel():
    level = Automation({"control": "amp", "channel": 1}, [(0.0, 0.5), (0.2, 1.0)])
    own = Automation({"control": "amp"}, [(0.0, 0.25), (0.2, 0.25)])

    def tune():
        level.play()
        Event(freq=220, dur=1.0, legato=0.1, channel=1).play()              # 1000
        Event(freq=330, dur=1.0, legato=0.1, channel=0).play()              # 1001
        Event(freq=440, dur=1.0, legato=0.1, channel=1, automation=[own]).play()  # 1002
        yield 0.05
        Event(freq=550, dur=1.0, legato=0.1, channel=1).play()              # 1003, later
        yield 1.0

    score = played(tune)
    on_it = sets(score, "amp", 1000)
    assert on_it[0] == (0.0, pytest.approx(0.5)), "the channel's value, as it is made"
    assert len(on_it) > 50 and max(t for t, _ in on_it) < 0.1, "to its off"
    assert not sets(score, "amp", 1001), "another channel"
    assert {v for _, v in sets(score, "amp", 1002)} == {0.25}, "its own curve wins"
    late = sets(score, "amp", 1003)
    assert late[0][0] == pytest.approx(0.05)
    assert late[0][1] == pytest.approx(0.5 + 0.5 * 0.05 / 0.2, abs=0.02), \
        "where the channel's curve stands when the note starts"


def test_a_stopped_channel_curve_sets_nothing_more():
    level = Automation({"control": "amp"}, [(0.0, 0.0), (1.0, 1.0)])

    def tune():
        level.play()
        Event(freq=220, dur=1.0, legato=0.5).play()
        yield 0.1
        level.stop()
        yield 1.0

    score = played(tune)
    assert max(t for t, _ in sets(score, "amp")) <= 0.1 + 2 * WINDOW + STEP


def test_an_event_with_no_curve_is_the_two_bundles_it_always_was():
    def tune():
        Event(freq=440, dur=1.0).play()
        yield 1.0

    score = played(tune)
    assert [[addr for addr, _ in messages] for _, messages in score] == \
        [["/synth_new"], ["/node_set"]]


def test_an_offline_render_plays_an_event_s_curves():
    """End to end: the strict offline render takes every bundle -- none of
    them reaches a node that is gone -- and the curve is heard."""
    def tune(curve):
        def body():
            Event(freq=440, amp=0.5, dur=0.4, legato=0.5, automation=curve).play()
            yield 0.5
        return body

    def rms(curve):
        session = Session.nrt()
        with session:
            play(Routine(tune(curve)))
        take = session.render(sample_rate=48_000.0, channels=1)
        head = take.samples[: int(0.1 * 48_000)]
        return (sum(x * x for x in head) / len(head)) ** 0.5

    silent = Automation({"control": "amp"}, [(0.0, 0.0), (1.0, 0.0)])
    assert rms([]) > 1e-3
    assert rms([silent]) < rms([]) / 20


def test_render_events_keeps_an_event_s_curves_and_a_channel_s():
    own = Automation({"control": "cutoff"}, [(0.0, 200.0), (0.5, 2000.0)])
    level = Automation({"control": "amp", "channel": 0}, [(0.0, 0.0), (2.0, 1.0)])
    timeline = Timeline()
    timeline.add(0.0, level)
    timeline.add(1.0, Event(freq=440, dur=1.0, automation=[own]))

    data = timeline.render_events().data()
    (event,) = data["events"]
    (kept,) = event["automation"]
    assert kept["target"] == {"control": "cutoff"}
    assert [(p["at"], p["value"]) for p in kept["points"]] == [(0.0, 200.0), (0.5, 2000.0)]
    assert "automation" not in event["data"]
    (channel,) = data["automation"]
    assert channel["target"] == {"control": "amp", "channel": 0}
    assert [p["at"] for p in channel["points"]] == [0.0, 2.0]


def test_a_note_played_outside_any_clock_writes_its_curve_into_a_score():
    """Offline there is no wall to wake on: the score takes every value as the
    note is played, each at its own second."""
    from clausters.base import OscNrtInterface

    server = Server(interface=OscNrtInterface())
    ramp = Automation({"control": "amp"}, [(0.0, 0.0), (0.1, 1.0)])
    Event(freq=440, dur=1.0, legato=0.5, automation=[ramp]).play(server)
    bundles = sorted(server.interface.score.bundles, key=lambda b: b[0])
    score = [(t, [(a, args) for a, args, _ in _osclib.decode_packet(p)]) for t, p in bundles]
    values = sets(score, "amp")
    assert values[0] == (0.0, 0.0) and values[-1][1] == 1.0
    assert [t for t, _ in values[1:3]] == pytest.approx([STEP, 2 * STEP])
    assert values[-1][0] == pytest.approx(0.1, abs=2 * STEP)


class _Recording:
    """A real-time interface that keeps what it is sent."""

    time_mode = "unix"

    def __init__(self):
        self.bundles = []
        self.lock = threading.Lock()

    def send_msg(self, target, addr, *args):
        pass

    def send_bundle(self, target, when, *messages):
        with self.lock:
            self.bundles.append((when, messages))

    def recv(self, timeout):
        return None

    def close(self):
        pass


def test_a_note_played_outside_any_clock_is_followed_on_the_wall():
    """With no clock to wake on, the emitter wakes itself: it sends a little
    ahead of now and stops at the note's off."""
    server = Server(interface=_Recording())
    try:
        ramp = Automation({"control": "amp"}, [(0.0, 0.0), (1.0, 1.0)])
        began = time.time()
        Event(freq=440, dur=0.3, legato=1.0, automation=[ramp]).play(server)
        time.sleep(0.05)
        with server.interface.lock:
            early = [when for when, messages in server.interface.bundles
                     if messages[0][0] == "/node_set" and messages[0][2] == "amp"]
        assert early and max(early) - began < 0.05 + 3 * WINDOW + server.latency
        time.sleep(0.45)
        with server.interface.lock:
            sent = [(when, messages[0]) for when, messages in server.interface.bundles
                    if messages[0][0] == "/node_set"]
        (release,) = [when for when, message in sent if message[2] == "gate"]
        last = max(when for when, message in sent if message[2] == "amp")
        assert release - 0.1 < last < release, "to its off, and no further"
        assert not server.curves._axes, "nothing left to follow"
    finally:
        server.close()


def test_a_curve_plays_only_on_a_destination_that_sets_controls():
    with pytest.raises(TypeError, match="a curve plays on a server"):
        Automation({"control": "amp"}, [(0.0, 0.0)]).play(object())
