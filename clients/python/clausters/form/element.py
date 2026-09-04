"""The arrangement — elements and their temporal character.

The client-side layer under a multitrack editor of recursive granularity: it
places elements in time, groups them recursively and renders them. An `Element`
is an arbitrarily delimited entity that produces a unit of meaning and can be
decomposed or combined — *generated* (the rendered thing, editable and
random-access) or a *generator* (the algorithm that renders it, forward-only),
with the change of state between them. It is a **thin adornment** over the objects the client
already has (`clausters.seq.Event`, `clausters.seq.Timeline`, a `Vector`, a
`Pattern`, a def): it carries the temporal metadata (`onset`, `duration`, and the
derived temporal *character*) and belongs to an `Aggregate`, while it **delegates
playing** to the wrapped item's ``play(destination)`` — the double-dispatch
seam every leaf item in the client already shares. The arrangement does not
reimplement or subclass those objects.

The five primitives map one-to-one onto what the client already has:

- `Clang`     — *event/clip*: parameters grouped into one action (internally
  simultaneous), with its own onset/duration. Wraps `clausters.seq.Event`.
- `Sequence`  — *List*: strict order with no concrete time, only sequence.
  Wraps a Python list or a `Pattern`.
An element's ``onset`` is in **beats** and its ``duration`` is in the unit of
what it is made of (`Element.duration_unit`) — seconds for samples, beats for
events — because a placement is a musical decision and a recording's length is
not. `clausters.form.render.flatten` is where the two meet.

- `Vector`    — *Vector*: a list at constant time (audio or control samples).
  Wraps `clausters.defs.Buffer`. `Segments` is the same primitive assembled from
  **several** windows — which buffer, from which frame, for how long — read as
  one thing; it is not a sixth primitive, it is what a list at constant time
  looks like when the constant time comes from more than one place.
- `Track`     — *Set*: mixed placement of elements, a DAW track. Wraps
  `clausters.seq.Timeline`.
- `Generator` — *Function*: a generator element — server DSP (a def) or a
  sequence generator (`Pbind`/`Routine`).

Grouping and rendering live in `clausters.form.aggregate` and
`clausters.form.render`. This module is pure and transport-agnostic (factorable
into ``clausters-core`` in a future port).
"""

#: The unit a length is in. An **onset** is always in beats — a placement is a
#: musical decision and takes the unit of what contains it — and a **duration**
#: is in the unit of its own data: `SECONDS` for audio (a take's length is
#: ``frames / sample_rate``, a wall-clock fact no tempo change moves), `BEATS`
#: for a succession of events (a note is musical, and a tempo change is supposed
#: to shorten it). `Element.duration_unit` says which, derived from what the
#: element wraps rather than stored, and `clausters.form.render.flatten`
#: converts on the way to a timeline, which is ordered by one number and cannot
#: hold two bases.
from .._native import BEATS, SECONDS  # noqa: E402  (the vocabulary's one home)

#: The windows are **not** the arrangement's: a segment is about the contents,
#: not about where it sits in a piece, so `Segment` and the runs live beside the
#: structures (`clausters.segments`) and this module reads them like any other
#: reader. Re-exported here because `Segments` is the element that places one.
from ..segments import (BufferSegments, NoteSegments, Segment,  # noqa: E402,F401
                        SegmentRun)


def to_beats(length: float, unit: str, tempo: float) -> float:
    """``length`` (in ``unit``) as beats at ``tempo`` beats per second.

    The affine spelling, correct only while one tempo governs the whole stretch.
    `end_beat` is the one that holds under a tempo that changes, and every
    caller that knows where the length *starts* should use that instead: a
    length in beats has no length until it is told where it sits.
    """
    return float(length) * float(tempo) if unit == SECONDS else float(length)


