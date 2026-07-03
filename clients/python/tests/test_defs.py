"""C3 tests: Faust signal graphs, FaustDef payloads, resource allocators, the
Server round-trip (over a fake connection), and the end-to-end vertical slice
(build a graph -> /d_faust -> /s_new -> control -> render)."""

import pytest

from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.base import _osclib as osc
from clausters.defs import (
    AddAction,
    AudioBusAllocator,
    BufferAllocator,
    FaustDef,
    NodeIdAllocator,
    Server,
)
from clausters.defs import signals as S


def _ffi_or_skip():
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


# ---- signals: lowercase callables compose the JSON signal tree ----

def test_signal_functions_and_operators_build_the_tree():
    freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
    expr = S.sin(freq * 2.0) * 0.5
    node = expr.to_json()
    assert node == {
        "op": "mul",
        "in": [
            {"op": "sin", "in": [{"op": "mul", "in": [
                {"op": "hslider", "label": "freq", "init": 330.0,
                 "min": 20.0, "max": 20000.0, "step": 0.01},
                2.0]}]},
            0.5,
        ],
    }


def test_recursion_and_self():
    phasor = S.rec(lambda s: (s + 0.01) % 1.0)
    node = phasor.to_json()
    assert node["op"] == "recursion"
    assert node["in"][0]["op"] == "rem"          # `% 1.0`
    assert node["in"][0]["in"][0]["in"][0] == {"op": "self"}


def test_foreign_constant_and_sample_rate():
    # fconst/fvar carry ctype/name/file; sr() is the ma.SR clamp built on them.
    assert S.fconst("int", "fSamplingFreq", "<math.h>").to_json() == {
        "op": "fconst", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}
    assert S.fvar("real", "x").to_json() == {
        "op": "fvar", "ctype": "real", "name": "x", "file": ""}
    sr = S.sr().to_json()
    assert sr["op"] == "min"                      # min(192000, max(1, fconst))
    assert sr["in"][0] == 192000.0
    inner = sr["in"][1]
    assert inner["op"] == "max" and inner["in"][0] == 1.0
    assert inner["in"][1] == {
        "op": "fconst", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}


def test_pi_and_tau_are_plain_literals():
    # PI/TAU are floats (ma.PI is a literal too), becoming constants in graphs.
    assert S.TAU == pytest.approx(2.0 * S.PI)
    assert (S.sin(S.TAU) * 1.0).to_json()["in"][0]["in"][0] == pytest.approx(S.TAU)


# ---- FaustDef payloads and controls ----

def test_faustdef_signal_dump_and_controls():
    import json

    freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
    fdef = FaustDef.from_signals("d", S.sin(freq) * 0.2)
    payload = json.loads(fdef.dump_def())
    assert list(payload) == ["signals"] and len(payload["signals"]) == 1
    assert fdef.control_names() == ["freq"]
    assert fdef.reserved == ("out", "in")


def test_faustdef_source_dump():
    fdef = FaustDef.from_source("s", "process = _;")
    assert fdef.dump_def() == "process = _;"


# ---- resource allocators ----

def test_node_id_allocator_reuses_freed():
    a = NodeIdAllocator(start=1000)
    assert (a.alloc(), a.alloc()) == (1000, 1001)
    a.free(1000)
    assert a.alloc() == 1000


def test_audio_bus_allocator_reserves_outputs():
    a = AudioBusAllocator(size=128, reserved=2)
    b2 = a.alloc(2)
    assert b2.index == 2 and b2.channels == 2     # above the 2 hardware outs
    assert a.alloc(1).index == 4
    a.free(b2)
    assert a.alloc(2).index == 2                   # exact-width reuse


def test_buffer_allocator():
    a = BufferAllocator()
    assert (a.alloc(), a.alloc()) == (0, 1)
    a.free(0)
    assert a.alloc() == 0


# ---- Server over a fake communication interface ----

class _FakeInterface:
    """A Server communication interface that records sent messages and replays
    queued replies — the Server's comms surface, no socket."""

    time_mode = "unix"

    def __init__(self):
        self.sent = []          # decoded (addr, args)
        self._replies = []      # queued reply packets (bytes)

    def queue_reply(self, addr, *args):
        self._replies.append(osc.message(addr, *args))

    def send_msg(self, target, addr, *args):
        self.sent.append((addr, list(args)))

    def send_bundle(self, target, when, *messages):
        for m in messages:
            self.sent.append((m[0], list(m[1:])))

    def recv(self, timeout):
        return self._replies.pop(0) if self._replies else None

    def close(self):
        pass


def test_server_builds_s_new_correctly():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    synth = srv.synth("foo", {"freq": 440.0}, target=0, action=AddAction.TAIL)
    assert synth.id == 1000 and synth.defname == "foo"
    assert iface.sent[-1] == ("/s_new", ["foo", 1000, 1, 0, "freq", 440.0])


