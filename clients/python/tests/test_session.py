"""C5 leftover: ergonomic defaults without global state. A Session bundles a
Server and a clock explicitly, and several coexist (no globals)."""

import pytest

from clausters import Session
from clausters.base import MonotonicTimebase, TempoClock
from clausters.defs import Server
from clausters.seq import Pbind, Pseq


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def test_session_and_default_are_the_same_kind_of_environment():
    # The default session and an explicit Session share one base: both are
    # Environments (server + isolated random context). main *is* a session.
    from clausters import main, default_session
    from clausters.base import Environment

    assert issubclass(Session, Environment)
    assert isinstance(main, Environment)
    assert default_session is main
    for env in (main, Session.nrt(tempo=1.0)):
        assert hasattr(env, "server") and hasattr(env, "seed") and hasattr(env, "rng")


def test_nrt_session_plays_and_renders():
    _embed_or_skip()
    s = Session.nrt(tempo=2.0)
    s.play(Pbind(instrument="default", freq=Pseq([262.0, 330.0, 392.0, 523.0]),
                 dur=0.5, amp=0.2))
    try:
        samples, frames = s.render()
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames > 0
    assert max(abs(x) for x in samples) > 0.0


def test_nrt_render_with_workers_is_bit_identical():
    # A parallel group of independent voices: `workers` must only change
    # wall-clock time, never the samples.
    _embed_or_skip()
    from clausters.defs import SynthDef, control, out, sine

    def build():
        s = Session.nrt(tempo=1.0)
        server = s.server
        server.add_synthdef(SynthDef(
            "par_voice", out(0.0, sine(control("freq", 330.0)) * 0.1)))
        band = server.group()
        server.send_msg("/g_parallel", band.id, 1)
        voices = [server.synth("par_voice", {"freq": 220.0 * (i + 1)},
                               target=band.id) for i in range(4)]

        def score():
            yield 0.5
            server.send_bundle(*[("/n_free", v.id) for v in voices])

        from clausters.base.stream import Routine
        s.clock.play(Routine(score))
        return s

    try:
        seq, frames = build().render(channels=2)
        par, frames2 = build().render(channels=2, workers=2)
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames == frames2 and frames > 0
    assert list(seq) == list(par)


def test_boot_workers_becomes_the_cli_flag(monkeypatch):
    # Server.boot(workers=N) must launch `clausters --workers N` (before any
    # explicit server_args, so those stay the escape hatch that wins).
    import clausters.launch as launch

    captured = {}

    class FakeProcess:
        def __init__(self, options=None, **kwargs):
            captured.update(kwargs)
            self.host, self.port, self.shm = "127.0.0.1", 57997, None

        def start(self):
            return self

    monkeypatch.setattr(launch, "ServerProcess", FakeProcess)
    server = Server.boot(workers=3, server_args=("--tcp",), transport="udp",
                         _adopt_default=False)
    try:
        assert captured["extra_args"] == ["--workers", "3", "--tcp"]
    finally:
        server.interface.close()
    # None emits no flag: the server keeps its config-file default.
    Server.boot(transport="udp", _adopt_default=False).interface.close()
    assert captured["extra_args"] == []


def _embed_session_or_skip():
    """An embedded live session, or skip if the embed library is unusable here
    (not built with embed,realtime, or no audio device available)."""
    _embed_or_skip()
    from clausters import Session
    from clausters.errors import LibraryError, ServerError
    try:
        return Session.embed(tempo=4.0, latency=0.1)
    except (LibraryError, ServerError, OSError) as e:
        pytest.skip(f"embedded server not available: {e}")


def test_embed_session_drives_in_process_server():
    # The embedded server is just another OSC destination: the same Session /
    # Pbind API drives it, request/reply works through the embed interface, and
    # the in-process engine actually advances.
    s = _embed_session_or_skip()
    try:
        embed = s.server.interface.server          # the Clausters handle
        assert s.server.status()[0] == 1           # request/reply over embed

        s.play(Pbind(instrument="default", freq=Pseq([440.0, 550.0, 660.0]),
                     dur=0.25, amp=0.2))
        c0 = embed.clock
        s.run(0.6)
        assert embed.clock > c0                     # engine ran in-process
        assert isinstance(s.server.query_tree(), dict)  # another reply path
    finally:
        s.close()