def tempo_map_of(tempo_map=None, tempo: float = 1.0):
    """The map to measure with: the one given, or ``tempo`` as a single
    constant segment — which is the affine ratio every one of these
    conversions used to be, so a caller that names no map gets exactly what it
    always got."""
    if tempo_map is not None:
        return tempo_map
    from ..base.time import TempoMap

    return TempoMap(float(tempo))


def end_beat(at: float, length: float, unit: str, tempo_map) -> float:
    """The beat that ``length`` (in ``unit``) reaches, starting at beat ``at``.

    Two positions, never a length and a ratio. A length in **beats** is already
    on the axis and simply lands at ``at + length``; a length in **seconds** is a
    wall-clock fact whose end depends on how the tempo runs across it, which is
    what the piece's map (`clausters.base.TempoMap`) answers.
    """
    if unit != SECONDS:
        return float(at) + float(length)
    return tempo_map.beats_at(tempo_map.secs_at(float(at)) + float(length))


#: The temporal character of an element, derived from which of ``onset`` and
#: ``duration`` are present. ``segment`` has both; ``punctual`` has an
#: onset but no duration; ``relative`` has a duration but no onset; ``abstract``
#: has neither (a pure context/container that only a parent gives concrete time).
SEGMENT = "segment"
PUNCTUAL = "punctual"
RELATIVE = "relative"
ABSTRACT = "abstract"


def temporal_character(onset, duration) -> str:
    """The temporal character for a given ``onset``/``duration`` pair (the pure
    rule behind `Element.temporal_character`)."""
    has_onset = onset is not None
    has_duration = duration is not None
    if has_onset and has_duration:
        return SEGMENT
    if has_onset:
        return PUNCTUAL
    if has_duration:
        return RELATIVE
    return ABSTRACT


