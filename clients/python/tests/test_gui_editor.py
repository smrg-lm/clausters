"""The multitrack editor driver (`clausters.gui.editor`) — the model↔GuiDef bridge.

No server and no GUI host: the forward render is a pure function of the model
tree, so these check the mapping rule (lanes, clips, bodies), the beats↔timeline-
samples unit bridge, and the id registry that the edit-back path writes through.
"""

import pytest

from clausters.defs.buffer import Buffer as ServerBuffer
from clausters.gui.editor import Editor
from clausters.model import Buffer, Event, Group, Sequence, Track
from clausters.seq.event import Event as SeqEvent
from clausters.seq.timeline import Timeline

SR = 48_000.0
TEMPO = 2.0          # beats per second (120 bpm)
BEAT = SR / TEMPO    # 24000 timeline samples per beat


def song() -> Group:
    """A two-lane composition: a take on one lane, a melody on another."""
    take = Buffer(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=4.0)
    audio = Group([(0.0, take)], name="audio")
    melody = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                             (1.0, SeqEvent(midinote=64, dur=1.0)),
                             (2.0, SeqEvent(midinote=67, dur=2.0))]))
    lead = Group([(2.0, melody)], name="lead")
    return Group([(0.0, audio), (0.0, lead)], name="song")


def editor(material=None, **kwargs) -> Editor:
    return Editor(material or song(), sample_rate=SR, tempo=TEMPO, **kwargs)


def lanes(tree: dict) -> list:
    return [c for c in tree["children"] if c["type"] == "track"]


def clips(lane: dict) -> list:
    return lane.get("children", [])


# ---- the unit bridge: beats (model) <-> timeline samples (view) ----

def test_one_beat_is_sample_rate_over_tempo_timeline_units():
    ed = editor()
    assert ed.units_per_beat == pytest.approx(BEAT)
    assert ed.beats_to_units(2.5) == pytest.approx(2.5 * BEAT)
    # The round trip is the one an edit-back takes: pixels -> samples -> beats.
    assert ed.units_to_beats(ed.beats_to_units(3.25)) == pytest.approx(3.25)


def test_a_musical_quant_becomes_the_lanes_drag_grid():
    tree = editor(quant=0.25).render()
    assert all(lane["snap"] == pytest.approx(BEAT / 4) for lane in lanes(tree))
    # No quant: no grid (the host then snaps to whole samples).
    assert "snap" not in lanes(editor().render())[0]


# ---- the mapping rule: root members are lanes, their members are clips ----

def test_each_root_member_becomes_a_lane_named_after_its_material():
    tree = editor().render()
    assert [lane["label"] for lane in lanes(tree)] == ["audio", "lead"]
    # The bottom lane rules the shared axis (one ruler under the stack).
    assert lanes(tree)[-1]["ruler"] == "beats"
    assert "ruler" not in lanes(tree)[0]


def test_a_buffer_clip_names_the_server_buffer_and_spans_its_frames():
    audio = lanes(editor().render())[0]
    (take,) = clips(audio)
    assert take["type"] == "clip"
    assert take["buffer"] == 7                    # fetched over the host's leg
    assert take["dur"] == pytest.approx(4 * BEAT)  # the take's own length, 1:1
    assert take["offset"] == 0.0


def test_an_events_material_draws_a_piano_roll_placed_by_its_offset():
    lead = lanes(editor().render())[1]
    (roll,) = clips(lead)
    # Placed at beat 2 of the song, in timeline samples.
    assert roll["offset"] == pytest.approx(2 * BEAT)
    # Three notes as flat (start, dur, pitch) triples, in timeline units.
    assert roll["notes"] == pytest.approx([
        0.0, 0.8 * BEAT, 60.0,          # dur 1 beat, legato 0.8 -> sustain 0.8
        1 * BEAT, 0.8 * BEAT, 64.0,
        2 * BEAT, 1.6 * BEAT, 67.0,
    ])
    # The pitch axis covers the notes with headroom.
    assert roll["min"] <= 58.0 and roll["max"] >= 69.0


def test_a_generator_lane_shows_the_notes_its_pattern_will_play():
    # The change of state made visible: a bounced pattern draws as a roll.
    from clausters.seq.pattern import Pbind, Pseq

    seq = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    (lane,) = lanes(editor(Group([(0.0, seq)], name="gen")).render())
    (roll,) = clips(lane)
    assert [roll["notes"][i] for i in (2, 5)] == [60.0, 62.0]


# ---- the base level: a nested group collapses to a summary, or expands ----

def test_a_nested_group_is_a_labeled_rectangle_until_it_is_expanded():
    inner = Group([(0.0, Event(SeqEvent(midinote=60, dur=1.0))),
                   (1.0, Event(SeqEvent(midinote=62, dur=1.0)))], name="motif")
    outer = Group([(0.0, inner)], name="section")
    ed = editor(Group([(0.0, outer)], name="song"))

    (lane,) = lanes(ed.render())
    (summary,) = clips(lane)
    assert summary["label"] == "motif"
    assert "notes" not in summary and "buffer" not in summary  # a bare rectangle
    assert summary["dur"] == pytest.approx(2 * BEAT)           # it spans its members

    # Expanded, the group resolves into a lane of its own with its members as clips.
    ed.expand(inner)
    expanded = lanes(ed.render())
    assert [lane["label"] for lane in expanded] == ["motif"]
    assert len(clips(expanded[0])) == 2


# ---- the registry the edit-back writes through ----

def test_every_clip_registers_the_placement_it_came_from():
    ed = editor()
    tree = ed.render()
    ids = [c["id"] for lane in lanes(tree) for c in clips(lane)]
    assert set(ids) == set(ed._clips)
    for wid in ids:
        owner, member = ed._clips[wid]
        # The handle is the model's own: moving it moves the material.
        assert member in owner.handles


def test_a_render_is_stable_across_calls():
    ed = editor()
    assert ed.render() == ed.render()
