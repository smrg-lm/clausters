"""C3 tests: Faust signal graphs, FaustDef payloads, resource allocators, the
Server round-trip (over a fake connection), and the end-to-end vertical slice
(build a graph -> /def_send faust -> /synth_new -> control -> render)."""

import pytest

from clausters.base.timebase import LogicalTimebase
from clausters.base import OscNrtInterface, Routine, TempoClock
from clausters.base import _osclib as osc
from clausters.defs import (
    AddAction,
    AudioBusAllocator,
    BufferAllocator,
    Bus,
    FaustDef,
    Group,
    NodeIdAllocator,
    NodeMap,
    Server,
    Synth,
    format_load,
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

def _spaces(**shape):
    """A client's id spaces over a server of this shape."""
    from clausters import _native
    base = dict(max_nodes=1, audio_buses=128, outputs=2, control_buses=128,
                buffers=4)
    base.update(shape)
    return _native.IdSpaces(**base)


def test_node_id_allocator_recycles_and_never_wraps():
    # max_nodes=1 gives the partition's client range 4 ids from 1000.
    a = NodeIdAllocator(_spaces(max_nodes=1))
    assert (a.alloc(), a.alloc()) == (1000, 1001)
    # Every freed id becomes allocatable again: with frees keeping pace the
    # space never exhausts, however many ids pass through.
    for _ in range(100):
        a.free(a.alloc())
    assert a.in_use == 2
    # Exhaustion is an explicit error, never a wrapped counter.
    a.alloc(), a.alloc()
    with pytest.raises(RuntimeError, match="out of node ids"):
        a.alloc()
    # Foreign ids (the server's ranges, other clients) are ignored quietly:
    # every /node_end on the server is reported here, not only ours.
    a.free(999)
    a.free(1004)


def test_node_id_allocator_unbounded_for_scores():
    a = NodeIdAllocator(_spaces(max_nodes=1, score=True))   # the NRT/score variant
    assert all(a.alloc() == 1000 + i for i in range(10_000))


def test_audio_bus_allocator_reserves_outputs_and_graph_top():
    a = AudioBusAllocator(_spaces(outputs=2))
    b2 = a.alloc(2)
    assert b2.index == 2 and b2.channels == 2     # above the 2 hardware outs
    assert a.alloc(1).index == 4
    a.free(b2)
    # Next-fit rotates: fresh space first, the freed run again on wrap.
    assert a.alloc(2).index == 5
    assert a.alloc(57).index == 7                  # rest of the free space
    assert a.alloc(2).index == 2                   # wrapped onto the freed run
    # The GraphDef private range is the **top half** of the space, never handed
    # out: 128 - 2 reserved - 64 = 62 allocatable. A share and not a fixed
    # count, so a server booted with more buses gives both sides more -- which
    # is the whole reason the count is configurable.
    a2 = AudioBusAllocator(_spaces(outputs=2))
    assert a2.alloc(62).index == 2
    with pytest.raises(RuntimeError, match="out of audio buses"):
        a2.alloc(1)


def test_the_audio_space_starts_above_the_servers_own_outputs():
    """The outputs reserved are the server's count, not a default of two."""
    assert AudioBusAllocator(_spaces(outputs=6)).alloc(1).index == 6


def test_splitting_a_share_keeps_what_is_held_and_gives_away_the_second_half():
    """A launcher that starts a second client on this server splits its ids:
    the handle keeps the first half and every id it holds, and hands out the
    other."""
    from clausters.base.ids import IdShare

    srv = Server(interface=_FakeInterface())
    held = srv.nodes.alloc()
    other = srv.split_share()
    assert (srv.share, other) == (IdShare(0, 2), IdShare(1, 2))
    assert srv.ids.contains(srv.ids.NODES, held)
    assert srv.nodes.alloc() == held + 1


def test_bus_commands_go_through_the_bus():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    bus = Bus.control(server=srv)
    assert bus.server is srv and bus.rate == "control"
    bus.set(0.25)
    assert iface.sent[-1] == ("/bus_set", [bus.index, 0.25])
    iface.queue_reply("/bus_get.reply", bus.index, 0.25)
    assert bus.get() == 0.25
    bus.free()
    assert srv.control_buses.in_use == 0


def test_bus_watch_taps_the_bus():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    bus = Bus.audio(2, server=srv)
    bus.watch()
    assert iface.sent[-1] == ("/bus_tap", [bus.index, 1])
    bus.watch(False)
    assert iface.sent[-1] == ("/bus_tap", [bus.index, 0])


def test_bus_allocator_refuses_double_free():
    a = AudioBusAllocator(_spaces(outputs=2))
    b = a.alloc(2)
    a.free(b)
    with pytest.raises(RuntimeError, match="double free"):
        a.free(b)


def test_buffer_allocator():
    a = BufferAllocator(_spaces(buffers=4))
    assert (a.alloc(), a.alloc()) == (0, 1)
    a.free(0)
    for _ in range(100):                           # recycles, never exhausts
        a.free(a.alloc())
    a.alloc(), a.alloc(), a.alloc()
    with pytest.raises(RuntimeError, match="out of buffer slots"):
        a.alloc()
    with pytest.raises(RuntimeError, match="double free"):
        a.free(5)


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


def test_synth_new_builds_s_new_correctly():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    synth = Synth("foo", {"freq": 440.0}, target=0, action=AddAction.TAIL,
                      server=srv)
    assert synth.id == 1000 and synth.defname == "foo" and synth.server is srv
    assert iface.sent[-1] == ("/synth_new", ["foo", 1000, 1, 0, "freq", 440.0])


def test_node_commands_go_through_the_node():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    node = Synth("foo", server=srv)
    node.set({"freq": 220.0})
    assert iface.sent[-1] == ("/node_set", [node.id, "freq", 220.0])
    node.free()
    assert iface.sent[-1] == ("/node_free", [node.id])


def test_group_new_and_graph_build_their_own_message():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    group = Group(server=srv)
    assert iface.sent[-1] == ("/group_new", [group.id, 1, 0])
    inst = Group.graph("chain", {"gain": 0.8}, server=srv)
    assert iface.sent[-1] == ("/graph_new", ["chain", inst.id, 1, 0, "gain", 0.8])
    voice = inst.voice({"freq": 440.0})
    assert iface.sent[-1] == ("/graph_newVoice", [inst.id, voice.id, "freq", 440.0])
    other = Group.graph("chain", server=srv)
    slot = inst.add_slot("clips")
    assert slot.move_slot(other) is slot
    assert iface.sent[-1] == ("/graph_moveSlot", [slot.id, other.id])


def test_a_group_orders_and_parallelizes_itself():
    # The two modes the server's own analysis drives, and the shape they share:
    # a flag on the group, answered by the group so it chains onto the
    # constructor. Both were reachable only by spelling the message until they
    # became verbs -- a client test doing `send_msg` was the tell.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    group = Group(server=srv)

    assert group.auto_order() is group
    assert iface.sent[-1] == ("/group_sortMode", [group.id, 1])
    group.auto_order(False)
    assert iface.sent[-1] == ("/group_sortMode", [group.id, 0])

    assert group.parallel() is group
    assert iface.sent[-1] == ("/group_parallel", [group.id, 1])
    group.parallel(False)
    assert iface.sent[-1] == ("/group_parallel", [group.id, 0])

    # Chained onto the creation, which is how a piece reads: one expression
    # that makes the group and says how it runs.
    band = Group(server=srv).parallel()
    assert iface.sent[-2] == ("/group_new", [band.id, 1, 0])
    assert iface.sent[-1] == ("/group_parallel", [band.id, 1])


def test_a_wait_for_done_ignores_another_commands_done():
    """A ``/done`` names the command it closes, so one left in flight by an
    earlier async send is not the next wait's to take.

    This is the bug that shape prevents: a def send answers ``/done`` and
    nobody waited for it, a buffer alloc waits for ``/done`` and takes that
    one, and the write that follows lands on a buffer the server has not
    allocated yet -- with no error anywhere to say why."""
    iface = _FakeInterface()
    srv = Server(interface=iface)
    # The def send's, still in flight, then this command's own.
    iface.queue_reply("/done", "/def_send", "graph")
    iface.queue_reply("/done", "/buffer_alloc", 5)
    addr, args = srv.request("/buffer_alloc", 5, 64, 1, expect=("/done", "/fail"))
    assert (addr, args[0]) == ("/done", "/buffer_alloc"), \
        "the wait went past the def send's done and took its own"


def test_a_fail_is_reported_to_whoever_is_waiting():
    """The rule stops at ``/done``: an error still ends the wait whatever
    command it names, which is what lets a batched write close on one barrier
    and still raise instead of losing the failure."""
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/fail", "/buffer_setRange", "no buffer allocated at 5")
    addr, args = srv.request("/server_sync", 1,
                             expect=("/server_sync.reply", "/fail"))
    assert (addr, args[0]) == ("/fail", "/buffer_setRange")


def test_faustdef_send_waits_for_done_and_raises_on_fail():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    fdef = FaustDef.from_source("ok", "process = _;")

    iface.queue_reply("/done", "/def_send", "faust", "ok")
    assert fdef.send(srv) == "ok"
    assert iface.sent[-1][0] == "/def_send"

    iface.queue_reply("/fail", "/def_send", "faust", "boom")
    with pytest.raises(RuntimeError):
        fdef.send(srv)

    # wait=False is fire-and-forget: sends /def_send faust without expecting a reply.
    assert fdef.send(srv, wait=False) == "ok"
    assert iface.sent[-1][0] == "/def_send"


def test_server_sync_round_trips_synced_id():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/server_sync.reply", 1)
    assert srv.sync() == 1
    assert iface.sent[-1][0] == "/server_sync"
    assert iface.sent[-1][1] == [1]


def test_node_map_and_set_layout():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    node = Synth("foo", server=srv)
    node.set({"in": 4.0, "out": 0.0})             # reserved controls via dict
    assert iface.sent[-1] == ("/node_set", [1000, "in", 4.0, "out", 0.0])
    bus = Bus.audio(1, server=srv)
    node.map("in", bus, audio=True)
    assert iface.sent[-1] == ("/node_mapAudio", [1000, "in", bus.index])


def test_server_stream_buses_subscribes_and_cancels():
    # /bus_stream: periodic /bus_set snapshots of control buses (the network
    # counterpart of the shared-memory segment, e.g. browser meters).
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/done", "/bus_stream")
    addr, args = srv.stream_buses(33, 10, Bus.control(server=srv))
    assert addr == "/done" and args[0] == "/bus_stream"
    assert iface.sent[-1] == ("/bus_stream", [33, 10, 0])
    # period <= 0 (or no buses) cancels the subscription.
    iface.queue_reply("/done", "/bus_stream")
    srv.stream_buses(0)
    assert iface.sent[-1] == ("/bus_stream", [0])


def test_node_run_pause_resume_emit_n_run():
    # S4: /node_run pauses (flag 0) / resumes (flag 1) a node.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    node = Synth("foo", server=srv)
    node.pause()
    assert iface.sent[-1] == ("/node_run", [1000, 0])
    node.resume()
    assert iface.sent[-1] == ("/node_run", [1000, 1])
    Group.from_id(1234, srv).run(False)                   # a handle for a reported id
    assert iface.sent[-1] == ("/node_run", [1234, 0])


def test_building_a_node_creates_it_and_from_id_only_names_one():
    # Building a Synth or a Group *is* creating it: the id comes from the
    # allocator and the command goes out. from_id names one that already
    # exists, and sends nothing.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    group = Group(server=srv)
    assert iface.sent[-1] == ("/group_new", [group.id, 1, 0])
    synth = Synth("blip", {"freq": 440}, target=group, server=srv)
    assert iface.sent[-1] == ("/synth_new", ["blip", synth.id, 1, group.id, "freq", 440.0])

    before = len(iface.sent)
    handle = Synth.from_id(4242, "blip", srv)
    assert (handle.id, handle.defname, handle.server) == (4242, "blip", srv)
    assert Group.from_id(99, srv).id == 99
    assert len(iface.sent) == before


def test_a_target_is_a_node_or_its_id():
    # target=group and target=group.id are the same thing.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    group = Group(server=srv)
    Synth("blip", target=group, server=srv)
    by_object = iface.sent[-1]
    Synth("blip", target=group.id, server=srv)
    by_id = iface.sent[-1]
    assert by_object[1][2:] == by_id[1][2:]


def test_the_governed_group_is_a_node_or_its_id():
    # transport_group used to be the one call that took the bare integer, so
    # `server.transport_group(monitor)` raised a TypeError where every other
    # place a node is named accepts the object.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    group = Group(server=srv)
    iface.queue_reply("/done", "/transport_group")
    srv.transport_group(group)
    assert iface.sent[-1] == ("/transport_group", [group.id])
    iface.queue_reply("/done", "/transport_group")
    srv.transport_group(None)
    assert iface.sent[-1] == ("/transport_group", [-1])


def test_records_print_readably_and_agree_with_their_container():
    # str is the readable line, repr stays the dataclass form -- and a Tree
    # draws a synth by printing its own NodeInfo, so the two cannot drift.
    from clausters.defs import BufferInfo, ControlInfo, DefInfo, NodeInfo, NodeMap, Tree

    node = NodeInfo(id=7, defname="beep", controls={"freq": 440.0, "amp": 0.2},
                    maps=[NodeMap(control=1, bus=3)])
    assert str(node) == "7 beep  freq=440 amp<-c3"
    assert str(Tree(NodeInfo(id=0, is_group=True, head=7), [Tree(node)])) == (
        "group 0\n  7 beep  freq=440 amp<-c3")
    assert "NodeInfo(" in repr(node)

    assert str(NodeInfo(id=9, is_group=True)) == "group 9 (empty)"
    assert str(NodeInfo(id=9, exists=False)) == "9 (gone)"
    assert str(BufferInfo(bufnum=2, frames=100, channels=1,
                          sample_rate=48000.0)) == "buffer 2: 100 frames x 1 ch @ 48000 Hz"
    assert str(BufferInfo(bufnum=2, frames=0, channels=0, sample_rate=0.0,
                          exists=False)) == "buffer 2 (empty)"
    assert str(DefInfo("beep", "synth", [ControlInfo("freq", 440.0)])) == (
        "beep (synth): freq=440 kr")
    assert str(DefInfo("nope", "", [], exists=False)) == "nope (not loaded)"


def test_done_action_full_set():
    # S4: the client mirrors scsynth's full 0-15 done-action enum.
    from clausters.defs import DoneAction

    assert (DoneAction.NONE, DoneAction.PAUSE_SELF, DoneAction.FREE_SELF) == (0, 1, 2)
    assert DoneAction.FREE_SELF_AND_NEXT == 4
    assert DoneAction.FREE_ALL_IN_GROUP == 13
    assert DoneAction.FREE_GROUP == 14
    assert DoneAction.FREE_SELF_RESUME_NEXT == 15


# ---- end-to-end vertical slice: graph -> /def_send faust -> /synth_new -> control -> render ----

def _sine_def(name="c3sine", default_freq=330.0):
    freq = S.hslider("freq", default_freq, 20.0, 20000.0, 0.01)
    phasor = S.rec(lambda s: (s + freq / 48000.0) % 1.0)
    return FaustDef.from_signals(name, S.sin(phasor * 6.283185307179586) * 0.2)


def test_faustdef_renders_through_the_seam():
    _ffi_or_skip()
    fdef = _sine_def()
    server = Server(interface=OscNrtInterface())   # NRT mode (no live server)
    clock = TempoClock(tempo=1.0, timebase=LogicalTimebase())

    def play():
        # def first, then instantiate (same beat; score keeps insertion order)
        server.send_bundle(("/def_send", "faust", fdef.name, fdef.dump_def()))
        server.send_bundle(("/synth_new", fdef.name, 1000, 1, 0))
        yield 0.5
        server.send_bundle(("/node_set", 1000, "freq", 660.0))  # control by clock
        yield 0.5
        server.send_bundle(("/node_free", 1000))
        server.send_bundle(("/node_free", 0))                   # closes the render

    clock.play(Routine(play))
    clock.render()
    try:
        _st0 = server.render(sample_rate=48_000.0, channels=2)
        samples, frames = _st0.samples, _st0.frames
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
    from clausters.defs.info import parse_query_tree
    # detail=2; root 0 -> group 1000 "voices" -> synth 1001 (beep, freq
    # mapped to c5). Every entry is id, childCount, name.
    args = [2, 0, 1, "", 1000, 1, "voices", 1001, -1, "beep", 2,
            "freq", 330.0, "amp", 0.2, 1, 0, 5, 0, "-", "0"]
    tree = parse_query_tree(args)
    assert tree.id == 0 and tree.info.is_group and tree.info.head == 1000
    group = tree.children[0]
    assert group.info.is_group and group.info.parent == 0
    assert (group.info.head, group.info.tail) == (1001, 1001)

    # Every entry is a full NodeInfo: what the tree adds is the nesting, and
    # the siblings and head/tail follow from it.
    synth = group.children[0].info
    assert synth.id == 1001 and synth.defname == "beep" and synth.parent == 1000
    assert synth.controls == {"freq": 330.0, "amp": pytest.approx(0.2)}
    assert synth.maps == [NodeMap(control=0, bus=5, audio=False)]
    assert (synth.reads, synth.writes) == ("-", "0")
    assert [i.id for i in tree.walk()] == [0, 1000, 1001]
    assert tree.find(1001).info is synth

    # repr identifies, str draws.
    assert repr(tree) == "Tree(0 group, 1 children)"
    assert group.info.name == "voices"
    assert str(tree).splitlines() == [
        "group 0",
        '  group 1000 "voices"',
        "    1001 beep  freq<-c5 amp=0.2",
    ]


def test_parse_query_tree_siblings_and_empty_group():
    from clausters.defs.info import parse_query_tree
    # detail=0: no controls on the wire, three children of the root.
    args = [0, 0, 3, "", 1001, -1, "a", 1002, -1, "b", 100, 0, ""]
    tree = parse_query_tree(args)
    a, b, empty = (t.info for t in tree.children)
    assert (a.prev, a.next) == (-1, 1002)
    assert (b.prev, b.next) == (1001, 100)
    assert empty.is_group and (empty.head, empty.tail) == (-1, -1)
    assert str(tree).splitlines()[-1] == "  group 100 (empty)"


def test_parse_n_info_synth_group_and_absent():
    from clausters.defs.info import parse_n_info
    synth = [1001, 1000, -1, -1, 0, "beep", 1, "freq", 330.0, 1, 0, 5, 0, "-", "0"]
    info = parse_n_info(synth)
    assert info.id == 1001 and info.parent == 1000 and not info.is_group
    assert info.exists
    assert info.defname == "beep" and info.controls == {"freq": pytest.approx(330.0)}
    assert info.maps == [NodeMap(control=0, bus=5, audio=False)]
    assert info.reads == "-" and info.writes == "0"

    group = [1000, 0, -1, -1, 1, 1001, 1001, "voices"]
    g = parse_n_info(group)
    assert g.is_group and g.head == 1001 and g.tail == 1001 and g.exists
    assert g.name == "voices"

    # isGroup = -1: the node is not there. A state, not an exception.
    gone = parse_n_info([4242, -1, -1, -1, -1])
    assert gone.id == 4242 and not gone.exists


def test_quit_drops_the_connection_so_the_next_boot_is_reachable():
    # The sequence that used to strand a handle: quit the server, boot another
    # on the same address, ask it anything. The carrier held a connection to
    # the process that had stopped, so the first request timed out and the next
    # raised BrokenPipeError -- with every command, not just the new one.
    class _Carrier(_FakeInterface):
        disconnects = 0

        def disconnect(self):
            type(self).disconnects += 1

    iface = _Carrier()
    srv = Server(interface=iface)
    srv.quit()
    assert iface.sent[-1][0] == "/server_quit"
    assert _Carrier.disconnects == 1, "the handle says so; no socket can tell it"


def test_a_carrier_without_the_verb_is_not_broken_by_it():
    # Asked, not required -- the same posture the event loop takes to `gone`:
    # a carrier with no connection to drop (a score, an in-process engine, one
    # this module never heard of) is handed nothing it must implement.
    iface = _FakeInterface()
    assert not hasattr(iface, "disconnect")
    Server(interface=iface).quit()
    assert iface.sent[-1][0] == "/server_quit"


def test_load_differences_a_window_of_its_own():
    # The server's counters are cumulative since boot, so the first call has no
    # interval to measure and every one after it reports the share of the time
    # since *this* client's previous call -- which is what lets two clients
    # poll one server without disturbing each other.
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/server_load.reply", 10.0, 3,
                      "audio", 0, 1.0, 100, "dsp", 0, 0.2, 40, "net", 0, 0.5, 40)
    first = srv.load()
    assert [row.name for row in first] == ["audio", "dsp 0", "net"]
    assert all(row.share is None for row in first)

    iface.queue_reply("/server_load.reply", 12.0, 3,
                      "audio", 0, 1.4, 130, "dsp", 0, 0.2, 40, "net", 0, 0.6, 55)
    second = srv.load()
    assert second[0].share == pytest.approx(0.2)    # 0.4 s of work in 2 s
    assert second[1].share == pytest.approx(0.0)    # a worker that took no stage
    assert second[2].share == pytest.approx(0.05)
    assert second[0].calls == 130 and second[0].busy == pytest.approx(1.4)

    block = format_load(second)
    assert block.splitlines()[0] == "server load"
    assert "dsp 0" in block and "20.0%" in block