class Element:
    """Base of the arrangement: temporal metadata over a wrapped item.

    An element carries an optional ``onset`` (in beats, relative to its context)
    and ``duration`` (in the unit of what it wraps — see `duration_unit`) and
    wraps an underlying client object it delegates to. The
    concrete onset of an element typically comes from its *placement* inside a
    `clausters.form.aggregate.Aggregate`, not from the element itself, so a standalone
    leaf commonly has a duration but no onset (a ``relative`` character).

    Args:
        wraps: the underlying object playing delegates to (or ``None`` for a
            pure container like an `Aggregate`).
        onset: start in beats relative to the context, or ``None``.
        duration: length in this element's own unit (`duration_unit`), or
            ``None``.
        name: a label for this element — what a lane is called in the editor,
            and, for an element wrapping something the document cannot own (a
            pattern, a routine), the **key a reopened session finds it by**. It
            is a label and not an identity: nothing addresses an element by
            name, and two elements may share one, which is what naming *the
            same algorithm used twice* looks like.
    """

    def __init__(self, wraps=None, onset=None, duration=None, resident=False,
                 *, name=None):
        self.wraps = wraps
        #: A label, and the key an unowned leaf is handed back by. See the class
        #: docstring; the document carries it as the node's `name`.
        self.name = name
        self.onset = None if onset is None else float(onset)
        self.duration = None if duration is None else float(duration)
        #: Whether this element's audio is produced by a def running **on the
        #: server** rather than by messages the arrangement flattens. Such an
        #: element is a generator with no index (see `locatable`).
        self.resident = bool(resident)
        #: **Mixing: the composition's, not the view's.** Whether this element
        #: is silenced (`mute`), whether it is one of the elements soloed
        #: (`solo`), and the gain its events sound at (`level`, a factor over
        #: an event's own ``amp``). They are set by the editor's lane header and
        #: by hand, they are honoured by `clausters.form.render.flatten`, and
        #: they travel in the node's configuration -- so a piece reopens muted
        #: the way it was left. A lane's *height* is the other kind of thing and
        #: is deliberately absent: it says nothing about what the piece is, so
        #: no document carries it.
        self.mute = False
        self.solo = False
        self.level = 1.0

    @property
    def locatable(self) -> bool:
        """Whether a position on this element means anything.

        A **generated** element has an index: the arrangement flattens it to
        messages at absolute beats, so a transport can put itself anywhere on
        it. A **resident generator** — a def producing its own audio on the
        server, a stochastic process, a demand-rate sequence — has none. Its
        position *is* its internal state, and no number moves it: the only thing
        a transport can do to it is stop it and let it carry on.

        This is the same asymmetry the arrangement is built around, reaching the
        transport. Pause is symmetric and works for both; locate is not. A
        generator becomes locatable by being **rendered** — the change of state
        from generator to generated — after which it is a buffer like any other.
        """
        return not self.resident

    @property
    def duration_unit(self) -> str:
        """The unit `duration` is in: `SECONDS` for the elements whose data
        is samples (`Vector`, `Segments`), and for anything wrapped that
        measures itself in seconds (a `clausters.seq.Automation`'s curve is an
        envelope, and an envelope's segment times are real time); `BEATS`
        otherwise.

        Derived from what the element is made of rather than stored, so nothing
        can write one unit and read the other. An object that wants to answer
        for itself declares its own ``duration_unit``."""
        return getattr(self.wraps, "duration_unit", None) or BEATS

    @property
    def temporal_character(self) -> str:
        """This element's character (`SEGMENT`/`PUNCTUAL`/`RELATIVE`/`ABSTRACT`),
        derived from the presence of ``onset`` and ``duration``."""
        return temporal_character(self.onset, self.duration)

    # -- windows: what a trim, a split and a join ask an element --------------
    #
    # **The question is the contents', never the class's.** Cutting is defined
    # wherever there is an addressable time axis -- samples, notes, events,
    # segments -- so the verb asks the element whether it has one instead of
    # testing what it is. What genuinely answers no is a **generator**: not
    # "cannot be cut" but *not until it is rendered*, which is the change of
    # state the model already has a verb for.

    def window_start(self):
        """Where this element **reads from** inside what it holds, or ``None``
        when it holds no window at all.

        In the unit the contents are *addressed* in -- frames for samples, beats
        for events -- which is the same coordinate
        `clausters.segments.Segment.start` is in and for the same reason.
        """
        return None

    def windowed(self, at: float, length: float, rate: float = 0.0):
        """The element the **second half** of a cut at ``at`` reads, or ``None``
        when this element cannot be cut.

        ``at`` and ``length`` are in this element's own unit
        (`duration_unit`), and ``rate`` is the sample rate to bridge with when
        the contents are addressed in frames and the source does not know its
        own -- the one number an element may need from the caller.

        The **first** half is never built: it is the element it always was, with
        its placement shortened, which is the arrangement's rule (a placement is
        a window onto an element, never a rewrite of it) and what makes an undo
        of a split one step. Nothing is copied and nothing is lost either way --
        lengthening a half brings back exactly what the cut hid.
        """
        return None

    def play(self, destination):
        """Delegate playing to the wrapped item's ``play(destination)`` — the
        double-dispatch seam shared by `clausters.seq.Event`,
        `clausters.seq.timeline.OscItem`/`MidiItem` and
        `clausters.seq.Automation`.

        Container and pattern-backed elements (`Aggregate`, `Track`, a `Sequence`
        wrapping a `Pattern`) are **not** directly playable this way — they are
        rendered by ``render()``. Delegating here requires the wrapped
        object to follow the ``play(destination)`` protocol.
        """
        play = getattr(self.wraps, "play", None)
        if play is None:
            raise NotImplementedError(
                f"{type(self).__name__} is not directly playable; use render()"
            )
        return play(destination)

    def to_timeline(self, base: float = 0.0, *, tempo: float = 1.0):
        """Flatten this element to a flat `clausters.seq.Timeline` in absolute
        beats (accumulating nested placement offsets), converting any length
        measured in seconds at ``tempo``. See `clausters.form.render`."""
        from .render import to_timeline

        return to_timeline(self, base, tempo=tempo)

    def render(self, destination, clock=None, *, at: float = 0.0, quant=None,
               ports=None):
        """Render this element onto ``destination`` — the change of state to
        sound. A concrete element flattens and plays through a
        `clausters.seq.Playhead` over ``clock`` (returns the playhead); a logical
        `Aggregate` sends and instances a `GraphDef` on the server (returns the
        instance). See `clausters.form.render.render`."""
        from .render import render

        return render(self, destination, clock, at=at, quant=quant, ports=ports)


