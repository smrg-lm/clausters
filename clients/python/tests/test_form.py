"""The concrete model (Fase 1A) — pure structure and temporal algebra.

No server: these check the temporal *character* of an element (from its
onset/duration), the temporal *relation* derived from a `Group`'s member
placements, the thin-wrapper delegation of `play`, and group editing by handle.
Rendering onto the server/NRT is a later phase.
"""

import json
import struct

import pytest

from clausters.form import (
    ABSTRACT,
    CONCRETE,
    LOGICAL,
    MIXED,
    PUNCTUAL,
    RELATIVE,
    SEGMENT,
    SIMULTANEOUS,
    SUCCESSIVE,
    Buffer,
    Event,
    Generator,
    Group,
    Element,
    Sequence,
    Track,
    flatten,
    temporal_character,
)


# ---- temporal character (onset/duration -> character) ----

def test_temporal_character_table():
    assert temporal_character(0.0, 2.0) == SEGMENT
    assert temporal_character(0.0, None) == PUNCTUAL
    assert temporal_character(None, 2.0) == RELATIVE
    assert temporal_character(None, None) == ABSTRACT


def test_material_character_property():
    assert Element(onset=1.0, duration=4.0).temporal_character == SEGMENT
    assert Element(onset=1.0).temporal_character == PUNCTUAL
    assert Element(duration=4.0).temporal_character == RELATIVE
    assert Element().temporal_character == ABSTRACT


def test_onset_duration_are_floats():
    m = Element(onset=1, duration=2)
    assert isinstance(m.onset, float) and isinstance(m.duration, float)
    assert Element().onset is None and Element().duration is None


# ---- the five primitives are thin wrappers ----

def test_event_duration_defaults_from_dur():
    # A bare event has an intrinsic duration but its onset comes from context,
    # so a standalone event is `relative`.
    ev = Event({"freq": 440, "dur": 2.0})
    assert ev.duration == 2.0
    assert ev.onset is None
    assert ev.temporal_character == RELATIVE


def test_event_explicit_duration_wins():
    ev = Event({"freq": 440, "dur": 2.0}, onset=1.0, duration=0.5)
    assert ev.onset == 1.0 and ev.duration == 0.5
    assert ev.temporal_character == SEGMENT


def test_track_wraps_fresh_timeline():
    from clausters.seq import Timeline

    tr = Track()
    assert isinstance(tr.wraps, Timeline)
    given = Timeline()
    assert Track(given).wraps is given


def test_wrappers_carry_their_object():
    seq = Sequence([1, 2, 3])
    assert seq.wraps == [1, 2, 3]
    sentinel = object()
    assert Buffer(sentinel).wraps is sentinel
    assert Generator(sentinel).wraps is sentinel


# ---- play delegation (the double-dispatch seam) ----

class _Dest:
    def __init__(self):
        self.played = []


def test_event_play_delegates_to_wrapped_event(monkeypatch):
    ev = Event({"freq": 440})
    seen = {}
    monkeypatch.setattr(ev.wraps, "play", lambda dest: seen.setdefault("dest", dest))
    dest = _Dest()
    ev.play(dest)
    assert seen["dest"] is dest


def test_container_material_is_not_directly_playable():
    with pytest.raises(NotImplementedError):
        Group().play(_Dest())
    with pytest.raises(NotImplementedError):
        Element().play(_Dest())


# ---- Group editing by handle ----

def test_group_kind_validated():
    assert Group(kind=LOGICAL).kind == LOGICAL
    with pytest.raises(ValueError):
        Group(kind="bogus")


def test_group_seed_forms():
    a, b = Element(duration=1.0), Element(duration=1.0)
    g = Group([a, (2.0, b), (4.0, 1.0, Element(duration=9.0))])
    offsets = [off for off, _dur, _mat in g.members]
    assert offsets == [0.0, 2.0, 4.0]
    # the (offset, dur, element) triple overrides the element's own duration
    assert g.members[2][1] == 1.0


def test_group_add_remove_move_by_handle():
    g = Group()
    h1 = g.add(Element(duration=1.0), 0.0)
    h2 = g.add(Element(duration=1.0), 1.0)
    assert len(g) == 2
    g.move(h1, 5.0)
    assert h1.offset == 5.0
    g.remove(h2)
    assert len(g) == 1
    g.clear()
    assert len(g) == 0


