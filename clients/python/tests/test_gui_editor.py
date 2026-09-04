"""The multitrack editor driver (`clausters.gui.editing`) — the arrangement↔GuiDef bridge.

No server and no GUI host: the forward draw is a pure function of the arrangement
tree, so these check the mapping rule (lanes, clips, bodies), the beats↔timeline-
samples unit bridge, and the id registry that the edit-back path writes through.
"""

import itertools

import pytest

from clausters.defs import SynthDef, control, in_, out, sine
from clausters.defs.ugens import points_to_env
from clausters.defs.buffer import Buffer as ServerBuffer
from clausters.form import (Aggregate, Element, Generator, Clang, Sequence,
                            Track, Vector)
from clausters.form.aggregate import LOGICAL
from clausters.form.document import FIRST_VERSION, to_document
from clausters.base import _osclib
from clausters.gui.editing import FormEditor
from clausters.gui.host import _prop_args
from clausters.gui.editing.formeditor import _logical_patch
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
#: Timeline samples per **second**, the other half of the bridge: an onset is in
#: beats and a length is in the unit of its own data, so a take's and a curve's
#: lengths are drawn against this one and no tempo enters them.
SEC = SR


def song() -> Aggregate:
    """A two-lane composition: a take on one lane, a melody on another."""
    # 96 000 frames is two seconds at 48 kHz, and a take's duration is in
    # seconds -- so this one is as long as its samples, whatever the tempo.
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=2.0)
    audio = Aggregate([(0.0, take)], name="audio")
    melody = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                             (1.0, SeqEvent(midinote=64, dur=1.0)),
                             (2.0, SeqEvent(midinote=67, dur=2.0))]))
    lead = Aggregate([(2.0, melody)], name="lead")
    return Aggregate([(0.0, audio), (0.0, lead)], name="song")


def editor(element=None, **kwargs) -> FormEditor:
    return FormEditor(element or song(), sample_rate=SR, tempo=TEMPO, **kwargs)


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


def test_a_clip_is_drawn_where_the_clock_plays_it_across_a_tempo_change():
    """The measured defect: at 48 kHz with the tempo doubled at beat 2, beat 8
    used to be drawn at 384 000 units — the line reaching it after 8.0 s of wall
    clock while the clock played it at 5.0 s, three seconds apart.

    The axis is real time, so the only right answer is the second the clock
    plays that beat at. Drawing it needs the whole tempo history, not the tempo
    in force now, which is why an editor holds a map and not a ratio.
    """
    from clausters.base import TempoMap

    tempo = TempoMap(1.0)
    tempo.push(2.0, 2.0)                       # doubled at beat 2 (second 2.0)
    ed = editor(tempo_map=tempo)
    assert ed.beats_to_units(8.0) == pytest.approx(5.0 * SR)
    # The beats before the change keep the seconds they actually fell on: it is
    # the tempo *history* that is drawn, not an extrapolation of the last slope.
    assert ed.beats_to_units(1.0) == pytest.approx(1.0 * SR)
    # And the edit-back inverts it, so a drag still round-trips.
    assert ed.units_to_beats(ed.beats_to_units(8.0)) == pytest.approx(8.0)
    # The transport places the sweeping line from the same function, which is
    # what makes the line and the sound agree rather than merely be close.
    assert ed.transport.beats_to_samples(8.0) == pytest.approx(5.0 * SR)


def test_a_length_in_beats_is_measured_where_it_sits():
    """A length in beats is not a duration: the same two beats are two seconds
    at the start and one after the tempo doubles, so the bridge takes the onset
    and never a beat count alone."""
    from clausters.base import TempoMap

    tempo = TempoMap(1.0)
    tempo.push(4.0, 2.0)
    ed = editor(tempo_map=tempo)
    beats_element = Element(Clang(SeqEvent({"freq": 440})).wraps)
    assert ed.length_to_units(2.0, beats_element, at=0.0) == pytest.approx(2.0 * SR)
    assert ed.length_to_units(2.0, beats_element, at=4.0) == pytest.approx(1.0 * SR)


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
    assert take["dur"] == pytest.approx(2 * SEC)  # the take's own length, 1:1
    assert take["offset"] == 0.0


def test_a_vector_spans_its_frames_only_when_it_has_no_duration():
    """A buffer read but never queried has no frame count client-side; its
    element's `duration` is what places it, and must win over the frames."""
    unqueried = Vector(ServerBuffer(bufnum=3), duration=2.0)   # frames unknown (0)
    (lane,) = lanes(editor(Aggregate([(0.0, unqueried)], name="take")).draw())
    (c,) = clips(lane)
    assert c["dur"] == pytest.approx(2 * SEC)

    # With no duration either, the take's own frames are its length.
    sized = Vector(ServerBuffer(bufnum=3, frames=int(1.5 * BEAT), sample_rate=SR))
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
    """A `Track` of two notes and one OSC item on an editable timeline."""
    from clausters.form import Track
    from clausters.seq.event import Event as SeqEvent
    from clausters.seq.timeline import OscItem, Timeline

    tl = Timeline()
    tl.add(0.0, SeqEvent(dict(midinote=60, dur=1.0, amp=0.5)))
    tl.add(1.0, SeqEvent(dict(midinote=64, dur=0.5, amp=0.8)))
    tl.add(0.5, OscItem("/cue"))
    return Track(tl), tl


def _roll(ed, element=None, host=None):
    """The dedicated roll `open_pianoroll` composes: the editor, its tree and
    the widget that draws the notes.

    It is a `NotesEditor` of its own -- the same one `edit(timeline)` opens --
    joined to this composition's editing context, so what follows is read off
    the composed editor and not off the multitrack.
    """
    ed.open_pianoroll(host or _FakeHost(), element)
    roll = ed.composed[-1]
    tree = roll.draw()
    return roll, tree, next(iter(roll.view.widgets))


def test_an_element_opens_as_a_dedicated_piano_roll():
    track, _tl = _track_with_a_take()
    ed = FormEditor(track, sample_rate=SR, tempo=TEMPO, quant=0.25)
    _editor, tree, _wid = _roll(ed, track)
    (roll,) = tree["children"][:1]
    assert roll["type"] == "notes"
    # Notes as quintuples (pitch is the 3rd), the OSC item on its own lane.
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
    ed = FormEditor(take, sample_rate=SR, tempo=TEMPO)
    ed.open_signal(_FakeHost(), take)
    view = ed.composed[-1].draw()["children"][0]
    assert view["type"] == "signal"
    assert view["measure"] == "peak rms"
    # The heavy view the shipped waveform is: navigable, over the one source,
    # ruling real time.
    assert "navigable" not in view or view["navigable"] == 1
    assert (view["buffer"], view["channels"]) == (7, 2)
    assert view["axes"]["x"]["unit"] == "time"
    # It is the editor's target as a lane is -- the playhead line is drawn on
    # it -- and it is the composed editor that holds the take, which is what
    # makes a stroke here an edit of this piece.
    assert view["id"] in ed._playline()
    assert ed.composed[-1].structure is take.wraps
    assert ed.composed[-1].composed_over is take


def test_the_bare_envelope_is_a_shorter_stack():
    take = Vector(ServerBuffer(bufnum=7, frames=1000, channels=1, sample_rate=SR))
    ed = FormEditor(take, sample_rate=SR, tempo=TEMPO)
    host = _FakeHost()
    ed.open_signal(host, take, layers=("peak",))
    assert ed.composed[-1].draw()["children"][0]["measure"] == "peak"
    # ...and assigning it on the open view is **one message**, not a redraw:
    # the picture, the axis, the zoom and the playhead stay where they are.
    ed.layers = ("peak", "rms")
    assert ed.layers == ("peak", "rms")
    assert host.sets[-1][1] == {"measure": "peak rms"}
    assert host.defines == []


def test_a_selection_swept_on_a_signal_view_is_of_that_element():
    # 96 000 frames is two seconds at 48 kHz, and a take's duration is in
    # seconds -- so this one is as long as its samples, whatever the tempo.
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=2.0)
    ed = FormEditor(take, sample_rate=SR, tempo=TEMPO)
    ed.open_signal(_FakeHost(), take)
    wid = ed.composed[-1].draw()["children"][0]["id"]
    assert ed.composed[-1].apply("/gui_event", [wid, SEQ, UNSTATED, "selection",
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
    ed = FormEditor(gen, sample_rate=SR, tempo=TEMPO)
    with pytest.raises(ValueError, match="no samples"):
        ed.open_signal(_FakeHost(), gen)
    # Nothing was left open, and nothing was composed: the refusal is the
    # answer to the call, not something the first repaint discovers.
    assert ed.window is None and ed.composed == []


def test_an_unknown_measure_is_refused_by_name():
    take = Vector(ServerBuffer(bufnum=7, frames=1000, channels=1, sample_rate=SR))
    ed = FormEditor(take, sample_rate=SR, tempo=TEMPO)
    with pytest.raises(ValueError, match="loudness"):
        ed.open_signal(_FakeHost(), take, layers=("peak", "loudness"))


def test_a_note_edit_rewrites_the_editable_timeline():
    track, tl = _track_with_a_take()
    ed = FormEditor(track, sample_rate=SR, tempo=TEMPO)
    roll, _tree, wid = _roll(ed, track)
    # Move pitch 60 -> 62 and add a note; times/durs in timeline units.
    edited = [0.0, BEAT, 62, 100, 0, 1.0 * BEAT, 0.5 * BEAT, 67, 90, 0]
    assert roll.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *edited]) is True
    items = tl.range(0.0, float("inf"))
    pitches = [it.get("midinote") for _b, it in items if hasattr(it, "get")]
    assert pitches == [62, 67]                       # the notes were rewritten
    # The OSC item on the same timeline is preserved.
    from clausters.seq.timeline import OscItem
    assert any(isinstance(it, OscItem) for _b, it in items)


