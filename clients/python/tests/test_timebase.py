"""Follow-up to C5: the pacing timebase is selectable — the OS monotonic clock
(default, NTP-timetagged bundles) or the server's sample clock (events emitted
by absolute sample via ``/sched``). Robust tests for **both** options.
"""

import struct

import pytest

from clausters.base import (
    MonotonicTimebase,
    OscNrtInterface,
    Routine,
    SampleClockTimebase,
    TempoClock,
)
from clausters.base import _osclib as osc
from clausters.base.main import main
from clausters.defs import Server
from clausters.seq import Pbind, Pseq
from clausters.defs import Synth


def _ffi_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


class _FakeIface:
    """Records sent messages (decoded) and timetagged bundles."""

    time_mode = "unix"

    def __init__(self):
        self.sent = []

    def send_msg(self, target, addr, *args):
        self.sent.append(osc.decode(osc.message(addr, *args)))

    def send_bundle(self, target, when, *messages):
        self.sent.append(("#bundle", when, [m[0] for m in messages]))

    def recv(self, timeout):
        return None

    def close(self):
        pass


def _running_routine(clock, beat):
    """Pretend a routine is being resumed at ``beat`` on a started RT clock."""
    r = type("R", (), {})()
    r.clock = clock
    r._logical_beat = beat
    main.current_tt = r
    return r


def _inner_addr(raw: bytes) -> str:
    length = struct.unpack(">i", raw[16:20])[0]
    return osc.decode(raw[20:20 + length])[0]


# ---- monotonic timebase: NTP bundle ----

def test_monotonic_rt_emits_ntp_bundle():
    _ffi_or_skip()
    clock = TempoClock(tempo=1.0)                 # MonotonicTimebase (default)
    assert clock.timebase.kind == "monotonic"
    clock._mode = "rt"
    clock._mono_start = 0.0
    clock._unix_start = 1000.0
    server = Server(interface=_FakeIface(), latency=0.0)
    _running_routine(clock, 1.0)
    try:
        server.send_bundle(("/s_new", "x", 1, 1, 0))
    finally:
        main.current_tt = None
    kind, when, addrs = server.interface.sent[-1]
    assert kind == "#bundle"                      # not /sched
    assert when == 1000.0 + 1.0                   # unix_start + beats2secs(1.0)
    assert addrs == ["/s_new"]


# ---- sample-clock timebase: pacing + /sched ----

def test_sample_clock_paces_against_the_counter():
    _ffi_or_skip()
    state = {"s": 0}
    tb = SampleClockTimebase(lambda: state["s"], 48_000.0)
    clock = TempoClock(tempo=2.0, timebase=tb)    # 2 beats/s
    clock.start()
    try:
        state["s"] = 48_000                       # advance the sample clock 1 s
        assert clock.beats() == pytest.approx(2.0)
    finally:
        clock.stop()


def test_sample_clock_emits_sched_with_exact_sample():
    _ffi_or_skip()
    tb = SampleClockTimebase(lambda: 96_000, 48_000.0)   # origin = sample 96000 (2.0 s)
    clock = TempoClock(tempo=1.0, timebase=tb)
    clock._mode = "rt"
    clock._mono_start = tb.now()                  # simulate a started RT clock
    server = Server(interface=_FakeIface(), latency=0.0)
    _running_routine(clock, 1.0)                  # event at logical beat 1.0 (= 1.0 s)
    try:
        server.send_bundle(("/s_new", "default", 1000, 1, 0))
    finally:
        main.current_tt = None
    addr, args = server.interface.sent[-1]
    assert addr == "/sched"
    assert args[0] == 96_000 + 48_000             # origin + 1.0 s of samples
    assert isinstance(args[1], (bytes, bytearray)) and args[1][:8] == b"#bundle\x00"


