"""The multitrack editor driver (`clausters.gui.editor`) — the arrangement↔GuiDef bridge.

No server and no GUI host: the forward draw is a pure function of the arrangement
tree, so these check the mapping rule (lanes, clips, bodies), the beats↔timeline-
samples unit bridge, and the id registry that the edit-back path writes through.
"""

import pytest

from clausters.defs.buffer import Buffer as ServerBuffer
from clausters.gui.editor import Editor
from clausters.form import Buffer, Event, Group, Sequence, Track
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


def editor(element=None, **kwargs) -> Editor:
    return Editor(element or song(), sample_rate=SR, tempo=TEMPO, **kwargs)


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
    tree = editor(quant=0.25).draw()
    assert all(lane["snap"] == pytest.approx(BEAT / 4) for lane in lanes(tree))
    # No quant: no grid (the host then snaps to whole samples).
    assert "snap" not in lanes(editor().draw())[0]


# ---- the mapping rule: root members are lanes, their members are clips ----

def test_each_root_member_becomes_a_lane_named_after_its_material():
    tree = editor().draw()
    assert [lane["label"] for lane in lanes(tree)] == ["audio", "lead"]
    # The bottom lane rules the shared axis (one ruler under the stack).
    assert lanes(tree)[-1]["ruler"] == "beats"
    assert "ruler" not in lanes(tree)[0]


def test_a_buffer_clip_names_the_server_buffer_and_spans_its_frames():
    audio = lanes(editor().draw())[0]
    (take,) = clips(audio)
    assert take["type"] == "clip"
    assert take["buffer"] == 7                    # fetched over the host's leg
    assert take["dur"] == pytest.approx(4 * BEAT)  # the take's own length, 1:1
    assert take["offset"] == 0.0


def test_a_buffer_spans_its_frames_only_when_it_has_no_duration():
    """A buffer read but never queried has no frame count client-side; its
    element's `duration` is what places it, and must win over the frames."""
    unqueried = Buffer(ServerBuffer(bufnum=3), duration=2.0)   # frames unknown (0)
    (lane,) = lanes(editor(Group([(0.0, unqueried)], name="take")).draw())
    (c,) = clips(lane)
    assert c["dur"] == pytest.approx(2 * BEAT)

    # With no duration either, the take's own frames are its length.
    sized = Buffer(ServerBuffer(bufnum=3, frames=int(1.5 * BEAT)))
    (lane,) = lanes(editor(Group([(0.0, sized)], name="take")).draw())
    assert clips(lane)[0]["dur"] == pytest.approx(1.5 * BEAT)


def test_an_events_material_draws_a_piano_roll_placed_by_its_offset():
    lead = lanes(editor().draw())[1]
    (roll,) = clips(lead)
    # Placed at beat 2 of the song, in timeline samples.
    assert roll["offset"] == pytest.approx(2 * BEAT)
    # Three notes as flat (start, dur, pitch, velocity, channel) quintuples, in
    # timeline units. The default event amp (0.1) maps to velocity 13.
    assert roll["notes"] == pytest.approx([
        0.0, 0.8 * BEAT, 60.0, 13, 0,   # dur 1 beat, legato 0.8 -> sustain 0.8
        1 * BEAT, 0.8 * BEAT, 64.0, 13, 0,
        2 * BEAT, 1.6 * BEAT, 67.0, 13, 0,
    ])
    # The pitch axis covers the notes with headroom.
    assert roll["min"] <= 58.0 and roll["max"] >= 69.0


def test_a_generator_lane_shows_the_notes_its_pattern_will_play():
    # The change of state made visible: a bounced pattern draws as a roll.
    from clausters.seq.pattern import Pbind, Pseq

    seq = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    (lane,) = lanes(editor(Group([(0.0, seq)], name="gen")).draw())
    (roll,) = clips(lane)
    # Pitch is the 3rd of each (start, dur, pitch, velocity, channel) quintuple.
    assert [roll["notes"][i] for i in (2, 7)] == [60.0, 62.0]


def _track_material():
    """A `Track` of two notes and one OSC event on an editable timeline."""
    from clausters.form import Track
    from clausters.seq.event import Event as SeqEvent
    from clausters.seq.timeline import OscEvent, Timeline

    tl = Timeline()
    tl.add(0.0, SeqEvent(dict(midinote=60, dur=1.0, amp=0.5)))
    tl.add(1.0, SeqEvent(dict(midinote=64, dur=0.5, amp=0.8)))
    tl.add(0.5, OscEvent("/cue"))
    return Track(tl), tl