def test_a_note_edit_keeps_what_the_roll_cannot_say_and_moves_only_the_sustain():
    """Found by ear, editing the composer's melody: one dragged note and the
    whole lane came back fully legato with its notes running together.

    A roll draws what a note **sounds** (its ``sustain``, ``dur * legato``) and
    the edit-back wrote that number into ``dur`` with a ``legato`` of 1 — for
    *every* note in the payload, since the payload is the whole lane. So a note
    nobody touched had its grid length quietly shortened to what it had been
    sounding and its articulation replaced, and anything the roll cannot say
    (the instrument, the author's own keys) was dropped with the event it was
    rebuilt from."""
    from clausters.seq.timeline import Timeline

    track = Track(Timeline([
        (0.0, SeqEvent(instrument="reed", midinote=60, dur=1.0, legato=0.8, amp=0.4)),
        (1.0, SeqEvent(instrument="reed", midinote=64, dur=1.0, legato=0.8, amp=0.4)),
    ]))
    ed = FormEditor(track, sample_rate=SR, tempo=TEMPO)
    roll, tree, wid = _roll(ed, track)
    drawn = _find(tree, lambda n: "notes" in n)["notes"]
    assert drawn[1] == pytest.approx(0.8 * BEAT), "a note is drawn as it sounds"

    # The second note is dragged out to two beats; the first is left alone.
    edited = list(drawn)
    edited[6] = 2.0 * BEAT
    assert roll.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *edited]) is True

    first, second = [it for _b, it in track.wraps]
    # The note nobody touched is the note it was, in every key.
    assert (first.get("dur"), first.get("legato")) == (1.0, 0.8)
    assert first.sustain() == pytest.approx(0.8)
    # The edited one sounds what the hand drew, and keeps the rest.
    assert second.sustain() == pytest.approx(2.0)
    assert (second.get("dur"), second.get("legato")) == (1.0, 0.8)
    assert [it.get("instrument") for it in (first, second)] == ["reed", "reed"]
    assert [it.get("amp") for it in (first, second)] == [0.4, 0.4]
    # ...and the picture round-trips: what is redrawn is what was sent.
    again = _find(roll.draw(), lambda n: "notes" in n)["notes"]
    assert again == pytest.approx(edited)


def test_a_note_edit_survives_an_event_that_is_not_plain_data():
    """Found by crash, editing a note in a piece that had been played: an
    `Event` that has sounded carries its `server`, the intent travels to the
    crate as JSON, and handing the event over raw is a `TypeError` in the middle
    of a drag. The config goes through the conversion's own door, which writes a
    reference for what it cannot serialize."""
    import json

    from clausters.seq.timeline import Timeline

    class _Sounded:
        """Stands in for what `Event.play` leaves on an event — an object."""

    played = SeqEvent(midinote=60, dur=1.0, server=_Sounded())
    track = Track(Timeline([(0.0, played)]))
    ed = FormEditor(track, sample_rate=SR, tempo=TEMPO)
    roll, tree, wid = _roll(ed, track)
    notes = _find(tree, lambda n: "notes" in n)["notes"]

    assert roll.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *notes]) is True
    (item,) = [it for _b, it in track.wraps]
    # What the document can hold, it keeps -- the instrument is the one the old
    # rebuild-from-five-numbers dropped.
    assert item.get("instrument") == "default"
    assert item.sustain() == pytest.approx(0.8)
    # And what it holds is JSON, which is what broke: the object that is not
    # plain data comes back as the reference the conversion writes for it (see
    # "an edit round-trips a note through the document" in the plan).
    json.dumps(to_document(track))


def test_a_generator_element_is_read_only_in_the_piano_roll():
    from clausters.seq.pattern import Pbind, Pseq

    gen = Sequence(Pbind(midinote=Pseq([60, 62], 1), dur=1.0))
    ed = FormEditor(gen, sample_rate=SR, tempo=TEMPO)
    roll, tree, wid = _roll(ed, gen)
    # **Said before the hand tries**: the notes are a rendering of an
    # algorithm, so the widget itself refuses the press.
    assert _find(tree, lambda n: "notes" in n)["notes_editable"] == 0
    # ...and the edit is ignored if one arrives anyway.
    assert roll.apply("/gui_event",
                      [wid, SEQ, UNSTATED, "notes", 0.0, BEAT, 65, 100, 0]) is False
    assert roll.dirty is False


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
    # The OSC item sharing the timeline is preserved.
    from clausters.seq.timeline import OscItem
    assert any(isinstance(it, OscItem) for _b, it in items)


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
    # Four seconds of curve beside eight beats of notes: at 120 bpm those are
    # the same stretch of time, which is what makes the two one clip.
    tl = Timeline([(0.0, SeqEvent(midinote=60, dur=8.0))])
    attached = Aggregate([(0.0, Track(tl, duration=8.0)),
                      (0.0, Element(env, duration=4.0))], name="sweep")
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, attached)], name="sweep"))], name="song"))
    (lane,) = lanes(ed.draw())
    (c,) = clips(lane)
    assert c["notes"] and c["points"]

    assert ed.apply("/gui_event", [c["id"], SEQ, UNSTATED, "notes", 0.0, 4 * BEAT, 67, 100, 0]) is True
    assert [it.get("midinote") for _b, it in tl.range(0.0, float("inf"))] == [67]


def test_a_layered_clip_routes_a_curve_edit_to_the_member_that_carries_it():
    """The mirror of the note case, and the one that was broken: the aggregate
    is not a leaf, so a `configure` addressed to *it* replaced an empty
    configuration and the crate had nowhere to keep the points — the edit
    reported success, changed nothing and left no undo behind."""
    from clausters.form import Element
    from clausters.seq import Automation

    env = Automation.from_points([(0, 200.0, 1, 0.0), (2, 900.0, 2, 0.0),
                                  (4, 300.0, 1, 0.0)], target=None, name="sweep")
    # The notes half is a `Clang` — a rendering, with no editable timeline —
    # which is what the composer's sweep lane holds and what made the whole clip
    # read as locked.
    # Eight beats of clang and four seconds of curve: the same stretch at 120
    # bpm, so the aggregate is simultaneous and draws as one layered clip.
    attached = Aggregate([(0.0, Clang(SeqEvent(instrument="drone", dur=8.0))),
                          (0.0, Element(env, duration=4.0))], name="sweep")
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, attached)], name="sweep"))], name="song"))
    (lane,) = lanes(ed.draw())
    (c,) = clips(lane)
    assert c["points"], "the curve is drawn"
    # The roll's refusal is the roll's: it does not reach the curve over it.
    assert c.get("notes_editable") == 0
    assert "editable" not in c, "the clip-wide key would lock the curve too"

    assert ed.apply("/gui_event", [c["id"], SEQ, UNSTATED, "points",
                                   0.0, 300.0, 1, 0.0,
                                   2 * SEC, 500.0, 1, 0.0,
                                   4 * SEC, 100.0, 1, 0.0]) is True
    assert env.to_points()[0:2] == pytest.approx([0.0, 300.0])
    assert env.to_points()[4:6] == pytest.approx([2.0, 500.0])
    # ...and it is an edit like any other, so it is in the history.
    assert ed.can_undo and ed.undo_label == "edit the curve"
    ed.undo()
    assert env.to_points()[0:2] == pytest.approx([0.0, 200.0])


def test_editing_a_curve_does_not_move_the_axis_it_is_drawn_against():
    """A break-point's place on screen is its value **against the clip's value
    axis**, so an axis recomputed from the break-points moves every point
    whenever one is dragged -- the curve jumps under the hand editing it, and
    the point being dragged is the only one that appears to stay put."""
    from clausters.form import Element
    from clausters.seq import Automation

    env = Automation.from_points([(0, 200.0, 1, 0.0), (2, 900.0, 2, 0.0),
                                  (4, 300.0, 1, 0.0)], target=None, name="sweep")
    # Eight beats of clang and four seconds of curve: the same stretch at 120
    # bpm, so the aggregate is simultaneous and draws as one layered clip.
    attached = Aggregate([(0.0, Clang(SeqEvent(instrument="drone", dur=8.0))),
                          (0.0, Element(env, duration=4.0))], name="sweep")
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, attached)], name="sweep"))], name="song"))
    first = _find(ed.draw(), lambda n: "points" in n)
    axis = (first["points_min"], first["points_max"])

    def drag(value):
        (lane,) = lanes(ed.draw())
        (c,) = clips(lane)
        assert ed.apply("/gui_event", [c["id"], SEQ, UNSTATED, "points",
                                       0.0, 200.0, 1, 0.0,
                                       2 * SEC, value, 2, 0.0,
                                       4 * SEC, 300.0, 1, 0.0]) is True
        return _find(ed.draw(), lambda n: "points" in n)

    # Up to the ceiling the host clamps a drag to, and back down again.
    for value in (axis[1], 400.0, 250.0):
        again = drag(value)
        assert (again["points_min"], again["points_max"]) == axis

    # It **widens** for a curve that no longer fits -- a script's edit, an undo
    # of a taller one -- because the picture must show the data. It never
    # narrows, which is what keeps a drag from rescaling.
    env.env = points_to_env([0.0, 200.0, 1, 0.0, 2.0, 4000.0, 1, 0.0])
    ed.refresh()
    wide = _find(ed.draw(), lambda n: "points" in n)
    assert wide["points_min"] == axis[0], "the floor it had is kept"
    assert wide["points_max"] > 4000.0, "and the ceiling grew to hold the curve"


