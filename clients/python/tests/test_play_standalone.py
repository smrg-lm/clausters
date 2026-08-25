"""The default session and the free-standing ``play``: sounding without a
`Session`. Everything that does not run in an explicit session runs in the
default session (`main`); a bare event plays immediately (no clock), self-
releasing on wall time.
"""

import pytest

from clausters import Event, Routine, play, main, default_session
from clausters.defs import (
    FaustDef, Server, SynthDef, as_def, boxes, chans, control, disk_out,
    expr_channels, out, send_trig, signals, sine,
)
from clausters.defs.ugens import (
    detect_silence, free_self, free_self_when_done, line, pause_self,
)
from clausters.defs.node import Group, Synth
from clausters.base._oscinterface import OscNrtInterface
from clausters.seq.pattern import Pbind, Pseq
from clausters.seq.timeline import Playhead, Timeline
from clausters.defs import Buffer


@pytest.fixture
def clean_default():
    """Save/restore the process-wide default-session slots a test mutates."""
    server, clock = main.server, main.default_clock
    main.server, main.default_clock = None, None
    yield
    main.server, main.default_clock = server, clock


def _nrt_server():
    return Server(interface=OscNrtInterface())


def test_default_session_is_main():
    assert default_session is main


def test_resolve_server_raises_when_unbooted(clean_default):
    with pytest.raises(RuntimeError):
        main.resolve_server()


def test_plain_server_does_not_adopt_default(clean_default):
    # Only a free-standing Server().boot() adopts the default session; a plain
    # constructor (as Session.nrt/embed use) must not.
    _nrt_server()
    assert main.server is None


def test_event_play_resolves_ambient_server(clean_default):
    server = _nrt_server()
    main.server = server               # stands in for a free-standing boot
    node_id = Event(degree=0).play()   # no destination: resolves the default
    assert node_id is not None
    # /synth_new immediate at t=0, release scheduled at t=sustain (dur*legato=0.8).
    times = sorted(t for t, _ in server.interface.score.bundles)
    assert times == pytest.approx([0.0, 0.8])


def test_event_play_immediate_uses_send_msg_and_release(clean_default):
    server = _nrt_server()
    ev = Event(dur=2.0, legato=0.5)    # sustain = 1.0
    ev.play(server)                    # explicit destination, no clock
    times = sorted(t for t, _ in server.interface.score.bundles)
    assert times == pytest.approx([0.0, 1.0])


def test_free_play_dispatches_event(clean_default):
    server = _nrt_server()
    main.server = server
    node_id = play(Event(degree=2))
    assert node_id is not None
    assert len(server.interface.score.bundles) == 2


def test_free_play_rejects_unplayable(clean_default):
    with pytest.raises(TypeError):
        play(object())


def test_get_default_clock_belongs_to_default_session(clean_default):
    clock = main.get_default_clock(start=False)
    assert clock.session is main
    assert main.get_default_clock(start=False) is clock   # created once


def test_routine_play_resolves_the_default_clock(clean_default):
    """A bare ``Routine(f).play()`` needs no session, no server and no clock of
    its own: it lands on the default session's, created and started on demand."""
    r = Routine(lambda: (yield 1.0))
    try:
        assert r.play() is r
        assert main.default_clock is not None and main.default_clock._running
    finally:
        if main.default_clock is not None:
            main.default_clock.stop()


def test_routine_play_runs_on_an_existing_default_clock(clean_default):
    beats = []

    def gen():
        beats.append(main.current_tt._logical_beat)
        yield 1.0
        beats.append(main.current_tt._logical_beat)

    clock = main.get_default_clock(start=False)   # already there: not started
    Routine(gen).play()
    assert not clock._running
    clock.render()
    assert beats == [0.0, 1.0]


def test_routine_run_plays_as_a_decorator(clean_default):
    """``@Routine.run`` leaves the name bound to a routine already scheduled --
    it plays, as in sclang; it is not a constructor alias."""
    beats = []
    clock = main.get_default_clock(start=False)

    @Routine.run
    def melody():
        beats.append(main.current_tt._logical_beat)
        yield 0.5
        beats.append(main.current_tt._logical_beat)

    assert isinstance(melody, Routine)
    clock.render()
    assert beats == [0.0, 0.5]