def test_server_add_faustdef_waits_for_done_and_raises_on_fail():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    fdef = FaustDef.from_source("ok", "process = _;")

    iface.queue_reply("/done", "/d_faust", "ok")
    assert srv.add_faustdef(fdef) == "ok"
    assert iface.sent[-1][0] == "/d_faust"

    iface.queue_reply("/fail", "/d_faust", "boom")
    with pytest.raises(RuntimeError):
        srv.add_faustdef(fdef)

    # wait=False is fire-and-forget: sends /d_faust without expecting a reply.
    assert srv.add_faustdef(fdef, wait=False) == "ok"
    assert iface.sent[-1][0] == "/d_faust"


def test_server_sync_round_trips_synced_id():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/synced", 1)
    assert srv.sync() == 1
    assert iface.sent[-1][0] == "/sync"
    assert iface.sent[-1][1] == [1]


def test_server_map_and_set_layout():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    node = srv.synth("foo")
    srv.set(node, {"in": 4.0, "out": 0.0})        # reserved controls via dict
    assert iface.sent[-1] == ("/n_set", [1000, "in", 4.0, "out", 0.0])
    bus = srv.audio_bus(1)
    srv.map(node, "in", bus, audio=True)
    assert iface.sent[-1] == ("/n_mapa", [1000, "in", bus.index])


def test_server_run_pause_resume_emit_n_run():
    # S4: /n_run pauses (flag 0) / resumes (flag 1) a node.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    node = srv.synth("foo")
    srv.pause(node)
    assert iface.sent[-1] == ("/n_run", [1000, 0])
    srv.resume(node)
    assert iface.sent[-1] == ("/n_run", [1000, 1])
    srv.run(1234, False)                          # a bare id, a whole group
    assert iface.sent[-1] == ("/n_run", [1234, 0])


def test_done_action_full_set():
    # S4: the client mirrors scsynth's full 0-15 done-action enum.
    from clausters.defs import DoneAction

    assert (DoneAction.NONE, DoneAction.PAUSE_SELF, DoneAction.FREE_SELF) == (0, 1, 2)
    assert DoneAction.FREE_SELF_AND_NEXT == 4
    assert DoneAction.FREE_ALL_IN_GROUP == 13
    assert DoneAction.FREE_GROUP == 14
    assert DoneAction.FREE_SELF_RESUME_NEXT == 15


# ---- end-to-end vertical slice: graph -> /d_faust -> /s_new -> control -> render ----

def _sine_def(name="c3sine", default_freq=330.0):
    freq = S.hslider("freq", default_freq, 20.0, 20000.0, 0.01)
    phasor = S.rec(lambda s: (s + freq / 48000.0) % 1.0)
    return FaustDef.from_signals(name, S.sin(phasor * 6.283185307179586) * 0.2)


def test_faustdef_renders_through_the_seam():
    _ffi_or_skip()
    fdef = _sine_def()
    server = Server(interface=OscNrtInterface())   # NRT mode (no live server)
    clock = TempoClock(tempo=1.0)

    def play():
        # def first, then instantiate (same beat; score keeps insertion order)
        server.send_bundle(("/d_faust", fdef.name, fdef.dump_def()))
        server.send_bundle(("/s_new", fdef.name, 1000, 1, 0))
        yield 0.5
        server.send_bundle(("/n_set", 1000, "freq", 660.0))  # control by clock
        yield 0.5
        server.send_bundle(("/n_free", 1000))
        server.send_bundle(("/n_free", 0))                   # closes the render

    clock.play(Routine(play))
    clock.render()
    try:
        samples, frames = server.render(sample_rate=48_000.0, channels=2)
    except (OSError, RuntimeError, AttributeError) as e:
        pytest.skip(f"embed+faust library not built/usable: {e}")
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


# ---- node-tree reply parsers (server-free) ----

def test_parse_query_tree():
    from clausters.defs.server import _parse_query_tree
    # flag=1; root 0 -> group 1000 -> synth 1001 (beep, freq/amp)
    args = [1, 0, 1, 1000, 1, 1001, -1, "beep", 2, "freq", 330.0, "amp", 0.2]
    tree = _parse_query_tree(args)
    assert tree == {
        "id": 0,
        "children": [{
            "id": 1000,
            "children": [{
                "id": 1001, "def": "beep",
                "controls": {"freq": 330.0, "amp": pytest.approx(0.2)},
            }],
        }],
    }


def test_parse_n_info_synth_and_group():
    from clausters.defs.server import _parse_n_info
    synth = [1001, 1000, -1, -1, 0, "beep", 1, "freq", 330.0, 1, 0, 5, 0, "-", "0"]
    info = _parse_n_info(synth)
    assert info["id"] == 1001 and info["parent"] == 1000 and not info["is_group"]
    assert info["def"] == "beep" and info["controls"] == {"freq": pytest.approx(330.0)}
    assert info["maps"] == [{"control": 0, "bus": 5, "audio": False}]
    assert info["reads"] == "-" and info["writes"] == "0"

    group = [1000, 0, -1, -1, 1, 1001, 1001]
    g = _parse_n_info(group)
    assert g["is_group"] and g["head"] == 1001 and g["tail"] == 1001