def test_a_resumed_clock_keeps_its_beat_axis():
    """`stop`/`start` holds the beat, and both origins move with it — so an
    event emitted after a restart is stamped for *now*, not for where the
    clock would have been had it never stopped. Driven by a hand-moved sample
    counter, so there is no wall clock in the assertion."""
    _ffi_or_skip()
    state = {"s": 0}
    tb = SampleClockTimebase(lambda: state["s"], 48_000.0)
    clock = TempoClock(tempo=1.0, timebase=tb)   # 1 beat = 1 second
    clock.start()
    try:
        state["s"] = 96_000                      # 2 s of counter -> beat 2
        assert clock.beats() == pytest.approx(2.0)
        clock.stop()
        assert clock.beats() == pytest.approx(2.0), "the beat is held"

        state["s"] = 480_000                     # 10 s in: the clock is stopped
        assert clock.beats() == pytest.approx(2.0), "a stopped clock does not run"

        clock.start()
        assert clock.beats() == pytest.approx(2.0), "it resumes where it stopped"
        # The pacing origin moved back by the held beat, so beat 2 is now.
        assert clock.pacing_origin == pytest.approx(10.0 - 2.0)

        server = Server(interface=_FakeIface(), latency=0.0)
        _running_routine(clock, 2.0)             # the routine is at beat 2
        try:
            server.send_bundle(("/s_new", "default", 1000, 1, 0))
        finally:
            main.current_tt = None
        addr, args = server.interface.sent[-1]
        assert addr == "/sched"
        assert args[0] == 480_000, "scheduled for now, not for the pre-stop axis"
    finally:
        clock.stop()


def test_latency_shifts_the_scheduled_sample():
    _ffi_or_skip()
    tb = SampleClockTimebase(lambda: 0, 48_000.0)
    clock = TempoClock(tempo=1.0, timebase=tb)
    clock._mode = "rt"
    clock._mono_start = 0.0
    server = Server(interface=_FakeIface(), latency=0.25)   # 0.25 s lookahead
    _running_routine(clock, 0.0)
    try:
        server.send_bundle(("/s_new", "default", 1, 1, 0))
    finally:
        main.current_tt = None
    _, args = server.interface.sent[-1]
    assert args[0] == round(0.25 * 48_000)        # 12000


# ---- the logical (NRT) timing is timebase-independent ----

def test_nrt_render_is_timebase_independent():
    _ffi_or_skip()

    def starts_for(timebase):
        server = Server(interface=OscNrtInterface())
        clock = TempoClock(tempo=1.0, timebase=timebase)
        Pbind(instrument="default", freq=Pseq([100.0, 200.0, 300.0, 400.0]),
              dur=0.5).play(clock, server)
        clock.render()
        return sorted(w for w, raw in server.interface.score.bundles
                      if _inner_addr(raw) == "/s_new")

    mono = starts_for(MonotonicTimebase())
    samp = starts_for(SampleClockTimebase(lambda: 0, 48_000.0))
    assert mono == samp == [0.0, 0.5, 1.0, 1.5]


# ---- immediate against timed, offline ----

def test_nrt_immediate_sends_land_at_the_start_of_the_score():
    """A message has no time: in a bundle it would carry the immediate timetag,
    and alone it means the same. One interface serves real time and the score,
    so this is not an offline behaviour — it is only *visible* offline, where an
    immediate send is stamped 0.0 however far into a routine it was called.
    Creating a node this way from a routine is an error; the timed path is
    below."""
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)

    def routine():
        Synth.new("default", {"freq": 100.0}, server=server)
        yield 0.5
        Synth.new("default", {"freq": 200.0}, server=server)
        yield 1.0

    clock.play(Routine(routine))
    clock.render()
    starts = sorted(w for w, raw in server.interface.score.bundles
                    if _inner_addr(raw) == "/s_new")
    assert starts == [0.0, 0.0]


def test_nrt_send_bundle_carries_the_routines_logical_beat():
    """The other half of the pair: `send_bundle` stamps the beat the routine has
    accumulated by yielding, so this is how a routine places an event in time."""
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)

    def routine():
        for _ in range(3):
            server.send_bundle(("/s_new", "default", -1, 0, 0))
            yield 0.5

    clock.play(Routine(routine))
    clock.render()
    starts = sorted(w for w, raw in server.interface.score.bundles
                    if _inner_addr(raw) == "/s_new")
    assert starts == [0.0, 0.5, 1.0]


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001
                kind = type(e).__name__
                skip = kind in ("Skipped", "OutcomeException")
                print(f"{'skip' if skip else 'FAIL'} {name}: {e}")
                if not skip:
                    traceback.print_exc()