def test_routine_pause_keeps_its_place_and_stop_rewinds(clean_default):
    seen = []

    def gen():
        for i in range(4):
            seen.append(i)
            yield 1.0

    clock = main.get_default_clock(start=False)

    r = Routine(gen).play()
    clock.render(until_beat=1.0)          # two wakes: 0 and 1
    assert seen == [0, 1] and r.state == "running"

    r.pause()
    clock.render()                        # nothing left in the queue
    assert seen == [0, 1] and r.state == "paused"

    r.play()                              # resumes at the yield it stopped on
    clock.render()
    assert seen == [0, 1, 2, 3]

    seen.clear()
    r.stop()                              # rewound: the next play starts over
    assert r.state == "init"
    r.play()
    clock.render()
    assert seen == [0, 1, 2, 3]


def test_a_raising_routine_does_not_take_the_clock_down(clean_default, capsys):
    """The clock drives every other routine: one that raises loses its place in
    the schedule and nothing else."""
    survivor = []

    def boom():
        yield 1.0
        raise ValueError("the routine's problem, not the clock's")

    def other():
        for _ in range(3):
            survivor.append(1)
            yield 1.0

    clock = main.get_default_clock(start=False)
    bad = Routine(boom).play()
    Routine(other).play()
    clock.render()

    assert survivor == [1, 1, 1]          # the clock kept driving the other one
    assert bad.state == "done"            # ...and dropped the raising one
    assert "ValueError" in capsys.readouterr().err


def test_free_play_accepts_an_event_dict(clean_default):
    server = _nrt_server()
    main.server = server
    node_id = play({"degree": 2, "dur": 0.5})
    assert node_id is not None
    assert len(server.interface.score.bundles) == 2   # /synth_new + release


def test_free_play_accepts_a_generator(clean_default):
    beats = []

    def gen():
        beats.append(main.current_tt._logical_beat)
        yield 1.0
        beats.append(main.current_tt._logical_beat)

    # Both forms: the genfunc, and an already-created generator object.
    play(gen, clock=main.get_default_clock(start=False))
    main.default_clock.render()
    assert beats == [0.0, 1.0]

    beats.clear()
    play(gen(), clock=main.default_clock)   # the clock resumes past beat 1.0
    main.default_clock.render()
    assert len(beats) == 2 and beats[1] - beats[0] == 1.0


def test_free_play_sounds_a_bare_ugen_expression(clean_default):
    server = _nrt_server()
    main.server = server
    node = play(sine(440.0) * 0.1)
    assert isinstance(node, Synth)
    # the ephemeral def (/def_send synth at 0) plus its /synth_new
    assert len(server.interface.score.bundles) == 2


def test_free_play_instances_a_def_with_controls(clean_default):
    server = _nrt_server()
    main.server = server
    from clausters.defs import control

    sdef = SynthDef("beep", out(0.0, sine(control("freq", 440.0))))
    node = play(sdef, controls={"freq": 220.0})
    assert isinstance(node, Synth) and node.defname == "beep"


def test_free_play_plays_a_timeline(clean_default):
    server = _nrt_server()
    main.server = server
    tl = Timeline()
    tl.add(0.0, Event(degree=0, dur=0.5))
    tl.add(1.0, Event(degree=2, dur=0.5))
    playhead = play(tl, clock=main.get_default_clock(start=False))
    assert isinstance(playhead, Playhead)
    main.default_clock.render()
    assert len(server.interface.score.bundles) == 4   # two notes, two releases


def test_play_rejects_an_arrangement_element_with_a_pointer_to_render(clean_default):
    from clausters.form import Clang

    with pytest.raises(TypeError, match="render"):
        play(Clang(Event(degree=0)))


def test_free_play_sounds_a_buffer_through_the_stock_instrument(clean_default):
    server = _nrt_server()
    main.server = server
    buf = Buffer.alloc(4800, 1, server=server)          # 0.1 s at 48 kHz
    node = play(buf)
    assert isinstance(node, Synth) and node.defname == "_playbuf1"
    # /buffer_alloc + /def_send synth + /synth_new at 0, /node_free when the take ends.
    times = sorted(t for t, _ in server.interface.score.bundles)
    assert times[-1] == pytest.approx(0.1)
    # `rate` is a musical ratio: it scales the free time too (fresh score).
    server2 = _nrt_server()
    main.server = server2
    buf2 = Buffer.alloc(4800, 1, server=server2)
    play(buf2, controls={"rate": 2.0})
    times = sorted(t for t, _ in server2.interface.score.bundles)
    assert times[-1] == pytest.approx(0.05)


