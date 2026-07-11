"""The compositional model (Fase 1A) — pure structure and temporal algebra.

No server: these check the temporal *character* of a material (from its
onset/duration), the temporal *relation* derived from a `Group`'s member
placements, the thin-wrapper delegation of `play`, and group editing by handle.
Realization onto the server/NRT is a later phase.
"""

import pytest

from clausters.model import (
    ABSTRACT,
    COMPOSITIONAL,
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
    Material,
    Sequence,
    Track,
    temporal_character,
)


# ---- temporal character (onset/duration -> character) ----

def test_temporal_character_table():
    assert temporal_character(0.0, 2.0) == SEGMENT
    assert temporal_character(0.0, None) == PUNCTUAL
    assert temporal_character(None, 2.0) == RELATIVE
    assert temporal_character(None, None) == ABSTRACT


def test_material_character_property():
    assert Material(onset=1.0, duration=4.0).temporal_character == SEGMENT
    assert Material(onset=1.0).temporal_character == PUNCTUAL
    assert Material(duration=4.0).temporal_character == RELATIVE
    assert Material().temporal_character == ABSTRACT


def test_onset_duration_are_floats():
    m = Material(onset=1, duration=2)
    assert isinstance(m.onset, float) and isinstance(m.duration, float)
    assert Material().onset is None and Material().duration is None


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
        Material().play(_Dest())


# ---- Group editing by handle ----

def test_group_kind_validated():
    assert Group(kind=LOGICAL).kind == LOGICAL
    with pytest.raises(ValueError):
        Group(kind="bogus")


def test_group_seed_forms():
    a, b = Material(duration=1.0), Material(duration=1.0)
    g = Group([a, (2.0, b), (4.0, 1.0, Material(duration=9.0))])
    offsets = [off for off, _dur, _mat in g.members]
    assert offsets == [0.0, 2.0, 4.0]
    # the (offset, dur, material) triple overrides the material's own duration
    assert g.members[2][1] == 1.0


def test_group_add_remove_move_by_handle():
    g = Group()
    h1 = g.add(Material(duration=1.0), 0.0)
    h2 = g.add(Material(duration=1.0), 1.0)
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
        g.add(Material(duration=dur), offset)
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
    g.add(Material(), 3.0)
    g.add(Material(), 3.0)
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
    # material durations differ, but the placement dur makes them tile
    g = Group(kind=COMPOSITIONAL)
    g.add(Material(duration=99.0), 0.0, dur=2.0)
    g.add(Material(duration=99.0), 2.0, dur=2.0)
    assert g.temporal_relation() == SUCCESSIVE