def test_defs_query_collects_until_done():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    # Two /def_query.reply replies then the /done terminator that closes the batch.
    iface.queue_reply("/def_query.reply", "one", "synth", 1, "freq", 440.0, "kr")
    iface.queue_reply("/def_query.reply", "two", "graph", 0)
    iface.queue_reply("/done", "/def_query")

    infos = srv.query_defs()
    assert iface.sent[-1] == ("/def_query", [])
    assert [d.name for d in infos] == ["one", "two"]
    assert [d.family for d in infos] == ["synth", "graph"]
    assert infos[0].controls[0].name == "freq"
    assert infos[0].controls[0].default == 440.0
    assert infos[0].controls[0].rate == "kr"


def test_defs_query_passes_names_and_flags_unknown():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/def_query.reply", "nope", "", 0)
    iface.queue_reply("/done", "/def_query")

    infos = srv.query_defs("nope")
    assert iface.sent[-1] == ("/def_query", ["nope"])
    # An unknown def is reported, not raised: one bad name never fails a batch.
    assert infos[0].exists is False
    assert infos[0].controls == []


def test_parse_def_info_faust_ranges_and_graph_targets():
    from clausters.defs.info import parse_def_info as _parse_def_info

    # A faust param appends (min, max, step) after the shared triple...
    faust = _parse_def_info(["f", "faust", 1, "amp", 0.2, "kr", 0.0, 1.0, 0.001])
    assert faust.controls[0].min == 0.0 and faust.controls[0].max == 1.0
    assert faust.controls[0].step == 0.001

    # ...and a graph port appends its target count and the tuples it drives.
    graph = _parse_def_info(
        ["g", "graph", 1, "gain", 0.5, "kr", 2,
         0, "level", 1.0, 0.0,
         1, "amp", 0.5, 0.25]
    )
    port = graph.controls[0]
    assert port.default == 0.5
    assert port.targets == ((0, "level", 1.0, 0.0), (1, "amp", 0.5, 0.25))
    # The other families declare no range.
    assert port.min is None


