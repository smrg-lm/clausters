"""Rendering — the *change of state* from the arrangement to sound.

A concrete `Group` is rendered by **flattening** it: a tree-walk that
accumulates the nested placement offsets into absolute beats, producing a flat
`clausters.seq.Timeline` of items that each know how to `play(destination)`. That
timeline is then played by a `clausters.seq.Playhead` — RT (timetagged bundles)
or NRT (a score for `Session.render`) purely by which destination and clock it
holds, sample-identical, with no scheduling path of its own. This mirrors
`Timeline.from_pattern`: the arrangement reuses the sequencing layer rather than
duplicating it.

Scope of this phase (the concrete path):

- `Group{concrete}` — flattened recursively; each member's ``offset`` (and
  any nested group's) accumulates into the child's absolute beat.
- `Track` — its `Timeline`'s items are shifted by the placement beat.
- `Event` — placed as a single item at its beat.
- `Sequence`/`Generator` wrapping an **event pattern** (a `Pbind`) — *bounced*
  in the same pass (its change of state); a `Sequence` of elements is laid out
  successively by their durations.
- An **abstract** element (no onset/duration, no content) contributes context,
  not an event.

A `Buffer` is *data*: it sounds through the **instrument** that plays it (a def
whose ``buf`` control takes the buffer number), so a `Buffer` with an
``instrument`` emits one event playing it — the audio clip — and one without
contributes structure only. A `Group{logical}` takes the other path entirely (it
becomes a `GraphDef`); instancing a bare def still needs an instrument of its own
and raises a clear `NotImplementedError` here.
"""

from ..defs.node import Group as NodeGroup
from .group import CONCRETE, LOGICAL, Group
from .element import Buffer, Element, Event, Generator, Sequence, Track


def flatten(element, base: float = 0.0) -> list:
    """Flatten ``element`` into ``(absolute_beat, item)`` pairs, sorted by beat,
    accumulating nested placement offsets onto ``base``. The items are playable
    (they follow the ``play(destination)`` protocol)."""
    out: list = []
    _emit(element, float(base), out)
    out.sort(key=lambda pair: pair[0])
    return out


def to_timeline(element, base: float = 0.0):
    """Flatten ``element`` into a flat `clausters.seq.Timeline` in absolute
    beats — the structure a `Playhead` plays and a transport seeks."""
    from ..seq.timeline import Timeline

    timeline = Timeline()
    for beat, item in flatten(element, base):
        timeline.add(beat, item)
    return timeline


def render(element, destination, clock=None, *, at: float = 0.0, quant=None,
           ports=None):
    """Render ``element`` onto ``destination``.

    A **concrete** element (a `Group`, `Track`, `Event`, …) is flattened to
    a timeline and played through a `Playhead` over ``clock`` — RT (start/run the
    clock) or NRT (`clock.render()` then ``destination.render()``, or
    `Session.render`), sample-identical; returns the `Playhead`.

    A **logical** `Group` is translated to a `clausters.defs.GraphDef`, sent
    (``/def_send graph``) and instanced (``/graph_new``, with ``ports`` overriding the
    surface defaults) on the `Server` ``destination``; returns the instance
    group. The seam is the destination, not the element.
    """
    if isinstance(element, Group) and element.kind == LOGICAL:
        return render_logical(element, destination, ports=ports)

    from ..seq.timeline import Playhead

    if not isinstance(element, Group) and element.wraps is None:
        raise ValueError(
            "an abstract element (no content) is pure context; it has nothing to render"
        )
    timeline = to_timeline(element, float(element.onset or 0.0))
    playhead = Playhead(timeline, clock, destination)
    playhead.play(at=at, quant=quant)
    return playhead


def render_logical(group, server, *, ports=None):
    """Send a logical group's `GraphDef` (`Group.to_graphdef`) and instance it on
    ``server``. Returns the instance group (the handle from
    `clausters.defs.Group.graph`)."""
    gdef = group.to_graphdef()
    gdef.send(server)
    return NodeGroup.graph(gdef.name, ports, server=server)


# ---- the flatten dispatch ----

def _emit(element, base: float, out: list, dur=None):
    """Flatten ``element`` at ``base``, honouring the **placement length** its
    group gave it: a placement ``dur`` *trims* what the element plays (the DAW
    rule — a clip's length is what you hear of it), so events past the placement's
    end are dropped and a single-event element sounds for exactly that long. A
    placement with no length lets the element be its own."""
    placed: list = []
    _emit_element(element, base, placed)
    if dur is not None:
        end = base + float(dur)
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


def _emit_element(element, base: float, out: list):
    if isinstance(element, Group):
        if element.kind != CONCRETE:
            raise NotImplementedError(
                "a logical Group is rendered as a GraphDef, not flattened"
            )
        for member in element.handles:
            _emit(member.element, base + member.offset, out, member.dur)
    elif isinstance(element, Track):
        for beat, item in element.wraps:
            out.append((base + beat, item))
    elif isinstance(element, Event):
        out.append((base, element.wraps))
    elif isinstance(element, (Sequence, Generator)):
        _emit_sequence(element.wraps, base, out)
    elif isinstance(element, Buffer):
        # A buffer is data; the instrument is what makes it sound (a def whose
        # `buf` control plays it). Without one it is structure only — it draws in
        # the editor and contributes its extent, but emits no event.
        if element.instrument is not None:
            out.append((base, element.to_event()))
    elif isinstance(element, Element):
        if element.wraps is None:
            return  # an abstract context element yields no event
        if hasattr(element.wraps, "play"):
            out.append((base, element.wraps))
        else:
            raise NotImplementedError(
                f"cannot render an element wrapping {type(element.wraps).__name__}"
            )
    else:
        raise TypeError(f"not an Element: {element!r}")


def _emit_sequence(wrapped, base: float, out: list):
    """A List/Function backed by an event pattern is bounced; a list of elements
    is laid out successively by their durations."""
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
            out.append((base + beat, item))
    elif isinstance(wrapped, Timeline):
        for beat, item in wrapped:
            out.append((base + beat, item))
    elif hasattr(wrapped, "play"):
        # Something that plays itself -- an automation curve, and whatever else a
        # script hands over. The conversion writes every element it has no body
        # for as a *generator* leaf, so resolving one back on open gives a
        # `Generator` where the author wrote a bare `Element`; the two must play
        # the same thing or a reopened piece would sound different from the one
        # that was saved.
        out.append((base, wrapped))
    else:
        cursor = base
        for item in wrapped:
            if not isinstance(item, Element):
                raise NotImplementedError(
                    "a Sequence of raw values is data (a parameter), not events"
                )
            _emit(item, cursor, out)
            cursor += item.duration or 0.0