# ---- the derived temporal relation ----

def _grp(*placements):
    """A group of members at the given (offset, dur) placements."""
    g = Group()
    for offset, dur in placements:
        g.add(Element(duration=dur), offset)
    return g


def test_relation_empty_is_none():
    assert Group().temporal_relation() is None


def test_relation_single_member_is_simultaneous():
    assert _grp((0.0, 2.0)).temporal_relation() == SIMULTANEOUS


def test_relation_simultaneous():
    # same start and same end
    assert _grp((0.0, 2.0), (0.0, 2.0), (0.0, 2.0)).temporal_relation() == SIMULTANEOUS


def test_relation_simultaneous_durationless():
    # all start together and all have unknown (None) length -> still simultaneous
    g = Group()
    g.add(Element(), 3.0)
    g.add(Element(), 3.0)
    assert g.temporal_relation() == SIMULTANEOUS


def test_relation_successive():
    # tiling contiguously: 0-2, 2-3, 3-5 (order-independent)
    assert _grp((2.0, 1.0), (0.0, 2.0), (3.0, 2.0)).temporal_relation() == SUCCESSIVE


def test_relation_mixed_gap():
    # a gap between members -> neither simultaneous nor contiguous
    assert _grp((0.0, 2.0), (3.0, 2.0)).temporal_relation() == MIXED


def test_relation_mixed_overlap():
    assert _grp((0.0, 2.0), (1.0, 2.0)).temporal_relation() == MIXED


def test_relation_placement_dur_drives_derivation():
    # element durations differ, but the placement dur makes them tile
    g = Group(kind=CONCRETE)
    g.add(Element(duration=99.0), 0.0, dur=2.0)
    g.add(Element(duration=99.0), 2.0, dur=2.0)
    assert g.temporal_relation() == SUCCESSIVE


# ---- render: flatten to absolute beats (Fase 1B, pure) ----

def test_flatten_accumulates_nested_offsets():
    from clausters.seq.event import Event as SeqEvent

    inner = Group([(0.0, Event({"dur": 1.0})), (2.0, Event({"dur": 1.0}))])
    outer = Group([(10.0, inner), (5.0, Event({"dur": 1.0}))])
    flat = flatten(outer)
    assert [beat for beat, _ in flat] == [5.0, 10.0, 12.0]  # sorted; 5, 10+0, 10+2
    # the items are the wrapped seq.Events (the play(destination) seam)
    assert all(isinstance(item, SeqEvent) for _, item in flat)


def test_flatten_track_shifts_its_timeline():
    from clausters.seq import OscEvent, Timeline

    a, b = OscEvent("/x", "a"), OscEvent("/x", "b")
    track = Track(Timeline([(0.0, a), (1.0, b)]))
    flat = flatten(Group([(4.0, track)]))
    assert flat == [(4.0, a), (5.0, b)]


def test_sequence_of_materials_is_laid_out_successively():
    flat = flatten(Sequence([Event({"dur": 2.0}), Event({"dur": 3.0}), Event({"dur": 1.0})]))
    assert [beat for beat, _ in flat] == [0.0, 2.0, 5.0]


def test_abstract_material_yields_no_event():
    flat = flatten(Group([(0.0, Element()), (1.0, Event({"dur": 1.0}))]))
    assert [beat for beat, _ in flat] == [1.0]


def test_to_timeline_is_sorted():
    tl = Group([(2.0, Event({"dur": 1.0})), (0.0, Event({"dur": 1.0}))]).to_timeline()
    assert [beat for beat, _ in tl] == [0.0, 2.0]


def test_flatten_logical_group_is_deferred():
    with pytest.raises(NotImplementedError):
        flatten(Group(kind=LOGICAL))


def test_a_buffer_without_an_instrument_has_no_sound_of_its_own():
    # Data, not an audio clip: it contributes structure (and draws in the editor),
    # but nothing plays it, so flattening emits no event — and asking for the
    # event it would play says exactly what is missing.
    assert flatten(Group([(0.0, Buffer(object()))])) == []
    with pytest.raises(NotImplementedError, match="instrument"):
        Buffer(object()).to_event()


def test_render_bare_abstract_is_an_error():
    with pytest.raises(ValueError):
        Element().render(None, None)