class Clang(Element):
    """*event/clip*: parameters grouped into one action, internally simultaneous.

    Wraps a `clausters.seq.Event` (or a plain ``dict`` of parameters), and
    equally a `clausters.seq.timeline.OscItem` or `MidiItem` — an action that
    happens at one moment is a clang whether it is a note or a message, which is
    what `Element.play`'s double dispatch has always assumed and what a timeline
    written into a document is read back as. Anything that plays itself is taken
    as it is; anything else is the parameters of an event.

    Its ``duration`` defaults to the event's ``dur`` when not given explicitly;
    its ``onset`` usually comes from its placement in an `Aggregate`.
    """

    def __init__(self, event, onset=None, duration=None, *, name=None):
        from ..seq.event import Event as SeqEvent

        wrapped = event if callable(getattr(event, "play", None)) \
            else SeqEvent(event)
        if duration is None:
            dur = wrapped.get("dur") if isinstance(wrapped, dict) else None
            if dur is not None:
                duration = float(dur)
        super().__init__(wraps=wrapped, onset=onset, duration=duration,
                         name=name)


class Sequence(Element):
    """*List*: strict order with no concrete time — only sequence.

    Wraps a Python list or a `clausters.seq.pattern.Pattern`. The items can be
    numbers, events, notes or whole elements; the structure fixes only their
    successive order. Rendering bounces a pattern-backed sequence; a list is
    interpreted by its content.
    """

    def __init__(self, items, onset=None, duration=None, *, name=None):
        super().__init__(wraps=items, onset=onset, duration=duration, name=name)


