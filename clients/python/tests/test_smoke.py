"""C1 smoke tests: the package imports, the native core is reachable, and the
offline render works end to end.

The native cdylibs are built by cargo (see ../README.md). Tests that need a
library that has not been built are skipped rather than failed, so the suite is
safe to run before building — but the C1 verification runs them all.
"""

import pytest

import clausters
from clausters import _native
from clausters.base import _osclib as osc


def _native_or_skip():
    try:
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")
    return _native


def test_package_reexports_what_a_piece_names():
    # The top level holds the verbs, the hosts, the server's resources, the def
    # formats and the timing types -- plus the layer modules themselves.
    for name in ("play", "render", "plot", "scope",
                 "Session", "Server", "GuiHost",
                 "Node", "Synth", "Group", "Bus", "Buffer",
                 "SynthDef", "FaustDef", "GraphDef",
                 "TempoClock", "Routine", "Event", "Timeline", "Playhead",
                 "base", "defs", "seq", "form", "gui", "ipc", "launch", "errors"):
        assert name in clausters.__all__, name
        assert hasattr(clausters, name), name
    assert clausters._native is _native


def test_enumerative_and_plumbing_names_stay_in_their_module():
    # Too many to spell out flat: the UGen callables, the value patterns, the
    # widgets. Reached through the layer above, not instantiated: the
    # transports and the process launchers.
    for name in ("sine", "Pbind", "knob",
                 "Clausters", "ShmClient", "ServerProcess", "GuiProcess",
                 "default_shm_path", "CommandError"):
        assert name not in clausters.__all__, name
    assert clausters.defs.sine and clausters.seq.Pbind and clausters.gui.knob
    assert clausters.ipc.Clausters and clausters.ipc.ShmClient
    assert clausters.launch.ServerProcess and clausters.launch.GuiProcess
    assert clausters.errors.CommandError
    # ClaustersError is the one error at the top level: the root you catch when
    # you do not care which leaf it was.
    assert clausters.ClaustersError in clausters.errors.CommandError.__mro__


def test_builtins_scalar_and_list():
    n = _native_or_skip()
    # scalar add returns a float, matching the server's BinaryOp by construction
    assert n.binary(n.BinaryOp.ADD, 1.5, 2.0) == pytest.approx(3.5)
    # broadcasting a constant over a list returns an array('f')
    out = n.binary(n.BinaryOp.MUL, [1.0, 2.0, 3.0], 2.0)
    assert list(out) == [2.0, 4.0, 6.0]
    # unary over a list
    assert list(n.unary(n.UnaryOp.NEG, [1.0, -2.0])) == [-1.0, 2.0]
    # a higher-math op (mirrors Faust's formula)
    assert n.unary(n.UnaryOp.SQRT, 9.0) == pytest.approx(3.0)


def test_white_noise_is_deterministic_and_in_range():
    n = _native_or_skip()
    a = n.white_noise(42, 64)
    b = n.white_noise(42, 64)
    assert list(a) == list(b)
    assert all(-1.0 <= s < 1.0 for s in a)


def test_tempoclock_conversions():
    n = _native_or_skip()
    # 120 bpm = 2 beats/s, beat 0 at second 0: beat 2 is at second 1.
    assert n.beats_to_secs(2.0, 0.0, 0.0, 2.0) == pytest.approx(1.0)
    assert n.secs_to_beats(2.0, 0.0, 0.0, 1.0) == pytest.approx(2.0)
    assert n.secs_to_samples(1.0, 48_000.0) == 48_000


def test_bar_beat_reads_the_quant_grid():
    n = _native_or_skip()
    # Beat 9.5 on a 4-beat bar: bar 2, beat 1.5 within it (0-based) — the
    # display complement of quant_delay, shared with the GUI's beats ruler.
    assert n.bar(9.5, 4.0) == pytest.approx(2.0)
    assert n.beat_in_bar(9.5, 4.0) == pytest.approx(1.5)
    # No grid: everything is bar 0, the position passes through.
    assert n.bar(9.5, 0.0) == 0.0
    assert n.beat_in_bar(9.5, 0.0) == pytest.approx(9.5)


def test_perceptual_frequency_scales_round_trip():
    n = _native_or_skip()
    # 1 kHz sits at ~1000 mel and ~8.5 bark; the closed forms invert exactly.
    assert n.hz_to_mel(1000.0) == pytest.approx(1000.0, abs=0.1)
    assert n.hz_to_bark(1000.0) == pytest.approx(8.53, abs=0.05)
    for hz in (100.0, 1000.0, 12_000.0):
        assert n.mel_to_hz(n.hz_to_mel(hz)) == pytest.approx(hz)
        assert n.bark_to_hz(n.hz_to_bark(hz)) == pytest.approx(hz)


def test_osc_bundle_builds():
    msg = osc.message("/synth_new", "default", 1000, 1, 0, "freq", 440.0)
    b = osc.score_bundle(0.0, msg)
    assert b.startswith(b"#bundle\x00")
    # framed score is a sequence of length-prefixed packets
    assert len(osc.score(b)) == 4 + len(b)


def test_render_default_synth():
    sc = osc.score(
        osc.score_bundle(
            0.0,
            osc.message("/synth_new", "default", 1000, 1, 0, "freq", 440.0, "amp", 0.2),
        ),
        osc.score_bundle(0.2, osc.message("/node_free", 1000)),
        osc.score_bundle(0.3, osc.message("/node_free", 0)),  # closes the render
    )
    try:
        _st0 = clausters.render(sc, sample_rate=48_000.0, channels=2)
        samples, frames = _st0.samples, _st0.frames
    except (OSError, RuntimeError, AttributeError) as e:
        # AttributeError: a libclausters was found but lacks the embed exports
        # (built without --features embed). Build it, or set CLAUSTERS_LIB.
        pytest.skip(f"embed library not built/usable: {e}")
    assert frames > 0
    assert len(samples) == frames * 2
    assert max(abs(s) for s in samples) > 0.0


if __name__ == "__main__":
    # Allow running without pytest installed: execute every test, skip-aware.
    import traceback

    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            try:
                fn()
                print(f"ok   {name}")
            except BaseException as e:  # noqa: BLE001 — smoke harness
                kind = type(e).__name__
                print(f"{'skip' if kind in ('Skipped', 'OutcomeException') else 'FAIL'} {name}: {e}")
                if kind not in ("Skipped", "OutcomeException"):
                    traceback.print_exc()
