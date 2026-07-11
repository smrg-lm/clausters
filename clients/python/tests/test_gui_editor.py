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


def test_a_buffer_spans_its_frames_only_when_it_has_no_duration():
    """A buffer read but never queried has no frame count client-side; its
    material's `duration` is what places it, and must win over the frames."""
    unqueried = Buffer(ServerBuffer(bufnum=3), duration=2.0)   # frames unknown (0)
    (lane,) = lanes(editor(Group([(0.0, unqueried)], name="take")).render())
    (c,) = clips(lane)
    assert c["dur"] == pytest.approx(2 * BEAT)

    # With no duration either, the take's own frames are its length.
    sized = Buffer(ServerBuffer(bufnum=3, frames=int(1.5 * BEAT)))
    (lane,) = lanes(editor(Group([(0.0, sized)], name="take")).render())
    assert clips(lane)[0]["dur"] == pytest.approx(1.5 * BEAT)


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
        placed = ed._clips[wid]
        # The handle is the model's own: moving it moves the material.
        assert placed.member in placed.owner.handles


def test_a_render_is_stable_across_calls():
    ed = editor()
    assert ed.render() == ed.render()


# ---- the edit-back: a dragged clip becomes a placement in the model ----

def clip_event(wid: int, offset: float, dur: float) -> tuple:
    """The payload the host sends when a clip is dragged or resized."""
    return ("/gui_event", [wid, "clip", offset, dur])


def test_a_dragged_clip_moves_the_material_in_beats():
    ed = editor(quant=0.25)
    tree = ed.render()
    roll = clips(lanes(tree)[1])[0]           # the lead's piano-roll, at beat 2
    placed = ed._clips[roll["id"]]
    owner, member = placed.owner, placed.member

    # Dragged two beats later and resized to three (the host sends samples).
    assert ed.apply(*clip_event(roll["id"], 4 * BEAT, 3 * BEAT))
    assert member.offset == pytest.approx(4.0)
    assert member.dur == pytest.approx(3.0)
    assert (member.offset, member.dur) == (owner.members[0][0], owner.members[0][1])


def test_an_edit_snaps_to_the_musical_grid():
    ed = editor(quant=0.5)
    roll = clips(lanes(ed.render())[1])[0]
    member = ed._clips[roll["id"]].member
    # A hair off a half-beat boundary (the wire carries 32-bit floats): the model
    # gets the grid value, not the noise.
    ed.apply(*clip_event(roll["id"], 3.51 * BEAT, 2.0 * BEAT))
    assert member.offset == pytest.approx(3.5)


def test_a_clip_in_a_placed_group_converts_back_through_its_base():
    """A clip's offset is absolute on the shared axis; a placement is relative to
    its group. Dragging a clip inside a group that starts at beat 4 must move it
    by the delta, not stamp the absolute position onto the member."""
    note = Event(SeqEvent(midinote=60, dur=1.0))
    section = Group([(1.0, note)], name="section")        # the note at beat 1 of it
    ed = editor(Group([(4.0, section)], name="song"))     # the section at beat 4
    (c,) = clips(lanes(ed.render())[0])
    assert c["offset"] == pytest.approx(5 * BEAT)         # absolute: 4 + 1

    member = ed._clips[c["id"]].member
    ed.apply(*clip_event(c["id"], 6 * BEAT, c["dur"]))    # dragged one beat right
    assert member.offset == pytest.approx(2.0)            # relative to the section


def test_moving_a_clip_leaves_its_length_alone():
    """A drag carries the clip's unchanged `dur` along; writing it back (snapped)
    would silently reshape the material. Only what moved is written."""
    ed = editor(quant=1.0)
    roll = clips(lanes(ed.render())[1])[0]
    member = ed._clips[roll["id"]].member
    assert member.dur is None                             # the model set no length

    ed.apply(*clip_event(roll["id"], 5 * BEAT, roll["dur"]))
    assert member.offset == pytest.approx(5.0)
    assert member.dur is None                             # untouched by the move


def test_render_apply_render_is_a_fixed_point():
    ed = editor(quant=0.25)
    before = ed.render()
    # Feed every clip its own placement back: nothing moved, so nothing changes.
    for lane in lanes(before):
        for c in clips(lane):
            ed.apply(*clip_event(c["id"], c["offset"], c["dur"]))
    assert ed.render() == before


def test_unknown_messages_are_ignored():
    ed = editor()
    ed.render()
    assert not ed.apply("/gui_event", [1, "points", 0.0, 1.0])   # a bpf edit
    assert not ed.apply("/gui_event", [999_999, "clip", 0.0, 1.0])  # unknown id
    assert not ed.apply("/clock.reply", [1234.0])


# ---- realization: the edited composition plays what the screen shows ----

def _embed_or_skip():
    try:
        from clausters import _native

        _native.lib()
    except OSError as e:
        pytest.skip(f"clausters-ffi not built: {e}")


def _inner_addr(raw: bytes) -> str:
    import struct

    from clausters.base import _osclib as osc

    length = struct.unpack(">i", raw[16:20])[0]
    addr, _ = osc.decode(raw[20:20 + length])
    return addr


def test_the_edited_composition_realizes_where_it_was_dropped():
    """A clip dragged in the GUI lands, in the score, at the beat it was dropped
    on — the whole loop (render → edit-back → model → realize) in one assertion.
    NRT, so no socket and no port clash."""
    _embed_or_skip()
    from clausters.base import OscNrtInterface, TempoClock
    from clausters.defs import Server
    from clausters.seq.event import Event as SeqEvent

    def starts(build):
        server = Server(interface=OscNrtInterface())
        clock = TempoClock(tempo=TEMPO)
        build(server, clock)
        clock.render()
        return sorted(when for when, raw in server.interface.score.bundles
                      if _inner_addr(raw) == "/s_new")

    def edited(server, clock):
        note = Event(SeqEvent(instrument="default", freq=440.0, dur=1.0))
        song = Group([(0.0, Group([(0.0, note)], name="lead"))], name="song")
        ed = Editor(song, sample_rate=SR, tempo=TEMPO, quant=1.0)
        lane = lanes(ed.render())[0]
        (c,) = clips(lane)
        ed.apply(*clip_event(c["id"], 3 * BEAT, 1 * BEAT))  # dragged to beat 3
        ed.realize(server, clock)

    # The score is in seconds: beat 3 at 2 beats/sec sounds at 1.5 s — the unit
    # bridge closing on the far side.
    assert starts(edited) == [3.0 / TEMPO]
