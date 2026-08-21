"""The multitrack editor driver (`clausters.gui.editor`) — the arrangement↔GuiDef bridge.

No server and no GUI host: the forward draw is a pure function of the arrangement
tree, so these check the mapping rule (lanes, clips, bodies), the beats↔timeline-
samples unit bridge, and the id registry that the edit-back path writes through.
"""

import itertools

import pytest

from clausters.defs import SynthDef, control, in_, out, sine
from clausters.defs.buffer import Buffer as ServerBuffer
from clausters.form import (Aggregate, Element, Generator, Clang, Sequence,
                            Track, Vector)
from clausters.form.aggregate import LOGICAL
from clausters.form.document import FIRST_VERSION
from clausters.gui.editor import Editor, _logical_patch
from clausters.seq.event import Event as SeqEvent
from clausters.seq.timeline import Timeline

#: The stamp every `/gui_event` carries as its second argument. Any non-zero
#: number does here: what the editor answers with it is the host's business, and
#: these tests have no host attached (`_acknowledge` is a no-op without one).
SEQ = 1
#: A version of zero: the host cannot say what state the gesture was made
#: against, so the edit applies unchecked. The staleness gate has its own tests;
#: every other one here is about routing.
UNSTATED = 0

SR = 48_000.0
TEMPO = 2.0          # beats per second (120 bpm)
BEAT = SR / TEMPO    # 24000 timeline samples per beat


def song() -> Aggregate:
    """A two-lane composition: a take on one lane, a melody on another."""
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=4.0)
    audio = Aggregate([(0.0, take)], name="audio")
    melody = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                             (1.0, SeqEvent(midinote=64, dur=1.0)),
                             (2.0, SeqEvent(midinote=67, dur=2.0))]))
    lead = Aggregate([(2.0, melody)], name="lead")
    return Aggregate([(0.0, audio), (0.0, lead)], name="song")


def editor(element=None, **kwargs) -> Editor:
    return Editor(element or song(), sample_rate=SR, tempo=TEMPO, **kwargs)


def fx_chain() -> Aggregate:
    """A logical aggregate: a source writing bus ``mix``, a terminal sink reading it
    (and writing hardware bus 0 itself). The members carry SynthDefs, so their
    ports derive from the def; the shared bus name is the cord."""
    src = SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))))
    sink = SynthDef("gsink", out(0, in_(control("in")) * control("amp", 0.3)))
    g = Aggregate(kind=LOGICAL, name="chain", buses=[("mix", "audio")])
    g.add(Generator(src, controls={"out": "mix"}))
    g.add(Generator(sink, controls={"in": "mix"}))
    return g


# A lane, a clip and the free-standing ruler are one container — a `field` —
# told apart by what is on it: a placement makes it a clip, a bare strip of a
# given thickness is the ruler, everything else is a lane.
def is_lane(n: dict) -> bool:
    return n["type"] == "field" and "dur" not in n and "h" not in n


def is_ruler(n: dict) -> bool:
    return n["type"] == "field" and "h" in n and "label" not in n


def is_plane(n: dict) -> bool:
    return n["type"] == "plane" and "boxes" not in n


def is_patch(n: dict) -> bool:
    return n["type"] == "plane" and "boxes" in n


def lanes(tree: dict) -> list:
    return [c for c in tree["children"] if is_lane(c)]


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

def test_each_root_member_becomes_a_lane_named_after_its_element():
    tree = editor().draw()
    assert [lane["label"] for lane in lanes(tree)] == ["audio", "lead"]
    # The shared axis is ruled by a free-standing strip under the stack, not by
    # the bottom lane's own `ruler` - which would be reserved out of that lane's
    # height, costing it a strip of itself.
    assert is_ruler(tree["children"][-1])
    assert all("ruler" not in lane for lane in lanes(tree))
    assert "ruler" not in lanes(tree)[0]


def test_a_vector_clip_names_the_server_buffer_and_spans_its_frames():
    audio = lanes(editor().draw())[0]
    (take,) = clips(audio)
    assert take["type"] == "field" and "dur" in take
    assert take["buffer"] == 7                    # fetched over the host's leg
    assert take["dur"] == pytest.approx(4 * BEAT)  # the take's own length, 1:1
    assert take["offset"] == 0.0


def test_a_vector_spans_its_frames_only_when_it_has_no_duration():
    """A buffer read but never queried has no frame count client-side; its
    element's `duration` is what places it, and must win over the frames."""
    unqueried = Vector(ServerBuffer(bufnum=3), duration=2.0)   # frames unknown (0)
    (lane,) = lanes(editor(Aggregate([(0.0, unqueried)], name="take")).draw())
    (c,) = clips(lane)
    assert c["dur"] == pytest.approx(2 * BEAT)

    # With no duration either, the take's own frames are its length.
    sized = Vector(ServerBuffer(bufnum=3, frames=int(1.5 * BEAT)))
    (lane,) = lanes(editor(Aggregate([(0.0, sized)], name="take")).draw())
    assert clips(lane)[0]["dur"] == pytest.approx(1.5 * BEAT)


def test_an_events_element_draws_a_piano_roll_placed_by_its_offset():
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
    (lane,) = lanes(editor(Aggregate([(0.0, seq)], name="gen")).draw())
    (roll,) = clips(lane)
    # Pitch is the 3rd of each (start, dur, pitch, velocity, channel) quintuple.
    assert [roll["notes"][i] for i in (2, 7)] == [60.0, 62.0]


def _track_with_a_take():
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
    track, _tl = _track_with_a_take()
    ed = Editor(track, sample_rate=SR, tempo=TEMPO, quant=0.25)
    ed._mode, ed._roll_element = "pianoroll", track
    (roll,) = ed.draw()["children"][:1]
    assert roll["type"] == "notes"
    # Notes as quintuples (pitch is the 3rd), the OSC event on its own lane.
    assert [roll["notes"][i] for i in (2, 7)] == [60.0, 64.0]
    assert roll["osc"] == [pytest.approx(0.5 * BEAT), "/cue"]
    assert roll["axes"]["x"]["unit"] == "beats"


def test_a_rendered_element_opens_as_one_measured_waveform():
    """The dedicated signal view: the editor-grade `waveform`, measuring twice.

    The stack is a prop of the one view rather than a pile of widgets, because
    every view of a signal paints its own field before it draws -- two of them
    on one rectangle are not layers, the second hides the first. One view is
    also one axis, one ruler, one selection, one playhead and one upload.
    """
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=2,
                               sample_rate=SR), duration=4.0)
    ed = Editor(take, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._signal_element = "signal", take
    view = ed.draw()["children"][0]
    assert view["type"] == "signal"
    assert view["measure"] == "peak rms"
    # The heavy view the shipped waveform is: navigable, over the one source,
    # ruling real time.
    assert "navigable" not in view or view["navigable"] == 1
    assert (view["buffer"], view["channels"]) == (7, 2)
    assert view["axes"]["x"]["unit"] == "time"
    # It is the editor's target as a lane is (the playhead and `locate` address
    # it) and a signal view (a selection swept in it is *of this element*).
    wid = view["id"]
    assert ed._lanes[wid] is take and ed._signals[wid] is take


def test_the_bare_envelope_is_a_shorter_stack():
    take = Vector(ServerBuffer(bufnum=7, frames=1000, channels=1, sample_rate=SR))
    ed = Editor(take, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._signal_element = "signal", take
    ed.layers = ("peak",)
    assert ed.draw()["children"][0]["measure"] == "peak"


def test_a_selection_swept_on_a_signal_view_is_of_that_element():
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=4.0)
    ed = Editor(take, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._signal_element = "signal", take
    wid = ed.draw()["children"][0]["id"]
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "selection",
                                   1.0 * BEAT, 2.0 * BEAT]) is False
    assert ed.selection["start"] == pytest.approx(1.0)
    assert ed.selection["len"] == pytest.approx(2.0)
    # ...and it names the element, which is what an operation over the range
    # is handed. A sweep on a lane with no element behind it names none.
    assert ed.selection.get("nodes")


