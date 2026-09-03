"""Segments: a window onto material, and a run of windows read as one.

A **segment** is a window: which source, from where, for how long. A **run** of
them is what a **join** assembles and what a **split** takes apart, read back to
back as a single thing. Neither idea belongs to the arrangement -- a window is
about the *material*, not about where the material sits in a piece -- so they
live here, beside the structures, and `clausters.form` reads them like any other
reader.

**What is general and what is the source's.** The order of the windows, where
each one starts inside the run, how long the run is, where a cut falls and what
two runs make when joined: all of that is arithmetic over lengths, and it is
written once, in `SegmentRun`. What only the source knows is the little that is
left, and it is exactly the two hooks a subclass fills in:

- **How a position advances by a length** (`SegmentRun.advanced`), because a
  window's `start` is in the unit the source is *addressed* in -- frames for
  samples, beats for events -- while a length is in the unit the source
  *measures*. For notes the two are one; for samples they are a sample rate
  apart. This is the whole of what a cut needs to move the second half's start.
- **What one window is played as** (`SegmentRun.to_events`), which is a
  buffer-reading event for samples and the items inside the window for a
  timeline.

**Nothing is copied.** A run refers to its sources, so cutting one and joining
the halves back gives the run it started from, and re-lengthening a window
brings back the material the cut hid rather than nothing -- the same placement
rule a trimmed take already follows, one level up. That property is why notes
want windows too, and not a destructive cut that throws the notes away.
"""

from ._native import BEATS, SECONDS


class Segment:
    """One window: which source, from which position, for how long.

    ``start`` is in the unit the **source is addressed in** (frames for samples,
    beats for a timeline of events) and ``duration`` in the unit the source
    **measures** (seconds for samples, beats for events). The run says which,
    through `SegmentRun.unit`, and bridges the two in one place
    (`SegmentRun.advanced`) so nothing else has to know a sample rate.
    """

    __slots__ = ("source", "start", "duration")

    def __init__(self, source, start=0.0, duration=None):
        self.source = source
        self.start = float(start)
        self.duration = 0.0 if duration is None else float(duration)

    @classmethod
    def of(cls, spec) -> "Segment":
        """A segment from a triple ``(source, start, duration)``, a pair
        ``(source, duration)``, or one of these."""
        if isinstance(spec, Segment):
            return spec
        items = tuple(spec)
        if len(items) == 3:
            return cls(items[0], items[1], items[2])
        if len(items) == 2:
            return cls(items[0], 0.0, items[1])
        raise TypeError(
            "a segment is (source, start, duration) or (source, duration), "
            f"not {spec!r}"
        )

    @property
    def buffer(self):
        """The source, under the name a run of samples has always called it."""
        return self.source

    def __repr__(self) -> str:
        return (f"{type(self).__name__}({self.source!r}, start={self.start}, "
                f"duration={self.duration})")

    def __eq__(self, other) -> bool:
        return (isinstance(other, Segment) and other.source is self.source
                and other.start == self.start
                and other.duration == self.duration)


class SegmentRun:
    """Several windows read as one: the general run, and the arithmetic that
    is the same whatever the windows are onto.

    Subclasses say what the material is -- `BufferSegments` over samples,
    `NoteSegments` over a timeline of events -- by answering `unit`, `advanced`
    and `to_events`. Everything else here is length arithmetic and holds for
    both.
    """

    #: The unit the segments' lengths are in, and therefore the run's own.
    unit = BEATS

    def __init__(self, segments):
        self.segments = [Segment.of(s) for s in segments]

    # -- what a subclass answers ------------------------------------------

    def advanced(self, start: float, by: float) -> float:
        """``start`` moved forward by the length ``by``, in the source's own
        addressing unit.

        The one bridge between the two units a window carries. The default is
        the case where there is nothing to bridge, which is every source
        addressed in what it measures.
        """
        return float(start) + float(by)

    def to_events(self, at_second_or_beat=None, **_):
        """What this run is played as. A subclass answers; a bare run is data
        and sounds through whoever reads it."""
        raise NotImplementedError(
            f"{type(self).__name__} does not say how its windows are played"
        )

    # -- the arithmetic, written once -------------------------------------

    @property
    def total(self) -> float:
        """The run's length: its segments', added up, in `unit`."""
        return sum(seg.duration for seg in self.segments)

    def placed(self) -> list:
        """``(offset, segment)`` pairs -- where each window starts *inside* the
        run, which is what both rendering and drawing lay out from. In `unit`
        throughout, like the lengths they accumulate."""
        out, cursor = [], 0.0
        for seg in self.segments:
            out.append((cursor, seg))
            cursor += seg.duration
        return out

    def cut(self, at: float) -> tuple:
        """The run split at ``at`` (in `unit`, from the run's own start): two
        runs of the same kind, over the same sources.

        The window the cut falls inside becomes two windows -- the first ends
        early, the second opens where the first stopped -- so nothing is copied
        and nothing is lost: joining them back gives this run, and lengthening
        either half brings its hidden material out again. A cut at or past
        either end gives one empty run and one whole one, which is the honest
        answer to a cut that took nothing.
        """
        at = float(at)
        head, tail = [], []
        for offset, seg in self.placed():
            end = offset + seg.duration
            if end <= at:
                head.append(Segment(seg.source, seg.start, seg.duration))
            elif offset >= at:
                tail.append(Segment(seg.source, seg.start, seg.duration))
            else:
                first = at - offset
                head.append(Segment(seg.source, seg.start, first))
                tail.append(Segment(seg.source,
                                    self.advanced(seg.start, first),
                                    seg.duration - first))
        return self.like(head), self.like(tail)

    def joined(self, other: "SegmentRun") -> "SegmentRun":
        """This run followed by ``other``: the inverse of `cut`, and the reason
        both are the same action over any material."""
        return self.like(list(self.segments) + list(other.segments))

    def like(self, segments) -> "SegmentRun":
        """Another run of this kind over ``segments``, carrying whatever
        configuration this one has. Subclasses that add configuration override
        it; the arithmetic above only ever builds runs through here."""
        return type(self)(segments)

    def __len__(self) -> int:
        return len(self.segments)

    def __iter__(self):
        return iter(self.segments)

    def __repr__(self) -> str:
        return f"{type(self).__name__}({self.segments!r})"


