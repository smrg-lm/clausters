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

    An element carries an optional ``onset`` and ``duration`` (in beats, relative
    to its context) and wraps an underlying client object it delegates to. The
    concrete onset of an element typically comes from its *placement* inside a
    `clausters.form.aggregate.Aggregate`, not from the element itself, so a standalone
    leaf commonly has a duration but no onset (a ``relative`` character).

    Args:
        wraps: the underlying object playing delegates to (or ``None`` for a
            pure container like an `Aggregate`).
        onset: start in beats relative to the context, or ``None``.
        duration: length in beats, or ``None``.
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
        #: Whether this element's material is produced by a def running **on the
        #: server** rather than by messages the arrangement flattens. Such an
        #: element is a generator with no index (see `locatable`).
        self.resident = bool(resident)

    @property
    def locatable(self) -> bool:
        """Whether a position on this element means anything.

        A **generated** element has an index: the arrangement flattens it to
        messages at absolute beats, so a transport can put itself anywhere on
        it. A **resident generator** — a def producing its own material on the
        server, a stochastic process, a demand-rate sequence — has none. Its
        position *is* its internal state, and no number moves it: the only thing
        a transport can do to it is stop it and let it carry on.

        This is the same asymmetry the arrangement is built around, reaching the
        transport. Pause is symmetric and works for both; locate is not. A
        generator becomes locatable by being **rendered** — the change of state
        from generator to generated — after which it is material like any other.
        """
        return not self.resident

    @property
    def temporal_character(self) -> str:
        """This element's character (`SEGMENT`/`PUNCTUAL`/`RELATIVE`/`ABSTRACT`),
        derived from the presence of ``onset`` and ``duration``."""
        return temporal_character(self.onset, self.duration)

    def play(self, destination):
        """Delegate playing to the wrapped item's ``play(destination)`` — the
        double-dispatch seam shared by `clausters.seq.Event`,
        `clausters.seq.timeline.OscEvent`/`MidiEvent` and
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

    def to_timeline(self, base: float = 0.0):
        """Flatten this element to a flat `clausters.seq.Timeline` in absolute
        beats (accumulating nested placement offsets). See
        `clausters.form.render`."""
        from .render import to_timeline

        return to_timeline(self, base)

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

    Wraps a `clausters.seq.Event` (or a plain ``dict`` of parameters). Its
    ``duration`` defaults to the event's ``dur`` when not given explicitly; its
    ``onset`` usually comes from its placement in an `Aggregate`.
    """

    def __init__(self, event, onset=None, duration=None, *, name=None):
        from ..seq.event import Event as SeqEvent

        wrapped = event if isinstance(event, SeqEvent) else SeqEvent(event)
        if duration is None:
            dur = wrapped.get("dur")
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
        duration: length in beats — how long the clip sounds. Give it for a take
            placed in time (an event's default length is used otherwise).
        start: the first frame of the buffer this element reads. An element is a
            **window onto a segment** of its material, not the whole of it: a
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
        #: its window onto the material. Trimming a clip moves it; splitting one
        #: in two gives each half a window of its own over the same buffer.
        self.start = float(start)
        #: Whether the window **wraps** around the material: past the last frame
        #: it begins again, which is what stretching an element beyond the
        #: buffer means when a loop is what it is.
        self.loop = bool(loop)

    def to_event(self):
        """The event that plays this buffer: the `instrument` def with the buffer
        number in its ``buf`` control, sounding for the element's ``duration``.

        ``legato`` is 1 so the take sounds its whole length (the note default of
        0.8 would cut it short — a sampled take is not a note with a gap), and
        ``amp`` is 1 for the same reason at the other end: the note default
        mixes an event **20 dB down**, which is a headroom convention for
        stacking notes and simply attenuates recorded material. A take arrives
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
            params["dur"] = float(self.duration)
        params.update(self.controls)
        return SeqEvent(params)


class Segments(Element):
    """*Several windows read as one*: material assembled from segments of one or
    more buffers, which sound as a single thing.

    A `Vector` is one window onto one buffer. This is what a **join** makes when
    the fragments do not come from one place: a list of
    ``(buffer, start, duration)`` — the buffer to read, the frame to read it
    from, and how long that segment lasts in beats — read back to back. It is
    the same memory-view idea one level up: nothing is copied, and cutting one
    of these apart again gives back windows over the same buffers.

    Rendering emits **one event per segment**, each at its own offset inside the
    element and each carrying its own window, so the segments sound continuous
    on one instrument. The editor draws it as **one clip** holding one take per
    segment, each over its own stretch of the clip.

    Args:
        segments: the material, as ``(buffer, start, duration)`` triples (a
            plain ``(buffer, duration)`` reads that buffer from its first
            frame). ``start`` is in frames, ``duration`` in beats.
        instrument: the def that plays them — one def for all of them, since
            what this element *is* is one thing to play (see `Vector`).
        controls: extra event parameters passed to that def.
        onset: start in beats relative to the context, or ``None``.
        duration: length in beats; the sum of the segments' when not given.
    """

    def __init__(self, segments, onset=None, duration=None, *, instrument=None,
                 controls=None, name=None):
        parsed = [Segment.of(s) for s in segments]
        if duration is None and parsed:
            duration = sum(seg.duration for seg in parsed)
        super().__init__(wraps=parsed, onset=onset, duration=duration, name=name)
        self.instrument = instrument
        self.controls = dict(controls or {})

    @property
    def segments(self) -> list:
        """The segments, in reading order — the element's own material."""
        return list(self.wraps or ())

    def placed(self) -> list:
        """The segments with the beat each one **starts at** inside this
        element: ``(offset, segment)`` pairs, which is what both rendering and
        drawing lay out from."""
        out, cursor = [], 0.0
        for seg in self.segments:
            out.append((cursor, seg))
            cursor += seg.duration
        return out

    def to_events(self) -> list:
        """One ``(offset, event)`` per segment: the instrument playing that
        buffer, from that frame, for that long. The offsets are relative to the
        element, exactly as an aggregate's members' are."""
        from ..seq.event import Event as SeqEvent

        if self.instrument is None:
            raise NotImplementedError(
                "a Segments needs an instrument to be rendered as audio "
                "(Segments(..., instrument='take'): a def whose `buf` control "
                "plays a buffer, reading `start` for the window)"
            )
        out = []
        for offset, seg in self.placed():
            params = dict(instrument=self.instrument, buf=seg.buffer.bufnum,
                          legato=1.0, amp=1.0, dur=float(seg.duration))
            if seg.start:
                params["start"] = float(seg.start)
            params.update(self.controls)
            out.append((offset, SeqEvent(params)))
        return out


class Segment:
    """One segment of a `Segments`: which buffer, from which frame, for how
    long. A window, named the same way a `Vector` element's is."""

    __slots__ = ("buffer", "start", "duration")

    def __init__(self, buffer, start=0.0, duration=None):
        self.buffer = buffer
        self.start = float(start)
        self.duration = 0.0 if duration is None else float(duration)

    @classmethod
    def of(cls, spec) -> "Segment":
        """A segment from a triple ``(buffer, start, duration)``, a pair
        ``(buffer, duration)``, or one of these."""
        if isinstance(spec, Segment):
            return spec
        items = tuple(spec)
        if len(items) == 3:
            return cls(items[0], items[1], items[2])
        if len(items) == 2:
            return cls(items[0], 0.0, items[1])
        raise TypeError(
            "a segment is (buffer, start, duration) or (buffer, duration), "
            f"not {spec!r}"
        )

    def __repr__(self) -> str:
        return (f"Segment({self.buffer!r}, start={self.start}, "
                f"duration={self.duration})")

    def __eq__(self, other) -> bool:
        return (isinstance(other, Segment) and other.buffer is self.buffer
                and other.start == self.start and other.duration == self.duration)


class Track(Element):
    """*Set*: mixed placement of elements — a DAW track.

    Wraps a `clausters.seq.Timeline` (free placement of items by beat). A fresh
    empty `Timeline` is created when none is given.
    """

    def __init__(self, timeline=None, onset=None, duration=None, *, name=None):
        if timeline is None:
            from ..seq.timeline import Timeline

            timeline = Timeline()
        super().__init__(wraps=timeline, onset=onset, duration=duration,
                         name=name)


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