def test_a_generator_has_no_samples_and_the_refusal_says_so():
    """The generated/generator distinction, asked at the door: notes can be
    bounced for a picture, samples cannot be invented. It is the *call* that
    refuses -- before a window exists, so nothing is left open."""
    from clausters.seq.pattern import Pbind, Pseq

    gen = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    ed = Editor(gen, sample_rate=SR, tempo=TEMPO)
    with pytest.raises(ValueError, match="no samples"):
        ed.open_signal(None, gen)
    assert ed.window is None
    # ...and the draw refuses too, since a mode set by hand must not build a
    # tree with no element in it either.
    ed._mode, ed._signal_element = "signal", gen
    with pytest.raises(ValueError, match="no samples"):
        ed.draw()


def test_an_unknown_measure_is_refused_by_name():
    take = Vector(ServerBuffer(bufnum=7, frames=1000, channels=1, sample_rate=SR))
    ed = Editor(take, sample_rate=SR, tempo=TEMPO)
    with pytest.raises(ValueError, match="loudness"):
        ed.open_signal(None, take, layers=("peak", "loudness"))


def test_a_note_edit_rewrites_the_editable_timeline():
    track, tl = _track_with_a_take()
    ed = Editor(track, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._roll_element = "pianoroll", track
    ed.draw()  # builds the roll registry
    wid = next(iter(ed._rolls))
    # Move pitch 60 -> 62 and add a note; times/durs in timeline units.
    edited = [0.0, BEAT, 62, 100, 0, 1.0 * BEAT, 0.5 * BEAT, 67, 90, 0]
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *edited]) is True
    items = tl.range(0.0, float("inf"))
    pitches = [it.get("midinote") for _b, it in items if hasattr(it, "get")]
    assert pitches == [62, 67]                       # the notes were rewritten
    # The OSC event on the same timeline is preserved.
    from clausters.seq.timeline import OscEvent
    assert any(isinstance(it, OscEvent) for _b, it in items)


def test_a_generator_element_is_read_only_in_the_piano_roll():
    from clausters.seq.pattern import Pbind, Pseq

    gen = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    ed = Editor(gen, sample_rate=SR, tempo=TEMPO)
    ed._mode, ed._roll_element = "pianoroll", gen
    ed.draw()
    wid = next(iter(ed._rolls))
    # A generator is forward-only: the edit is ignored (no editable timeline).
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", 0.0, BEAT, 65, 100, 0]) is False
    assert ed.dirty is False


def test_a_note_edited_in_a_clip_body_reaches_the_arrangement():
    """A roll body carries no id of its own, so its notes arrive tagged with the
    **clip's** — and the multitrack must resolve that to the element the body
    draws, or the note moves on screen and nowhere else."""
    track, tl = _track_with_a_take()
    ed = editor(Aggregate([(0.0, Aggregate([(2.0, track)], name="lead"))], name="song"))
    (lane,) = lanes(ed.draw())
    (roll,) = clips(lane)
    assert roll["notes"]                                  # it is a roll body

    # Pitch 60 -> 62, moved half a beat in; times are relative to the clip.
    edited = [0.5 * BEAT, BEAT, 62, 100, 0, 1.0 * BEAT, 0.5 * BEAT, 64, 90, 0]
    assert ed.apply("/gui_event", [roll["id"], SEQ, UNSTATED, "notes", *edited]) is True
    assert ed.dirty is True
    items = tl.range(0.0, float("inf"))
    assert [(b, it.get("midinote")) for b, it in items
            if hasattr(it, "get") and it.get("midinote")] == [(0.5, 62), (1.0, 64)]
    # The OSC event sharing the timeline is preserved.
    from clausters.seq.timeline import OscEvent
    assert any(isinstance(it, OscEvent) for _b, it in items)


def test_a_generator_clip_body_is_read_only_and_the_refusal_says_why():
    from clausters.seq.pattern import Pbind, Pseq

    gen = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, gen)], name="bass"))], name="song"))
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (roll,) = clips(lane)
    # It draws the notes it will play, but there is no timeline to write onto.
    assert roll["notes"]
    assert ed.apply("/gui_event", [roll["id"], SEQ, UNSTATED, "notes", 0.0, BEAT, 65, 100, 0]) is False
    assert ed.dirty is False
    # The note springs back -- and the answer says why, or a body that refuses
    # every drag teaches "sometimes it does not work" rather than "not here".
    _, _, reason = host.answers[-1]
    assert reason and "generator" in reason


def test_a_layered_clip_routes_a_note_edit_to_the_member_that_carries_it():
    """A simultaneous aggregate draws as one clip with layered bodies, so the
    notes under the cursor belong to a *member* — the editable one, not the
    aggregate."""
    from clausters.form import Element
    from clausters.seq import Automation

    env = Automation.from_points([(0, 200.0, 1, 0.0), (4, 900.0, 2, 0.0)],
                                 target=None, name="sweep")
    tl = Timeline([(0.0, SeqEvent(midinote=60, dur=4.0))])
    attached = Aggregate([(0.0, Track(tl, duration=4.0)),
                      (0.0, Element(env, duration=4.0))], name="sweep")
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, attached)], name="sweep"))], name="song"))
    (lane,) = lanes(ed.draw())
    (c,) = clips(lane)
    assert c["notes"] and c["points"]

    assert ed.apply("/gui_event", [c["id"], SEQ, UNSTATED, "notes", 0.0, 4 * BEAT, 67, 100, 0]) is True
    assert [it.get("midinote") for _b, it in tl.range(0.0, float("inf"))] == [67]


# ---- the base level: a nested aggregate collapses to a summary, or expands ----

def test_a_nested_aggregate_is_a_labeled_rectangle_until_it_is_expanded():
    inner = Aggregate([(0.0, Clang(SeqEvent(midinote=60, dur=1.0))),
                   (1.0, Clang(SeqEvent(midinote=62, dur=1.0)))], name="motif")
    outer = Aggregate([(0.0, inner)], name="section")
    ed = editor(Aggregate([(0.0, outer)], name="song"))

    (lane,) = lanes(ed.draw())
    (summary,) = clips(lane)
    assert summary["label"] == "motif"
    assert "notes" not in summary and "buffer" not in summary  # a bare rectangle
    assert summary["dur"] == pytest.approx(2 * BEAT)           # it spans its members

    # Expanded, the aggregate resolves into a lane of its own with its members
    # as clips.
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
    return ("/gui_event", [wid, SEQ, UNSTATED, "clip", offset, dur])


def test_a_dragged_clip_moves_the_clip_in_beats():
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


def test_a_clip_in_a_placed_aggregate_converts_back_through_its_base():
    """A clip's offset is absolute on the shared axis; a placement is relative to
    its aggregate. Dragging a clip inside an aggregate that starts at beat 4 must
    move it
    by the delta, not stamp the absolute position onto the member."""
    note = Clang(SeqEvent(midinote=60, dur=1.0))
    section = Aggregate([(1.0, note)], name="section")        # the note at beat 1 of it
    ed = editor(Aggregate([(4.0, section)], name="song"))     # the section at beat 4
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
    """Records the `/gui_set`s the editor sends (the lanes' playhead chrome) and
    the acknowledgements it answers events with, and hands out widget ids like
    the real host's recycling pool."""

    def __init__(self):
        self.sets = []
        #: ``(seq, corrections)`` per answered event -- corrections empty when
        #: the editor did exactly what the gesture asked.
        self.acks = []
        #: ``(seq, doc_version, reason)`` per answered event: what the host
        #: learns about the state its next gesture will name back.
        self.answers = []
        self._ids = itertools.count(10_000)
        #: Messages `poll` hands out, and what `dispatch` was asked to route.
        self.inbox = []
        self.dispatched = []

    def alloc_id(self):
        return next(self._ids)

    def open(self, tree, id=None):
        return 1

    def set(self, id, **props):
        self.sets.append((id, props))

    def ack(self, seq, doc_version=0, generations=(), reason=None):
        self.acks.append((seq, []))
        self.answers.append((seq, doc_version, reason))

    def push(self, seq, *sets, doc_version=0, generations=(), reason=None):
        self.acks.append((seq, list(sets)))
        self.answers.append((seq, doc_version, reason))

    def define(self, id, tree):
        pass

    def poll(self, timeout=0.0):
        return self.inbox.pop(0) if self.inbox else None

    def dispatch(self, addr, args):
        self.dispatched.append((addr, list(args)))
        return True