class Vector(Element):
    """*Vector*: a list at constant time — audio or control samples.

    Wraps a `clausters.defs.Buffer`. An automation sampled at a constant interval
    is a control buffer (the List/Vector duality of the arrangement).

    A buffer is *data*, so rendering it as an **audio clip** needs an instrument:
    the def that plays it, named by ``instrument`` (a synth whose ``buf`` control
    takes the buffer number, as a sampler's does). Rendering then emits one
    event playing that def — `to_event`. Without an instrument the element is
    still perfectly good structure (and the editor draws its take), it simply has
    no sound of its own.

    Args:
        buffer: the `clausters.defs.Buffer` on the server.
        instrument: the def that plays it (its ``buf`` control gets the buffer
            number), or ``None`` for a buffer that is data only.
        controls: extra event parameters passed to that def (``amp``, ``rate``…).
        onset: start in beats relative to the context, or ``None``.
        duration: length in **seconds** — how long the clip sounds. Give it for
            a take placed in time (an event's default length is used
            otherwise); it is seconds and not beats because a recording's length
            is ``frames / sample_rate``, which a tempo change does not move.
        start: the first frame of the buffer this element reads. An element is a
            **window onto a segment** of its buffer, not the whole of it: a
            trimmed take reads from further in and the frames before it are
            still there, which is what lets a trim be undone and a split give
            two windows over one buffer.
        loop: whether that window wraps around the buffer — past the last frame
            it begins again.

    A window that is not the whole buffer travels to the instrument as the
    ``start``/``loop`` event parameters, so a def that reads them (a sampler
    whose ``PlayBuf`` takes a ``start_pos`` and a ``loop``) plays exactly the
    segment the editor draws. An element reading its buffer from the beginning
    sends neither, so a def written before windows existed is sent what it
    always was.
    """

    def __init__(self, buffer, onset=None, duration=None, *, instrument=None,
                 controls=None, start=0.0, loop=False, name=None):
        super().__init__(wraps=buffer, onset=onset, duration=duration, name=name)
        self.instrument = instrument
        self.controls = dict(controls or {})
        #: The **first frame of the buffer this element reads** — the head of
        #: its window onto the buffer. Trimming a clip moves it; splitting one
        #: in two gives each half a window of its own over the same buffer.
        self.start = float(start)
        #: Whether the window **wraps** around the buffer: past the last frame
        #: it begins again, which is what stretching an element beyond the
        #: buffer means when a loop is what it is.
        self.loop = bool(loop)

    @property
    def duration_unit(self) -> str:
        """`SECONDS`: this element's data is samples, and their seconds were
        fixed when they were recorded — a tempo change does not shorten a take."""
        return SECONDS

    def window_start(self):
        """The frame this element reads from -- it has had a window since
        trimming existed."""
        return self.start

    def windowed(self, at: float, length: float, rate: float = 0.0):
        """The same buffer, read from ``at`` seconds further in. The frames
        neither half shows are still there, which is why stretching either one
        brings them back."""
        hz = float(getattr(self.wraps, "sample_rate", 0.0) or rate or 0.0)
        return Vector(self.wraps, duration=float(length) - float(at),
                      instrument=self.instrument, controls=self.controls,
                      start=self.start + float(at) * hz, loop=self.loop,
                      name=self.name)

    def to_event(self, tempo_map=None, at: float = 0.0):
        """The event that plays this buffer: the `instrument` def with the buffer
        number in its ``buf`` control, sounding for the element's ``duration``.

        ``tempo_map`` is what the length crosses on: this element's duration is
        in seconds and an event's ``dur`` is in beats, because an event is
        played by a clock. It is the only conversion, and it happens here rather
        than in the structure. ``at`` is the beat the take starts on, which the
        crossing needs: the same stretch of seconds is a different number of
        beats depending on where the tempo has got to.

        ``legato`` is 1 so the take sounds its whole length (the note default of
        0.8 would cut it short — a sampled take is not a note with a gap), and
        ``amp`` is 1 for the same reason at the other end: the note default
        mixes an event **20 dB down**, which is a headroom convention for
        stacking notes and simply attenuates recorded audio. A take arrives
        at the level it was recorded at; anything else is a mix decision, so it
        goes in ``controls`` (which overrides both).
        """
        from ..seq.event import Event as SeqEvent

        if self.instrument is None:
            raise NotImplementedError(
                "a Vector needs an instrument to be rendered as an audio clip "
                "(Vector(buf, instrument='take'): a def whose `buf` control plays it)"
            )
        params = dict(instrument=self.instrument, buf=self.wraps.bufnum,
                      legato=1.0, amp=1.0)
        # The window, so what is heard is the segment that is drawn — and only
        # when there is one to state, so a def that never heard of windows is
        # sent exactly what it was always sent.
        if self.start:
            params["start"] = float(self.start)
        if self.loop:
            params["loop"] = 1.0
        if self.duration is not None:
            # Two positions, not a length times a ratio: the take's seconds are
            # fixed, and how many beats they cover depends on where it starts.
            tempo_map = tempo_map_of(tempo_map)
            params["dur"] = end_beat(at, self.duration, SECONDS, tempo_map) - at
        params.update(self.controls)
        return SeqEvent(params)