# ---- render: NRT equivalence to a hand-built timeline (needs the FFI) ----

def _embed_or_skip():
    try:
        from clausters import _native

        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def _inner_addr(raw: bytes) -> str:
    # raw = "#bundle\0"(8) + timetag(8) + [i32 len][message]
    from clausters.base import _osclib as osc

    length = struct.unpack(">i", raw[16:20])[0]
    addr, _ = osc.decode(raw[20:20 + length])
    return addr


def test_render_matches_handbuilt_timeline_nrt():
    """A concrete group rendered through the arrangement produces the same score
    (the same /synth_new start beats) as the equivalent flat timeline played by a
    Playhead by hand — proving the flatten is correct and the change of state
    deterministic. NRT only (OscNrtInterface), so no socket and no port clash."""
    _embed_or_skip()
    from clausters.base import OscNrtInterface, TempoClock
    from clausters.defs import Server
    from clausters.seq import Playhead, Timeline
    from clausters.seq.event import Event as SeqEvent

    def _starts(build):
        server = Server(interface=OscNrtInterface())
        clock = TempoClock(tempo=1.0)
        build(server, clock)
        clock.render()
        return sorted(
            when
            for when, raw in server.interface.score.bundles
            if _inner_addr(raw) == "/synth_new"
        )

    def by_model(server, clock):
        Group([
            (0.0, Event(SeqEvent(instrument="default", freq=440.0, dur=1.0))),
            (2.0, Group([
                (0.0, Event(SeqEvent(instrument="default", freq=550.0, dur=1.0))),
                (1.0, Event(SeqEvent(instrument="default", freq=660.0, dur=1.0))),
            ])),
        ]).render(server, clock)

    def by_hand(server, clock):
        tl = Timeline([
            (0.0, SeqEvent(instrument="default", freq=440.0, dur=1.0)),
            (2.0, SeqEvent(instrument="default", freq=550.0, dur=1.0)),
            (3.0, SeqEvent(instrument="default", freq=660.0, dur=1.0)),
        ])
        Playhead(tl, clock, server).play()

    assert _starts(by_model) == _starts(by_hand) == [0.0, 2.0, 3.0]


def test_a_buffer_sounds_through_the_instrument_that_plays_it():
    """A buffer is data: it needs an instrument (a def whose `buf` control plays
    it) to become an audio clip. With one, flattening emits the event that plays
    it; without one it is structure only — it contributes no event."""
    from clausters.defs.buffer import Buffer as ServerBuffer

    buf = ServerBuffer(bufnum=5, frames=1000)
    clip = Buffer(buf, duration=2.0, instrument="sampler", controls={"amp": 0.5})

    ((beat, event),) = flatten(Group([(4.0, clip)]))
    assert beat == 4.0
    assert event["instrument"] == "sampler"
    assert event["buf"] == 5
    assert event["dur"] == 2.0
    assert event["amp"] == 0.5
    # A take sounds its whole length: the note default (legato 0.8) would cut it.
    assert event.sustain() == 2.0

    # Data with no instrument: no event, no error.
    assert flatten(Group([(0.0, Buffer(buf, duration=2.0))])) == []


def test_a_placement_length_trims_what_the_material_plays():
    """A clip's length is what you hear of it: a placement `dur` drops the events
    past its end and sizes a single-event element to it — the DAW rule, and what
    resizing a clip in the editor must actually change."""
    from clausters.defs.buffer import Buffer as ServerBuffer
    from clausters.seq.event import Event as SeqEvent
    from clausters.seq.timeline import Timeline

    # A track of four one-beat notes, placed with a two-beat length: the last two
    # fall outside it and are not played.
    notes = Track(Timeline([(float(i), SeqEvent(midinote=60 + i, dur=1.0))
                            for i in range(4)]))
    group = Group()
    member = group.add(notes, 0.0, dur=2.0)
    assert [beat for beat, _ in flatten(group)] == [0.0, 1.0]

    # Lengthened, the whole track sounds again.
    group.move(member, 0.0, dur=4.0)
    assert len(flatten(group)) == 4

    # A take shortened by its placement sounds for exactly that long (its own
    # event is untouched — a placement never rewrites the element).
    take = Buffer(ServerBuffer(bufnum=1, frames=100), duration=4.0,
                  instrument="sampler")
    song = Group([(0.0, 1.5, take)])
    ((_beat, event),) = flatten(song)
    assert event["dur"] == 1.5
    assert take.to_event()["dur"] == 4.0