def test_a_sweep_becomes_the_crates_typed_selection():
    """The marquee's payload arrives as a `Selection` a script can hand to an
    operation: the time numbers in the arrangement's own unit, the value range
    in the element's, and what it is a selection *of* where the widget names
    one thing."""
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    # Swept on a lane: the shared time axis, so it is of nothing in particular.
    lane = next(iter(ed._lanes))
    changed = ed.apply("/gui_event", [lane, SEQ, UNSTATED, "selection", BEAT, 2 * BEAT])
    assert not changed, "a selection is screen state, not an edit"
    assert ed.selection["start"] == pytest.approx(1.0)
    assert ed.selection["len"] == pytest.approx(2.0)
    assert "value" not in ed.selection
    assert "nodes" not in ed.selection

    # Swept on a clip, with height: it names that element, and the range comes
    # through in the domain's own units rather than converted to anything.
    clip = next(iter(ed._clips))
    ed.apply("/gui_event",
             [clip, SEQ, UNSTATED, "selection", 0.0, 2 * BEAT, -0.5, 0.25])
    assert ed.selection["value"] == {"min": -0.5, "max": 0.25}
    assert len(ed.selection["nodes"]) == 1

    # And it resolves to the samples underneath, through the crate.
    under = ed.resolve_selection()
    assert under, "the take is under the sweep"
    assert all("range" in u and "source" in u for u in under)


def test_a_cut_removes_the_placement_and_undo_puts_it_back():
    """A cut is an edit like any other: it goes through the crate, so it is
    undoable, and what comes back is what the document decided."""
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    clip = next(iter(ed._clips))
    placed = ed._clips[clip]
    at = placed.base + placed.member.offset
    span = placed.member.length or 0.0
    before = len(placed.owner.handles)
    # A selection covering the whole clip: the placement leaves.
    changed = ed.apply("/gui_event", [clip, SEQ, UNSTATED, "cut",
                                      at * BEAT, span * BEAT])
    assert changed
    assert len(placed.owner.handles) == before - 1

    # ...and the history has it, because it was an edit and not a gesture.
    assert ed.can_undo
    ed.undo()
    assert len(placed.owner.handles) == before, "undo puts the placement back"


def test_follow_reschedules_what_is_playing_and_starts_nothing():
    """`follow` means *what is sounding follows the edit*. Found by dragging a
    clip in the whole-loop example with the transport stopped: the drag pressed
    play. An edit is not a transport action, and a stopped piece stays stopped —
    the edit marks the composition and the next play reads it."""
    class _Transport:
        playing = False

    ed = editor(follow=True)
    ed.transport = _Transport()           # the one thing the guard reads
    ed._destination = object()            # as the first play would have left it
    passes = []
    ed.rerender = lambda **kw: passes.append(kw)

    ed._changed()
    assert passes == [], "an edit while stopped must not start a pass"
    assert ed.dirty is True, "but it is remembered, so the next play reads it"

    ed.transport.playing = True
    ed._changed()
    assert len(passes) == 1, "what is sounding does follow the edit"


def test_a_redo_tells_the_host_where_the_clip_now_is():
    """Found by hand: redo moved the model and the picture only caught up on the
    *next* undo — one step behind, which reads as a history that stopped
    working. A redo adopts the whole document rather than projecting intents, so
    it has to keep the **drawn record** in step the way the undo path does: a
    correction is read straight out of that registry."""
    ed = editor()
    host = _FakeHost()
    ed.open(host)
    wid = clips(lanes(ed.draw())[0])[0]["id"]
    placed = ed._clips[wid]

    ed.apply("/gui_event", [wid, SEQ, UNSTATED, "clip", 2.0 * BEAT, placed.dur])
    assert placed.member.offset == pytest.approx(2.0)
    ed.undo()
    ed.redo()
    assert placed.member.offset == pytest.approx(2.0), "the model went forward"
    (_seq, sets) = host.acks[-1]
    assert dict(sets)[wid]["offset"] == pytest.approx(2.0 * BEAT), \
        "and the host was told so"


def test_a_cut_across_a_clip_is_refused_with_its_reason():
    """A selection cutting *through* a clip implies a new length for the
    samples under it, which is not a placement edit. The refusal says so
    instead of doing nothing."""
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    clip = next(iter(ed._clips))
    placed = ed._clips[clip]
    at = placed.base + placed.member.offset
    span = placed.member.length or 0.0
    before = len(placed.owner.handles)
    changed = ed.apply("/gui_event", [clip, SEQ, UNSTATED, "cut",
                                      at * BEAT, span * BEAT / 2.0])
    assert not changed
    assert len(placed.owner.handles) == before
    _, _, reason = host.answers[-1]
    assert reason and "samples" in reason


def test_a_sample_paste_is_refused_because_the_audio_has_an_owner():
    """The clipboard travels with the request, and this editor says what it is
    not: an arrangement places elements, and a nameless block of audio has
    neither a source nor a source's owner until the samples half lands."""
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    clip = next(iter(ed._clips))
    changed = ed.apply("/gui_event", [clip, SEQ, UNSTATED, "paste", 0.0,
                                      "samples", '{"content":{"kind":"samples"}}'])
    assert not changed
    _, _, reason = host.answers[-1]
    assert reason and "samples" in reason


def test_a_locate_moves_the_transport_and_the_lanes_cursor():
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    # A click on a lane's ruler: the host sends "locate" in timeline units.
    ed.apply("/gui_event", [next(iter(ed._lanes)), SEQ, UNSTATED, "locate", 3 * BEAT])
    assert ed.position == pytest.approx(3.0)
    assert not ed.dirty, "seeking is a transport action, not an edit"

    # Every lane draws the cursor there, and the clock anchor is off (stopped).
    for _id, props in host.sets[-len(ed._lanes):]:
        assert props["playhead"] == pytest.approx(3 * BEAT)
        assert props["playhead_at"] == -1.0


def test_the_transport_reads_the_composition_extent_on_each_use():
    """The transport parks its cursor at the piece's end when a pass runs out, and
    a clip dragged past the end makes the piece longer — so the end it parks at is
    read from the arrangement each time, never snapshotted."""
    ed = editor(quant=1.0)
    tree = ed.draw()
    lead = clips(lanes(tree)[1])[0]
    ed.apply(*clip_event(lead["id"], 12 * BEAT, lead["dur"]))
    assert ed.transport.extent() == ed.extent() > 12.0


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
    ed.apply("/gui_event", [99_999, SEQ, UNSTATED, "locate", 3 * BEAT])
    assert ed.position == 0.0, "another editor's lane is not ours to seek from"
    ed.apply("/gui_closed", [ed.window])
    assert ed.window is None


def test_unknown_messages_are_ignored():
    ed = editor()
    ed.draw()
    assert not ed.apply("/gui_event", [1, SEQ, UNSTATED, "points", 0.0, 1.0])   # a bpf edit
    assert not ed.apply("/gui_event", [999_999, SEQ, UNSTATED, "clip", 0.0, 1.0])  # unknown id
    assert not ed.apply("/clock_query.reply", [1234.0])


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
    song = Aggregate([(2.0, Aggregate([(0.0, Element(auto))], name="filter"))], name="song")
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