def test_undoing_a_curve_edit_tells_the_host_what_to_draw():
    """An undo that moves the model and says nothing is a dead button: the host
    goes on drawing the shape the hand left.

    The case that needed saying: a **layered** clip draws an aggregate, and the
    curve an edit configures is a *member* of it — so the widget an undo has to
    correct is not the one the edited element is registered against."""
    from clausters.form import Element
    from clausters.seq import Automation

    env = Automation.from_points([(0, 200.0, 1, 0.0), (2, 900.0, 2, 0.0),
                                  (4, 300.0, 1, 0.0)], target=None, name="sweep")
    # Eight beats of clang and four seconds of curve: the same stretch at 120
    # bpm, so the aggregate is simultaneous and draws as one layered clip.
    attached = Aggregate([(0.0, Clang(SeqEvent(instrument="drone", dur=8.0))),
                          (0.0, Element(env, duration=4.0))], name="sweep")
    ed = editor(Aggregate([(0.0, Aggregate([(0.0, attached)], name="sweep"))], name="song"))
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)

    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "points",
                                   0.0, 300.0, 1, 0.0,
                                   2 * SEC, 500.0, 1, 0.0,
                                   4 * SEC, 100.0, 1, 0.0]) is True
    host.acks.clear()

    assert ed.undo() is True
    assert env.to_points()[0:2] == pytest.approx([0.0, 200.0]), "the model stepped back"
    pushed = {wid: props for _seq, sets in host.acks for wid, props in sets}
    assert clip["id"] in pushed, "and the clip was told to draw it"
    assert list(pushed[clip["id"]]["points"])[0:2] == pytest.approx([0.0, 200.0])


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

def test_the_editor_opens_on_the_ambient_host_like_every_other_view():
    """The editor was the one resource that had to be handed a host. It now
    resolves the ambient one, the same rule `View.open`, `plot` and `scope`
    already shared."""
    from clausters.gui import set_ambient_host

    host = _FakeHost()
    ed = editor()
    previous = set_ambient_host(host)
    try:
        assert ed.open() == 1
    finally:
        set_ambient_host(previous)
    assert ed._host is host


def test_an_undone_take_clip_answers_with_a_correction_the_wire_can_carry():
    """Found by use, undoing a clip move in the composer example: the editor
    raised on the way out with ``OSC has no bool tag here; use int``.

    A correction for a clip over samples states the window the undo put back,
    and a window carries a ``loop`` -- a flag, which OSC has no tag for. The
    rule was written down and applied in one of the two places props become
    arguments: `GuiHost.set` coerced a flag and `GuiHost.push` did not, so the
    values a *gesture* states crossed and the values an *acknowledgement*
    carries did not. One helper now, which is the shape of the bug: two
    spellings of one rule, and only one of them knew it."""
    from clausters.form import Vector

    class Buf:
        bufnum, frames, channels, sample_rate = 1, int(SR), 1, SR

    take = Vector(Buf(), duration=1.0, instrument="take")
    piece = Aggregate([(0.0, Aggregate([(0.0, take)], name="drums"))], name="piece")
    host = _FakeHost()
    ed = FormEditor(piece, sample_rate=SR, tempo=TEMPO)
    ed.open(host)
    wid = next(iter(ed._clips))
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "clip", BEAT, SEC, 0.0]) is True
    # The undo answers with the placement and the window, and the fake host
    # builds the very message the real one would: a bool here raises.
    ed.undo()
    sent = [props for _seq, sets in host.acks for _w, props in sets]
    assert any("loop" in p for p in sent), f"the window came back: {sent}"


def test_a_clip_keeps_the_length_it_was_drawn_at_while_autofit_is_off():
    """Found by use: moving a note earlier shortened the clip it was in.

    The third face of the auto-fit, and the one a reader meets oftenest. A
    placement that states no length is drawn *as long as its content*, so a note
    edit that moved the end of the phrase resized the clip -- and with it the
    lane's extent and everything the shared axis is measured against. Under
    `autofit` it is held by the same rule the pitch domain follows: only ever
    growing, and never for a length the reader stated themselves.
    """
    from clausters.seq.timeline import Timeline

    def piece():
        lead = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                               (3.0, SeqEvent(midinote=64, dur=1.0))]))
        return Aggregate([(0.0, Aggregate([(0.0, lead)], name="lead"))], name="piece")

    def clip(tree):
        node = _find(tree, lambda n: "notes" in n)
        return node["id"], node["notes"], node["dur"]

    # On: the clip is as long as what is in it, both ways.
    ed = FormEditor(piece(), sample_rate=SR, tempo=TEMPO, autofit=True)
    wid, notes, before = clip(ed.draw())
    earlier = list(notes)
    earlier[5] = BEAT            # the second note back to beat 1
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *earlier]) is True
    assert clip(ed.draw())[2] < before, "it followed the content in"

    # Off: the same edit leaves the clip where it was drawn...
    ed = FormEditor(piece(), sample_rate=SR, tempo=TEMPO, autofit=False)
    wid, notes, before = clip(ed.draw())
    earlier = list(notes)
    earlier[5] = BEAT
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *earlier]) is True
    assert clip(ed.draw())[2] == before, "the clip kept its length"
    # ...and the registry says the same, or the next edit would be measured
    # against a picture nobody is drawing.
    assert ed._clips[wid].dur == before

    # A length the reader states is theirs, and is drawn as stated.
    assert ed.apply("/gui_event",
                    [wid, SEQ, UNSTATED, "clip", 0.0, before / 2.0, 0.0]) is True
    assert clip(ed.draw())[2] == pytest.approx(before / 2.0, abs=1.0)


def test_a_held_length_survives_the_clip_changing_lane():
    """Found by use: a clip edited and then dragged to another lane snapped back
    to its content's length, with `autofit` off.

    The hold was right and its key was not. A clip that changes lane
    **reparents** — the handle leaves one aggregate and a fresh one joins
    another — and a hold filed under the object it was derived from is lost at
    exactly that door, so the draw re-derived the length and the clip shrank
    under the hand. The undo of a cut is the same door: the placement comes back
    as a new handle.

    The document already keeps the identity that survives both — the node id,
    carried across the reparent on purpose — and that is what `_hold_key` files
    under.
    """
    from clausters.seq.timeline import Timeline

    def piece():
        lead = Track(Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                               (3.0, SeqEvent(midinote=64, dur=1.0))]))
        bass = Track(Timeline([(0.0, SeqEvent(midinote=48, dur=1.0))]))
        return Aggregate([(0.0, Aggregate([(0.0, lead)], name="lead")),
                          (0.0, Aggregate([(0.0, bass)], name="bass"))],
                         name="piece")

    def clip(ed):
        node = _find(ed.draw(), lambda n: "notes" in n and len(n["notes"]) >= 10)
        return node["id"], list(node["notes"]), node["dur"]

    ed = FormEditor(piece(), sample_rate=SR, tempo=TEMPO, autofit=False)
    wid, notes, before = clip(ed)
    earlier = list(notes)
    earlier[5] = BEAT            # the second note back to beat 1
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *earlier]) is True
    assert clip(ed)[2] == before, "the clip kept its length"

    # ...and it is still the clip the reader was looking at after it changes
    # lane. Dropped clear of what is already there: a drop that starts where
    # another clip starts is a different rule (simultaneous members draw as one
    # layered clip), and this is about the hold.
    lane = [w for w, e in ed._lanes.items() if getattr(e, "name", None) == "bass"][-1]
    wid = clip(ed)[0]
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "lane", lane, 4 * BEAT]) is True
    assert clip(ed)[2] == before, "the length crossed the lane with it"

    # With the switch on it follows the content there, as everywhere else.
    ed = FormEditor(piece(), sample_rate=SR, tempo=TEMPO, autofit=True)
    wid, notes, before = clip(ed)
    earlier = list(notes)
    earlier[5] = BEAT
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *earlier]) is True
    lane = [w for w, e in ed._lanes.items() if getattr(e, "name", None) == "bass"][-1]
    assert ed.apply("/gui_event", [clip(ed)[0], SEQ, UNSTATED, "lane", lane,
                                   4 * BEAT]) is True
    assert clip(ed)[2] < before, "it followed the content in"