def take(buffer, onset=None, duration=None, *, instrument=None, controls=None,
         start: float = 0.0, loop: bool = False, name=None,
         sample_rate: float = 0.0) -> "Vector":
    """A recorded or loaded buffer **placed in the arrangement**: a `Vector`
    whose length is the samples' own.

    This is where recording lands. `clausters.data.RecordingStream` follows
    takes as they are written and `clausters.defs.Buffer` holds them, but
    neither puts one in a piece — and the arithmetic that does (frames over the
    rate they were recorded at) was left to every caller, which is one
    conversion written once per script and wrong in the one that forgot the
    channel count is not in it.

    Args:
        buffer: the samples — a `clausters.defs.Buffer`, a
            `clausters.data.TakeShape`, or the
            `clausters.form.document.FrozenSource` a document hands back for a
            source this process has not resolved.
        onset: start in beats, or ``None`` (the placement usually says).
        duration: the length in **seconds**, when the caller knows better than
            the buffer does — a take still recording, whose buffer is as long
            as it will be rather than as long as it is.
        instrument: the def that plays it; without one the take is structure
            (it draws and it extends the piece, and it emits no event), which
            is the `Vector` rule and not a special case here.
        controls: what that def is given.
        start: the frame the window opens at, and ``loop`` whether it wraps.
        name: the label a lane carries.
        sample_rate: the rate to measure the length at, when the buffer does
            not know its own (a `TakeShape` carries no rate).

    Returns:
        The `Vector`. Its ``duration`` is ``None`` when nothing knows the rate,
        which is the honest answer: the length is then the placement's.
    """
    rate = float(sample_rate or getattr(buffer, "sample_rate", 0.0) or 0.0)
    frames = float(getattr(buffer, "frames", 0) or 0)
    if duration is None and rate > 0.0 and frames > 0.0:
        duration = frames / rate
    return Vector(buffer, onset=onset, duration=duration, instrument=instrument,
                  controls=controls, start=start, loop=loop, name=name)