def test_a_curve_is_drawn_with_the_shape_it_has():
    """A `points` argument of *tuples* is read as ``(t, v, curve_spec)`` and
    resolved, so handing the builder already-resolved ``(t, v, shape, curve)``
    quads re-read the shape number as a curvature: a linear segment (shape 1)
    was drawn as the custom shape with curvature 1.0 -- and edited back that
    way, so an envelope changed shape by being looked at. The flat form is kept
    verbatim, which is what this pins."""
    from clausters.defs.ugens import Env
    from clausters.seq.automation import Automation

    curve = Element(Automation(Env([0.0, 1.0], [2.0]), None, name="cutoff"),
                    duration=2.0)
    ed = editor(Aggregate([(0.0, curve)], name="song"))
    (c,) = clips(lanes(ed.draw())[0])
    # [t, v, shape, curve] quads: linear is shape 1 with no curvature.
    assert c["points"][2:4] == [1, 0.0]
    assert c["points"][6:8] == [1, 0.0]


def test_editing_the_curve_in_place_writes_it_back_onto_the_automation():
    song, auto = automation_song()
    ed = editor(song)
    (curve,) = clips(lanes(ed.draw())[0])

    # The host sends the same flat "points" payload the bpf view sends — here the
    # peak dragged down to 3000 Hz and a beat later.
    edited = [0.0, 200.0, 1, 0.0,
              3 * BEAT, 3000.0, 2, 0.0,
              4 * BEAT, 800.0, 1, 0.0]
    assert ed.apply("/gui_event", [curve["id"], SEQ, UNSTATED, "points", *edited])

    # The automation's Env — its source of truth, what the next rendering plays.
    assert auto.to_points()[4:6] == pytest.approx([3.0, 3000.0])  # in beats again
    assert auto.duration() == pytest.approx(4.0)
    # And the redraw shows what was dropped.
    assert clips(lanes(ed.draw())[0])[0]["points"][4:6] == pytest.approx(
        [3 * BEAT, 3000.0])


def test_editing_the_curve_refills_the_control_buffer_it_is_played_from():
    """Found by ear: the drawn curve changed and the sweep sounded the same.

    An `Automation`'s `Env` is its source of truth, but what the lane synth
    reads is the **control buffer** `prepare` filled once — so rewriting the
    envelope alone changes what the next render schedules and not what it
    sounds."""
    song, auto = automation_song()
    ed = editor(song)
    (curve,) = clips(lanes(ed.draw())[0])
    refills = []
    auto.refill = lambda **kw: refills.append(kw)

    ed.apply("/gui_event", [curve["id"], SEQ, UNSTATED, "points",
                            0.0, 200.0, 1, 0.0, 4 * BEAT, 900.0, 1, 0.0])
    assert refills, "the buffer the curve is played from follows the curve"


def test_an_envelope_attached_to_its_event_is_one_clip_that_moves_as_one():
    """An aggregate whose members start and end together *is* one thing on the timeline
    (its temporal relation says so), so it draws as one clip with **layered**
    bodies — the envelope over the clang it shapes — and dragging it moves the
    whole aggregate, not one of its parts."""
    from clausters.form import Element
    from clausters.seq import Automation

    env = Automation.from_points([(0, 200.0, 1, 0.0), (4, 900.0, 2, 0.0)],
                                 target=None, name="sweep")
    voice = Clang(SeqEvent(midinote=60, dur=4.0))
    attached = Aggregate([(0.0, voice), (0.0, Element(env, duration=4.0))], name="sweep")
    assert attached.temporal_relation() == "simultaneous"

    ed = editor(Aggregate([(2.0, attached)], name="song"))
    (lane,) = lanes(ed.draw())
    (c,) = clips(lane)
    # One clip, both bodies — and each on its own value axis.
    assert c["notes"] and c["points"]
    assert c["min"] < 60.0 < c["max"]                     # the notes' pitch axis
    assert c["points_min"] < 200.0 and c["points_max"] > 900.0   # the curve's

    # Dragging it moves the aggregate: the clang and its envelope stay together.
    ed.apply(*clip_event(c["id"], 6 * BEAT, c["dur"]))
    placed = ed._clips[c["id"]]
    assert placed.member.element is attached
    assert placed.member.offset == pytest.approx(6.0)
    from clausters.form.render import flatten

    assert min(b for b, _ in flatten(ed.element)) == pytest.approx(6.0)


# ---- the logical aggregate: deferred to the directed patcher driver ----

def test_a_logical_aggregate_is_skipped_until_the_directed_driver():
    """A logical aggregate draws as a directed `graph` patch, which needs the members'
    port directions from their defs — the directed patcher's Python driver (P3).
    Until then the Editor skips it rather than draw the old bus-as-node view; a
    directed patch is built directly with `clausters.defs.GraphPatch` (see
    `examples/gui_patch1.py`)."""
    from clausters.form import Generator
    from clausters.form.aggregate import LOGICAL

    chain = Aggregate([Generator("gsrc", controls={"out": "mix"})], kind=LOGICAL,
                  name="chain", buses=["mix"])
    tree = editor(Aggregate([(0.0, chain)], name="song")).draw()
    children = tree.get("children", [])
    assert [c for c in children if is_patch(c)] == []
    assert [c for c in children if is_lane(c)] == [], \
        "a logical member is neither a lane nor (yet) a patch"


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
                      if _inner_addr(raw) == "/synth_new")

    def edited(server, clock):
        note = Clang(SeqEvent(instrument="default", freq=440.0, dur=1.0))
        song = Aggregate([(0.0, Aggregate([(0.0, note)], name="lead"))], name="song")
        ed = Editor(song, sample_rate=SR, tempo=TEMPO, quant=1.0)
        lane = lanes(ed.draw())[0]
        (c,) = clips(lane)
        ed.apply(*clip_event(c["id"], 3 * BEAT, 1 * BEAT))  # dragged to beat 3
        ed.render(server, clock)

    # The score is in seconds: beat 3 at 2 beats/sec sounds at 1.5 s — the unit
    # bridge closing on the far side.
    assert starts(edited) == [3.0 / TEMPO]


# ---- logical aggregates: a directed graph patch, not a timeline lane ----

def test_logical_patch_derives_boxes_and_cords_from_the_defs():
    patch, handles = _logical_patch(fx_chain())
    w = patch.to_widget()
    # Ports read off each SynthDef: gsrc has one outlet, gsink one inlet (terminal).
    assert w["boxes"][0] == {"def": "gsrc", "inlets": [], "outlets": ["out"]}
    assert w["boxes"][1] == {"def": "gsink", "inlets": ["in"], "outlets": []}
    # The shared bus "mix" is the cord: gsrc.out -> gsink.in.
    assert w["cords"] == [0, 0, 1, 0]
    assert len(handles) == 2


def test_editor_draws_a_root_logical_aggregate_as_a_graph():
    ed = Editor(fx_chain(), sample_rate=SR, tempo=TEMPO)
    tree = ed.draw()
    scrolls = [c for c in tree["children"] if is_plane(c)]
    assert len(scrolls) == 1, "the logical aggregate is a pan/zoom graph workspace"
    view = scrolls[0]["children"][0]
    assert is_patch(view)
    assert view["cords"] == [0, 0, 1, 0]
    # Registered so an edit-back resolves to the aggregate it draws.
    assert view["id"] in ed._patches
    assert ed._patches[view["id"]][0] is ed.element


def test_a_logical_aggregate_among_concrete_lanes_draws_as_a_patch_lane():
    # A concrete root with a track lane and a logical aggregate beside it.
    melody = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0))]))
    root = Aggregate([(0.0, Aggregate([(0.0, melody)], name="lead")),
                  (0.0, fx_chain())], name="song")
    tree = Editor(root, sample_rate=SR, tempo=TEMPO).draw()
    kids = tree["children"]
    assert any(is_lane(c) for c in kids) and any(is_plane(c) for c in kids)
    # The ruler is its own strip under the stack; a patch lane has no time axis,
    # so a window that drew *only* one gets no ruler at all.
    assert is_ruler(kids[-1])
    only_patch = Editor(fx_chain(), sample_rate=SR).draw()["children"]
    assert len(only_patch) == 1 and is_plane(only_patch[0])
    assert is_patch(only_patch[0]["children"][0])


