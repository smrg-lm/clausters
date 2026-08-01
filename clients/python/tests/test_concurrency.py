"""Follow-up to C5: the execution context is thread-local, so several clocks —
and a live RT clock next to an offline NRT render — run in one script without
clobbering each other. This is the no-global-state litmus test.
"""

import struct
import threading

import pytest

from clausters.base import OscNrtInterface, OscUdpInterface, Routine, TempoClock
from clausters.base import _osclib as osc
from clausters.base.main import main
from clausters.defs import Server
from clausters.seq import Pbind, Pseq


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def _inner_msg(raw: bytes):
    length = struct.unpack(">i", raw[16:20])[0]
    return osc.decode(raw[20:20 + length])


def _starts(score):
    return sorted(w for w, raw in score.bundles if _inner_msg(raw)[0] == "/synth_new")


def _freqs(score):
    out = []
    for _, raw in score.bundles:
        addr, args = _inner_msg(raw)
        if addr == "/synth_new":
            out.append(args[args.index("freq") + 1])
    return sorted(out)


def test_current_tt_is_thread_local():
    main.current_tt = "main-thread"
    seen = {}

    def worker():
        seen["before"] = main.current_tt   # a fresh thread starts at None
        main.current_tt = "worker"
        seen["after"] = main.current_tt

    t = threading.Thread(target=worker)
    t.start()
    t.join()

    assert seen["before"] is None
    assert seen["after"] == "worker"
    assert main.current_tt == "main-thread"  # the worker did not clobber us
    main.current_tt = None


def test_two_clocks_render_independently():
    _embed_or_skip()
    results = {}

    def run(tag, freqs):
        server = Server(interface=OscNrtInterface())
        clock = TempoClock(tempo=1.0)
        Pbind(instrument="default", freq=Pseq(freqs), dur=0.5, amp=0.2).play(clock, server)
        clock.render()
        results[tag] = server.interface.score

    ta = threading.Thread(target=run, args=("a", [100.0, 200.0, 300.0, 400.0]))
    tb = threading.Thread(target=run, args=("b", [1000.0, 2000.0, 3000.0, 4000.0]))
    ta.start(); tb.start(); ta.join(); tb.join()

    # exact, yield-driven timing in both — no cross-thread interference
    assert _starts(results["a"]) == [0.0, 0.5, 1.0, 1.5]
    assert _starts(results["b"]) == [0.0, 0.5, 1.0, 1.5]
    # and each score kept its own frequencies
    assert _freqs(results["a"]) == [100.0, 200.0, 300.0, 400.0]
    assert _freqs(results["b"]) == [1000.0, 2000.0, 3000.0, 4000.0]


def test_rt_and_nrt_in_the_same_script():
    _embed_or_skip()
    # A live RT clock churning on a background thread (emits to a socket with no
    # listener — harmless), set up to thrash the execution context fast.
    rt_server = Server(interface=OscUdpInterface().start())
    rt_clock = TempoClock(tempo=50.0)

    def churn():
        while True:
            rt_server.send_bundle(("/server_status",))
            yield 0.02

    rt_clock.play(Routine(churn))
    rt_clock.start()
    try:
        # Meanwhile, build an NRT score on the main thread: it must stay exact.
        nrt_server = Server(interface=OscNrtInterface())
        nrt_clock = TempoClock(tempo=1.0)
        Pbind(instrument="default", freq=Pseq([262.0, 330.0, 392.0, 523.0]),
              dur=0.5, amp=0.2).play(nrt_clock, nrt_server)
        for _ in range(20):           # repeat to widen the race window
            nrt_clock.render()
            assert _starts(nrt_server.interface.score) == [0.0, 0.5, 1.0, 1.5]
            nrt_server.interface.score.bundles.clear()
            nrt_clock = TempoClock(tempo=1.0)
            Pbind(instrument="default", freq=Pseq([262.0, 330.0, 392.0, 523.0]),
                  dur=0.5, amp=0.2).play(nrt_clock, nrt_server)
    finally:
        rt_clock.stop()
        rt_server.close()


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