def test_ugens_query_parses_signatures():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/ugen_query.reply", "Sine", 1, "ar", "kr,ar", "normal", "", 0, "", "",
                      1, "freq", 440.0)
    iface.queue_reply("/ugen_query.reply", "EnvGen", -1, "ar", "ar", "normal", "", 0, "", "",
                      2, "gate", 1.0, "level_scale", 1.0)
    iface.queue_reply("/done", "/ugen_query")

    ugens = srv.query_ugens()
    assert iface.sent[-1] == ("/ugen_query", [])
    sine, env = ugens
    assert sine.arity == 1 and sine.variadic is False
    assert sine.rates == ("kr", "ar") and sine.default_rate == "ar"
    assert [(i.name, i.default) for i in sine.inputs] == [("freq", 440.0)]
    # A variadic kind names only its fixed head.
    assert env.variadic is True and env.arity == -1
    assert [i.name for i in env.inputs] == ["gate", "level_scale"]


def test_buffers_query_lists_allocated_slots():
    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/buffer_query.reply", 0, 50, 1, 48000.0, 3, 100, 2, 44100.0)

    bufs = srv.query_buffers()
    assert iface.sent[-1] == ("/buffer_query", [])
    assert [(b.bufnum, b.frames, b.channels) for b in bufs] == [(0, 50, 1), (3, 100, 2)]
    assert bufs[1].sample_rate == 44100.0


def test_introspection_batch_raises_on_fail():
    from clausters.errors import CommandError

    iface = _FakeInterface()
    srv = Server(interface=iface)
    iface.queue_reply("/fail", "/def_query", "expected string def names")
    with pytest.raises(CommandError):
        srv.query_defs()