def test_a_wire_edit_rewrites_the_members_controls_onto_a_shared_bus():
    # Two unconnected members (no controls): wire src.out -> sink.in.
    src = SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))))
    sink = SynthDef("gsink", out(0, in_(control("in")) * control("amp", 0.3)))
    g = Aggregate(kind=LOGICAL, name="chain")
    hs = g.add(Generator(src))
    hk = g.add(Generator(sink))
    ed = Editor(g, sample_rate=SR, tempo=TEMPO)
    tree = ed.draw()
    wid = [c for c in tree["children"] if is_plane(c)][0]["children"][0]["id"]

    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "wire", 0, "out", 1, "in"]) is True
    # Both members now name one internal bus; the aggregate declares it (audio).
    bus = hs.element.controls["out"]
    assert bus and hk.element.controls["in"] == bus
    assert bus in g.bus_names
    # And it compiles: the GraphDef wires both members to that bus.
    spec = g.to_graphdef("chain").spec()
    wired = {m["def"]: m.get("controls", {}) for m in spec["members"]}
    assert wired["gsrc"]["out"] == wired["gsink"]["in"]
    assert ed.dirty is True


def test_a_wire_reuses_an_existing_bus_for_fan_out():
    src = SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))))
    sink = SynthDef("gsink", out(0, in_(control("in"))))
    g = Aggregate(kind=LOGICAL, name="chain", buses=[("mix", "audio")])
    g.add(Generator(src, controls={"out": "mix"}))   # already writes "mix"
    hk = g.add(Generator(sink))                       # unwired sink
    ed = Editor(g, sample_rate=SR, tempo=TEMPO)
    tree = ed.draw()
    wid = [c for c in tree["children"] if is_plane(c)][0]["children"][0]["id"]
    ed.apply("/gui_event", [wid, SEQ, UNSTATED, "wire", 0, "out", 1, "in"])
    # The sink joins the source's existing bus, not a fresh one.
    assert hk.element.controls["in"] == "mix"
    assert g.bus_names == ["mix"]


def test_a_graph_box_move_persists_its_position_across_a_redraw():
    ed = Editor(fx_chain(), sample_rate=SR, tempo=TEMPO)
    tree = ed.draw()
    wid = [c for c in tree["children"] if is_plane(c)][0]["children"][0]["id"]
    # A move is presentation only: the composition did not change.
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "move", 1, 300.0, 120.0]) is False
    # It survives a redraw (keyed by the aggregate, not the widget id).
    view = [c for c in ed.draw()["children"] if is_plane(c)][0]["children"][0]
    assert view["boxes"][1]["x"] == pytest.approx(300.0)
    assert view["boxes"][1]["y"] == pytest.approx(120.0)


# ---- the acknowledgement: an owner answers with a value ----


def test_a_refused_note_edit_sends_the_notes_back_rather_than_saying_nothing():
    """The case that made this whole mechanism necessary: a generator's notes are
    a *rendering* of an algorithm, so the edit is refused -- and until now the
    refusal was silence, which left the host drawing the note where the hand put
    it until the next whole-tree redefine, which for a placement edit never
    comes."""
    from clausters.seq.pattern import Pbind, Pseq

    gen = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, gen)], name="bass"))], name="song"))
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)

    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "notes", 0.0, BEAT, 65, 100, 0]) is False

    (seq, corrections) = host.acks[-1]
    assert seq == SEQ
    assert len(corrections) == 1, "a refusal answers with a value, not with nothing"
    widget_id, props = corrections[0]
    assert widget_id == clip["id"]
    assert props["notes"] == clip["notes"], "the notes as they still are"


def test_a_snapped_clip_answers_with_where_it_actually_landed():
    """The other half, and the quieter one: the editor snaps a placement to the
    musical grid, the host drew it where it was released, and nothing used to
    say so -- the two disagreed by up to half a grid step, silently."""
    ed = editor(
        Aggregate([(0.0, Aggregate([(0.0, Clang(SeqEvent(dur=1.0), duration=1.0))],
                           name="lead"))], name="song"),
        quant=1.0,
    )
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)

    # Dropped at 4.3 beats on a one-beat grid: the arrangement puts it at 4.
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "clip", 4.3 * BEAT, float(BEAT)])

    (_, corrections) = host.acks[-1]
    assert corrections, "a transformed edit answers with the value it became"
    _, props = corrections[0]
    assert props["offset"] == pytest.approx(4.0 * BEAT)


def test_an_edit_taken_as_given_is_still_answered():
    """Sent always, including when nothing was corrected: the stamp is what
    retires the host's pending drawing, and an edit that is never answered is one
    the host waits on forever."""
    ed = editor(
        Aggregate([(0.0, Aggregate([(0.0, Clang(SeqEvent(dur=1.0), duration=1.0))],
                           name="lead"))], name="song")
    )
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)

    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "clip", 2.0 * BEAT, float(BEAT)])
    assert host.acks[-1] == (SEQ, [])


def test_an_event_from_another_editors_window_is_not_answered():
    """A poll loop may be shared between two editors, and an editor that answered
    for a window it does not own would retire a pending edit the real owner has
    not applied."""
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, Clang(SeqEvent(dur=1.0)))], name="lead"))],
                      name="song"))
    host = _FakeHost()
    ed.open(host)
    ed.draw()
    # Opening announced the version (stamp zero: it retires nothing), and that
    # is the only thing this editor has said.
    assert host.acks == [(0, [])]
    assert ed.apply("/gui_event", [999_999, SEQ, UNSTATED, "clip", 0.0, float(BEAT)]) is False
    assert host.acks == [(0, [])]


# ---- the version, and staleness ----

def two_clips() -> Editor:
    """Two lanes, one clip each -- the smallest composition in which one edit
    can supersede another."""
    return editor(
        Aggregate([
            (0.0, Aggregate([(0.0, Clang(SeqEvent(dur=1.0), duration=1.0))], name="lead")),
            (0.0, Aggregate([(0.0, Clang(SeqEvent(dur=1.0), duration=1.0))], name="bass")),
        ], name="song"),
        quant=1.0,
    )


def test_every_acknowledgement_carries_the_version_the_next_gesture_names_back():
    """The whole of the staleness round trip, and it costs one integer: the
    editor answers with the composition's version, the host stores it, and the
    host names it back on its next event."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lane, _) = lanes(ed.draw())
    (clip,) = clips(lane)

    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "clip", 2.0 * BEAT, float(BEAT)])
    (_, version, _) = host.answers[-1]
    assert version == ed._version
    assert version > FIRST_VERSION, "an applied edit moved it"


def test_an_edit_made_against_a_superseded_version_is_refused_and_answered():
    """O4's acceptance. The composition moved between the picture the gesture
    was made against and the gesture arriving -- here by another edit, which is
    what a script or a second editor looks like from in here. The late one comes
    back as the state that holds rather than landing on top of what arrived in
    between."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lead, bass) = lanes(ed.draw())
    (late,) = clips(lead)
    (other,) = clips(bass)
    drawn_at = ed._version
    placement = ed._clips[late["id"]]
    was = (placement.offset, placement.dur)

    # Something else edits the composition first.
    ed.apply("/gui_event", [other["id"], SEQ, drawn_at, "clip", 3.0 * BEAT, float(BEAT)])
    assert ed._version != drawn_at

    stale = ed.apply(
        "/gui_event",
        [late["id"], SEQ + 1, drawn_at, "clip", 6.0 * BEAT, float(BEAT)],
    )
    assert stale is False, "a stale edit is not applied"
    assert (placement.offset, placement.dur) == was, "and it moved nothing"

    (seq, corrections) = host.acks[-1]
    assert seq == SEQ + 1, "it is still answered, or the host waits forever"
    assert corrections, "and answered with a value, like every other outcome"
    _, props = corrections[0]
    assert props["offset"] == pytest.approx(was[0])
    (_, _, reason) = host.answers[-1]
    assert reason, "with a reason: someone else changed this, not 'not here'"