def test_play_buffer_stock_instrument_renders_audible_output(clean_default):
    # End to end through the offline render: the stock def actually compiles
    # (BufSampleRate/SampleRate rescaling included) and sounds the take.
    try:
        from clausters import _native
        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")
    from clausters import Session
    from clausters.defs import Env
    from clausters.seq.automation import _env_gen_args

    session = Session.nrt(tempo=1.0)
    server = session.server
    buf = Buffer.alloc(4800, 1, server=server)          # 0.1 s at 48 kHz
    # Fill it with a constant 1.0 (the env generator, level 1 throughout).
    buf.gen("env", *_env_gen_args(Env([1.0, 1.0], [1.0])))
    play(buf, server=server)
    _st0 = session.render(sample_rate=48_000.0, channels=1)
    samples, frames = _st0.samples, _st0.frames
    assert frames >= 4800
    assert max(abs(x) for x in samples) > 0.9, "the take sounds at unity"


def test_free_play_triggers_an_automation_immediately(clean_default):
    from clausters.defs import Env
    from clausters.seq.automation import Automation

    server = _nrt_server()
    main.server = server
    auto = Automation(Env([200.0, 800.0, 200.0], [0.1, 0.3]),
                      target=(5, "freq"))
    node = play(auto)                           # prepares and triggers
    assert node is not None
    assert auto.buf is not None and auto.bus is not None
    # Outside a clock the curve's beats read as seconds: freed at 0.4.
    times = sorted(t for t, _ in server.interface.score.bundles)
    assert times[-1] == pytest.approx(0.4)


def test_free_play_falls_back_to_the_timeline_item_protocol(clean_default):
    server = _nrt_server()
    main.server = server
    seen = []

    class Item:
        def play(self, destination):
            seen.append(destination)
            return "played"

    assert play(Item()) == "played"
    assert seen == [server]


def _addrs(server) -> list:
    """The OSC addresses in the NRT score, in insertion order."""
    import struct

    from clausters.base import _osclib as osc

    out = []
    for _t, raw in server.interface.score.bundles:
        length = struct.unpack(">i", raw[16:20])[0]
        addr, _ = osc.decode(raw[20:20 + length])
        out.append(addr)
    return out


def test_node_handles_free_themselves(clean_default):
    server = _nrt_server()
    main.server = server
    node = play(sine(440.0) * 0.1)       # a Synth handle, server attached
    assert node.server is server
    node.free()
    assert _addrs(server)[-1] == "/node_free"


def test_event_play_returns_the_completed_event(clean_default):
    from clausters.base.builtins import midicps

    server = _nrt_server()
    main.server = server
    e = play({"degree": 0, "dur": 8.0})     # long on purpose: interruptible
    assert isinstance(e, Event)
    assert e["node"] is not None and e["server"] is server
    assert e["freq"] == pytest.approx(midicps(60.0))
    assert e["sustain"] == pytest.approx(8.0 * 0.8)
    e.free()                                # cut it now, sustain be damned
    assert _addrs(server)[-1] == "/node_free"


def test_event_release_closes_the_gate_or_frees(clean_default):
    server = _nrt_server()
    main.server = server
    gated = play(Event(instrument="default", degree=0, dur=8.0))
    gated.release()                         # the default releases by gate
    assert _addrs(server)[-1] == "/node_set"
    plain = play(Event(instrument="beep", degree=0, dur=8.0))
    plain.release()                         # a gate-less def just frees
    assert _addrs(server)[-1] == "/node_free"
    # An unplayed event (or a rest) is a no-op, not an error.
    Event(degree=0).free()
    rest_ev = play(Event(type="rest"))
    rest_ev.free()
    rest_ev.release()


def test_automation_stops_early(clean_default):
    from clausters.defs import Env
    from clausters.seq.automation import Automation

    server = _nrt_server()
    main.server = server
    auto = play(Automation(Env([200.0, 800.0], [60.0]), target=(5, "freq")))
    assert isinstance(auto, Automation)     # the verb returns the stoppable
    assert auto.node is not None
    auto.stop()                             # a minute of sweep, cut now
    assert auto.node is None
    assert _addrs(server)[-1] == "/node_free"
    auto.stop()                             # idempotent


# ---- as_def: the shared expression -> def coercion ----

def test_as_def_wraps_a_bare_ugen_in_out():
    sdef = as_def(sine(440.0))
    assert isinstance(sdef, SynthDef)
    assert sdef.roots[0].kind == "Out"


def test_as_def_keeps_an_output_or_side_effect_root():
    assert as_def(out(1.0, sine(440.0))).roots[0].kind == "Out"
    assert as_def(send_trig(sine(1.0))).roots[0].kind == "SendTrig"