def test_the_views_do_not_re_frame_themselves_while_autofit_is_off():
    """Found by use: dragging a note re-centred the whole roll, and every
    structural edit refitted the time axis.

    The general answer is one switch, and it has two halves because the two
    windows are owned by different sides. The **time** axis is the host's, so
    the lanes carry the `autofit` prop and the host stops moving the window when
    the extent changes. The **pitch domain** is a prop this editor derives from
    the notes, so it is held here — and held by *growing*, never freezing: a
    drag clamps a pitch into the window the roll is drawn over, so a domain that
    could not widen would be a roll that can never be written above its own top
    line. Monotone is what makes it stable; an edge only ever moves outward."""
    from clausters.seq.timeline import Timeline

    def piece():
        lead = Track(Timeline([
            (0.0, SeqEvent(midinote=60, dur=1.0)),
            (1.0, SeqEvent(midinote=64, dur=1.0)),
        ]))
        return Aggregate([(0.0, Aggregate([(0.0, lead)], name="lead"))], name="piece")

    def window(tree):
        roll = _find(tree, lambda n: "notes" in n)
        return roll["id"], roll["notes"], (roll["min"], roll["max"])

    # On (the host's own default): the domain follows the notes both ways.
    ed = FormEditor(piece(), sample_rate=SR, tempo=TEMPO, autofit=True)
    wid, notes, before = window(ed.draw())
    assert _find(ed.draw(), lambda n: n.get("label") == "lead")["axes"]["x"]["autofit"] is True
    moved = list(notes)
    moved[7] = 62.0
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *moved]) is True
    assert window(ed.draw())[2] != before, "it re-centred on what is left"

    # Off: the same edit moves nothing, and one that lands outside widens it.
    ed = FormEditor(piece(), sample_rate=SR, tempo=TEMPO, autofit=False)
    wid, notes, before = window(ed.draw())
    assert _find(ed.draw(), lambda n: n.get("label") == "lead")["axes"]["x"]["autofit"] is False
    moved = list(notes)
    moved[7] = 62.0
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *moved]) is True
    assert window(ed.draw())[2] == before, "the roll stayed where it was drawn"
    out = list(window(ed.draw())[1])
    out[7] = 84.0
    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *out]) is True
    _wid, _notes, after = window(ed.draw())
    assert after[0] == before[0] and after[1] > before[1], (
        f"it grew to hold the new pitch and did not slide: {before} -> {after}")


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
        #: The whole trees `define` was handed -- one per redefine.
        self.defines = []
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
        # **Built as the real host builds them**, so a correction that cannot
        # cross the wire fails here rather than in a running window. It could
        # not before, and that is how an undo of a clip over samples -- whose
        # window carries a `loop` -- reached a person as a traceback: OSC has no
        # bool tag, and nothing between the prop and the socket said so.
        for wid, props in sets:
            _osclib.message("/gui_set", int(wid), *_prop_args(props))
        self.acks.append((seq, list(sets)))
        self.answers.append((seq, doc_version, reason))

    def define(self, id, tree):
        #: How many whole trees this host was handed -- what says a redefine
        #: happened, since a redefine is the only channel a widget that was not
        #: there can arrive by.
        self.defines.append((id, tree))

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
    # The curve's own length -- an `Env`'s segment times are seconds, so four
    # of them are four seconds of picture and not four beats of one.
    assert curve["dur"] == pytest.approx(4 * SEC)
    # The break-points ride as the bpf flat quads, their times in timeline units.
    assert curve["points"][0:2] == pytest.approx([0.0, 200.0])
    assert curve["points"][4:6] == pytest.approx([2 * SEC, 4000.0])
    assert curve["points"][8:10] == pytest.approx([4 * SEC, 800.0])
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
              3 * SEC, 3000.0, 2, 0.0,
              4 * SEC, 800.0, 1, 0.0]
    assert ed.apply("/gui_event", [curve["id"], SEQ, UNSTATED, "points", *edited])

    # The automation's Env — its source of truth, what the next rendering plays.
    assert auto.to_points()[4:6] == pytest.approx([3.0, 3000.0])  # in seconds again
    assert auto.duration() == pytest.approx(4.0)
    # And the redraw shows what was dropped.
    assert clips(lanes(ed.draw())[0])[0]["points"][4:6] == pytest.approx(
        [3 * SEC, 3000.0])


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
                            0.0, 200.0, 1, 0.0, 4 * SEC, 900.0, 1, 0.0])
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
    voice = Clang(SeqEvent(midinote=60, dur=8.0))
    attached = Aggregate([(0.0, voice), (0.0, Element(env, duration=4.0))], name="sweep")
    # Eight beats and four seconds are one stretch at 120 bpm -- which the
    # relation can only say once it is told the tempo, since the two members are
    # measured in different units.
    assert attached.temporal_relation(TEMPO) == "simultaneous"

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
    Until then the FormEditor skips it rather than draw the old bus-as-node view; a
    directed patch is built directly with `clausters.defs.GraphPatch` (see
    `examples/editors/patch1.py`)."""
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
        ed = FormEditor(song, sample_rate=SR, tempo=TEMPO, quant=1.0)
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
    ed = FormEditor(fx_chain(), sample_rate=SR, tempo=TEMPO)
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
    tree = FormEditor(root, sample_rate=SR, tempo=TEMPO).draw()
    kids = tree["children"]
    assert any(is_lane(c) for c in kids) and any(is_plane(c) for c in kids)
    # The ruler is its own strip under the stack; a patch lane has no time axis,
    # so a window that drew *only* one gets no ruler at all.
    assert is_ruler(kids[-1])
    only_patch = FormEditor(fx_chain(), sample_rate=SR).draw()["children"]
    assert len(only_patch) == 1 and is_plane(only_patch[0])
    assert is_patch(only_patch[0]["children"][0])


def test_a_wire_edit_rewrites_the_members_controls_onto_a_shared_bus():
    # Two unconnected members (no controls): wire src.out -> sink.in.
    src = SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))))
    sink = SynthDef("gsink", out(0, in_(control("in")) * control("amp", 0.3)))
    g = Aggregate(kind=LOGICAL, name="chain")
    hs = g.add(Generator(src))
    hk = g.add(Generator(sink))
    ed = FormEditor(g, sample_rate=SR, tempo=TEMPO)
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
    ed = FormEditor(g, sample_rate=SR, tempo=TEMPO)
    tree = ed.draw()
    wid = [c for c in tree["children"] if is_plane(c)][0]["children"][0]["id"]
    ed.apply("/gui_event", [wid, SEQ, UNSTATED, "wire", 0, "out", 1, "in"])
    # The sink joins the source's existing bus, not a fresh one.
    assert hk.element.controls["in"] == "mix"
    assert g.bus_names == ["mix"]


def test_a_graph_box_move_persists_its_position_across_a_redraw():
    ed = FormEditor(fx_chain(), sample_rate=SR, tempo=TEMPO)
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

def two_clips() -> FormEditor:
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


def test_a_drag_reporting_as_it_goes_is_not_stale_against_its_own_answers():
    """A drag emits a value per frame, each stamped with the version the *host*
    holds -- and a host only learns a new one when an acknowledgement reaches
    it. Refusing those is refusing the drag: every step after the first comes
    back as a resync and the picture snaps to the first frame, over and over,
    which is a curve trembling under the hand editing it."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lead, _bass) = lanes(ed.draw())
    (clip,) = clips(lead)
    drawn_at = ed._version
    placement = ed._clips[clip["id"]]

    # Five frames of one drag, every one of them naming the version the host
    # had when the gesture began.
    for beat in (1.0, 2.0, 3.0, 4.0, 5.0):
        assert ed.apply("/gui_event", [clip["id"], SEQ, drawn_at, "clip",
                                       beat * BEAT, float(BEAT)]) is True
    # `_Placed` keeps the drawn placement in timeline units, not beats.
    assert placement.offset == pytest.approx(5.0 * BEAT), "the last frame is where it is"
    assert ed._version != drawn_at

    # A change by no gesture at all raises the floor, so a step arriving after
    # it is refused -- which is what the version is for. The offset has to be a
    # *new* one: a step asking for where the clip already sits changes nothing
    # and would answer False whatever the rule said.
    ed.refresh()
    assert ed.apply("/gui_event", [clip["id"], SEQ + 2, drawn_at, "clip",
                                   7.0 * BEAT, float(BEAT)]) is False
    assert placement.offset == pytest.approx(5.0 * BEAT), "and it did not move"


def test_two_gestures_inside_one_round_trip_are_both_applied():
    """The acknowledgement is not lost, and nothing is saturated: the host
    stamps every event with the version it was last *told*, and it is told only
    when an answer arrives. Two gestures begun inside one round trip therefore
    name the same version, and refusing the second one because the first had
    already moved the composition is refusing a hand for being faster than a
    poll loop. Only a route the host never saw overtakes an edit."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lead, bass) = lanes(ed.draw())
    (first,) = clips(lead)
    (second,) = clips(bass)
    drawn_at = ed._version

    assert ed.apply("/gui_event", [first["id"], SEQ, drawn_at, "clip",
                                   2.0 * BEAT, float(BEAT)]) is True
    # The answer to that one has not reached the host, so this names the same
    # version -- which is what "before the acknowledgement" looks like on the
    # wire, whichever carrier is under it.
    assert ed.apply("/gui_event", [second["id"], SEQ + 1, drawn_at, "clip",
                                   3.0 * BEAT, float(BEAT)]) is True
    assert ed._clips[first["id"]].offset == pytest.approx(2.0 * BEAT)
    assert ed._clips[second["id"]].offset == pytest.approx(3.0 * BEAT)
    (_, corrections) = host.acks[-1]
    assert corrections == [], "and neither was answered with a snap back"


def test_an_edit_made_against_a_superseded_version_is_refused_and_answered():
    """O4's acceptance. The composition moved between the picture the gesture
    was made against and the gesture arriving -- by a route the host never saw,
    which is what a script or a second editor is from in here. The late one
    comes back as the state that holds rather than landing on top of what
    arrived in between."""
    ed = two_clips()
    host = _FakeHost()
    ed.open(host)
    (lead, bass) = lanes(ed.draw())
    (late,) = clips(lead)
    (other,) = clips(bass)
    drawn_at = ed._version
    placement = ed._clips[late["id"]]
    was = (placement.offset, placement.dur)

    # The composition moves by a route the host never saw -- a script editing
    # the arrangement behind the editor's back and saying so, which is also what
    # a second editor and a redefine look like from in here. Another *gesture*
    # is not that: its versions are ones this host is about to be told about.
    ed.apply("/gui_event", [other["id"], SEQ, drawn_at, "clip", 3.0 * BEAT, float(BEAT)])
    ed.refresh()
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
    ed.refresh()

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

    `FormEditor.poll` is the loop that takes the message off the socket, so a
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
    # 96 000 frames is two seconds at 48 kHz, and a take's duration is in
    # seconds -- so this one is as long as its samples, whatever the tempo.
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=2.0)
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
    # **The roll's own key.** `editable` is the clip's and reaches every body it
    # carries, which locked the envelope of a layered clip along with the notes.
    assert clip.get("notes_editable") == 0, "and says they are read-only"
    assert "editable" not in clip

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
    """A one-lane composition holding one four-**second** take, and its element
    (a take's length is in seconds, whatever the tempo)."""
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


def test_an_undone_first_resize_gives_the_element_its_own_length_back():
    """The inverse of the first resize of a clip carries **no** duration at all,
    because before it the placement stated none -- and absence is a value: the
    member takes the element's own length again. Read as "leave the length
    alone", the log stepped back, `undo` answered True and the clip kept the
    size the hand had given it, which is a dead button on every clip nobody had
    resized yet."""
    ed = editor(quant=0.25)
    host = _FakeHost()
    ed.open(host)
    (_, lead) = lanes(ed.draw())
    (roll,) = clips(lead)
    placed = ed._clips[roll["id"]]
    member = placed.member
    assert member.dur is None, "nothing has stated a length for it"
    was = placed.dur

    assert ed.apply(*clip_event(roll["id"], member.offset * BEAT, 1.0 * BEAT)) is True
    assert member.dur == pytest.approx(1.0)
    host.acks.clear()

    assert ed.undo() is True
    assert member.dur is None, "the placement states no length again"
    assert placed.dur == pytest.approx(was), "and the drawn record says the same"
    pushed = {wid: props for _seq, sets in host.acks for wid, props in sets}
    assert pushed[roll["id"]]["dur"] == pytest.approx(was), "and the host was told"


def test_an_undone_trim_puts_the_window_back_on_a_take_that_configures_nothing():
    """The same rule one level down. A trim states the placement *and* the
    window over the samples in one `setmembers`, so its inverse states the
    member as it was -- and a take nobody has configured has no configuration in
    it at all. Skipped as "nothing to write", the clip went back to its old size
    still reading the frames the trim had left it on: the right rectangle over
    the wrong sound."""
    # A **bare** take: no window and no configuration at all, so the member the
    # inverse states carries no `config` key for the projection to find.
    # 96 000 frames is two seconds at 48 kHz, and a take's duration is in
    # seconds -- so this one is as long as its samples, whatever the tempo.
    take = Vector(ServerBuffer(bufnum=7, frames=int(4 * BEAT), channels=1,
                               sample_rate=SR), duration=2.0)
    song = Aggregate([(0.0, Aggregate([(0.0, take)], name="audio"))], name="song")
    ed = editor(song)
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    placed = ed._clips[clip["id"]]
    was = (placed.offset, placed.dur)

    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "clip",
                                   BEAT, 3 * BEAT, BEAT]) is True
    assert take.start == pytest.approx(BEAT)
    host.acks.clear()

    assert ed.undo() is True
    assert take.start == pytest.approx(0.0), "the frames the trim hid are back"
    assert (placed.offset, placed.dur) == pytest.approx(was), "and so is the clip"
    pushed = {wid: props for _seq, sets in host.acks for wid, props in sets}
    assert clip["id"] in pushed, "the clip a trim moved is not the lane it names"
    assert pushed[clip["id"]]["start"] == pytest.approx(0.0), "window and all"