class Segments(Element):
    """*Several windows read as one*: data assembled from segments of one or
    more buffers, which sound as a single thing.

    A `Vector` is one window onto one buffer. This is what a **join** makes when
    the fragments do not come from one place, and what a **split** takes apart
    again. The windows themselves are not the arrangement's -- they are
    `clausters.segments.BufferSegments`, the general run this element *places* --
    so nothing about assembling contents has to be written twice for the two
    kinds of contents that have a time axis.

    Rendering emits **one event per segment**, each at its own offset inside the
    element and each carrying its own window, so the segments sound continuous
    on one instrument. The editor draws it as **one clip** holding one take per
    segment, each over its own stretch of the clip.

    Args:
        segments: the runs, as ``(buffer, start, duration)`` triples (a
            plain ``(buffer, duration)`` reads that buffer from its first
            frame). ``start`` is the **frame** the window opens at and
            ``duration`` its length in **seconds** -- the addressing unit and
            the measured one, which `clausters.segments.SegmentRun.advanced`
            is the single bridge between.
        instrument: the def that plays them -- one def for all of them, since
            what this element *is* is one thing to play (see `Vector`).
        controls: extra event parameters passed to that def.
        onset: start in beats relative to the context, or ``None``.
        duration: length in **seconds**; the sum of the segments' when not
            given.
    """

    def __init__(self, segments, onset=None, duration=None, *, instrument=None,
                 controls=None, name=None):
        # **A run of any contents, or the windows to make one of.** A list of
        # ``(buffer, start, seconds)`` is the samples case and stays what it
        # always was; a run handed in whole is what a join over notes makes,
        # and this element places it without knowing which it is.
        run = (segments if isinstance(segments, SegmentRun)
               else BufferSegments(segments, instrument=instrument,
                                   controls=controls))
        if duration is None and len(run):
            duration = run.total
        super().__init__(wraps=run.segments, onset=onset, duration=duration,
                         name=name)
        #: The windows themselves, as the general structure they are.
        self.run = run
        self.instrument = instrument
        self.controls = dict(controls or {})

    @property
    def duration_unit(self) -> str:
        """The run's own -- `SECONDS`, because these windows are onto samples.
        Asked of the data rather than stated here, which is what lets the same
        element place a run of any contents."""
        return self.run.unit

    @property
    def segments(self) -> list:
        """The segments, in reading order -- the element's own data."""
        return self.run.segments

    def placed(self) -> list:
        """The segments with the second each one **starts at** inside this
        element: ``(offset, segment)`` pairs, which is what both rendering and
        drawing lay out from."""
        return self.run.placed()

    def window_start(self):
        """Zero: a run's window is in its segments, each of which carries its
        own -- so there is no single frame this element reads from, and a trim
        moves the windows rather than a head."""
        return 0.0

    def windowed(self, at: float, length: float, rate: float = 0.0):
        """The windows past the cut, with the one the cut falls inside cut in
        two -- which is `clausters.segments.SegmentRun.cut`, the arithmetic this
        element places rather than reimplements.

        A tail that is **one run of one source** comes back as the single window
        it is -- a `Vector` over samples, a `Track` over a timeline
        (`clausters.segments.SegmentRun.contiguous`). That is not an
        optimization, it is what makes a cut and a join inverses instead of a
        pile of wrappers."""
        _, tail = self.run.cut(float(at))
        if tail.contiguous:
            return _single_window(tail, self.instrument, self.controls,
                                  self.name)
        return Segments(tail, instrument=self.instrument,
                        controls=self.controls, name=self.name)

    def to_events(self, tempo_map=None, at: float = 0.0) -> list:
        """One ``(offset, event)`` per segment: the instrument playing that
        buffer, from that frame, for that long. The offsets are relative to the
        element, exactly as an aggregate's members' are -- and in **beats**,
        converted here from the seconds the windows are measured in, because
        what comes out of this is played by a clock.

        ``tempo_map`` is the piece's, and ``at`` the beat this element starts
        on: each window is placed and sized from where it actually falls, so a
        tempo change inside the element moves the segments after it and not the
        ones before."""
        from ..seq.event import Event as SeqEvent

        if self.run.unit == BEATS:
            # **Windows onto timelines are already events**: what each one shows
            # is the items inside it, placed from the run's own zero and in
            # beats, so there is nothing to build and nothing to convert. An
            # instrument is the samples case's -- a note carries its own.
            return list(self.run.items())
        if self.instrument is None:
            raise NotImplementedError(
                "a Segments needs an instrument to be rendered as audio "
                "(Segments(..., instrument='take'): a def whose `buf` control "
                "plays a buffer, reading `start` for the window)"
            )
        tempo_map = tempo_map_of(tempo_map)
        out = []
        for offset, seg in self.placed():
            # Both numbers are seconds and both are placed, not scaled: the
            # window opens at the beat those seconds reach from `at`, and lasts
            # to the beat its own seconds reach from there.
            onset = end_beat(at, offset, SECONDS, tempo_map)
            end = end_beat(onset, seg.duration, SECONDS, tempo_map)
            params = dict(instrument=self.instrument, legato=1.0, amp=1.0,
                          dur=end - onset)
            params.update(self.run.event_params(seg))
            params.update(self.controls)
            out.append((onset - at, SeqEvent(params)))
        return out


def _single_window(run, instrument=None, controls=None, name=None):
    """The element **one run of one source** is: a `Vector` over samples, a
    `Track` over a timeline.

    A join that ends up with one window is the window it was cut from, and says
    so rather than staying a list of one -- which is what makes a cut and a join
    inverses. One place, so the two kinds cannot drift into answering it
    differently.
    """
    first = run.segments[0]
    if run.unit == SECONDS:
        return Vector(first.source, duration=run.total, instrument=instrument,
                      controls=controls, start=first.start, name=name)
    return Track(first.source, duration=run.total, start=first.start, name=name)