def test_an_edit_made_against_the_current_version_applies():
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lane, _) = lanes(ed.draw())
    (clip,) = clips(lane)

    assert ed.apply(
        "/gui_event",
        [clip["id"], SEQ, ed._version, "clip", 2.0 * BEAT, float(BEAT)],
    ) is True


def test_a_host_that_cannot_name_a_version_is_applied_unchecked():
    """Zero is *unstated*, not a version: an older host, or one no owner has
    reported a version to, keeps the behavior it had before there were any."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lead, bass) = lanes(ed.draw())
    (clip,) = clips(lead)
    (other,) = clips(bass)
    ed.apply("/gui_event", [other["id"], SEQ, UNSTATED, "clip", 3.0 * BEAT, float(BEAT)])

    assert ed.apply(
        "/gui_event",
        [clip["id"], SEQ + 1, UNSTATED, "clip", 2.0 * BEAT, float(BEAT)],
    ) is True


def test_a_stale_note_edit_is_answered_with_the_notes_as_they_stand():
    """The same gate in the roll's own terms: what goes back is the widget's
    kind of value, so the host adopts it with the one drop-and-adopt rule."""
    tl = Timeline([(0.0, SeqEvent(midinote=60, dur=1.0))])
    ed = editor(Aggregate([
        (0.0, Aggregate([(0.0, Track(tl, duration=1.0))], name="lead")),
        (0.0, Aggregate([(0.0, Clang(SeqEvent(dur=1.0), duration=1.0))], name="bass")),
    ], name="song"))
    host = _FakeHost()
    ed.open(host)
    (lead, bass) = lanes(ed.draw())
    (roll,) = clips(lead)
    (other,) = clips(bass)
    drawn_at = ed._version
    ed.apply("/gui_event", [other["id"], SEQ, drawn_at, "clip", 3.0 * BEAT, float(BEAT)])

    assert ed.apply(
        "/gui_event",
        [roll["id"], SEQ + 1, drawn_at, "notes", 0.0, BEAT, 65, 100, 0],
    ) is False
    (_, corrections) = host.acks[-1]
    assert corrections
    _, props = corrections[0]
    assert "notes" in props


def test_opening_tells_the_host_which_version_it_is_drawing():
    """Or the opening gesture is the one edit nobody can tell is stale: the host
    would name zero until the first acknowledgement came back. The stamp is zero
    -- it retires nothing, since the host's own numbering starts at one -- so
    this carries the version and nothing else."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    assert host.answers == [(0, FIRST_VERSION, None)]


# ---- the history: undo and redo, through the crate's log ----

def test_a_run_of_gestures_undoes_back_to_where_it_started():
    """H2's acceptance. The log is the crate's, not this editor's: a history
    kept here would see only the gestures *it* made, so a script editing the
    arrangement or a second view would leave it describing a composition that
    has moved on — and undo would then write a state nobody was ever in."""
    ed = editor(quant=0.25)
    tree = ed.draw()
    roll = clips(lanes(tree)[1])[0]
    member = ed._clips[roll["id"]].member
    start = member.offset

    assert not ed.can_undo, "an unedited composition has nothing to undo"
    for beats in (4.0, 6.5, 1.0):
        assert ed.apply(*clip_event(roll["id"], beats * BEAT, 2 * BEAT))
    assert member.offset == pytest.approx(1.0)
    assert ed.can_undo
    assert ed.undo_label == "move the clip"

    while ed.can_undo:
        assert ed.undo()
    assert member.offset == pytest.approx(start), "exactly, not approximately"
    assert ed.can_redo


def test_a_redo_puts_the_clip_back_where_the_undo_took_it_from():
    ed = editor(quant=0.25)
    tree = ed.draw()
    roll = clips(lanes(tree)[1])[0]
    member = ed._clips[roll["id"]].member

    ed.apply(*clip_event(roll["id"], 5 * BEAT, 2 * BEAT))
    edited = member.offset
    assert ed.undo()
    assert member.offset != pytest.approx(edited)
    assert ed.redo()
    assert member.offset == pytest.approx(edited)


def test_undo_on_an_untouched_editor_is_false_rather_than_a_crash():
    ed = editor()
    assert ed.undo() is False
    assert ed.redo() is False
    assert ed.can_undo is False
    assert ed.undo_label is None


def test_what_the_grid_did_is_what_gets_replayed():
    """The crate records the *effective* edit, so a redo does not snap a second
    time — harmless with a grid, wrong the moment a rule is not idempotent. It
    is also the reason this editor no longer snaps: one place decides."""
    ed = editor(quant=1.0)
    tree = ed.draw()
    roll = clips(lanes(tree)[1])[0]
    member = ed._clips[roll["id"]].member

    ed.apply(*clip_event(roll["id"], 4.3 * BEAT, 2 * BEAT))
    assert member.offset == pytest.approx(4.0), "the crate snapped it"
    ed.undo()
    ed.redo()
    assert member.offset == pytest.approx(4.0), "and the redo lands on the same beat"


def test_an_undo_tells_the_host_what_to_draw_instead():
    """An inverse is an ordinary edit, so the picture has to follow it the way
    it follows any other — and without a redefine, which would re-mint every id
    and drop whatever gesture was in flight."""
    ed = editor(quant=0.25)
    host = _FakeHost()
    ed.open(host)
    roll = clips(lanes(ed.draw())[1])[0]
    drawn_at = ed._clips[roll["id"]].offset      # where the host drew it

    ed.apply(*clip_event(roll["id"], 5 * BEAT, 2 * BEAT))
    host.acks.clear()
    assert ed.undo()
    corrections = {wid: props for _seq, sets in host.acks for wid, props in sets}
    assert roll["id"] in corrections, "the undo answered with a value, not only a stamp"
    # **And the value is the restored one.** Asserting only *which* widget was
    # answered passed for months while the answer said "keep drawing it where
    # the hand dropped it": a correction is read out of the drawn registry, and
    # the undo path moved the placement without keeping that registry in step.
    # Model undone, picture unchanged, and the button read as dead.
    assert corrections[roll["id"]]["offset"] == pytest.approx(drawn_at)


def test_the_windows_undo_shortcut_reaches_the_history():
    """The host addresses undo to the **window**, not to a widget — undo is
    aimed at no place under the cursor, which is why it is not a gesture-plan
    step. The editor answers it here rather than routing it, because a history
    step is a walk through the log and not an edit to the tree."""
    ed = editor(quant=0.25)
    host = _FakeHost()
    ed.open(host)
    roll = clips(lanes(ed.draw())[1])[0]
    member = ed._clips[roll["id"]].member
    start = member.offset

    ed.apply(*clip_event(roll["id"], 5 * BEAT, 2 * BEAT))
    moved = member.offset
    assert moved != pytest.approx(start)

    win = ed._window
    assert ed.apply("/gui_event", [win, SEQ, UNSTATED, "undo"])
    assert member.offset == pytest.approx(start)
    assert ed.apply("/gui_event", [win, SEQ, UNSTATED, "redo"])
    assert member.offset == pytest.approx(moved)


def test_another_windows_shortcut_is_not_this_editors_to_answer():
    """A poll loop may be shared. Answering a window this editor did not open
    would step a history nobody asked it to."""
    ed = editor(quant=0.25)
    host = _FakeHost()
    ed.open(host)
    roll = clips(lanes(ed.draw())[1])[0]
    ed.apply(*clip_event(roll["id"], 5 * BEAT, 2 * BEAT))
    moved = ed._clips[roll["id"]].member.offset

    assert not ed.apply("/gui_event", [ed._window + 999, SEQ, UNSTATED, "undo"])
    assert ed._clips[roll["id"]].member.offset == pytest.approx(moved)


