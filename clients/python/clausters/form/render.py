"""Rendering — the *change of state* from the arrangement to sound.

A concrete `Aggregate` is rendered by **flattening** it: a tree-walk that
accumulates the nested placement offsets into absolute beats, producing a flat
`clausters.seq.Timeline` of items that each know how to `play(destination)`. That
timeline is then played by a `clausters.seq.Playhead` — RT (timetagged bundles)
or NRT (a score for `Session.render`) purely by which destination and clock it
holds, sample-identical, with no scheduling path of its own. This mirrors
`Timeline.from_pattern`: the arrangement reuses the sequencing layer rather than
duplicating it.

**The two units meet here.** An onset is in beats and a length is in the unit
of its own data — a take's is seconds, a phrase of events' is beats — and a
timeline is a list ordered by *one* number, so it cannot hold both. `flatten`
takes the clock's ``tempo`` and converts as it lays each item down; the tree
itself keeps every leaf in the unit that leaf is in.

Scope of this phase (the concrete path):

- `Aggregate{concrete}` — flattened recursively; each member's ``offset`` (and
  any nested aggregate's) accumulates into the child's absolute beat.
- `Track` — its `Timeline`'s items are shifted by the placement beat.
- `Clang` — placed as a single item at its beat.
- `Sequence`/`Generator` wrapping an **event pattern** (a `Pbind`) — *bounced*
  in the same pass (its change of state); a `Sequence` of elements is laid out
  successively by their durations.
- An **abstract** element (no onset/duration, no content) contributes context,
  not an event.

**Mixing is part of the composition, and it is honoured here.** An element
carries `clausters.form.element.Element.mute`, `solo` and `level`, all three
inherited down the tree: a muted branch contributes nothing, one soloed element
anywhere silences every branch that is not on a soloed path, and a level
multiplies into the ``amp`` of the events below it. They travel in the
document, so a piece reopens mixed the way it was left — unlike a lane's
*height*, which says nothing about what the piece is and is carried by no
document.

A `Vector` is *data*: it sounds through the **instrument** that plays it (a def
whose ``buf`` control takes the buffer number), so a `Vector` with an
``instrument`` emits one event playing it — the audio clip — and one without
contributes structure only. A `Segments` is the same rule over several windows:
one event per segment, at its own offset inside the element, so what sounds
assembled from pieces of different buffers sounds continuous on one instrument. An `Aggregate{logical}` takes the other path entirely (it
becomes a `GraphDef`); instancing a bare def still needs an instrument of its own
and raises a clear `NotImplementedError` here.
"""

from ..defs.node import Group as NodeGroup
from .aggregate import CONCRETE, LOGICAL, Aggregate
from .element import (BEATS, Element, Generator, Clang, Segments, Sequence,
                      Track, Vector, end_beat, tempo_map_of)


def flatten(element, base: float = 0.0, *, tempo: float = 1.0, tempo_map=None,
            mixed: bool = True) -> list:
    """Flatten ``element`` into ``(absolute_beat, item)`` pairs, sorted by beat,
    accumulating nested placement offsets onto ``base``. The items are playable
    (they follow the ``play(destination)`` protocol).

    The piece's tempo is where the tree's two units meet. An onset is in beats
    and a length is in the unit of its own data
    (`clausters.form.element.Element.duration_unit`: a take's is seconds), and a
    timeline is ordered by **one** number — so the conversion belongs to the
    flattening and never to the structure. At the default tempo of one beat a
    second the two coincide, which is what a script that never set a tempo has
    always been running under.

    ``tempo_map`` (the piece's `clausters.base.TempoMap`, the clock's when
    there is one) is what the crossing goes through, so a length in seconds
    lands where it actually ends rather than where a single tempo would put it;
    ``tempo`` alone is that tempo as one segment.

    ``mixed`` is whether the composition's mixing is in force — mute, solo and
    level, all inherited down the tree. It is on for what sounds and off for
    what is **drawn**: a muted lane keeps its clips, its notes and its length,
    and a picture that emptied when the toggle was pressed would be reporting
    silence as absence."""
    out: list = []
    _emit(element, float(base), out, tempo_map=tempo_map_of(tempo_map, tempo),
          mix=_Mix.over(element, mixed))
    out.sort(key=lambda pair: pair[0])
    return out