def test_a_redefine_leaves_the_editor_able_to_edit():
    """A redefine moves the version so a gesture in flight comes back stale --
    and the **document** has to move with it. The crate refuses an edit whose
    `against` version is not the document's, ahead of it as loudly as behind
    (the two would not be talking about the same piece), so a version bumped on
    this side alone answered every later gesture with a refusal nobody asked
    for: the clip did not move, and there was not even a reason to show."""
    ed = editor(quant=0.25)
    host = _FakeHost()
    ed.open(host)
    (_, lead) = lanes(ed.draw())
    (roll,) = clips(lead)
    member = ed._clips[roll["id"]].member
    # One edit first, so the document exists: the versions can only diverge
    # once there is a document holding one.
    assert ed.apply(*clip_event(roll["id"], 3 * BEAT, 2 * BEAT)) is True

    ed.update()
    assert host.defines, "the window was redefined"

    (_, lead) = lanes(ed.draw())
    (roll,) = clips(lead)
    assert ed.apply(*clip_event(roll["id"], 5 * BEAT, 2 * BEAT)) is True
    assert member.offset == pytest.approx(5.0), "and the edit landed"


def test_a_structural_edit_redefines_the_window_and_so_does_its_undo():
    """A placement is a prop the host can be told about; a widget that was not
    there is not. The second half of a split -- and the clip an undone split
    takes away again -- can only arrive as a whole tree, so the editor that drew
    the window redefines it. Without this the document and the objects the
    script holds had two clips while the picture had one, until something
    happened to redraw."""
    song, _take = _take_song()
    ed = editor(song)
    host = _FakeHost()
    ed.open(host)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    host.defines.clear()

    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * BEAT]) is True
    assert len(host.defines) == 1, "the split redefined the window"
    assert len(clips(lanes(host.defines[-1][1])[0])) == 2, "with both halves in it"

    host.defines.clear()
    assert ed.undo() is True
    assert len(host.defines) == 1, "and so did the undo of it"
    assert len(clips(lanes(host.defines[-1][1])[0])) == 1, "back to the one clip"

    host.defines.clear()
    assert ed.redo() is True
    assert len(clips(lanes(host.defines[-1][1])[0])) == 2, "and the redo brings it back"


def test_a_placement_edit_does_not_redefine_the_window():
    """The other half of the rule, and the reason it is not "redraw after every
    edit": a redefine rebuilds every widget and drops what the host had in
    flight, which is exactly wrong for a drag."""
    ed = editor(quant=0.25)
    host = _FakeHost()
    ed.open(host)
    (_, lead) = lanes(ed.draw())
    (roll,) = clips(lead)
    host.defines.clear()
    assert ed.apply(*clip_event(roll["id"], 5 * BEAT, 2 * BEAT)) is True
    assert ed.undo() is True
    assert host.defines == [], "a placement travels as a prop, there and back"


def test_a_split_gives_two_windows_over_one_buffer():
    """The cut: the first half keeps the head it had and stops early, the second
    begins where it left off -- one buffer, two windows -- and it is one edit,
    so an undo puts the clip back whole."""
    song, take = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * SEC]) is True

    (lane,) = lanes(ed.draw())
    first, second = clips(lane)
    # A second of samples, cut a second in: the lengths are seconds and the
    # onset the cut gives the tail is beats, which at 120 bpm is two of them.
    assert first["dur"] == pytest.approx(SEC)
    assert second["offset"] == pytest.approx(2 * BEAT)
    assert second["dur"] == pytest.approx(3 * SEC)
    # The second reads on from where the first stops, over the same buffer.
    assert second["start"] == pytest.approx(SEC)
    assert second["buffer"] == first["buffer"] == 7

    assert ed.undo() is True
    (lane,) = lanes(ed.draw())
    (whole,) = clips(lane)
    assert whole["dur"] == pytest.approx(4 * SEC)


def test_a_join_puts_a_split_clip_back_together():
    """The inverse of the cut, and the case the arrangement can express: windows
    onto one buffer that continue each other become the one window they were cut
    from."""
    song, _ = _take_song()
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * SEC])
    (lane,) = lanes(ed.draw())
    first, second = clips(lane)

    assert ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                                   first["id"], second["id"]]) is True
    (lane,) = lanes(ed.draw())
    (joined,) = clips(lane)
    assert joined["dur"] == pytest.approx(4 * SEC)
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
    assert joined["dur"] == pytest.approx(2 * SEC)
    # One clip, one take per segment, each on its own half and reading its own
    # buffer from its own frame.
    takes = joined["children"]
    assert [t["buffer"] for t in takes] == [7, 8]
    assert [t["at"] for t in takes] == pytest.approx([0.0, SEC])
    assert [t["dur"] for t in takes] == pytest.approx([SEC, SEC])
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
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * SEC])
    (lane,) = lanes(ed.draw())
    first, second = clips(lane)
    ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                            first["id"], second["id"]])
    (lane,) = lanes(ed.draw())
    (joined,) = clips(lane)
    element = ed._clips[joined["id"]].member.element
    assert isinstance(element, Vector) and not isinstance(element, Segments)
    assert joined["dur"] == pytest.approx(4 * SEC)


def test_which_layer_a_hand_is_on_is_screen_state():
    """Selecting a layer changes nothing in the composition -- it is what a view
    is currently editing, which the document is explicit is never part of it --
    and it is kept so a driver can ask."""
    song, _ = _take_song()
    ed = editor(song)
    ed.open(_FakeHost())
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "layer", "points"]) is False
    assert ed.dirty is False

    # And it is asked for by the **placement**, not by the widget id it arrived
    # under: a widget id is the drawing's name for something and is minted
    # afresh every redefine, so state keyed by one is emptied by every
    # structural edit -- silently, since a missing key and the default layer are
    # the same answer.
    placed = ed._clips[clip["id"]]
    assert ed.edit_layer(placed.member.element, placed.member) == "points"
    ed.update()
    assert ed.edit_layer(placed.member.element, placed.member) == "points", \
        "screen state outlives the picture it was set on"


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
    assert ed.apply("/gui_event", [joined["id"], SEQ, UNSTATED, "split", SEC]) is True
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
    audio.add(seg, 0.0, 1.5)                     # a placement of 1.5 seconds
    ed = editor(Aggregate([(0.0, audio)], name="song"))
    (lane,) = lanes(ed.draw())
    (clip,) = clips(lane)
    takes = clip["children"]
    assert [t["buffer"] for t in takes] == [7, 8]
    assert [t["dur"] for t in takes] == pytest.approx([SEC, 0.5 * SEC])

    # The whole buffer is still there: the placement is what was shortened.
    assert [s.duration for s in seg.segments] == pytest.approx([1.0, 1.0])


def test_two_editors_over_one_composition_keep_one_history():
    """O19's acceptance, and the inverse of the defect it was opened for.

    Two windows over one composition used to mint a history each, so stepping
    one of them reverted across the other's edits and wrote a state nobody was
    ever in. The history belongs to the arrangement now, so both windows reach
    the same one."""
    piece = song()
    a, b = editor(piece), editor(piece)
    a.open(_FakeHost())
    b.open(_FakeHost())
    assert a._editing is b._editing, "one context, reached through the element"
    assert a._log is b._log

    clip = next(iter(a._clips))
    placed = a._clips[clip]
    at = placed.base + placed.member.offset
    a.apply("/gui_event", [clip, SEQ, UNSTATED, "clip",
                           (at + 2.0) * BEAT, (placed.member.length or 1.0) * BEAT])
    moved = placed.member.offset
    assert moved != placed.base

    # The second window can undo what the first did -- which is the half that
    # used to be false: `b.can_undo` was False, and b could not see the edit at
    # all even though it was showing the data it changed.
    assert b.can_undo
    assert b.undo() is True
    assert placed.member.offset != moved, "and the undo reached the arrangement"
    assert not a.can_undo, "one order, not two"