def test_a_poll_feeds_the_arrangement_and_the_windows_own_handlers():
    """A window carries both: the editor's widgets and the script's own strip.

    `Editor.poll` is the loop that takes the message off the socket, so a
    transport button beside the editor -- a widget this editor never drew --
    gets its handler run here. A drain that only fed the arrangement swallowed
    those: the button was pressed, the host reported it, nothing happened.
    """
    ed = editor()
    host = _FakeHost()
    ed.open(host)
    lane = next(iter(ed._lanes))
    host.inbox = [("/gui_event", [9_999, SEQ, UNSTATED, 1]),
                  ("/gui_event", [lane, SEQ, UNSTATED, "selection", BEAT, BEAT])]
    ed.poll()
    assert [addr for addr, _ in host.dispatched] == ["/gui_event", "/gui_event"]
    assert host.dispatched[0][1][0] == 9_999, "the strip's button reached its handler"
    assert ed.selection["start"] == pytest.approx(1.0), "and the sweep still landed"


# ---- O13: one document, held, and undo that reaches inside a clip ----------


def _notes(track) -> list:
    return [(round(beat, 3), item["midinote"]) for beat, item in track.wraps]


def test_a_note_edited_in_a_roll_is_undoable_and_redoable():
    """The entry this milestone exists for. A note edit used to rewrite the
    timeline straight onto the arrangement, so it left no entry in the log and
    could not be stepped back — undo worked for clips and for nothing inside
    one."""
    piece = song()
    track = piece.members[1][2].members[0][2]
    ed = editor(piece, quant=0.0)
    ed._history()
    before = _notes(track)

    moved = []
    for beat, item in track.wraps:
        pitch = 72 if item["midinote"] == 64 else item["midinote"]
        moved += [ed.beats_to_units(beat), ed.beats_to_units(item.get("dur") or 1.0),
                  pitch, 100, 0]
    assert ed._apply_notes(track, moved)
    assert 72 in [p for _, p in _notes(track)]

    assert ed.undo()
    assert _notes(track) == before, "exactly, not approximately"
    assert ed.redo()
    assert 72 in [p for _, p in _notes(track)]


def test_a_break_point_edited_on_a_curve_is_undoable_and_redoable():
    """The other half: a curve's points had to reach the document before an
    inverse could exist, since a leaf's config named the automation and nothing
    else."""
    from clausters.defs import Env
    from clausters.form import Element
    from clausters.seq.automation import Automation

    auto = Automation(Env([100.0, 400.0], [2.0]), "freq", name="sweep")
    lane = Aggregate([(0.0, Element(auto, duration=2.0))], name="sweep")
    ed = editor(Aggregate([(0.0, lane)], name="song"), quant=0.0)
    ed._history()
    before = list(auto.to_points())

    placed = ed._clips.get(next(iter(ed._clips), None)) if ed._clips else None
    handle = lane.handles[0]
    assert ed._apply_points(type("Placed", (), {"member": handle})(),
                            [ed.beats_to_units(0.0), 200.0, 1, 0.0,
                             ed.beats_to_units(2.0), 300.0, 1, 0.0])
    assert auto.to_points()[1] == 200.0

    assert ed.undo()
    assert list(auto.to_points()) == before
    assert ed.redo()
    assert auto.to_points()[1] == 200.0


def test_the_document_is_held_rather_than_rebuilt_per_gesture():
    """What the milestone is measured on: the handle survives a gesture, so an
    edit costs the edit and not the composition."""
    ed = editor(quant=0.0)
    _, first = ed._history()
    node = ed._node_id(ed.element.members[0][2])
    ed._record({"intent": "place", "node": node, "offset": 1.0}, "move")
    _, again = ed._history()
    assert again is first, "the same document, not a fresh one"


def test_a_script_editing_behind_the_editor_says_so_with_refresh():
    """The price of holding it, and the door that pays it. Without `refresh`
    the next edit would be made against a composition that has moved."""
    piece = song()
    ed = editor(piece, quant=0.0)
    _, first = ed._history()

    piece.add(Clang(SeqEvent(midinote=48)), offset=8.0)
    ed.refresh()
    _, after = ed._history()
    assert after is not first, "re-derived, so the new node is nameable"
    assert ed._node_id(piece.members[-1][2]) is not None


def test_dragging_the_second_of_two_windows_moves_the_second():
    """O14's acceptance, and the defect it closes: one take placed twice used to
    write two members carrying one id, so the crate applied the edit to the
    first match while the editor's index kept the last — two writes, two
    destinations, and the clip the hand moved came back to where it was."""
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=4.0)
    lane = Aggregate([(0.0, take), (4.0, take)], name="drums")
    ed = editor(Aggregate([(0.0, lane)], name="song"), quant=0.0)
    ed._history()

    first, second = lane.handles
    assert ed._node_id(take, first) != ed._node_id(take, second), "two windows, two names"

    outcome = ed._record({"intent": "place", "node": ed._node_id(take, second),
                          "offset": 6.0}, "move the clip")
    ed._project(outcome["effective"])
    assert [h.offset for h in lane.handles] == [0.0, 6.0], "the second one moved"

    assert ed.undo()
    assert [h.offset for h in lane.handles] == [0.0, 4.0]
    assert ed.redo()
    assert [h.offset for h in lane.handles] == [0.0, 6.0]


def test_a_clip_over_a_generator_draws_its_notes_read_only():
    """The picture must not follow a hand that cannot edit. A `Sequence` over a
    pattern is forward-only — its notes are a rendering of an algorithm — so the
    clip says so and the roll refuses the press, instead of offering the drag
    and unwinding it when the owner declines."""
    from clausters.seq.pattern import Pbind, Pseq

    lane = Aggregate([(0.0, Sequence(Pbind(midinote=Pseq([60, 64], 1), dur=1.0),
                                 name="line"))], name="pattern")
    ed = editor(Aggregate([(0.0, lane)], name="song"))
    clip = _find(ed.draw(), lambda n: "notes" in n)
    assert clip is not None, "a pattern lane draws its bounced notes"
    assert clip.get("editable") == 0, "and says they are read-only"

    # A track's own notes are editable, which is what says the flag is the
    # contents' and not the widget's.
    track = Aggregate([(0.0, Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0))])))],
                  name="lead")
    ed2 = editor(Aggregate([(0.0, track)], name="song"))
    editable = _find(ed2.draw(), lambda n: "notes" in n)
    assert editable is not None and "editable" not in editable


def _find(node, pred):
    """The first node in a GuiDef tree the predicate holds for."""
    if isinstance(node, dict):
        if pred(node):
            return node
        for child in node.get("children") or []:
            found = _find(child, pred)
            if found is not None:
                return found
    return None


# ---- a clip is a window onto a segment of its samples ----

def _take_song(**window) -> tuple:
    """A one-lane composition holding one four-beat take, and its element."""
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=4.0, instrument="take",
                  **window)
    return Aggregate([(0.0, Aggregate([(0.0, take)], name="audio"))], name="song"), take


def test_a_take_draws_the_window_it_reads():
    """A clip shows the segment its element reads. A whole-take clip says
    nothing about a window -- that is what reading from the first frame is --
    and a trimmed one says where it now begins."""
    song, _ = _take_song()
    (lane,) = lanes(editor(song).draw())
    (clip,) = clips(lane)
    assert "start" not in clip and "loop" not in clip

    song, _ = _take_song(start=2 * BEAT, loop=True)
    (lane,) = lanes(editor(song).draw())
    (clip,) = clips(lane)
    assert clip["start"] == pytest.approx(2 * BEAT)
    assert clip["loop"]


