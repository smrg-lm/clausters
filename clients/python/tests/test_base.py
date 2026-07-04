"""C2 base-layer tests.

The headline is the destination-swap seam: one routine + clock produces an NRT
score (rendered to audio) just by giving the **Server** an ``OscNrtInterface``
(C4: the clock only times; the Server emits). The rest unit-tests builtins
(native, f32), the operator-overloading base, the stream/routine protocol and
the native-backed clock math.
"""

import pytest

from clausters.base import builtins as B
from clausters.base import (
    AbstractObject,
    OscNrtInterface,
    OscTcpInterface,
    Routine,
    StopStream,
    TempoClock,
)
from clausters.defs import Server


def _ffi_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


# ---- builtins: scalar + list, f32 via the core ----

def test_builtins_scalar_and_list():
    _ffi_or_skip()
    assert B.add(1.5, 2.0) == pytest.approx(3.5)
    assert B.mul([1.0, 2.0, 3.0], 2.0) == [2.0, 4.0, 6.0]
    # two lists, cyclic extension of the shorter one (sc3 semantics)
    assert B.add([10.0, 20.0, 30.0, 40.0], [1.0, 2.0]) == [11.0, 22.0, 31.0, 42.0]
    assert B.sqrt([9.0, 16.0]) == [3.0, 4.0]


def test_builtins_are_f32_not_python_float():
    _ffi_or_skip()
    # 0.1 + 0.2 in f32 differs from Python's f64 0.30000000000000004; the
    # client matches the server because it rounds through the core.
    assert B.add(0.1, 0.2) != 0.1 + 0.2


def test_music_helpers():
    # Now computed through the core (f32), so they need the FFI like the rest.
    _ffi_or_skip()
    assert B.midicps(69) == pytest.approx(440.0)
    assert B.dbamp(0.0) == pytest.approx(1.0)
    assert B.cpsmidi(440.0) == pytest.approx(69.0)
    assert B.cpsoct(B.octcps(5.0)) == pytest.approx(5.0, abs=1e-4)


def test_extended_ops_s3():
    # S3: the new opcodes reach the core and compute the expected values.
    _ffi_or_skip()
    assert B.hypot(3.0, 4.0) == pytest.approx(5.0)
    assert B.clip2(5.0, 1.0) == pytest.approx(1.0)
    assert B.absdif(2.0, 5.0) == pytest.approx(3.0)
    assert B.squared(3.0) == pytest.approx(9.0)
    assert B.recip(4.0) == pytest.approx(0.25)
    assert B.sign(-2.0) == pytest.approx(-1.0)
    assert B.round(1.3, 0.5) == pytest.approx(1.5)
    assert B.sumsqr([1.0, 2.0], [2.0, 1.0]) == [pytest.approx(5.0), pytest.approx(5.0)]


def test_smoothing_windows_s8():
    # S8: the FFT-chain windows are the shared core, exposed for binary parity
    # (same shape the server's FFT/IFFT applies).
    _ffi_or_skip()
    import math
    from clausters import _native

    n = 64
    hann = _native.window(_native.Window.HANN, n)
    assert len(hann) == n
    # Periodic Hann: starts at 0, symmetric, in [0, 1].
    assert hann[0] == pytest.approx(0.0, abs=1e-6)
    for i in range(1, n // 2):
        assert hann[i] == pytest.approx(hann[n - i], abs=1e-6)
    for k in range(n):
        assert hann[k] == pytest.approx(0.5 - 0.5 * math.cos(2 * math.pi * k / n), abs=1e-6)
    # Rectangular is all ones; an unknown type falls back to Hann (like the server).
    assert list(_native.window(_native.Window.RECTANGULAR, 8)) == [1.0] * 8
    assert list(_native.window(999, n)) == list(hann)


# ---- absobject: operator overloading dispatches by selector ----

class _Recorder(AbstractObject):
    """Records the composed expression instead of evaluating it."""

    def __init__(self, tag):
        self.tag = tag

    def _compose_unop(self, selector):
        return _Recorder((selector, self.tag))

    def _compose_binop(self, selector, other):
        return _Recorder((selector, self.tag, other))

    def _rcompose_binop(self, selector, other):
        return _Recorder((selector, other, self.tag))

    def _compose_narop(self, selector, *args):
        return _Recorder((selector, self.tag, args))


def test_operator_overloading_uses_selectors():
    x = _Recorder("x")
    assert (x + 1).tag == ("add", "x", 1)
    assert (2 * x).tag == ("mul", 2, "x")  # reflected
    assert (-x).tag == ("neg", "x")
    assert x.midicps().tag == ("midicps", "x")
    assert x.max(3).tag == ("max", "x", 3)


# ---- stream / routine ----

def test_routine_yields_and_finishes():
    def counter(_):
        yield 1
        yield 2
        yield 3

    r = Routine(counter)
    assert [r.next(), r.next(), r.next()] == [1, 2, 3]
    with pytest.raises(StopStream):
        r.next()
    r.reset()
    assert r.next() == 1


def test_routine_receives_inval_on_resume():
    seen = []

    def echo(first):
        got = first
        while True:
            got = yield got
            seen.append(got)

    r = Routine(echo)
    assert r.next("a") == "a"      # initial arg flows through
    assert r.next("b") == "b"      # sent into the yield
    assert seen == ["b"]


# ---- clock math (native-backed) ----

def test_clock_beat_second_math():
    _ffi_or_skip()
    clk = TempoClock(tempo=2.0)  # 2 beats/s
    assert clk.beats2secs(2.0) == pytest.approx(1.0)
    assert clk.secs2beats(1.0) == pytest.approx(2.0)


def test_tcp_interface_constructs_and_frames():
    # C8: no longer a stub. It constructs without connecting and frames an OSC
    # packet with a 4-byte big-endian length prefix (full framing/reassembly
    # coverage is in tests/test_tcp.py).
    iface = OscTcpInterface(host="127.0.0.1", port=57110)
    assert iface.time_mode == "unix"
    framed = iface._frame(b"abcdef")
    assert framed == (6).to_bytes(4, "big") + b"abcdef"


# ---- the seam: one routine -> NRT score -> render ----

def test_routine_renders_through_nrt_interface():
    _ffi_or_skip()
    # The Server owns the interface and emits; the clock only times. Swap the
    # interface for an OscUdpInterface and the same routine plays live.
    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=2.0)

    def arpeggio():  # finds its clock via main.current_tt; emits via the server
        for i, freq in enumerate([262.0, 330.0, 392.0, 523.0, 659.0]):
            node = 1000 + i
            server.send_bundle(("/s_new", "default", node, 1, 0, "freq", freq, "amp", 0.2))
            server.send_bundle(("/n_free", node), delay_beats=0.9)
            yield 1.0
        server.send_bundle(("/n_free", 0))  # closes the render

    clock.play(Routine(arpeggio))
    clock.render()  # drain the queue logically; routine fills the score

    assert len(server.interface.score.bundles) > 5  # five notes + frees + close
    try:
        samples, frames = server.render(sample_rate=48_000.0, channels=2)
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames > 0
    assert max(abs(s) for s in samples) > 0.0


if __name__ == "__main__":
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001 — smoke harness
                kind = type(e).__name__
                skip = kind in ("Skipped", "OutcomeException")
                print(f"{'skip' if skip else 'FAIL'} {name}: {e}")
                if not skip:
                    traceback.print_exc()