# ---- render: logical group -> GraphDef (Fase 1C, pure) ----

def test_generator_def_name_from_string_or_object():
    class _Def:
        name = "foo"

    assert Generator("bar").def_name == "bar"
    assert Generator(_Def()).def_name == "foo"


def test_logical_group_translates_to_the_same_graphdef():
    """A logical group of two nodes (source -> sink through an internal bus)
    produces the same GraphDef spec as building it directly — the 1:1 mapping."""
    from clausters.defs import GraphDef

    g = Group(kind=LOGICAL, name="chain", buses=[("mix", "audio")])
    g.add(Generator("gsrc", controls={"out": "mix", "level": 1.0}))
    g.add(Generator("gsink", controls={"in": "mix", "out": "OUT"}))
    model_spec = g.to_graphdef().spec()

    gd = GraphDef("chain")
    mix = gd.bus("mix")
    gd.add("gsrc", {"out": mix, "level": 1.0})
    gd.add("gsink", {"in": mix, "out": "OUT"})

    assert model_spec == gd.spec()


def test_to_graphdef_requires_a_name():
    with pytest.raises(ValueError):
        Group(kind=LOGICAL).to_graphdef()


def test_declare_bus_adds_and_is_idempotent_by_name():
    g = Group(kind=LOGICAL, name="x", buses=[("mix", "audio")])
    assert g.bus_names == ["mix"]
    g.declare_bus("w0", rate="control")           # a new bus
    assert g.bus_names == ["mix", "w0"]
    g.declare_bus("mix", rate="control", channels=2)   # re-declare updates
    assert g.bus_names == ["mix", "w0"]
    spec = next(s for s in g._bus_specs if s["name"] == "mix")
    assert spec == {"name": "mix", "rate": "control", "channels": 2}


def test_logical_member_must_be_a_generator():
    g = Group(kind=LOGICAL, name="x")
    g.add(Event({"dur": 1.0}))
    with pytest.raises(TypeError):
        g.to_graphdef()


class _StubServer:
    """Records what a render sends, without any socket (no port clash)."""

    #: no score interface, so a def send takes the RT path and waits on the
    #: /done this stub answers with.
    interface = None

    def __init__(self):
        self.sent = []

    def request(self, addr, *args, timeout=5.0, expect=()):
        self.sent.append((addr, list(args)))
        return ("/done", [addr])

    def send_msg(self, addr, *args):
        self.sent.append((addr, list(args)))

    def _node_id(self):
        return 1000


def test_render_routes_a_logical_group_to_graphdef():
    g = Group(kind=LOGICAL, name="chain", buses=["mix"])
    g.add(Generator("gsrc", controls={"out": "mix"}))
    server = _StubServer()
    instance = g.render(server, ports={"gain": 0.5})
    assert instance.id == 1000 and instance.server is server
    sent_def, sent_new = server.sent
    assert sent_def[0] == "/def_send" and sent_def[1][0] == "graph"
    assert json.loads(sent_def[1][1])["name"] == "chain"
    assert sent_new == ("/graph_new", ["chain", 1000, 1, 0, "gain", 0.5])


# ---- what can be located at all ----

def test_a_flattened_element_is_locatable():
    """Everything the arrangement flattens becomes messages at absolute beats,
    so a position on it means something."""
    from clausters.form import Element
    from clausters.seq import Event as SeqEvent

    assert Element(SeqEvent(instrument="default")).locatable


def test_a_resident_generator_is_not_locatable():
    """A def generating its own material on the server has no index: its
    position *is* its internal state, and no number moves it."""
    from clausters.form import Element
    from clausters.seq import Event as SeqEvent

    assert not Element(SeqEvent(instrument="default"), resident=True).locatable


def test_a_group_is_locatable_only_if_every_member_is():
    """One resident generator makes the whole placement unlocatable: a position
    on the group would be a position on it too."""
    from clausters.form import Element, Group
    from clausters.seq import Event as SeqEvent

    g = Group([Element(SeqEvent(instrument="default"))])
    assert g.locatable
    g.add(Element(SeqEvent(instrument="default"), resident=True))
    assert not g.locatable