def test_a_trim_moves_the_window_and_is_undone_as_one():
    """The head trim the host reports: the clip begins later, is shorter, and
    reads its samples from further in -- and an undo gives back the frames it
    hid, because the window is a leaf's configuration and went through the log
    like every other edit."""
    song, take = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    # Trimmed one beat off the head: offset, duration and window all move by it.
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "clip",
                                   BEAT, 3 * BEAT, BEAT]) is True
    assert take.start == pytest.approx(BEAT)
    assert ed.undo() is True
    assert take.start == pytest.approx(0.0)


def test_a_split_gives_two_windows_over_one_buffer():
    """The cut: the first half keeps the head it had and stops early, the second
    begins where it left off -- one buffer, two windows -- and it is one edit,
    so an undo puts the clip back whole."""
    song, take = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * BEAT]) is True

    (lane,) = lanes(ed.draw())
    first, second = clips(lane)
    assert first["dur"] == pytest.approx(BEAT)
    assert second["offset"] == pytest.approx(BEAT)
    assert second["dur"] == pytest.approx(3 * BEAT)
    # The second reads on from where the first stops, over the same buffer.
    assert second["start"] == pytest.approx(BEAT)
    assert second["buffer"] == first["buffer"] == 7

    assert ed.undo() is True
    (lane,) = lanes(ed.draw())
    (whole,) = clips(lane)
    assert whole["dur"] == pytest.approx(4 * BEAT)


def test_a_join_puts_a_split_clip_back_together():
    """The inverse of the cut, and the case the arrangement can express: windows
    onto one buffer that continue each other become the one window they were cut
    from."""
    song, _ = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * BEAT])
    (lane,) = lanes(ed.draw())
    first, second = clips(lane)

    assert ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                                   first["id"], second["id"]]) is True
    (lane,) = lanes(ed.draw())
    (joined,) = clips(lane)
    assert joined["dur"] == pytest.approx(4 * BEAT)
    assert "start" not in joined


def test_clips_over_different_buffers_join_into_one_element():
    """Two windows onto *different* buffers read as one thing: the arrangement
    has an element for exactly that — a list of windows onto whatever buffers
    they come from, read back to back — so the join makes one, and the clip
    draws one take per segment over its own stretch."""
    from clausters.form import Segments

    a = Vector(ServerBuffer(bufnum=7, frames=int(BEAT), channels=1, sample_rate=SR),
               duration=1.0, instrument="take")
    b = Vector(ServerBuffer(bufnum=8, frames=int(BEAT), channels=1, sample_rate=SR),
               duration=1.0, instrument="take", start=100.0)
    song = Aggregate([(0.0, Aggregate([(0.0, a), (1.0, b)], name="audio"))], name="song")
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    first, second = clips(lane)
    assert ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                                   first["id"], second["id"]]) is True

    (lane,) = lanes(ed.draw())
    (joined,) = clips(lane)
    assert joined["dur"] == pytest.approx(2 * BEAT)
    # One clip, one take per segment, each on its own half and reading its own
    # buffer from its own frame.
    takes = joined["children"]
    assert [t["buffer"] for t in takes] == [7, 8]
    assert [t["at"] for t in takes] == pytest.approx([0.0, BEAT])
    assert [t["dur"] for t in takes] == pytest.approx([BEAT, BEAT])
    assert "start" not in takes[0] and takes[1]["start"] == pytest.approx(100.0)

    # ...and it plays as one thing: one event per segment, at its own offset.
    element = ed._clips[joined["id"]].member.element
    assert isinstance(element, Segments)
    events = element.to_events()
    assert [o for o, _ in events] == pytest.approx([0.0, 1.0])
    assert [dict(e)["buf"] for _, e in events] == [7, 8]

    # An undo puts the two clips back: what was joined was never copied.
    assert ed.undo() is True
    (lane,) = lanes(ed.draw())
    assert len(clips(lane)) == 2


def test_a_join_of_one_run_of_one_buffer_is_the_window_it_was_cut_from():
    """The other shape of the same verb, and it is what makes a join the inverse
    of a split: fragments that are one run of one buffer come back as the single
    window, not as a list of one."""
    from clausters.form import Segments

    song, _ = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * BEAT])
    (lane,) = lanes(ed.draw())
    first, second = clips(lane)
    ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                            first["id"], second["id"]])
    (lane,) = lanes(ed.draw())
    (joined,) = clips(lane)
    element = ed._clips[joined["id"]].member.element
    assert isinstance(element, Vector) and not isinstance(element, Segments)
    assert joined["dur"] == pytest.approx(4 * BEAT)


def test_which_layer_a_hand_is_on_is_screen_state():
    """Selecting a layer changes nothing in the composition -- it is what a view
    is currently editing, which the document is explicit is never part of it --
    and it is kept so a driver can ask."""
    song, _ = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "layer", "points"]) is False
    assert ed.dirty is False


def test_a_joined_clip_cuts_apart_into_the_windows_it_was_made_of():
    """The cut and the join are inverses over segments too: joining two buffers
    and cutting the result where they meet gives back two clips, each reading
    what it read before — nothing was copied, so nothing can be lost."""
    from clausters.form import Segments

    a = Vector(ServerBuffer(bufnum=7, frames=int(2 * BEAT), channels=1, sample_rate=SR),
               duration=1.0, instrument="take")
    b = Vector(ServerBuffer(bufnum=8, frames=int(2 * BEAT), channels=1, sample_rate=SR),
               duration=1.0, instrument="take", start=200.0)
    song = Aggregate([(0.0, Aggregate([(0.0, a), (1.0, b)], name="audio"))], name="song")
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    first, second = clips(lane)
    ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                            first["id"], second["id"]])

    # Cut it exactly where the two buffers meet.
    (lane,) = lanes(ed.draw())
    (joined,) = clips(lane)
    assert ed.apply("/gui_event", [joined["id"], SEQ, UNSTATED, "split", BEAT]) is True
    (lane,) = lanes(ed.draw())
    left, right = clips(lane)
    # Each half reads one buffer again, from its own frame. The **tail** is a
    # plain window rather than a list of one -- which is what makes a cut and a
    # join inverses instead of a pile of wrappers.
    tail = ed._clips[right["id"]].member.element
    assert isinstance(tail, Vector) and not isinstance(tail, Segments)
    assert (right["buffer"], right["start"]) == (8, pytest.approx(200.0))
    # The **head** is the element it always was, with its placement shortened:
    # it draws the one segment it now reaches...
    assert [t["buffer"] for t in left["children"]] == [7]
    head = ed._clips[left["id"]].member
    assert isinstance(head.element, Segments)
    assert [s.duration for s in head.element.segments] == pytest.approx([1.0, 1.0])
    # ...and lengthening it again brings the other one back, exactly as
    # lengthening a trimmed take brings its frames back.
    head.dur = 2.0
    (lane,) = lanes(ed.draw())
    assert [t["buffer"] for t in clips(lane)[0]["children"]] == [7, 8]


def test_a_segments_clip_shows_and_plays_only_what_its_placement_covers():
    """A placement is a window onto an element, and that holds for samples made
    of several segments: a clip shortened over it draws — and plays — the
    segments it reaches, and lengthening it again brings the rest back."""
    from clausters.form import Segments

    a = ServerBuffer(bufnum=7, frames=int(2 * BEAT), channels=1, sample_rate=SR)
    b = ServerBuffer(bufnum=8, frames=int(2 * BEAT), channels=1, sample_rate=SR)
    seg = Segments([(a, 0.0, 1.0), (b, 0.0, 1.0)], instrument="take")
    audio = Aggregate(name="audio")
    audio.add(seg, 0.0, 1.5)                       # a placement of 1.5 beats
    ed = editor(Aggregate([(0.0, audio)], name="song"))
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    takes = clip["children"]
    assert [t["buffer"] for t in takes] == [7, 8]
    assert [t["dur"] for t in takes] == pytest.approx([BEAT, 0.5 * BEAT])

    # The whole buffer is still there: the placement is what was shortened.
    assert [s.duration for s in seg.segments] == pytest.approx([1.0, 1.0])