def test_a_second_window_sees_the_first_windows_edit_in_its_label():
    """The label is how a person knows what a keystroke is about to move, and
    with one pile over one composition both windows read the same one."""
    piece = song()
    a, b = editor(piece), editor(piece)
    a.open(_FakeHost())
    b.open(_FakeHost())
    clip = next(iter(a._clips))
    placed = a._clips[clip]
    at = placed.base + placed.member.offset
    a.apply("/gui_event", [clip, SEQ, UNSTATED, "clip",
                           (at + 1.0) * BEAT, (placed.member.length or 1.0) * BEAT])
    assert a.undo_label == b.undo_label
    assert b.undo_label


def test_a_view_over_a_part_of_the_composition_reaches_the_same_history():
    """A dedicated roll of one track is a view over data the multitrack is
    already showing, so it edits through the composition's history rather than
    minting a second one over the same dataset."""
    from clausters.gui.editing import Editing

    piece = song()
    whole = editor(piece)
    whole.open(_FakeHost())
    whole._history()  # the derivation is what stamps the parts

    lane = piece.handles[0].element
    assert Editing.of(lane) is whole._editing
    assert editor(lane)._editing is whole._editing


def test_an_edit_in_one_window_reaches_the_other_as_props():
    """One history is only half of it: the second window has to be *told*.

    An acknowledgement goes to the window whose gesture it answered, so without
    this a second view goes on drawing a piece that moved under it — and then
    its undo steps an order it cannot see, which looks exactly like a dead
    button.

    It arrives as **props**, not as a redefine: a placement is a value, and
    rebuilding every widget under a hand that is not even in that window is what
    made the second one flicker on every step."""
    piece = song()
    a, b = editor(piece), editor(piece)
    a.open(_FakeHost())
    host_b = _FakeHost()
    b.open(host_b)
    host_b.defines.clear()
    host_b.acks.clear()

    clip = next(iter(a._clips))
    placed = a._clips[clip]
    at = placed.base + placed.member.offset
    a.apply("/gui_event", [clip, SEQ, UNSTATED, "clip",
                           (at + 2.0) * BEAT, (placed.member.length or 1.0) * BEAT])
    assert host_b.defines == [], "no redefine for a placement"
    assert host_b.acks, "but the other window was told what to draw"
    (_seq, corrections), = host_b.acks
    assert corrections, "and the correction carries the placement that moved"

    # And an undo from either reaches both the same way, once: a turn that goes
    # through `apply` into `undo` is still one gesture.
    host_b.acks.clear()
    assert a.undo() is True
    assert host_b.defines == []
    assert len(host_b.acks) == 1, "one gesture, one answer"


def test_a_structural_edit_in_one_window_redefines_the_other():
    """The case no prop can carry, from the other side: a widget that was not
    there a moment ago is not a value, so the second window is redrawn whole."""
    piece, _take = _take_song()
    a, b = editor(piece), editor(piece)
    a.open(_FakeHost())
    host_b = _FakeHost()
    b.open(host_b)
    (lane,) = lanes(a.draw())
    (clip,) = clips(lane)
    host_b.defines.clear()

    assert a.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", 1.0 * BEAT]) is True
    assert len(host_b.defines) == 1, "the split redrew the other window too"
    assert len(clips(lanes(host_b.defines[-1][1])[0])) == 2, "with both halves in it"


def test_a_closed_window_is_not_told_about_edits():
    """Closing a view is not an event of the history — what goes is this
    window's place in the list of who to tell."""
    piece = song()
    a, b = editor(piece), editor(piece)
    a.open(_FakeHost())
    host_b = _FakeHost()
    win = b.open(host_b)
    b.apply("/gui_closed", [win])
    defines = len(host_b.defines)

    clip = next(iter(a._clips))
    placed = a._clips[clip]
    at = placed.base + placed.member.offset
    a.apply("/gui_event", [clip, SEQ, UNSTATED, "clip",
                           (at + 2.0) * BEAT, (placed.member.length or 1.0) * BEAT])
    assert len(host_b.defines) == defines, "a closed window has no picture to keep"
    assert b.can_undo, "and it still shares the history"


def test_a_step_with_nothing_to_step_tells_nobody():
    """**A no-op is not a change**, and answering that it is one is how a window
    nobody touched came to redraw.

    A history at its end is the ordinary case — a person holds Ctrl+Z until it
    stops — and `apply` used to answer `True` for every one of those, so every
    other view of the composition was told to bring itself in step with an edit
    that never happened. One host and two windows, which is the arrangement the
    defect showed up in."""
    piece = song()
    host = _FakeHost()
    a, b = editor(piece), editor(piece)
    win_a = a.open(host)
    b.open(host)
    host.defines.clear()

    # The message the host sends for Ctrl+Z, fed to both editors the way one
    # poll loop feeds every editor it opened.
    for tag in ("undo", "redo"):
        assert a.apply("/gui_event", [win_a, SEQ, UNSTATED, tag]) is False
        assert b.apply("/gui_event", [win_a, SEQ, UNSTATED, tag]) is False
    assert host.defines == [], "no window was redrawn for a step that did not happen"


def test_a_resend_is_not_an_edit():
    """The crate's own rule, honoured by the editor: an edit that states what
    already holds is refused and does not move the version — so the editor must
    not answer that the composition changed, or every other view redraws for a
    gesture that did nothing.

    The notes route is the one that got this wrong twice over: it answered
    `True` unconditionally, *and* it minted a fresh node id for every note on
    every edit, so the same notes resent really did arrive as different members
    and the document changed for nothing."""
    piece = song()
    ed = editor(piece)
    ed.open(_FakeHost())

    clip = next(iter(ed._clips))
    placed = ed._clips[clip]
    at = placed.base + placed.member.offset
    move = [clip, SEQ, UNSTATED, "clip",
            (at + 2.0) * BEAT, (placed.member.length or 1.0) * BEAT]
    assert ed.apply("/gui_event", move) is True
    assert ed.apply("/gui_event", [*move[:1], SEQ + 1, *move[2:]]) is False

    roll = next(iter(ed._rolls))
    notes = [roll, SEQ, UNSTATED, "notes", 0, BEAT, 62, 100, 0]
    version = ed._version
    assert ed.apply("/gui_event", notes) is True
    assert ed.apply("/gui_event", [*notes[:1], SEQ + 1, *notes[2:]]) is False
    assert ed._version == version + 1, "one edit, one version"


def test_a_note_keeps_the_id_the_document_gave_it():
    """The reason the resend above used to be an edit: the payload a roll sends
    carries no ids, so the i-th note inherits the i-th note's — read off the
    events themselves. A note that came back unstamped was a new node to the
    next edit."""
    piece = song()
    ed = editor(piece)
    ed.open(_FakeHost())
    roll = next(iter(ed._rolls))
    ed.apply("/gui_event", [roll, SEQ, UNSTATED, "notes", 0, BEAT, 62, 100, 0])

    element = ed._rolls[roll]
    from clausters.form.document import ID_ATTR
    from clausters.gui.editing.formeditor import _editable_timeline, _pitch

    ids = [getattr(item, ID_ATTR, None)
           for _, item in _editable_timeline(element) if _pitch(item) is not None]
    assert ids and all(i is not None for i in ids), "every note carries its node"


def test_a_cord_is_an_edit_and_undoes_in_one_step():
    """A cord was the last gesture that wrote the arrangement directly, on the
    grounds that no intent described it — and the vocabulary had said what to do
    all along. An edit states the resulting value, and what a cord results in is
    the two members' controls and the aggregate's declared buses: three
    `configure`s, one entry, one undo."""
    src = SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))))
    sink = SynthDef("gsink", out(0, in_(control("in")) * control("amp", 0.3)))
    g = Aggregate(kind=LOGICAL, name="chain")
    hs = g.add(Generator(src))
    hk = g.add(Generator(sink))
    ed = editor(g)
    ed.open(_FakeHost())
    tree = ed.draw()
    wid = [c for c in tree["children"] if is_plane(c)][0]["children"][0]["id"]

    assert ed.apply("/gui_event", [wid, SEQ, UNSTATED, "wire", 0, "out", 1, "in"]) is True
    bus = hs.element.controls["out"]
    assert hk.element.controls["in"] == bus
    assert g.bus_names == [bus], "and the aggregate declares the bus it names"

    assert ed.can_undo, "a cord is an edit like any other"
    assert ed.undo_label == "draw a cord"
    assert ed.undo() is True
    assert not hs.element.controls, "both ends come back"
    assert not hk.element.controls
    assert g.bus_names == [], "and the bus goes with them"
    assert not ed.can_undo, "one gesture, one step"

    assert ed.redo() is True
    assert hk.element.controls["in"] == bus
    assert g.bus_names == [bus]


def test_a_logical_aggregates_buses_survive_the_document():
    """The other half, and the reason the cord could not be logged: a patch's
    buses were in no format at all. The cords survived a round trip — a member's
    controls are in its own config — while the buses they name did not, so a
    reopened patcher drew connections it could render none of."""
    from clausters.form.document import from_document, to_document

    g = Aggregate(kind=LOGICAL, name="chain", buses=[("mix", "audio")])
    g.add(Generator(SynthDef("gsrc", out(control("out"), sine(220.0))),
                    controls={"out": "mix"}))
    back = from_document(to_document(g))
    assert back.bus_names == ["mix"]
    assert back.bus_specs == [{"name": "mix", "rate": "audio", "channels": 1}]


