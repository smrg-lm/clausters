"""The concrete model (Fase 1A) — pure structure and temporal algebra.

No server: these check the temporal *character* of an element (from its
onset/duration), the temporal *relation* derived from an `Aggregate`'s member
placements, the thin-wrapper delegation of `play`, and aggregate editing by
handle.
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
    Aggregate,
    Element,
    Generator,
    Clang,
    Sequence,
    Track,
    Vector,
    flatten,
    temporal_character,
)


# ---- temporal character (onset/duration -> character) ----

def test_temporal_character_table():
    assert temporal_character(0.0, 2.0) == SEGMENT
    assert temporal_character(0.0, None) == PUNCTUAL
    assert temporal_character(None, 2.0) == RELATIVE
    assert temporal_character(None, None) == ABSTRACT


def test_element_character_property():
    assert Element(onset=1.0, duration=4.0).temporal_character == SEGMENT
    assert Element(onset=1.0).temporal_character == PUNCTUAL
    assert Element(duration=4.0).temporal_character == RELATIVE
    assert Element().temporal_character == ABSTRACT


def test_onset_duration_are_floats():
    m = Element(onset=1, duration=2)
    assert isinstance(m.onset, float) and isinstance(m.duration, float)
    assert Element().onset is None and Element().duration is None


# ---- the five primitives are thin wrappers ----

def test_clang_duration_defaults_from_dur():
    # A bare clang has an intrinsic duration but its onset comes from context,
    # so a standalone clang is `relative`.
    ev = Clang({"freq": 440, "dur": 2.0})
    assert ev.duration == 2.0
    assert ev.onset is None
    assert ev.temporal_character == RELATIVE


def test_clang_explicit_duration_wins():
    ev = Clang({"freq": 440, "dur": 2.0}, onset=1.0, duration=0.5)
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
    assert Vector(sentinel).wraps is sentinel
    assert Generator(sentinel).wraps is sentinel


# ---- play delegation (the double-dispatch seam) ----

class _Dest:
    def __init__(self):
        self.played = []


def test_clang_play_delegates_to_wrapped_event(monkeypatch):
    ev = Clang({"freq": 440})
    seen = {}
    monkeypatch.setattr(ev.wraps, "play", lambda dest: seen.setdefault("dest", dest))
    dest = _Dest()
    ev.play(dest)
    assert seen["dest"] is dest


def test_a_container_is_not_directly_playable():
    with pytest.raises(NotImplementedError):
        Aggregate().play(_Dest())
    with pytest.raises(NotImplementedError):
        Element().play(_Dest())


# ---- Aggregate editing by handle ----

def test_aggregate_kind_validated():
    assert Aggregate(kind=LOGICAL).kind == LOGICAL
    with pytest.raises(ValueError):
        Aggregate(kind="bogus")


def test_aggregate_seed_forms():
    a, b = Element(duration=1.0), Element(duration=1.0)
    g = Aggregate([a, (2.0, b), (4.0, 1.0, Element(duration=9.0))])
    offsets = [off for off, _dur, _mat in g.members]
    assert offsets == [0.0, 2.0, 4.0]
    # the (offset, dur, element) triple overrides the element's own duration
    assert g.members[2][1] == 1.0


def test_aggregate_add_remove_move_by_handle():
    g = Aggregate()
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

def _agg(*placements):
    """An aggregate of members at the given (offset, dur) placements."""
    g = Aggregate()
    for offset, dur in placements:
        g.add(Element(duration=dur), offset)
    return g


def test_relation_empty_is_none():
    assert Aggregate().temporal_relation() is None


def test_relation_single_member_is_simultaneous():
    assert _agg((0.0, 2.0)).temporal_relation() == SIMULTANEOUS


def test_relation_simultaneous():
    # same start and same end
    assert _agg((0.0, 2.0), (0.0, 2.0), (0.0, 2.0)).temporal_relation() == SIMULTANEOUS


def test_relation_simultaneous_durationless():
    # all start together and all have unknown (None) length -> still simultaneous
    g = Aggregate()
    g.add(Element(), 3.0)
    g.add(Element(), 3.0)
    assert g.temporal_relation() == SIMULTANEOUS


def test_relation_successive():
    # tiling contiguously: 0-2, 2-3, 3-5 (order-independent)
    assert _agg((2.0, 1.0), (0.0, 2.0), (3.0, 2.0)).temporal_relation() == SUCCESSIVE


def test_relation_mixed_gap():
    # a gap between members -> neither simultaneous nor contiguous
    assert _agg((0.0, 2.0), (3.0, 2.0)).temporal_relation() == MIXED


def test_relation_mixed_overlap():
    assert _agg((0.0, 2.0), (1.0, 2.0)).temporal_relation() == MIXED


def test_relation_placement_dur_drives_derivation():
    # element durations differ, but the placement dur makes them tile
    g = Aggregate(kind=CONCRETE)
    g.add(Element(duration=99.0), 0.0, dur=2.0)
    g.add(Element(duration=99.0), 2.0, dur=2.0)
    assert g.temporal_relation() == SUCCESSIVE


# ---- render: flatten to absolute beats (Fase 1B, pure) ----

def test_flatten_accumulates_nested_offsets():
    from clausters.seq.event import Event as SeqEvent

    inner = Aggregate([(0.0, Clang({"dur": 1.0})), (2.0, Clang({"dur": 1.0}))])
    outer = Aggregate([(10.0, inner), (5.0, Clang({"dur": 1.0}))])
    flat = flatten(outer)
    assert [beat for beat, _ in flat] == [5.0, 10.0, 12.0]  # sorted; 5, 10+0, 10+2
    # the items are the wrapped seq.Events (the play(destination) seam)
    assert all(isinstance(item, SeqEvent) for _, item in flat)


def test_flatten_track_shifts_its_timeline():
    from clausters.seq import OscItem, Timeline

    a, b = OscItem("/x", "a"), OscItem("/x", "b")
    track = Track(Timeline([(0.0, a), (1.0, b)]))
    flat = flatten(Aggregate([(4.0, track)]))
    assert flat == [(4.0, a), (5.0, b)]


def test_a_sequence_of_elements_is_laid_out_successively():
    flat = flatten(Sequence([Clang({"dur": 2.0}), Clang({"dur": 3.0}), Clang({"dur": 1.0})]))
    assert [beat for beat, _ in flat] == [0.0, 2.0, 5.0]


def test_a_sequence_of_sequences_advances_by_what_each_one_reaches():
    """An item that states no length is as long as what it lays down.

    Read as zero, every member of a `Sequence` of `Sequence`s landed on the
    first beat -- four bars played at once, which is what "the piece is drawn as
    an unreadable clip" was.
    """
    def bar(pitch):
        return Sequence([Clang({"dur": 1.0, "freq": pitch}) for _ in range(4)])

    flat = flatten(Sequence([bar(220.0), bar(330.0)]))
    assert [beat for beat, _ in flat] == [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]


def test_a_sequence_lays_a_muted_member_out_where_it_would_have_been():
    """Mute says what is heard, never where anything is: silencing one member
    must not pull the ones after it forward."""
    quiet = Sequence([Clang({"dur": 1.0}), Clang({"dur": 1.0})])
    quiet.mute = True
    flat = flatten(Sequence([quiet, Clang({"dur": 1.0})]))
    assert [beat for beat, _ in flat] == [2.0]


def test_an_abstract_element_yields_no_clang():
    flat = flatten(Aggregate([(0.0, Element()), (1.0, Clang({"dur": 1.0}))]))
    assert [beat for beat, _ in flat] == [1.0]


def test_to_timeline_is_sorted():
    tl = Aggregate([(2.0, Clang({"dur": 1.0})), (0.0, Clang({"dur": 1.0}))]).to_timeline()
    assert [beat for beat, _ in tl] == [0.0, 2.0]


def test_flatten_logical_aggregate_is_deferred():
    with pytest.raises(NotImplementedError):
        flatten(Aggregate(kind=LOGICAL))


def test_a_vector_without_an_instrument_has_no_sound_of_its_own():
    # Data, not an audio clip: it contributes structure (and draws in the editor),
    # but nothing plays it, so flattening emits no event — and asking for the
    # event it would play says exactly what is missing.
    assert flatten(Aggregate([(0.0, Vector(object()))])) == []
    with pytest.raises(NotImplementedError, match="instrument"):
        Vector(object()).to_event()


def test_a_frozen_generator_is_structure_and_emits_nothing():
    # What a session reopened somewhere the script is not running holds: the
    # document named an algorithm and nobody could supply one, so the element
    # carries the reference (or nothing). It draws and contributes its extent
    # like a vector with no instrument, and flattening it emits no event --
    # raising instead would make the whole piece unplayable over one lane.
    frozen = Aggregate([(0.0, Sequence("<Pbind object>")), (0.0, Generator(None))])
    assert flatten(frozen) == []


def test_a_resolved_leaf_plays_the_same_whichever_element_holds_it():
    # The conversion writes an element it has no body for as a generator leaf,
    # so opening one back gives a `Generator` where the author wrote a bare
    # `Element`. Both flatten to the same thing, or a reopened piece would sound
    # different from the one that was saved.
    class _Plays:
        def play(self, dest):
            pass

    playable = _Plays()
    assert (flatten(Aggregate([(1.0, Generator(playable))]))
            == flatten(Aggregate([(1.0, Element(playable))])))


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
    """A concrete aggregate rendered through the arrangement produces the same score
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
        Aggregate([
            (0.0, Clang(SeqEvent(instrument="default", freq=440.0, dur=1.0))),
            (2.0, Aggregate([
                (0.0, Clang(SeqEvent(instrument="default", freq=550.0, dur=1.0))),
                (1.0, Clang(SeqEvent(instrument="default", freq=660.0, dur=1.0))),
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


def test_a_vector_sounds_through_the_instrument_that_plays_it():
    """A vector wraps a buffer, and a buffer is data: it needs an instrument (a
    def whose `buf` control plays it) to become an audio clip. With one,
    flattening emits the event that plays it; without one it is structure only —
    it contributes no event."""
    from clausters.defs.buffer import Buffer as ServerBuffer

    buf = ServerBuffer(bufnum=5, frames=1000)
    clip = Vector(buf, duration=2.0, instrument="sampler", controls={"amp": 0.5})

    ((beat, event),) = flatten(Aggregate([(4.0, clip)]))
    assert beat == 4.0
    assert event["instrument"] == "sampler"
    assert event["buf"] == 5
    assert event["dur"] == 2.0
    assert event["amp"] == 0.5
    # A take sounds its whole length: the note default (legato 0.8) would cut it.
    assert event.sustain() == 2.0

    # Data with no instrument: no event, no error.
    assert flatten(Aggregate([(0.0, Vector(buf, duration=2.0))])) == []


def test_a_placement_length_trims_what_the_element_plays():
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
    lane = Aggregate()
    member = lane.add(notes, 0.0, dur=2.0)
    assert [beat for beat, _ in flatten(lane)] == [0.0, 1.0]

    # Lengthened, the whole track sounds again.
    lane.move(member, 0.0, dur=4.0)
    assert len(flatten(lane)) == 4

    # A take shortened by its placement sounds for exactly that long (its own
    # event is untouched — a placement never rewrites the element).
    take = Vector(ServerBuffer(bufnum=1, frames=100), duration=4.0,
                  instrument="sampler")
    song = Aggregate([(0.0, 1.5, take)])
    ((_beat, event),) = flatten(song)
    assert event["dur"] == 1.5
    assert take.to_event()["dur"] == 4.0


# ---- render: logical aggregate -> GraphDef (Fase 1C, pure) ----

def test_generator_def_name_from_string_or_object():
    class _Def:
        name = "foo"

    assert Generator("bar").def_name == "bar"
    assert Generator(_Def()).def_name == "foo"


def test_logical_aggregate_translates_to_the_same_graphdef():
    """A logical aggregate of two nodes (source -> sink through an internal bus)
    produces the same GraphDef spec as building it directly — the 1:1 mapping."""
    from clausters.defs import GraphDef

    g = Aggregate(kind=LOGICAL, name="chain", buses=[("mix", "audio")])
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
        Aggregate(kind=LOGICAL).to_graphdef()


def test_declare_bus_adds_and_is_idempotent_by_name():
    g = Aggregate(kind=LOGICAL, name="x", buses=[("mix", "audio")])
    assert g.bus_names == ["mix"]
    g.declare_bus("w0", rate="control")           # a new bus
    assert g.bus_names == ["mix", "w0"]
    g.declare_bus("mix", rate="control", channels=2)   # re-declare updates
    assert g.bus_names == ["mix", "w0"]
    spec = next(s for s in g._bus_specs if s["name"] == "mix")
    assert spec == {"name": "mix", "rate": "control", "channels": 2}


def test_logical_member_must_be_a_generator():
    g = Aggregate(kind=LOGICAL, name="x")
    g.add(Clang({"dur": 1.0}))
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


def test_render_routes_a_logical_aggregate_to_graphdef():
    g = Aggregate(kind=LOGICAL, name="chain", buses=["mix"])
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
    """A def generating its own audio on the server has no index: its
    position *is* its internal state, and no number moves it."""
    from clausters.form import Element
    from clausters.seq import Event as SeqEvent

    assert not Element(SeqEvent(instrument="default"), resident=True).locatable


def test_an_aggregate_is_locatable_only_if_every_member_is():
    """One resident generator makes the whole placement unlocatable: a position
    on the aggregate would be a position on it too."""
    from clausters.form import Aggregate, Element
    from clausters.seq import Event as SeqEvent

    g = Aggregate([Element(SeqEvent(instrument="default"))])
    assert g.locatable
    g.add(Element(SeqEvent(instrument="default"), resident=True))
    assert not g.locatable


# ---- mixing: what the composition says about being heard ----

def _piece():
    """Two lanes of one event each, so what is heard is countable."""
    from clausters.form import Track
    from clausters.seq import Timeline
    from clausters.seq.event import Event as SeqEvent

    a = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0, amp=0.5))]), name="a")
    b = Track(Timeline([(0.0, SeqEvent(midinote=48, dur=1.0, amp=1.0))]), name="b")
    return Aggregate([(0.0, a), (0.0, b)]), a, b