class Track(Element):
    """*Set*: mixed placement of elements — a DAW track.

    Wraps a `clausters.seq.Timeline` (free placement of items by beat). A fresh
    empty `Timeline` is created when none is given.

    Args:
        timeline: the `clausters.seq.Timeline` the track places.
        onset: start in beats relative to the context, or ``None``.
        duration: length in **beats** — how much of the timeline this element
            is. With ``start``, it is the *window*: what a clip of this track
            draws and plays.
        start: the beat of the timeline this element **reads from**. A track is
            a window onto its timeline exactly as a `Vector` is a window onto
            its buffer, and for the same reason: a trim reads from further in, a
            split gives two windows over one timeline, and the notes neither
            window shows are still on it — so lengthening either half brings
            them back. A cut is not a rewrite of the notes.
    """

    def __init__(self, timeline=None, onset=None, duration=None, *, start=0.0,
                 name=None):
        if timeline is None:
            from ..seq.timeline import Timeline

            timeline = Timeline()
        super().__init__(wraps=timeline, onset=onset, duration=duration,
                         name=name)
        #: The **beat of the timeline this element reads from** — the head of
        #: its window, the beats counterpart of `Vector.start`.
        self.start = float(start)

    def window_start(self):
        """The beat this element reads its timeline from."""
        return self.start

    def windowed(self, at: float, length: float, rate: float = 0.0):
        """The same timeline, read from ``at`` beats further in. Both units are
        beats here, so there is nothing to bridge — the notes outside either
        window are on the timeline, not gone."""
        return Track(self.wraps, duration=float(length) - float(at),
                     start=self.start + float(at), name=self.name)

    def items(self) -> list:
        """The ``(beat, item)`` pairs this element **shows**: its window's, placed
        from the element's own zero.

        The whole timeline when it has no window (a track written by a script is
        the timeline), and the window's contents shifted back to zero when a
        trim or a split gave it one. What falls outside is not here and is not
        gone."""
        timeline = self.wraps
        entries = list(timeline)
        if not self.start and self.duration is None:
            return [(float(beat), item) for beat, item in entries]
        end = (self.start + float(self.duration) if self.duration is not None
               else float("inf"))
        return [(float(beat) - self.start, item) for beat, item in entries
                if self.start <= float(beat) < end]


class Generator(Element):
    """*Function*: a generator element.

    Wraps either server DSP (a `SynthDef`/`FaustDef`/`GraphDef`, or a def name)
    or a sequence generator (a `Pbind`/`Routine`). Its *change of state* —
    evaluating the generator into a generated element — happens at rendering: a
    contained event pattern is bounced to a timeline; a def member of a
    logical `Aggregate` becomes a wired GraphDef member.

    Args:
        generator: the wrapped def (name or object) or sequence generator.
        controls: control values for a logical-graph member — numbers, an
            internal-bus name (a ``str`` matching an `Aggregate` bus), or ``"OUT"``
            (hardware). Used by `Aggregate.to_graphdef`.
        maps: control-bus bindings for a logical-graph member
            (``/node_map``), as a ``{control: bus_name}`` dict.
        rendered: what this generator **last produced**, as an ordinary
            `Element` — the change of state above, kept rather than recomputed.
            It is what a host with no language attached shows, since a
            generator is code and such a host has nothing to run it with; and
            it is what a saved session carries for the same reason a cache
            cannot, which is that a missing cache leaves nothing to draw.
    """

    def __init__(self, generator, onset=None, duration=None, *, controls=None,
                 maps=None, rendered=None, name=None):
        super().__init__(wraps=generator, onset=onset, duration=duration,
                         name=name)
        self.controls = controls
        self.maps = maps
        #: The last rendered result, or ``None`` before there is one. Read-only
        #: as far as editing goes: it is a rendering, not the composition, so an
        #: edit to it would be written over by the next render.
        self.rendered = rendered

    @property
    def def_name(self) -> str:
        """The member def name — the wrapped string itself, or the def object's
        ``name``."""
        return self.wraps if isinstance(self.wraps, str) else self.wraps.name
