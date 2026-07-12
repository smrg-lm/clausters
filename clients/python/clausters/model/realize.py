"""Realization — the *change of state* from the arrangement model to sound.

A compositional `Group` is realized by **flattening** it: a tree-walk that
accumulates the nested placement offsets into absolute beats, producing a flat
`clausters.seq.Timeline` of items that each know how to `play(destination)`. That
timeline is then played by a `clausters.seq.Playhead` — RT (timetagged bundles)
or NRT (a score for `Session.render`) purely by which destination and clock it
holds, sample-identical, with no realization path of its own. This mirrors
`Timeline.from_pattern`: the model reuses the sequencing layer rather than
duplicating it.

Scope of this phase (the compositional path):

- `Group{compositional}` — flattened recursively; each member's ``offset`` (and
  any nested group's) accumulates into the child's absolute beat.
- `Track` — its `Timeline`'s items are shifted by the placement beat.
- `Event` — placed as a single item at its beat.
- `Sequence`/`Generator` wrapping an **event pattern** (a `Pbind`) — *bounced*
  in the same pass (its change of state); a `Sequence` of materials is laid out
  successively by their durations.
- An **abstract** material (no onset/duration, no content) contributes context,
  not an event.

A `Buffer` is *data*: it sounds through the **instrument** that plays it (a def
whose ``buf`` control takes the buffer number), so a `Buffer` with an
``instrument`` emits one event playing it — the audio clip — and one without
contributes structure only. The logical path (`Group{logical}` → a `GraphDef`) is
the logical path; instancing a bare def still needs an instrument of its own and raises a
clear `NotImplementedError` here.
"""

from .group import COMPOSITIONAL, LOGICAL, Group
from .material import Buffer, Event, Generator, Material, Sequence, Track


def flatten(material, base: float = 0.0) -> list:
    """Flatten ``material`` into ``(absolute_beat, item)`` pairs, sorted by beat,
    accumulating nested placement offsets onto ``base``. The items are playable
    (they follow the ``play(destination)`` protocol)."""
    out: list = []
    _emit(material, float(base), out)
    out.sort(key=lambda pair: pair[0])
    return out


def to_timeline(material, base: float = 0.0):
    """Flatten ``material`` into a flat `clausters.seq.Timeline` in absolute
    beats — the structure a `Playhead` plays and a transport seeks."""
    from ..seq.timeline import Timeline

    timeline = Timeline()
    for beat, item in flatten(material, base):
        timeline.add(beat, item)
    return timeline


def realize(material, destination, clock=None, *, at: float = 0.0, quant=None,
            ports=None):
    """Realize ``material`` onto ``destination``.

    A **compositional** material (a `Group`, `Track`, `Event`, …) is flattened to
    a timeline and played through a `Playhead` over ``clock`` — RT (start/run the
    clock) or NRT (`clock.render()` then ``destination.render()``, or
    `Session.render`), sample-identical; returns the `Playhead`.

    A **logical** `Group` is translated to a `clausters.defs.GraphDef`, sent
    (``/d_graph``) and instanced (``/graph_new``, with ``ports`` overriding the
    surface defaults) on the `Server` ``destination``; returns the instance
    group. The seam is the destination, not the model.
    """
    if isinstance(material, Group) and material.kind == LOGICAL:
        return realize_logical(material, destination, ports=ports)

    from ..seq.timeline import Playhead

    if not isinstance(material, Group) and material.wraps is None:
        raise ValueError(
            "an abstract material (no content) is pure context; it has no realization"
        )
    timeline = to_timeline(material, float(material.onset or 0.0))
    playhead = Playhead(timeline, clock, destination)
    playhead.play(at=at, quant=quant)
    return playhead


def realize_logical(group, server, *, ports=None):
    """Send a logical group's `GraphDef` (`Group.to_graphdef`) and instance it on
    ``server``. Returns the instance group (`server.graph`'s handle)."""
    gdef = group.to_graphdef()
    server.add_graphdef(gdef)
    return server.graph(gdef.name, ports)


# ---- the flatten dispatch ----

def _emit(material, base: float, out: list, dur=None):
    """Flatten ``material`` at ``base``, honouring the **placement length** its
    group gave it: a placement ``dur`` *trims* what the material plays (the DAW
    rule — a clip's length is what you hear of it), so events past the placement's
    end are dropped and a single-event material sounds for exactly that long. A
    placement with no length lets the material be its own."""
    placed: list = []
    _emit_material(material, base, placed)
    if dur is not None:
        end = base + float(dur)
        placed = [(beat, _sized(item, min(dur, end - beat)))
                  for beat, item in placed if beat < end - 1e-9]
    out.extend(placed)


def _sized(item, dur: float):
    """An event resized to the placement's remaining length — a *copy*, since the
    material's own event is shared and must not be rewritten by a placement.
    Anything that is not an event (an automation, a raw OSC item) is untouched."""
    from ..seq.event import Event as SeqEvent

    if isinstance(item, SeqEvent) and item.get("dur") is not None:
        return SeqEvent({**item, "dur": float(dur)})
    return item


def _emit_material(material, base: float, out: list):
    if isinstance(material, Group):
        if material.kind != COMPOSITIONAL:
            raise NotImplementedError(
                "a logical Group is realized as a GraphDef, not flattened"
            )
        for member in material.handles:
            _emit(member.material, base + member.offset, out, member.dur)
    elif isinstance(material, Track):
        for beat, item in material.wraps:
            out.append((base + beat, item))
    elif isinstance(material, Event):
        out.append((base, material.wraps))
    elif isinstance(material, (Sequence, Generator)):
        _emit_sequence(material.wraps, base, out)
    elif isinstance(material, Buffer):
        # A buffer is data; the instrument is what makes it sound (a def whose
        # `buf` control plays it). Without one it is structure only — it draws in
        # the editor and contributes its extent, but emits no event.
        if material.instrument is not None:
            out.append((base, material.to_event()))
    elif isinstance(material, Material):
        if material.wraps is None:
            return  # an abstract context material yields no event
        if hasattr(material.wraps, "play"):
            out.append((base, material.wraps))
        else:
            raise NotImplementedError(
                f"cannot realize a material wrapping {type(material.wraps).__name__}"
            )
    else:
        raise TypeError(f"not a Material: {material!r}")


def _emit_sequence(wrapped, base: float, out: list):
    """A List/Function backed by an event pattern is bounced; a list of materials
    is laid out successively by their durations."""
    from ..seq.pattern import Pattern
    from ..seq.timeline import Timeline

    if isinstance(wrapped, Pattern):
        for beat, item in Timeline.from_pattern(wrapped):
            out.append((base + beat, item))
    elif isinstance(wrapped, Timeline):
        for beat, item in wrapped:
            out.append((base + beat, item))
    else:
        cursor = base
        for item in wrapped:
            if not isinstance(item, Material):
                raise NotImplementedError(
                    "a Sequence of raw values is data (a parameter), not events"
                )
            _emit(item, cursor, out)
            cursor += item.duration or 0.0