def test_an_element_renders_as_a_dedicated_piano_roll():
    track, _tl = _track_material()
    ed = Editor(track, sample_rate=SR, tempo=TEMPO, quant=0.25)
    ed._mode, ed._roll_element = "pianoroll", track
    (roll,) = ed.draw()["children"][:1]
    assert roll["type"] == "pianoroll"
    # Notes as quintuples (pitch is the 3rd), the OSC event on its own lane.
    assert [roll["notes"][i] for i in (2, 7)] == [60.0, 64.0]
    assert roll["osc"] == [pytest.approx(0.5 * BEAT), "/cue"]
    assert roll["ruler"] == "beats"


def test_a_note_edit_rewrites_the_editable_timeline():
    track, tl = _track_material()
    ed = Editor(track, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._roll_element = "pianoroll", track
    ed.draw()  # builds the roll registry
    wid = next(iter(ed._rolls))
    # Move pitch 60 -> 62 and add a note; times/durs in timeline units.
    edited = [0.0, BEAT, 62, 100, 0, 1.0 * BEAT, 0.5 * BEAT, 67, 90, 0]
    assert ed.apply("/gui_event", [wid, "notes", *edited]) is True
    items = tl.range(0.0, float("inf"))
    pitches = [it.get("midinote") for _b, it in items if hasattr(it, "get")]
    assert pitches == [62, 67]                       # the notes were rewritten
    # The OSC event on the same timeline is preserved.
    from clausters.seq.timeline import OscEvent
    assert any(isinstance(it, OscEvent) for _b, it in items)


def test_a_generator_material_is_read_only_in_the_piano_roll():
    from clausters.seq.pattern import Pbind, Pseq

    gen = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    ed = Editor(gen, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._roll_element = "pianoroll", gen
    ed.draw()
    wid = next(iter(ed._rolls))
    # A generator is forward-only: the edit is ignored (no editable timeline).
    assert ed.apply("/gui_event", [wid, "notes", 0.0, BEAT, 65, 100, 0]) is False
    assert ed.dirty is False


# ---- the base level: a nested group collapses to a summary, or expands ----

def test_a_nested_group_is_a_labeled_rectangle_until_it_is_expanded():
    inner = Group([(0.0, Event(SeqEvent(midinote=60, dur=1.0))),
                   (1.0, Event(SeqEvent(midinote=62, dur=1.0)))], name="motif")
    outer = Group([(0.0, inner)], name="section")
    ed = editor(Group([(0.0, outer)], name="song"))

    (lane,) = lanes(ed.draw())
    (summary,) = clips(lane)
    assert summary["label"] == "motif"
    assert "notes" not in summary and "buffer" not in summary  # a bare rectangle
    assert summary["dur"] == pytest.approx(2 * BEAT)           # it spans its members

    # Expanded, the group resolves into a lane of its own with its members as clips.
    ed.expand(inner)
    expanded = lanes(ed.draw())
    assert [lane["label"] for lane in expanded] == ["motif"]
    assert len(clips(expanded[0])) == 2


# ---- the registry the edit-back writes through ----

def test_every_clip_registers_the_placement_it_came_from():
    ed = editor()
    tree = ed.draw()
    ids = [c["id"] for lane in lanes(tree) for c in clips(lane)]
    assert set(ids) == set(ed._clips)
    for wid in ids:
        placed = ed._clips[wid]
        # The handle is the arrangement's own: moving it moves the element.
        assert placed.member in placed.owner.handles


def test_a_render_is_stable_across_calls():
    ed = editor()
    assert ed.draw() == ed.draw()


# ---- the edit-back: a dragged clip becomes a placement in the tree ----

def clip_event(wid: int, offset: float, dur: float) -> tuple:
    """The payload the host sends when a clip is dragged or resized."""
    return ("/gui_event", [wid, "clip", offset, dur])


def test_a_dragged_clip_moves_the_material_in_beats():
    ed = editor(quant=0.25)
    tree = ed.draw()
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
    roll = clips(lanes(ed.draw())[1])[0]
    member = ed._clips[roll["id"]].member
    # A hair off a half-beat boundary (the wire carries 32-bit floats): the tree
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
    (c,) = clips(lanes(ed.draw())[0])
    assert c["offset"] == pytest.approx(5 * BEAT)         # absolute: 4 + 1

    member = ed._clips[c["id"]].member
    ed.apply(*clip_event(c["id"], 6 * BEAT, c["dur"]))    # dragged one beat right
    assert member.offset == pytest.approx(2.0)            # relative to the section


def test_moving_a_clip_leaves_its_length_alone():
    """A drag carries the clip's unchanged `dur` along; writing it back (snapped)
    would silently reshape the element. Only what moved is written."""
    ed = editor(quant=1.0)
    roll = clips(lanes(ed.draw())[1])[0]
    member = ed._clips[roll["id"]].member
    assert member.dur is None                             # the placement set no length

    ed.apply(*clip_event(roll["id"], 5 * BEAT, roll["dur"]))
    assert member.offset == pytest.approx(5.0)
    assert member.dur is None                             # untouched by the move


def test_render_apply_render_is_a_fixed_point():
    ed = editor(quant=0.25)
    before = ed.draw()
    # Feed every clip its own placement back: nothing moved, so nothing changes.
    for lane in lanes(before):
        for c in clips(lane):
            ed.apply(*clip_event(c["id"], c["offset"], c["dur"]))
    assert ed.draw() == before


def test_an_edit_marks_the_arrangement_changed_until_it_is_rendered():
    """An edit does not interrupt what is playing; it marks the composition, and
    the next play (or a resume, or a seek) re-reads it — rendering always
    re-flattens the tree."""
    ed = editor(quant=1.0)
    roll = clips(lanes(ed.draw())[1])[0]
    assert not ed.dirty

    ed.apply(*clip_event(roll["id"], 5 * BEAT, roll["dur"]))
    assert ed.dirty

    # A resize is an edit too — and it is the one that used to be silent: the
    # placement's length now trims what the element plays.
    placed = ed._clips[roll["id"]]
    assert (placed.offset, placed.dur) == (5 * BEAT, roll["dur"])   # registry kept true
    ed.apply(*clip_event(roll["id"], 5 * BEAT, 1 * BEAT))
    assert ed._clips[roll["id"]].member.dur == pytest.approx(1.0)


def test_the_composition_grows_when_a_clip_is_dragged_past_the_end():
    """The piece's length is read from the arrangement, never fixed: drag a clip beyond
    the last one and the composition is longer — which is what a transport must ask
    to play it to its (new) end."""
    ed = editor(quant=1.0)
    tree = ed.draw()
    before = ed.extent()                          # the take (4 beats) is the longest

    lead = clips(lanes(tree)[1])[0]
    ed.apply(*clip_event(lead["id"], 12 * BEAT, lead["dur"]))
    assert before < 12.0
    assert ed.extent() > 12.0, "the piece now runs past the clip that was dragged out"


# ---- the transport: a cursor, and seeking to it ----

class _FakeHost:
    """Records the `/gui_set`s the editor sends (the lanes' playhead chrome)."""

    def __init__(self):
        self.sets = []

    def open(self, tree, id=None):
        return 1

    def set(self, id, **props):
        self.sets.append((id, props))


def test_a_locate_moves_the_transport_and_the_lanes_cursor():
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    # A click on a lane's ruler: the host sends "locate" in timeline units.
    ed.apply("/gui_event", [next(iter(ed._lanes)), "locate", 3 * BEAT])
    assert ed.position == pytest.approx(3.0)
    assert not ed.dirty, "seeking is a transport action, not an edit"

    # Every lane draws the cursor there, and the clock anchor is off (stopped).
    for _id, props in host.sets[-len(ed._lanes):]:
        assert props["playhead"] == pytest.approx(3 * BEAT)
        assert props["playhead_at"] == -1.0


def test_stop_returns_to_the_top_and_pause_keeps_the_position():
    ed = editor()
    ed.open(_FakeHost())
    ed.locate(5.0)
    assert ed.position == pytest.approx(5.0)
    ed.pause()                       # nothing playing: the position stands
    assert ed.position == pytest.approx(5.0)
    ed.stop()
    assert ed.position == 0.0


def test_a_second_editors_events_fall_through_untouched():
    """Two editors polled off one host (a dedicated piano-roll beside the
    multitrack): another window's close and another editor's lane are not ours."""
    ed = editor()
    ed.open(_FakeHost())
    assert ed.apply("/gui_closed", [ed.window + 1]) is False
    assert ed.window is not None, "another window's close is not ours"
    ed.apply("/gui_event", [99_999, "locate", 3 * BEAT])
    assert ed.position == 0.0, "another editor's lane is not ours to seek from"
    ed.apply("/gui_closed", [ed.window])
    assert ed.window is None


def test_unknown_messages_are_ignored():
    ed = editor()
    ed.draw()
    assert not ed.apply("/gui_event", [1, "points", 0.0, 1.0])   # a bpf edit
    assert not ed.apply("/gui_event", [999_999, "clip", 0.0, 1.0])  # unknown id
    assert not ed.apply("/clock.reply", [1234.0])


# ---- the automation clip: a curve as the body, edited in place ----

def automation_song() -> tuple:
    """A composition with one automation lane: a filter sweep over four beats."""
    from clausters.form import Element
    from clausters.seq import Automation

    # The flat bpf quads: (time, value, shape, curve) with the server's own shape
    # numbers (1 = linear, 2 = exponential) — the wire form the editor round-trips.
    auto = Automation.from_points(
        [(0, 200.0, 1, 0.0), (2, 4000.0, 2, 0.0), (4, 800.0, 1, 0.0)],
        target=None, name="cutoff")
    song = Group([(2.0, Group([(0.0, Element(auto))], name="filter"))], name="song")
    return song, auto


def test_an_automation_draws_as_a_curve_clip_on_the_timeline():
    song, auto = automation_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (curve,) = clips(lane)

    assert curve["offset"] == pytest.approx(2 * BEAT)   # placed at beat 2
    assert curve["dur"] == pytest.approx(4 * BEAT)      # the curve's own length
    # The break-points ride as the bpf flat quads, their times in timeline units.
    assert curve["points"][0:2] == pytest.approx([0.0, 200.0])
    assert curve["points"][4:6] == pytest.approx([2 * BEAT, 4000.0])
    assert curve["points"][8:10] == pytest.approx([4 * BEAT, 800.0])
    # The curve's *own* value axis covers it with headroom, so a point can be
    # dragged (a layered clip's `min`/`max` belong to the body underneath).
    assert curve["points_min"] < 200.0 and curve["points_max"] > 4000.0
    assert "notes" not in curve and "buffer" not in curve
    assert curve["label"] == "cutoff", "an envelope is named for what it drives"


def test_editing_the_curve_in_place_writes_it_back_onto_the_automation():
    song, auto = automation_song()
    ed = editor(song)
    (curve,) = clips(lanes(ed.draw())[0])

    # The host sends the same flat "points" payload the bpf view sends — here the
    # peak dragged down to 3000 Hz and a beat later.
    edited = [0.0, 200.0, 1, 0.0,
              3 * BEAT, 3000.0, 2, 0.0,
              4 * BEAT, 800.0, 1, 0.0]
    assert ed.apply("/gui_event", [curve["id"], "points", *edited])

    # The automation's Env — its source of truth, what the next rendering plays.
    assert auto.to_points()[4:6] == pytest.approx([3.0, 3000.0])  # in beats again
    assert auto.duration() == pytest.approx(4.0)
    # And the redraw shows what was dropped.
    assert clips(lanes(ed.draw())[0])[0]["points"][4:6] == pytest.approx(
        [3 * BEAT, 3000.0])


def test_an_envelope_attached_to_its_event_is_one_clip_that_moves_as_one():
    """A group whose members start and end together *is* one thing on the timeline
    (its temporal relation says so), so it draws as one clip with **layered**
    bodies — the envelope over the event it shapes — and dragging it moves the
    whole group, not one of its parts."""
    from clausters.form import Element
    from clausters.seq import Automation

    env = Automation.from_points([(0, 200.0, 1, 0.0), (4, 900.0, 2, 0.0)],
                                 target=None, name="sweep")
    voice = Event(SeqEvent(midinote=60, dur=4.0))
    attached = Group([(0.0, voice), (0.0, Element(env, duration=4.0))], name="sweep")
    assert attached.temporal_relation() == "simultaneous"

    ed = editor(Group([(2.0, attached)], name="song"))
    (lane,) = lanes(ed.draw())
    (c,) = clips(lane)
    # One clip, both bodies — and each on its own value axis.
    assert c["notes"] and c["points"]
    assert c["min"] < 60.0 < c["max"]                     # the notes' pitch axis
    assert c["points_min"] < 200.0 and c["points_max"] > 900.0   # the curve's

    # Dragging it moves the group: the event and its envelope stay together.
    ed.apply(*clip_event(c["id"], 6 * BEAT, c["dur"]))
    placed = ed._clips[c["id"]]
    assert placed.member.element is attached
    assert placed.member.offset == pytest.approx(6.0)
    from clausters.form.render import flatten

    assert min(b for b, _ in flatten(ed.element)) == pytest.approx(6.0)


# ---- the logical group: a patch, not a lane ----

def patch_song() -> tuple:
    """A composition with a logical group: source -> sink through a private bus."""
    from clausters.form import Generator
    from clausters.form.group import LOGICAL

    src = Generator("gsrc", controls={"out": "mix", "level": 1.0})
    sink = Generator("gsink", controls={"in": "mix", "out": "OUT"})
    chain = Group([src, sink], kind=LOGICAL, name="chain", buses=["mix"])
    return Group([(0.0, chain)], name="song"), chain, src


def patches(tree: dict) -> list:
    return [c for c in tree["children"] if c["type"] == "graph"]


def test_a_logical_group_draws_as_a_patch_not_a_lane():
    song, _chain, _src = patch_song()
    tree = editor(song).draw()
    assert lanes(tree) == [], "processing is not a timeline: no lane for it"
    (patch,) = patches(tree)

    assert patch["label"] == "chain"
    assert [m["name"] for m in patch["members"]] == ["gsrc", "gsink"]
    # A port per control that names a bus (a plain number is a value, not a wire).
    assert patch["members"][0]["ports"] == ["out"]
    assert patch["buses"] == ["mix", "OUT"]      # the group's own, then the hardware
    # The wires, flat: (member, control, bus).
    assert patch["wires"] == [0, "out", "mix", 1, "in", "mix", 1, "out", "OUT"]


def test_a_moved_box_persists_beside_the_group_and_redraws_placed():
    """The "move" edit-back: a member box dragged on the patch canvas keeps its
    position across redraws (the Editor owns the geometry, beside the logical
    group), a moved bus is keyed by its stable name, and the composition itself
    is reported unchanged — geometry is presentation, not sound."""
    song, _chain, _src = patch_song()
    ed = editor(song)
    (patch,) = patches(ed.draw())

    # Geometry is presentation: apply() reports no composition change.
    assert not ed.apply("/gui_event", [patch["id"], "move", "member", 1, 260.0, 40.0])
    assert not ed.apply("/gui_event", [patch["id"], "move", "bus", 1, 420.0, 180.0])

    (again,) = patches(ed.draw())
    assert again["members"][0] == {"name": "gsrc", "ports": ["out"]}, \
        "an unmoved box keeps the auto layout"
    assert again["members"][1]["x"] == 260.0 and again["members"][1]["y"] == 40.0
    # Bus 1 was "OUT": placed by name, the other stays a plain string.
    assert again["buses"][0] == "mix"
    assert again["buses"][1] == {"name": "OUT", "x": 420.0, "y": 180.0}


def test_rewiring_a_port_rewrites_the_logical_group():
    song, _chain, src = patch_song()
    ed = editor(song)
    (patch,) = patches(ed.draw())

    # Dropped on the hardware bus: the source now writes straight to OUT.
    assert ed.apply("/gui_event", [patch["id"], "wire", 0, "out", "OUT"])
    assert src.controls["out"] == "OUT"
    assert src.controls["level"] == 1.0, "the untouched controls stay"

    # The GraphDef the next rendering sends follows the patch: the source's
    # `out` now names the hardware, not the private bus.
    _song, chain, _src = patch_song()
    Editor(Group([(0.0, chain)], name="song"), sample_rate=SR, tempo=TEMPO)
    ed2 = editor(Group([(0.0, chain)], name="song"))
    (patch2,) = patches(ed2.draw())
    ed2.apply("/gui_event", [patch2["id"], "wire", 0, "out", "OUT"])
    spec = chain.to_graphdef().spec()
    assert spec["members"][0]["controls"]["out"] == "OUT"

    # Dropped on empty space: unwired (the control names no bus at all).
    assert ed.apply("/gui_event", [patch["id"], "wire", 0, "out", ""])
    assert "out" not in src.controls


# ---- rendering: the edited composition plays what the screen shows ----

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


def test_the_edited_composition_renders_where_it_was_dropped():
    """A clip dragged in the GUI lands, in the score, at the beat it was dropped
    on — the whole loop (render → edit-back → model → render) in one assertion.
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
        lane = lanes(ed.draw())[0]
        (c,) = clips(lane)
        ed.apply(*clip_event(c["id"], 3 * BEAT, 1 * BEAT))  # dragged to beat 3
        ed.render(server, clock)

    # The score is in seconds: beat 3 at 2 beats/sec sounds at 1.5 s — the unit
    # bridge closing on the far side.
    assert starts(edited) == [3.0 / TEMPO]
