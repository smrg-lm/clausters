"""Windows onto material, and runs of them: the arithmetic a cut and a join are.

The structure is general (`clausters.segments`) and the two kinds differ only
in what the base cannot know: how a position advances by a length, and what one
window holds. So the same checks are made twice, over samples and over notes,
and where they disagree the disagreement is the point.

The twin of `clients/web/tests/segments.test.ts`: same calls, same order.
"""

from clausters.seq.timeline import Timeline, OscItem
from clausters.segments import BufferSegments, NoteSegments, Segment


class FakeBuffer:
    """A buffer as a run reads one: a slot number and the rate its frames are
    measured at."""

    def __init__(self, bufnum, sample_rate=48000.0, frames=480000):
        self.bufnum = bufnum
        self.sample_rate = sample_rate
        self.frames = frames


def test_a_run_is_as_long_as_its_windows_and_says_where_each_one_starts():
    run = BufferSegments([(FakeBuffer(1), 0.0, 2.0), (FakeBuffer(2), 4800.0, 1.5)])
    assert run.total == 3.5
    assert [offset for offset, _ in run.placed()] == [0.0, 2.0]
    assert run.unit == "seconds"


def test_a_cut_inside_a_window_opens_the_second_half_where_the_first_stopped():
    # The bridge the base cannot cross on its own: the halves' lengths are in
    # seconds and the frame they open at is in frames, a sample rate apart.
    buffer = FakeBuffer(1, sample_rate=100.0)
    head, tail = BufferSegments([(buffer, 0.0, 2.0)]).cut(0.5)
    assert [(s.start, s.duration) for s in head] == [(0.0, 0.5)]
    assert [(s.start, s.duration) for s in tail] == [(50.0, 1.5)]


def test_a_cut_falling_between_windows_takes_whole_windows():
    run = BufferSegments([(FakeBuffer(1), 0.0, 2.0), (FakeBuffer(2), 0.0, 1.0)])
    head, tail = run.cut(2.0)
    assert len(head) == 1 and len(tail) == 1
    assert head.segments[0].source.bufnum == 1
    assert tail.segments[0].source.bufnum == 2


def test_a_cut_past_the_end_gives_the_whole_run_and_an_empty_one():
    run = BufferSegments([(FakeBuffer(1), 0.0, 2.0)])
    head, tail = run.cut(9.0)
    assert head.total == 2.0
    assert len(tail) == 0


def test_joining_the_halves_gives_the_run_back():
    run = BufferSegments([(FakeBuffer(1), 0.0, 2.0)], instrument="take")
    head, tail = run.cut(0.75)
    rejoined = head.joined(tail)
    assert rejoined.total == run.total
    # The configuration travels with the run, or a join would silence it.
    assert rejoined.instrument == "take"


def test_a_run_of_notes_measures_in_beats_and_has_nothing_to_bridge():
    timeline = Timeline()
    for beat in (0.0, 1.0, 2.0, 3.0):
        timeline.add(beat, OscItem("/n", beat))
    run = NoteSegments([(timeline, 0.0, 4.0)])
    assert run.unit == "beats"
    head, tail = run.cut(1.5)
    assert [(s.start, s.duration) for s in head] == [(0.0, 1.5)]
    assert [(s.start, s.duration) for s in tail] == [(1.5, 2.5)]


def test_a_note_window_hides_what_it_leaves_out_and_places_the_rest_at_zero():
    timeline = Timeline()
    for beat in (0.0, 1.0, 2.0, 3.0):
        timeline.add(beat, OscItem("/n", beat))
    _, tail = NoteSegments([(timeline, 0.0, 4.0)]).cut(1.5)
    # The window opens at beat 1.5, so the notes it holds are the last two and
    # they are placed from the run's own start -- and the ones it left out are
    # in the timeline, not gone.
    assert [beat for beat, _ in tail.items()] == [0.5, 1.5]
    assert len(timeline) == 4


def test_a_segment_reads_a_triple_a_pair_or_itself():
    buffer = FakeBuffer(1)
    assert Segment.of((buffer, 3.0)).start == 0.0
    assert Segment.of((buffer, 2.0, 3.0)).start == 2.0
    one = Segment(buffer, 1.0, 1.0)
    assert Segment.of(one) is one
