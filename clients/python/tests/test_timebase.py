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


# ---- an immediate send inside a routine is at the routine's logical time ----

def test_nrt_immediate_sends_follow_the_routine_clock():
    """`Server.synth` and friends send "immediately". Offline that has to mean
    the running routine's logical time, not the start of the score: a routine
    that makes a synth, yields a beat and makes another describes two events a
    beat apart, and rendering them both at zero turns a piece into one chord.
    (The bug this pins piled five sections of `examples/noise.py` onto time
    zero and read 1.31 peak where it should have read 0.46.)"""
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)

    def routine():
        server.synth("default", {"freq": 100.0})
        yield 0.5
        server.synth("default", {"freq": 200.0})
        yield 0.25
        server.synth("default", {"freq": 300.0})
        yield 1.0

    clock.play(Routine(routine))
    clock.render()
    starts = sorted(w for w, raw in server.interface.score.bundles
                    if _inner_addr(raw) == "/s_new")
    assert starts == [0.0, 0.5, 0.75]


def test_nrt_immediate_sends_outside_a_routine_stay_at_zero():
    """There is no logical time outside a routine, and the setup a score opens
    with — the defs, the buffers, the groups — belongs at zero."""
    server = Server(interface=OscNrtInterface())
    server.send_msg("/g_new", 1, 0, 0)
    assert [w for w, _ in server.interface.score.bundles] == [0.0]


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