def to_timeline(element, base: float = 0.0, *, tempo: float = 1.0, tempo_map=None,
                mixed: bool = True):
    """Flatten ``element`` into a flat `clausters.seq.Timeline` in absolute
    beats — the structure a `Playhead` plays and a transport seeks. ``tempo``
    is the clock's, in beats per second, and ``tempo_map`` its map when the
    tempo changes along the piece (see `flatten`)."""
    from ..seq.timeline import Timeline

    timeline = Timeline()
    for beat, item in flatten(element, base, tempo=tempo, tempo_map=tempo_map,
                              mixed=mixed):
        timeline.add(beat, item)
    return timeline


def render(element, destination, clock=None, *, at: float = 0.0, quant=None,
           ports=None):
    """Render ``element`` onto ``destination``.

    A **concrete** element (an `Aggregate`, `Track`, `Clang`, …) is flattened to
    a timeline and played through a `Playhead` over ``clock`` — RT (start/run the
    clock) or NRT (`clock.render()` then ``destination.render()``, or
    `Session.render`), sample-identical; returns the `Playhead`.

    A **logical** `Aggregate` is translated to a `clausters.defs.GraphDef`, sent
    (``/def_send graph``) and instanced (``/graph_new``, with ``ports`` overriding the
    surface defaults) on the `Server` ``destination``; returns the instance
    group. The seam is the destination, not the element.
    """
    if isinstance(element, Aggregate) and element.kind == LOGICAL:
        return render_logical(element, destination, ports=ports)

    from ..seq.timeline import Playhead

    if not isinstance(element, Aggregate) and element.wraps is None:
        raise ValueError(
            "an abstract element (no content) is pure context; it has nothing to render"
        )
    # The clock's own map, so what sounds and what an editor draws are measured
    # by one function rather than by two readings of it.
    tempo = float(getattr(clock, "tempo", 1.0) or 1.0)
    timeline = to_timeline(element, float(element.onset or 0.0), tempo=tempo,
                           tempo_map=getattr(clock, "map", None))
    playhead = Playhead(timeline, clock, destination)
    playhead.play(at=at, quant=quant)
    return playhead


def render_logical(aggregate, server, *, ports=None):
    """Send a logical aggregate's `GraphDef` (`Aggregate.to_graphdef`) and
    instance it on ``server``. Returns the instance group — a node-tree group,
    the handle from `clausters.defs.Group.graph`."""
    gdef = aggregate.to_graphdef()
    gdef.send(server)
    return NodeGroup.graph(gdef.name, ports, server=server)


# ---- mixing: what the composition says about being heard ----

class _Mix:
    """The mixing in force at one point of the walk: whether anything in the
    piece is soloed, whether this branch is, and the gain accumulated down to
    it.

    It is threaded through the walk rather than read off each element because
    all three are **inherited**: muting an aggregate silences its members, a
    lane's level multiplies its clips', and one soloed lane anywhere silences
    every branch that is not on a soloed path. A mute is the one that does not
    need threading -- it drops the branch where it is met.
    """

    __slots__ = ("soloing", "soloed", "gain", "honour")

    def __init__(self, soloing: bool, soloed: bool, gain: float,
                 honour: bool = True):
        self.soloing = soloing
        self.soloed = soloed
        self.gain = gain
        #: Whether the mix is in force at all. **Drawing reads the composition
        #: unmixed**: a muted lane still has its clips, its notes and its
        #: length, and a picture that vanished when the toggle was pressed
        #: would be reporting silence as absence. So a view flattens with
        #: ``mixed=False`` and what sounds flattens with the mix.
        self.honour = honour

    @classmethod
    def over(cls, element, mixed: bool = True) -> "_Mix":
        """The mix a whole piece starts under. Solo is piece-wide by
        definition -- it says *only these* -- so whether anything is soloed is
        a question about the tree and not about the element being walked."""
        return cls(mixed and _any_solo(element), False, 1.0, mixed)

    def silences(self, element) -> bool:
        """Whether this element's branch is dropped outright."""
        return self.honour and bool(getattr(element, "mute", False))

    def under(self, element) -> "_Mix":
        """The mix inside ``element``."""
        if not self.honour:
            return self
        level = float(getattr(element, "level", 1.0) or 0.0)
        soloed = self.soloed or bool(getattr(element, "solo", False))
        if soloed == self.soloed and level == 1.0:
            return self
        return _Mix(self.soloing, soloed, self.gain * level)

    def applied(self, item):
        """``item`` as it sounds under this mix, or ``None`` when it does not.

        The gain is written onto the event's ``amp`` — a **copy**, since the
        element's own event is shared and a mix must not rewrite it (the same
        rule `_sized` follows). Anything that is not an event carries no gain
        and passes through: an automation curve is a control signal, and
        scaling one is an edit of the curve rather than a mixer's business.
        """
        if not self.honour:
            return item
        if self.soloing and not self.soloed:
            return None
        if self.gain == 1.0:
            return item
        from ..seq.event import Event as SeqEvent

        if not isinstance(item, SeqEvent):
            return item
        return SeqEvent({**item, "amp": self.gain * float(item.get("amp", 1.0))})


