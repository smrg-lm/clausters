"""C6: tracking the server's sample clock over UDP. The least-squares model is
tested deterministically (no server); the live UDP path is exercised by a Bash
smoke (same-invocation rule), not here."""

import pytest

from clausters.base import OscNrtInterface, SampleClockTimebase
from clausters.defs import SampleClockModel, Server


def test_model_recovers_a_clean_line():
    m = SampleClockModel(nominal_rate=48_000.0)
    # sample = 1000 + 48000 * t  (no drift)
    for i in range(6):
        t = i * 0.05
        m.add_anchor(t, round(1000 + 48_000 * t), 48_000.0)
    assert m.sample_at(1.0) == pytest.approx(1000 + 48_000, abs=1)
    assert m.drift_ppm() == pytest.approx(0.0, abs=1.0)
    # inverse model
    assert m.local_time_of(1000 + 48_000 * 0.5) == pytest.approx(0.5, abs=1e-6)


def test_model_measures_crystal_drift():
    m = SampleClockModel(nominal_rate=48_000.0)
    # actual slope 48010 sa/s vs nominal 48000 -> +208.3 ppm
    for i in range(10):
        t = i * 0.1
        m.add_anchor(t, round(1000 + 48_010 * t), 48_000.0)
    assert m.drift_ppm() == pytest.approx(208.3, abs=2.0)
    assert m.span() == pytest.approx(0.9, abs=1e-6)


def test_single_anchor_falls_back_to_nominal_rate():
    m = SampleClockModel(nominal_rate=48_000.0)
    m.add_anchor(2.0, 96_000, 48_000.0)         # one anchor -> slope = nominal
    assert m.b == 48_000.0
    assert m.sample_at(3.0) == 96_000 + 48_000   # 1 s later


def test_server_yields_a_sample_timebase():
    # No live server needed: we only check the wiring (own socket, timebase).
    server = Server(interface=OscNrtInterface())
    sc = server.sample_clock()
    try:
        tb = sc.timebase()
        assert isinstance(tb, SampleClockTimebase)
        assert tb.sample_rate == sc.rate
        assert tb.now() >= 0.0
    finally:
        sc.close()


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