def test_a_copied_block_of_notes_pastes_onto_a_roll_and_undoes_in_one_step():
    """**Cut, copy and paste are one mechanism**, so a block a roll put on the
    clipboard lands through the same call a drag on a note goes through: one
    `setmembers`, one entry on the pile, one undo.

    It also settles the offset the two sides measure differently: the position
    the host swept is on the *timeline's* axis and a roll's notes are in the
    clip's own, so a clip placed at beat 2 holds its note 0 at beat 2.
    """
    timeline = Timeline([(0.0, SeqEvent(midinote=60, dur=1.0))])
    piece = Aggregate([(0.0, Aggregate([(2.0, Track(timeline))], name="lead"))],
                      name="song")
    ed = editor(piece)
    host = _FakeHost()
    ed.open(host)
    wid = clips(lanes(ed.draw())[0])[0]["id"]

    # What a roll's Ctrl+C puts on the clipboard: the typed document's text
    # kind, holding the flat quintuple array a `/gui_set notes` takes. Pasted
    # at beat 6 of the timeline, which is beat 4 of a clip placed at beat 2.
    block = '{"content":{"kind":"text","text":"[0, %g, 72, 100, 0]"}}' % BEAT
    changed = ed.apply("/gui_event", [wid, SEQ, UNSTATED, "paste",
                                      6.0 * BEAT, "text", block])

    assert changed, "a paste adds notes, so the composition moved"
    placed = [(beat, item["midinote"]) for beat, item in timeline]
    assert placed == [(0.0, 60), (4.0, 72)]

    ed.undo()
    assert [(beat, item["midinote"]) for beat, item in timeline] == [(0.0, 60)], \
        "one undo takes the whole block back"


def test_a_pasted_block_is_refused_where_no_roll_can_hold_it():
    """The reason names what is missing rather than talking about samples: a
    block of notes is written onto a roll, and a lane holds clips."""
    ed = editor()
    host = _FakeHost()
    ed.open(host)

    lane = next(iter(ed._lanes))
    block = '{"content":{"kind":"text","text":"[0, 480, 72, 100, 0]"}}'
    assert not ed.apply("/gui_event", [lane, SEQ, UNSTATED, "paste", 0.0, "text", block])
    _, _, reason = host.answers[-1]
    assert reason and "roll" in reason


# ---- the lane header: what is the composition's, and what is the view's ----

def test_a_lane_header_draws_the_compositions_mixing_and_writes_it_back():
    lane_element = Aggregate([(0.0, Clang(SeqEvent(midinote=60, dur=1.0)))],
                             name="drums")
    song = Aggregate([(0.0, lane_element)], name="song")
    ed = editor(song)
    (lane,) = lanes(ed.draw())
    assert (lane["mute"], lane["solo"], lane["level"]) == (False, False, 1.0), \
        "a control nothing draws is a control nobody can press"

    assert ed.apply("/gui_event", [lane["id"], SEQ, UNSTATED, "mute", 1]) is True
    assert lane_element.mute is True
    # It is an edit like any other, so it is in the history and it inverts.
    assert ed.can_undo and ed.undo_label == "mute the lane"
    ed.undo()
    assert lane_element.mute is False

    assert ed.apply("/gui_event", [lane["id"], SEQ, UNSTATED, "level", 0.25]) is True
    assert lane_element.level == pytest.approx(0.25)
    assert lanes(ed.draw())[0]["level"] == pytest.approx(0.25), \
        "redrawn from the composition, not remembered by the view"


def test_a_lane_resized_by_hand_changes_no_composition():
    # `height` is the view's: the host already resized the lane it was made on,
    # and no document carries it.
    ed = editor(Aggregate([(0.0, Clang(SeqEvent(midinote=60, dur=1.0)))], name="song"))
    (lane,) = lanes(ed.draw())
    assert ed.apply("/gui_event", [lane["id"], SEQ, UNSTATED, "height", 90.0]) is False
    assert ed.can_undo is False


def test_a_muted_lane_still_draws_everything_it_had():
    # A picture that emptied when the toggle was pressed would report silence as
    # absence.
    lane_element = Aggregate([(0.0, Clang(SeqEvent(midinote=60, dur=1.0)))],
                             name="drums")
    ed = editor(Aggregate([(0.0, lane_element)], name="song"))
    before = clips(lanes(ed.draw())[0])
    lane_element.mute = True
    after = clips(lanes(ed.draw())[0])
    assert len(after) == len(before) == 1
    assert after[0]["dur"] == before[0]["dur"]


# ---- the views the multitrack composes ----

class _FakeTake:
    """A server buffer, as the samples domain touches one: a number, a shape,
    and the two calls that read and write its frames."""

    def __init__(self, frames=16, channels=1):
        self.bufnum = 7
        self.frames = frames
        self.channels = channels
        self.sample_rate = SR
        self.name = "take"
        self.data = [0.0] * (frames * channels)

    def get_samples(self, start=0, count=-1, **kwargs):
        end = len(self.data) if count < 0 else start + count
        return list(self.data[start:end])

    def set_samples(self, samples, start=0, **kwargs):
        for i, value in enumerate(samples):
            self.data[start + i] = float(value)


def _blob(values) -> bytes:
    import struct

    return struct.pack(f"<{len(values)}f", *values)


def test_a_stroke_over_a_take_and_a_bend_of_its_curve_undo_in_one_order():
    """The whole claim of the composed editor, in one gesture each.

    A stroke on a take's samples and a bend of the curve over it are written in
    two different vocabularies — ``samples``, whose state the crate cannot hold,
    and the tree's, which is a document — and neither editor knows what the
    other did. What they share is the composition's editing context, so the two
    edits are **one order**, and an undo asked for in either window walks it:
    the stroke first, then the bend, because that is how they were made.
    """
    from clausters.seq import Automation

    take = Vector(_FakeTake(frames=8), duration=1.0, instrument="take")
    auto = Automation.from_points([(0, 200.0, 1, 0.0), (2, 900.0, 1, 0.0)],
                                  target=None, name="cutoff")
    song = Aggregate([(0.0, take), (0.0, Element(auto))], name="song")
    ed = editor(song)
    host = _FakeHost()
    ed.open(host)
    ed.open_signal(host, take)

    # The bend: the curve's clip in the multitrack, the ordinary points route.
    curve_clip = _find(ed.draw(), lambda n: "points" in n)["id"]
    assert ed.apply("/gui_event", [curve_clip, SEQ, UNSTATED, "points",
                                   0.0, 200.0, 1, 0.0,
                                   2.0 * SEC, 300.0, 1, 0.0]) is True
    assert auto.to_points()[5] == pytest.approx(300.0)

    # The stroke: the composed signal window, the samples domain, and the
    # inverse riding on the wire beside the run it replaced.
    wid = next(iter(ed.composed[-1].view.widgets))
    assert ed.composed[-1].apply("/gui_event", [wid, SEQ, UNSTATED, "draw", 0, 2,
                                           _blob([0.5, 0.5]),
                                           _blob([0.0, 0.0])]) is True
    assert take.wraps.data[2:4] == [0.5, 0.5]

    # One order, walked from the **multitrack** — the window that cannot read a
    # samples leg and must hand it to the editor that can.
    assert ed.undo_label == "draw the samples"
    assert ed.undo() is True
    assert take.wraps.data[2:4] == [0.0, 0.0]
    assert auto.to_points()[5] == pytest.approx(300.0), "the bend still stands"
    assert ed.undo() is True
    assert auto.to_points()[5] == pytest.approx(900.0)
    assert ed.can_undo is False

    # ...and forward again, from the **signal** window this time: the same one
    # order, read from the other end.
    assert ed.composed[-1].redo() is True
    assert auto.to_points()[5] == pytest.approx(300.0)
    assert ed.composed[-1].redo() is True
    assert take.wraps.data[2:4] == [0.5, 0.5]


def test_a_dedicated_roll_and_the_multitrack_step_one_history():
    """A roll of one track beside the multitrack over the same piece.

    They are two editors and one editing context, so the roll's note edit is in
    the same order as a clip dragged in the multitrack — and the multitrack is
    told what moved without either window being redefined.
    """
    track, tl = _track_with_a_take()
    song = Aggregate([(0.0, track)], name="song")
    ed = editor(song)
    # One host, two windows — which is what a script has.
    host = _FakeHost()
    ed.open(host)
    ed.open_pianoroll(host, track)
    roll = ed.composed[-1]

    # The window as it stands: one draw, so the ids read here are the ids the
    # registry holds (a second draw mints fresh ones from the host's pool).
    (clip,) = clips(lanes(ed.draw())[0])
    drawn = clip["notes"]
    wid = next(iter(roll.view.widgets))

    # Pitch 60 -> 62, in the roll.
    edited = list(drawn)
    edited[2] = 62.0
    host.acks.clear()
    assert roll.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", *edited]) is True
    assert [it.get("midinote") for _b, it in tl.range(0.0, float("inf"))
            if hasattr(it, "get")] == [62, 64]

    # The multitrack heard about it as **props**: no redefine, and the clip
    # drawing that track was handed the notes it now shows.
    assert host.defines == []
    pushed = [props for _seq, sets in host.acks for wid_, props in sets
              if wid_ == clip["id"]]
    assert pushed and "notes" in pushed[0]

    # One history: the multitrack's own undo walks the roll's edit.
    assert ed.undo_label == "edit the notes"
    assert ed.undo() is True
    assert [it.get("midinote") for _b, it in tl.range(0.0, float("inf"))
            if hasattr(it, "get")] == [60, 64]