def _any_solo(element) -> bool:
    """Whether anything in this tree is soloed."""
    if getattr(element, "solo", False):
        return True
    if isinstance(element, Aggregate):
        return any(_any_solo(handle.element) for handle in element.handles)
    if isinstance(element, Generator) and getattr(element, "rendered", None) is not None:
        return _any_solo(element.rendered)
    if isinstance(element, Sequence) and isinstance(element.wraps, (list, tuple)):
        return any(_any_solo(i) for i in element.wraps if isinstance(i, Element))
    return False


def _heard(out: list, beat: float, item, mix: _Mix):
    """Lay one item down at ``beat``, if this mix lets it be heard."""
    item = mix.applied(item)
    if item is not None:
        out.append((beat, item))


# ---- the flatten dispatch ----

def _emit(element, base: float, out: list, dur=None, *, tempo_map, mix: _Mix):
    """Flatten ``element`` at ``base``, honouring the **placement length** its
    aggregate gave it: a placement ``dur`` *trims* what the element plays (the DAW
    rule — a clip's length is what you hear of it), so events past the placement's
    end are dropped and a single-event element sounds for exactly that long. A
    placement with no length lets the element be its own.

    The placement's length is in the placed element's own unit — a clip of audio
    is trimmed in seconds — so it crosses to beats here, once, against the
    element it was written for."""
    if mix.silences(element):
        # A muted branch contributes nothing -- not its own events and not its
        # members'. It is the one part of the mix that needs no threading: it
        # is answered where it is met.
        return
    mix = mix.under(element)
    placed: list = []
    _emit_element(element, base, placed, tempo_map, mix)
    if dur is not None:
        # The placement's end, not its length turned into one: under a tempo
        # that changes, a length in seconds reaches a different beat depending
        # on where it starts, so the two positions are what say where it ends.
        end = end_beat(base, dur, element.duration_unit, tempo_map)
        dur = end - base
        placed = [(beat, _sized(item, min(dur, end - beat)))
                  for beat, item in placed if beat < end - 1e-9]
    out.extend(placed)


def _sized(item, dur: float):
    """An event resized to the placement's remaining length — a *copy*, since the
    element's own event is shared and must not be rewritten by a placement.
    Anything that is not an event (an automation, a raw OSC item) is untouched."""
    from ..seq.event import Event as SeqEvent

    if isinstance(item, SeqEvent) and item.get("dur") is not None:
        return SeqEvent({**item, "dur": float(dur)})
    return item


def _emit_element(element, base: float, out: list, tempo_map, mix: _Mix):
    if isinstance(element, Aggregate):
        if element.kind != CONCRETE:
            raise NotImplementedError(
                "a logical Aggregate is rendered as a GraphDef, not flattened"
            )
        for member in element.handles:
            _emit(member.element, base + member.offset, out, member.dur,
                  tempo_map=tempo_map, mix=mix)
    elif isinstance(element, Track):
        # `items()` and not the timeline: a track is a **window** onto it (a
        # trim reads from further in, a split gives two windows over one
        # timeline), so what sounds is what the window shows, placed from the
        # element's own zero. Without a window that is the whole timeline,
        # which is what a track written by a script is.
        for beat, item in element.items():
            _heard(out, base + beat, item, mix)
    elif isinstance(element, Clang):
        _heard(out, base, element.wraps, mix)
    elif isinstance(element, (Sequence, Generator)):
        _emit_sequence(element.wraps, base, out, tempo_map, mix)
    elif isinstance(element, Segments):
        # Several windows read as one thing: one event per segment, each at its
        # own offset inside the element and each carrying its own window, so
        # what sounds is continuous even though the source is not one buffer.
        # Without an instrument a run of *samples* is structure, exactly as a
        # `Vector` is -- a run of windows onto timelines needs none, because
        # what it holds are events that carry their own.
        if element.instrument is not None or element.duration_unit == BEATS:
            for offset, event in element.to_events(tempo_map, base):
                _heard(out, base + offset, event, mix)
    elif isinstance(element, Vector):
        # A buffer is data; the instrument is what makes it sound (a def whose
        # `buf` control plays it). Without one it is structure only — it draws in
        # the editor and contributes its extent, but emits no event.
        if element.instrument is not None:
            _heard(out, base, element.to_event(tempo_map, base), mix)
    elif isinstance(element, Element):
        if element.wraps is None:
            return  # an abstract context element yields no event
        if hasattr(element.wraps, "play"):
            _heard(out, base, element.wraps, mix)
        else:
            raise NotImplementedError(
                f"cannot render an element wrapping {type(element.wraps).__name__}"
            )
    else:
        raise TypeError(f"not an Element: {element!r}")


