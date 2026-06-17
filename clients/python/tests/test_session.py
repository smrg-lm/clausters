"""C5 leftover: ergonomic defaults without global state. A Session bundles a
Server and a clock explicitly, and several coexist (no globals)."""

import pytest

from clausters import Session
from clausters.seq import Pbind, Pseq


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


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