def test_embed_session_anchors_to_the_sample_clock_by_default():
    # Like live, an embed session sample-locks out of the box — but through a
    # direct in-process read of the shared counter (EmbedSampleClock), with no
    # UDP tracker: no socket, no round trips, no timeout to burn.
    from clausters.base.timebase import SampleClockTimebase
    from clausters.defs.clocksync import EmbedSampleClock

    s = _embed_session_or_skip()
    try:
        assert isinstance(s.clock.timebase, SampleClockTimebase)
        assert isinstance(s.clock._sample_clock, EmbedSampleClock)
        # the timebase reads the handle's counter directly
        embed = s.server.interface.server
        assert s.clock.timebase.current_sample() == pytest.approx(embed.clock, abs=8192)
        # lock_to is idempotent: a manual lock keeps the in-process reader
        sc = s.clock._sample_clock
        assert s.lock_to_server() is s
        assert s.clock._sample_clock is sc
    finally:
        s.close()


def test_embed_session_is_independent_from_others():
    # No global state: an embedded session coexists with an offline one, each
    # with its own server and clock.
    s = _embed_session_or_skip()
    try:
        b = Session.nrt(tempo=1.0)
        assert s.server is not b.server
        assert s.clock is not b.clock
        b.close()
    finally:
        s.close()


def test_two_sessions_are_independent():
    _embed_or_skip()
    a = Session.nrt(tempo=1.0)
    b = Session.nrt(tempo=1.0)
    a.play(Pbind(instrument="default", freq=Pseq([100.0, 200.0]), dur=0.5))
    b.play(Pbind(instrument="default", freq=Pseq([1000.0, 2000.0, 3000.0]), dur=0.5))
    a.clock.render()
    b.clock.render()

    # each session kept its own score (2 notes vs 3, each = /s_new + /n_free)
    assert len(a.server.interface.score.bundles) == 2 * 2
    assert len(b.server.interface.score.bundles) == 3 * 2
    # and they are genuinely separate objects — no shared global
    assert a.server is not b.server
    assert a.clock is not b.clock


def test_lock_to_offline_session_is_a_noop():
    # An NRT (score) server has no live clock; lock_to must leave the clock on
    # wall-clock OSC time (and not raise), so offline scripts keep working.
    s = Session.nrt(tempo=2.0)
    assert isinstance(s.clock.timebase, MonotonicTimebase)
    assert s.lock_to_server() is s            # chainable, no-op here
    assert isinstance(s.clock.timebase, MonotonicTimebase)
    assert s.clock._sample_clock is None
    s.close()


def test_lock_to_unreachable_master_falls_back_to_wall_clock():
    # No server is listening on this port: lock_to detects no master within the
    # timeout and stays on wall-clock OSC time (the "client with no Clausters
    # server" case), rather than raising.
    server = Server("127.0.0.1", 59999)
    clock = TempoClock(tempo=1.0)
    assert clock.lock_to(server, timeout=0.2) is clock
    assert isinstance(clock.timebase, MonotonicTimebase)
    assert clock._sample_clock is None
    clock.close()
    server.close()


def test_quant_snaps_to_the_clock_grid():
    # quant delays the start to the next multiple of `quant` beats on the
    # clock's own grid (beats() is the logical beat while not running).
    clock = TempoClock(tempo=2.0)
    clock._logical_beat = 0.0
    assert clock._quant_delay(4) == 0.0          # on a boundary -> now
    clock._logical_beat = 3.5
    assert clock._quant_delay(4) == pytest.approx(0.5)
    clock._logical_beat = 5.0
    assert clock._quant_delay(2) == pytest.approx(1.0)   # next even beat is 6
    assert clock._quant_delay(None) == 0.0       # no quant -> now
    assert clock._quant_delay(0) == 0.0


def test_quant_on_a_joined_wall_clock_transport():
    import time as _t

    # A wall-clock client joined to a transport whose origin was ~1 s ago at
    # tempo 2 bps -> ~2 beats elapsed; the next bar (quant 4) is ~2 beats off.
    clock = TempoClock(tempo=2.0)
    clock._transport = ("wall", _t.time() - 1.0, 2.0)
    assert 1.8 < clock._grid_beat() < 2.2
    assert 1.8 < clock._quant_delay(4) < 2.2


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