def test_a_composed_view_seeks_and_selects_for_the_piece_it_is_part_of():
    """A structure has no transport and no piece: a click on the ruler of a
    composed window is a seek of the composition, and a marquee swept there is a
    selection **of the element that view draws** — the same value a sweep on its
    clip would give."""
    take = Vector(_FakeTake(frames=8), duration=1.0, instrument="take")
    song = Aggregate([(0.0, take)], name="song")
    ed = editor(song)
    host = _FakeHost()
    ed.open(host)
    ed.open_signal(host, take)
    wid = next(iter(ed.composed[-1].view.widgets))

    ed.composed[-1].apply("/gui_event", [wid, SEQ, UNSTATED, "locate", 1.0 * BEAT])
    assert ed.position == pytest.approx(1.0)

    ed.composed[-1].apply("/gui_event", [wid, SEQ, UNSTATED, "selection",
                                    0.5 * BEAT, 1.0 * BEAT])
    assert ed.selection["start"] == pytest.approx(0.5)
    assert ed.selection.get("nodes"), "it is a selection of that element"


# ---- a clip is a view: what it holds decides what it admits ----

def test_a_clip_over_a_track_splits_into_two_windows_of_one_timeline():
    """Cutting a timeline of notes is as definable as cutting samples: the
    halves are two windows onto one timeline, the same placement rule in the
    other unit. Nothing is copied -- the notes neither window shows are still on
    the timeline, which is what makes the cut reversible."""
    ed = editor()
    (clip,) = clips(lanes(ed.draw())[1])
    was = clip["dur"]
    # One beat into the clip, which the melody has three notes in.
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", BEAT]) is True

    first, second = clips(lanes(ed.draw())[1])
    assert first["dur"] == pytest.approx(BEAT)
    assert second["offset"] == pytest.approx(3 * BEAT)   # the lane starts at 2
    assert second["dur"] == pytest.approx(was - BEAT)
    # The second reads the same timeline from a beat in -- it is a window, and
    # it draws the notes that window holds, placed from its own zero.
    assert second["start"] == pytest.approx(BEAT)
    assert [n for n in second["notes"][::5]] == [pytest.approx(0.0),
                                                 pytest.approx(BEAT)]

    assert ed.undo() is True
    (whole,) = clips(lanes(ed.draw())[1])
    assert whole["dur"] == pytest.approx(was)


def test_the_halves_of_a_split_track_join_back_into_the_window_they_were_cut_from():
    """The join is the inverse of the split over notes as it is over samples,
    or a cut would be a one-way door."""
    ed = editor()
    (clip,) = clips(lanes(ed.draw())[1])
    was = clip["dur"]
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", BEAT])
    first, second = clips(lanes(ed.draw())[1])
    assert ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                                   first["id"], second["id"]]) is True
    (whole,) = clips(lanes(ed.draw())[1])
    assert whole["dur"] == pytest.approx(was)
    assert "start" not in whole          # the window it was cut from
    assert [n for n in whole["notes"][::5]] == [pytest.approx(0.0),
                                                pytest.approx(BEAT),
                                                pytest.approx(2 * BEAT)]


def test_a_note_edited_inside_a_window_leaves_the_notes_outside_it_alone():
    """The roll draws a window's notes, so an edit is an edit *of those* -- the
    ones outside are not in the payload and must not read as deleted."""
    ed = editor()
    (clip,) = clips(lanes(ed.draw())[1])
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", BEAT])
    first, second = clips(lanes(ed.draw())[1])

    # The window's first note up a semitone, its own second left alone.
    notes = list(second["notes"])
    notes[2] += 1
    assert ed.apply("/gui_event", [second["id"], SEQ, UNSTATED, "notes",
                                   *notes]) is True

    first, second = clips(lanes(ed.draw())[1])
    assert second["notes"][2] == 65
    # The note outside the window is where it was, at the pitch it was.
    assert first["notes"][2] == 60


def test_a_roll_clip_trims_to_a_window_of_its_timeline():
    """The same drag on the same edge, over the other kind of contents: the clip begins
    later and reads its timeline from further in, and the notes it stops showing
    are still on it."""
    ed = editor()
    (clip,) = clips(lanes(ed.draw())[1])
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "clip",
                                   clip["offset"] + BEAT, clip["dur"] - BEAT,
                                   BEAT]) is True
    (trimmed,) = clips(lanes(ed.draw())[1])
    assert trimmed["start"] == pytest.approx(BEAT)
    assert [n for n in trimmed["notes"][::5]] == [pytest.approx(0.0),
                                                  pytest.approx(BEAT)]


def test_a_generator_is_not_refused_a_cut_but_told_to_be_rendered_first():
    """The one element that genuinely answers no, and it does not answer
    'never' -- a generator has no time axis until the change of state gives it
    one."""
    from clausters.seq.pattern import Pbind

    piece = Aggregate([(0.0, Aggregate([(0.0, Generator(Pbind(midinote=[60, 62]),
                                                        duration=2.0))],
                                       name="gen"))], name="song")
    ed = editor(piece)
    (clip,) = clips(lanes(ed.draw())[0])
    assert ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", BEAT]) is False
    assert "render" in (ed._reason or "")


def test_a_placement_that_came_back_from_an_undo_can_still_be_moved():
    """The index maps a node to *the object* an intent writes to, and a restored
    placement is a new handle for an old node. Left unlearned, every later move
    of that clip is applied by the crate, answered `True`, and written onto a
    placement nobody draws -- a clip that does not respond and says nothing."""
    a = Vector(ServerBuffer(bufnum=7, frames=int(8 * BEAT), channels=1, sample_rate=SR),
               duration=1.0, instrument="take")
    b = Vector(ServerBuffer(bufnum=8, frames=int(8 * BEAT), channels=1, sample_rate=SR),
               duration=1.0, instrument="take")
    song = Aggregate([(0.0, Aggregate([(0.0, a), (1.0, b)], name="audio"))],
                     name="song")
    ed = editor(song)
    first, second = clips(lanes(ed.draw())[0])
    ed.apply("/gui_event", [first["id"], SEQ, UNSTATED, "join",
                            first["id"], second["id"]])
    assert ed.undo() is True

    # The clip the undo put back, moved one beat along.
    _, restored = clips(lanes(ed.draw())[0])
    assert ed.apply("/gui_event", [restored["id"], SEQ + 1, UNSTATED, "clip",
                                   restored["offset"] + BEAT,
                                   restored["dur"]]) is True
    _, moved = clips(lanes(ed.draw())[0])
    assert moved["offset"] == pytest.approx(2 * BEAT)


def test_a_split_over_notes_writes_the_notes_once_and_reopens_sharing_them():
    """A window is a window once the piece is written down, too.

    The notes are nodes, so two halves written as two tracks would write every
    note twice with the same ids in each -- one identity, two parents, which is
    what the document refuses and what made a reopened piece hand the halves two
    timelines that drift apart from the first edit. They are `content` now: held
    once, named by both windows.
    """
    from clausters.form import to_document
    from clausters.form.document import from_document

    ed = editor()
    (clip,) = clips(lanes(ed.draw())[1])
    ed.apply("/gui_event", [clip["id"], SEQ, UNSTATED, "split", BEAT])

    document = to_document(ed.element)
    (held,) = document["content"]
    assert len(held["members"]) == 3            # the melody's notes, once
    lane = document["root"]["members"][1]["node"]["members"]
    windows = [m["node"] for m in lane]
    assert [w["kind"] for w in windows] == ["segments", "segments"]
    assert [w["segments"][0]["source"] for w in windows] == [
        {"node": held["id"]}, {"node": held["id"]}]
    assert [w["segments"][0]["start"] for w in windows] == [0.0, 1.0]
    # Two windows, two ids of their own -- and neither is the content's.
    assert len({w["id"] for w in windows} | {held["id"]}) == 3

    # Reopened, the halves read **one** timeline: the sharing is the document's,
    # not a fact about the session that made it.
    back = from_document(document)
    first, second = [h.element for h in back.handles[1].element.handles]
    assert first.wraps is second.wraps
    assert len(first.wraps) == 3
    assert (first.window_start(), second.window_start()) == (0.0, 1.0)


def test_clips_over_different_timelines_join_into_one_run_of_windows():
    """The join across timelines, which is the same verb one unit over: two roll
    clips holding different phrases read as one, and the document says so with
    two windows onto two pieces of content rather than a copy of either."""
    from clausters.form import Segments, to_document
    from clausters.form.document import from_document

    first = Timeline([(0.0, SeqEvent(midinote=60, dur=1.0)),
                      (1.0, SeqEvent(midinote=62, dur=1.0))])
    second = Timeline([(0.0, SeqEvent(midinote=67, dur=1.0))])
    lane = Aggregate([(0.0, Track(first, duration=2.0)),
                      (2.0, Track(second, duration=1.0))], name="notes")
    ed = editor(Aggregate([(0.0, lane)], name="song"))
    a, b = clips(lanes(ed.draw())[0])
    assert ed.apply("/gui_event", [a["id"], SEQ, UNSTATED, "join",
                                   a["id"], b["id"]]) is True

    (joined,) = clips(lanes(ed.draw())[0])
    assert joined["dur"] == pytest.approx(3 * BEAT)
    # One clip, the three notes of both phrases in reading order.
    assert joined["notes"][2::5] == [60.0, 62.0, 67.0]
    element = ed._clips[joined["id"]].member.element
    assert isinstance(element, Segments)

    # The document holds each timeline once and names both from one node.
    document = to_document(ed.element)
    windows = document["root"]["members"][0]["node"]["members"][0]["node"]["segments"]
    assert [w["source"] for w in windows] == [
        {"node": n["id"]} for n in document["content"]]
    assert from_document(document) is not None

    # And the cut takes it apart into the windows it was made of: the second
    # half is the timeline it came from, not a copy and not a list of one.
    assert ed.apply("/gui_event", [joined["id"], SEQ + 1, UNSTATED, "split",
                                   2 * BEAT]) is True
    _, tail = clips(lanes(ed.draw())[0])
    tail_element = ed._clips[tail["id"]].member.element
    assert isinstance(tail_element, Track)
    assert tail_element.wraps is second
