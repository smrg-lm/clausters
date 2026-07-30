"""Automation (the control-vector lane): the break-point curve is discretized
into a control buffer on the server (``/b_gen "env"``) and played onto a control
bus by the lane synth (``OutCtl``). A ``readbus`` synth exposes that control bus
as audio, so the rendered signal *is* the curve — proving the whole path
(``/b_gen "env"`` + ``OutCtl`` + the lane) end to end through the offline render.
"""

import pytest

from clausters import render
from clausters.base import OscNrtInterface, TempoClock
from clausters.base.stream import Routine
from clausters.defs import Server
from clausters.defs.synthdef import SynthDef
from clausters.defs.ugens import Env, control, in_ctl, out
from clausters.seq.automation import Automation, _env_gen_args


def _embed_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def test_env_gen_args_layout():
    # level0, then (level, time, shape, curve) per segment; shape resolved to
    # the server's numeric envelope-shape codes (1 = linear, 2 = exponential).
    env = Env([0.2, 0.8, 0.5], [1.0, 2.0], ["lin", "exp"])
    assert _env_gen_args(env) == [0.2, 0.8, 1.0, 1, 0.0, 0.5, 2.0, 2, 0.0]


def test_from_points_round_trips_through_env():
    # A bpf breakpoint list -> Env -> back: the segment shape lives on the point
    # the segment leaves (the last point is a placeholder).
    auto = Automation.from_points([(0, 0.2, 1, 0.0), (2, 0.8, 1, 0.0)], target=None)
    assert auto.duration() == 2.0
    assert auto.to_points()[:2] == [0.0, 0.2]


def test_automation_drives_control_bus_matches_curve():
    _embed_or_skip()
    sr = 48000

    server = Server(interface=OscNrtInterface())
    clock = TempoClock(tempo=1.0)
    # Expose a control bus as audio on out 0, so the render captures the curve.
    server.add_synthdef(SynthDef("readbus", out(0, in_ctl(control("bus", 0.0, "ir")))))

    # Linear 0.2 -> 0.8 over 2 beats (= 2 s at tempo 1).
    auto = Automation.from_points([(0, 0.2, 1, 0.0), (2, 0.8, 1, 0.0)], target=None)
    auto.prepare(server)
    server.synth("readbus", {"bus": auto.bus.index})

    def routine():
        auto.play(server)
        yield 2.1
        server.send_bundle(("/n_free", 0))  # close the score

    clock.play(Routine(routine))
    clock.render()

    try:
        _st0 = render(server.interface.score.bytes())
        samples, frames = _st0.samples, _st0.frames
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed library not built/usable: {e}")

    def value_at(sec):
        return samples[int(sec * sr) * 2]  # interleaved stereo, channel 0

    # The rendered control follows the line (discretization + interpolation
    # leave a small tolerance).
    assert value_at(0.05) == pytest.approx(0.2, abs=0.03)
    assert value_at(1.0) == pytest.approx(0.5, abs=0.03)
    assert value_at(1.9) == pytest.approx(0.8, abs=0.03)