def _slot(item) -> float:
    """The **place in time** a flattened item takes, in beats: an event's
    ``dur``, which is its slot and not its sounding length (``sustain``), so a
    detached note still occupies the beat it was written on. Anything that is
    not an event is punctual."""
    from ..seq.event import Event as SeqEvent

    if isinstance(item, SeqEvent):
        try:
            return float(item.get("dur") or 0.0)
        except (TypeError, ValueError):
            return 0.0
    return 0.0


def _reaches(element, tempo_map) -> float:
    """How far an element with **no stated duration** reaches, in beats: the end
    of the last thing it lays down, from its own zero.

    Laid down **unmixed**, and that is the point rather than an economy: mute
    and solo say what is heard, never where anything is. Measuring what a mix
    let through would make soloing one lane re-time the sequence in another,
    which is the one thing a reader would never look for.
    """
    laid: list = []
    _emit_element(element, 0.0, laid, tempo_map, _Mix.over(element, False))
    return max((beat + _slot(item) for beat, item in laid), default=0.0)


def _emit_sequence(wrapped, base: float, out: list, tempo_map, mix: _Mix):
    """A List/Function backed by an event pattern is bounced; a list of elements
    is laid out successively — each by its own duration, or by what it lays
    down when it states none."""
    from ..seq.pattern import Pattern
    from ..seq.timeline import Timeline

    if wrapped is None or isinstance(wrapped, str):
        # A **frozen** generator: the document named an algorithm and nothing in
        # this process supplied one, so what came back is the reference itself
        # (or nothing at all). It is structure — it draws, it contributes its
        # extent — and it emits no event, exactly as a buffer with no instrument
        # does. Raising here instead would make a reopened session unplayable
        # because one lane in it was written by a script that is not running.
        return
    if isinstance(wrapped, Pattern):
        for beat, item in Timeline.from_pattern(wrapped):
            _heard(out, base + beat, item, mix)
    elif isinstance(wrapped, Timeline):
        for beat, item in wrapped:
            _heard(out, base + beat, item, mix)
    elif hasattr(wrapped, "play"):
        # Something that plays itself -- an automation curve, and whatever else a
        # script hands over. The conversion writes every element it has no body
        # for as a *generator* leaf, so resolving one back on open gives a
        # `Generator` where the author wrote a bare `Element`; the two must play
        # the same thing or a reopened piece would sound different from the one
        # that was saved.
        _heard(out, base, wrapped, mix)
    elif not isinstance(wrapped, (list, tuple)):
        # **A def is not a list of elements.** A generator wrapping a `SynthDef`
        # is a *resident* one -- the server produces its audio, and there is
        # nothing here to lay out -- and iterating it walks its controls by
        # name, which fails with a `KeyError` about control `0` rather than
        # saying what happened. Named here so the extent rule reads it as "no
        # events" and a render says what it cannot do.
        raise NotImplementedError(
            f"cannot flatten a generator wrapping {type(wrapped).__name__}"
        )
    else:
        cursor = base
        for item in wrapped:
            if not isinstance(item, Element):
                raise NotImplementedError(
                    "a Sequence of raw values is data (a parameter), not events"
                )
            _emit(item, cursor, out, tempo_map=tempo_map, mix=mix)
            # Laid out successively on the beat axis, so each length crosses
            # from whatever unit its own data is in. An item that states no
            # length is as long as **what it lays down** -- a `Sequence` of
            # `Sequence`s says nothing about its members' lengths, and reading a
            # missing one as zero stacked every one of them on the first beat.
            cursor = (end_beat(cursor, item.duration, item.duration_unit, tempo_map)
                      if item.duration is not None
                      else cursor + _reaches(item, tempo_map))