def test_as_def_passes_defs_through_and_autonames_expressions():
    sdef = SynthDef("named", out(0.0, sine(440.0)))
    assert as_def(sdef) is sdef
    a, b = as_def(sine(1.0)), as_def(sine(2.0))
    assert a.name != b.name


def test_as_def_coerces_faust_expressions():
    fdef = as_def(signals.signal(0.5) * 0.1)
    assert isinstance(fdef, FaustDef) and fdef.kind == "signals"
    bdef = as_def(boxes.box(0.5) * 0.1)
    assert isinstance(bdef, FaustDef) and bdef.kind == "box"


def test_as_def_rejects_non_expressions():
    with pytest.raises(TypeError):
        as_def(3.14)


# ---- the channel list is an expression like any other ----

def test_as_def_lays_a_channel_list_on_consecutive_buses():
    sdef = as_def(sine(440.0).dup())
    buses = [o.inputs[0] for o in sdef.roots]
    assert [o.kind for o in sdef.roots] == ["Out", "Out"]
    assert buses == [0.0, 1.0]
    # dup is by reference: one Sine serialized, fanned out to both channels.
    assert [u["kind"] for u in sdef.spec()["ugens"]] == ["Sine", "Out", "Out"]


def test_as_def_keeps_a_list_of_sinks_as_roots():
    sdef = as_def(chans(out(4.0, sine(1.0)), out(9.0, sine(2.0))))
    assert [o.inputs[0] for o in sdef.roots] == [4.0, 9.0]


def test_a_sink_in_a_mixed_list_does_not_push_the_audio_off_bus_zero():
    # A sink already knows where its data goes, so it consumes no channel:
    # the members that are not sinks are the channels, from bus 0 up.
    sdef = as_def(chans(send_trig(sine(1.0)), sine(440.0), sine(660.0)))
    assert [o.kind for o in sdef.roots] == ["SendTrig", "Out", "Out"]
    assert [o.inputs[0] for o in sdef.roots[1:]] == [0.0, 1.0]


def test_playing_a_channel_list_sends_and_instances_it(clean_default):
    server = _nrt_server()
    main.server = server
    node = play(sine(440.0).dup())
    assert isinstance(node, Synth)
    assert "/def_send" in _addrs(server) and "/synth_new" in _addrs(server)


def test_a_control_is_a_graph_leaf_not_something_to_play(clean_default):
    # It reaches as_def (it is a SynthExpr) and is rejected there, by name --
    # one place decides what is coercible.
    main.server = _nrt_server()
    with pytest.raises(TypeError, match="Control"):
        play(control("freq"))


# ---- a sink is what delivers data out of the graph ----

def test_disk_out_is_a_sink_so_it_is_not_wrapped():
    sdef = as_def(disk_out("/tmp/rec.wav", sine(440.0)))
    assert sdef.roots[0].kind == "DiskOut"
    # Recording *and* hearing stays available, explicitly.
    assert as_def(out(0.0, disk_out("/tmp/rec.wav", sine(440.0)))) \
        .roots[0].kind == "Out"


def test_graph_management_ugens_are_not_sinks_and_stay_wrapped():
    # They pass their input through, so out(0, free_self_when_done(...)) is
    # the idiom -- wrapping them is right, side effect or not.
    for expr in (free_self(sine(1.0)),
                 pause_self(sine(1.0)),
                 free_self_when_done(line(1.0, 0.0, 1.0)),
                 detect_silence(sine(440.0))):
        assert as_def(expr).roots[0].kind == "Out"


# ---- how wide an expression is ----

def test_expr_channels_counts_the_buses_as_def_would_lay():
    assert expr_channels(sine(440.0)) == 1
    assert expr_channels(sine(440.0).dup(4)) == 4
    assert expr_channels(chans(send_trig(sine(1.0)), sine(1.0))) == 1
    # A sink routes itself: nothing here to infer.
    assert expr_channels(send_trig(sine(1.0))) == 0
    assert expr_channels(SynthDef("d", out(0.0, sine(1.0)))) is None


def test_free_play_pattern_resolves_server_and_clock(clean_default):
    server = _nrt_server()
    main.server = server
    # NRT: drive the default clock's render rather than real time.
    player = play(Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5),
                  clock=main.get_default_clock(start=False))
    main.default_clock.render()
    # three notes, each an /synth_new + release bundle
    assert len(server.interface.score.bundles) == 6
    player.stop()
