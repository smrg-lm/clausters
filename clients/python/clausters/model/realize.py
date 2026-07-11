"""Realization — the *change of state* from the model to sound (Fase 1B).

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

The logical path (`Group{logical}` → a `GraphDef`) is Fase 1C; a `Buffer` as a
timed audio clip and instancing a bare def both need an instrument and land in a
later phase — they raise a clear `NotImplementedError` here.
"""

from .group import COMPOSITIONAL, Group
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


def realize(material, destination, clock, *, at: float = 0.0, quant=None):
    """Realize ``material`` onto ``destination`` (a `Server` or a MIDI
    destination) over ``clock``: flatten it to a timeline and play it through a
    `Playhead`. Returns the `Playhead`.

    Live: start/run the clock. Offline: `clock.render()` then
    ``destination.render()`` (or use `Session.render`). Same bytes either way —
    the seam is the destination and clock, not the model.
    """
    from ..seq.timeline import Playhead

    if not isinstance(material, Group) and material.wraps is None:
        raise ValueError(
            "an abstract material (no content) is pure context; it has no realization"
        )
    timeline = to_timeline(material, float(material.onset or 0.0))
    playhead = Playhead(timeline, clock, destination)
    playhead.play(at=at, quant=quant)
    return playhead


# ---- the flatten dispatch ----

def _emit(material, base: float, out: list):
    if isinstance(material, Group):
        if material.kind != COMPOSITIONAL:
            raise NotImplementedError(
                "realizing a logical Group is Fase 1C (it emits a GraphDef)"
            )
        for offset, _dur, child in material.members:
            _emit(child, base + offset, out)
    elif isinstance(material, Track):
        for beat, item in material.wraps:
            out.append((base + beat, item))
    elif isinstance(material, Event):
        out.append((base, material.wraps))
    elif isinstance(material, (Sequence, Generator)):
        _emit_sequence(material.wraps, base, out)
    elif isinstance(material, Buffer):
        raise NotImplementedError(
            "a Buffer as a timed audio clip needs an instrument (later phase)"
        )
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