def test_a_muted_branch_contributes_nothing_and_its_members_with_it():
    from clausters.form import flatten

    piece, a, _b = _piece()
    assert len(flatten(piece)) == 2
    a.mute = True
    assert [item.get("midinote") for _, item in flatten(piece)] == [48]
    piece.mute = True
    assert flatten(piece) == [], "muting the piece mutes what is inside it"


def test_one_soloed_lane_silences_every_branch_that_is_not_on_a_soloed_path():
    from clausters.form import flatten

    piece, a, b = _piece()
    b.solo = True
    assert [item.get("midinote") for _, item in flatten(piece)] == [48]
    a.solo = True
    assert len(flatten(piece)) == 2, "solo says *only these*, and there can be two"


def test_a_level_multiplies_into_the_amp_of_what_is_under_it():
    from clausters.form import flatten

    piece, a, _b = _piece()
    a.level = 0.5
    piece.level = 0.5
    heard = {item.get("midinote"): item.get("amp") for _, item in flatten(piece)}
    assert heard[60] == pytest.approx(0.125), "0.5 * 0.5 over the event's own 0.5"
    assert heard[48] == pytest.approx(0.5), "the piece's level alone"


def test_a_mix_never_rewrites_the_event_it_measures():
    from clausters.form import flatten

    piece, a, _b = _piece()
    a.level = 0.5
    flatten(piece)
    assert a.wraps[0][1].get("amp") == 0.5, "the element's own event is shared"


def test_drawing_reads_the_composition_unmixed():
    # A muted lane keeps its clips, its notes and its length: a picture that
    # emptied when the toggle was pressed would report silence as absence.
    from clausters.form import flatten

    piece, a, _b = _piece()
    a.mute = True
    piece.handles[1].element.solo = True
    assert len(flatten(piece, mixed=False)) == 2
