"""The default session and the free-standing ``play``: sounding without a
`Session`. Everything that does not run in an explicit session runs in the
default session (`main`); a bare event plays immediately (no clock), self-
releasing on wall time.
"""

import pytest

from clausters import Event, play, main, default_session
from clausters.defs import (
    FaustDef, Server, SynthDef, as_def, boxes, out, send_trig, signals, sin_osc,
)
from clausters.defs.node import Group, Synth
from clausters.base._oscinterface import OscNrtInterface
from clausters.seq.pattern import Pbind, Pseq
from clausters.seq.timeline import Playhead, Timeline


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
    # Only a free-standing Server.boot() adopts the default session; a plain
    # constructor (as Session.nrt/embed use) must not.
    _nrt_server()
    assert main.server is None


def test_event_play_resolves_ambient_server(clean_default):
    server = _nrt_server()
    main.server = server               # stands in for a free-standing boot
    node_id = Event(degree=0).play()   # no destination: resolves the default
    assert node_id is not None
    # /s_new immediate at t=0, release scheduled at t=sustain (dur*legato=0.8).
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


def test_free_play_accepts_an_event_dict(clean_default):
    server = _nrt_server()
    main.server = server
    node_id = play({"degree": 2, "dur": 0.5})
    assert node_id is not None
    assert len(server.interface.score.bundles) == 2   # /s_new + release


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
    node = play(sin_osc(440.0) * 0.1)
    assert isinstance(node, Synth)
    # the ephemeral def (/d_recv at 0) plus its /s_new
    assert len(server.interface.score.bundles) == 2


def test_free_play_instances_a_def_with_controls(clean_default):
    server = _nrt_server()
    main.server = server
    from clausters.defs import control

    sdef = SynthDef("beep", out(0.0, sin_osc(control("freq", 440.0))))
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


def test_play_rejects_a_form_element_with_a_pointer_to_render(clean_default):
    from clausters.form import Event as FormEvent

    with pytest.raises(TypeError, match="render"):
        play(FormEvent(Event(degree=0)))


# ---- as_def: the shared expression -> def coercion ----

def test_as_def_wraps_a_bare_ugen_in_out():
    sdef = as_def(sin_osc(440.0))
    assert isinstance(sdef, SynthDef)
    assert sdef.outputs[0].kind == "Out"


def test_as_def_keeps_an_output_or_side_effect_root():
    assert as_def(out(1.0, sin_osc(440.0))).outputs[0].kind == "Out"
    assert as_def(send_trig(sin_osc(1.0))).outputs[0].kind == "SendTrig"


def test_as_def_passes_defs_through_and_autonames_expressions():
    sdef = SynthDef("named", out(0.0, sin_osc(440.0)))
    assert as_def(sdef) is sdef
    a, b = as_def(sin_osc(1.0)), as_def(sin_osc(2.0))
    assert a.name != b.name


def test_as_def_coerces_faust_expressions():
    fdef = as_def(signals.signal(0.5) * 0.1)
    assert isinstance(fdef, FaustDef) and fdef.kind == "signals"
    bdef = as_def(boxes.box(0.5) * 0.1)
    assert isinstance(bdef, FaustDef) and bdef.kind == "box"


def test_as_def_rejects_non_expressions():
    with pytest.raises(TypeError):
        as_def(3.14)


def test_free_play_pattern_resolves_server_and_clock(clean_default):
    server = _nrt_server()
    main.server = server
    # NRT: drive the default clock's render rather than real time.
    player = play(Pbind(instrument="default", degree=Pseq([0, 2, 4]), dur=0.5),
                  clock=main.get_default_clock(start=False))
    main.default_clock.render()
    # three notes, each an /s_new + release bundle
    assert len(server.interface.score.bundles) == 6
    player.stop()