class BufferSegments(SegmentRun):
    """A run of windows onto **samples**: which buffer, from which frame, for
    how long.

    Lengths are in seconds -- a recording's seconds were fixed when it was
    recorded and no tempo change moves them -- while a window's ``start`` is the
    frame it opens at, which is the coordinate the samples are already in and
    the one a def's ``start`` control reads. `advanced` is where the two meet,
    and it is the only place in this file that knows what a sample rate is.

    Args:
        segments: the windows, as ``(buffer, start_frame, seconds)`` triples.
        instrument: the def that plays them -- one for the whole run, since what
            this is is *one thing to play*.
        controls: extra event parameters given to that def.
    """

    unit = SECONDS

    def __init__(self, segments, *, instrument=None, controls=None):
        super().__init__(segments)
        self.instrument = instrument
        self.controls = dict(controls or {})

    def like(self, segments) -> "BufferSegments":
        return type(self)(segments, instrument=self.instrument,
                          controls=self.controls)

    def advanced(self, start: float, by: float) -> float:
        """A frame, moved forward by ``by`` **seconds**: the sample rate is the
        bridge, and the buffer is what knows it."""
        rate = float(getattr(self._first_source(), "sample_rate", 0.0) or 0.0)
        return float(start) + float(by) * rate if rate > 0.0 else float(start)

    def _first_source(self):
        return self.segments[0].source if self.segments else None

    @property
    def contiguous(self) -> bool:
        """Whether these windows are **one run of one buffer**: each opening
        exactly where the one before it stopped.

        What makes a join the inverse of a split rather than a pile of
        wrappers -- a run like this *is* the single window it was cut from, and
        says so, so cutting and rejoining leaves the composition it started
        with. A run of one is trivially one run.
        """
        if not self.segments:
            return False
        first = self.segments[0]
        expected = first.start
        for seg in self.segments:
            if seg.source is not first.source or abs(seg.start - expected) >= 0.5:
                return False
            expected = self.advanced(seg.start, seg.duration)
        return True

    def event_params(self, seg: Segment) -> dict:
        """What playing one window asks the instrument for: the buffer, and the
        frame the window opens at."""
        params = {"buf": seg.source.bufnum}
        if seg.start:
            params["start"] = float(seg.start)
        return params


class NoteSegments(SegmentRun):
    """A run of windows onto a **timeline of events**: which timeline, from
    which beat, for how many beats.

    The same structure `BufferSegments` is, over the material whose lengths are
    musical -- so both units are beats and `advanced` has nothing to bridge.
    A cut here hides notes rather than deleting them, which is what makes
    dragging the edge back out bring them back, exactly as it does for samples.
    """

    unit = BEATS

    def __init__(self, segments, *, instrument=None, controls=None):
        super().__init__(segments)
        self.instrument = instrument
        self.controls = dict(controls or {})

    def like(self, segments) -> "NoteSegments":
        return type(self)(segments, instrument=self.instrument,
                          controls=self.controls)

    def items(self) -> list:
        """``(beat, item)`` pairs of everything inside the windows, placed on
        the **run's** own axis: each window's items shifted to where that window
        sits in the run. What falls outside a window is not here and is not
        gone -- it is in the timeline, waiting for the window to open again."""
        out = []
        for offset, seg in self.placed():
            window = seg.source.range(seg.start, seg.start + seg.duration)
            for beat, item in window:
                out.append((offset + (beat - seg.start), item))
        return out
