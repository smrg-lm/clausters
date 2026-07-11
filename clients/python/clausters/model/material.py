"""The compositional model — materials and their temporal character.

This is *the model* (see ``third_party/modelo.pdf``): the client-side conceptual
layer under a multitrack editor of recursive granularity. A `Material` is an
arbitrarily delimited entity that produces a unit of meaning and can be
decomposed or combined. It is a **thin adornment** over the objects the client
already has (`clausters.seq.Event`, `clausters.seq.Timeline`, a `Buffer`, a
`Pattern`, a def): it carries the temporal metadata (`onset`, `duration`, and the
derived temporal *character*) and belongs to a `Group`, while it **delegates
realization** to the wrapped item's ``play(destination)`` — the double-dispatch
seam every leaf item in the client already shares. The model does not
reimplement or subclass those objects.

The five primitives (§2.4 of the design note) map one-to-one onto what exists:

- `Event`     — *event/clip*: parameters grouped into one action (internally
  simultaneous), with its own onset/duration. Wraps `clausters.seq.Event`.
- `Sequence`  — *List*: strict order with no concrete time, only sequence.
  Wraps a Python list or a `Pattern`.
- `Buffer`    — *Buffer*: a list at constant time (audio or control samples).
  Wraps `clausters.defs.Buffer`.
- `Track`     — *Set*: mixed placement of materials, a DAW track. Wraps
  `clausters.seq.Timeline`.
- `Generator` — *Function*: logical/generator material — server DSP (a def) or a
  sequence generator (`Pbind`/`Routine`).

Grouping and realization live in `clausters.model.group`. This module is pure
and transport-agnostic (factorable into ``clausters-core`` in a future port).
"""

#: The temporal character of a material, derived from which of ``onset`` and
#: ``duration`` are present (§2.3). ``segment`` has both; ``punctual`` has an
#: onset but no duration; ``relative`` has a duration but no onset; ``abstract``
#: has neither (a pure context/container that only a parent gives concrete time).
SEGMENT = "segment"
PUNCTUAL = "punctual"
RELATIVE = "relative"
ABSTRACT = "abstract"


def temporal_character(onset, duration) -> str:
    """The temporal character for a given ``onset``/``duration`` pair (the pure
    rule behind `Material.temporal_character`)."""
    has_onset = onset is not None
    has_duration = duration is not None
    if has_onset and has_duration:
        return SEGMENT
    if has_onset:
        return PUNCTUAL
    if has_duration:
        return RELATIVE
    return ABSTRACT


class Material:
    """Base of the compositional model: temporal metadata over a wrapped item.

    A material carries an optional ``onset`` and ``duration`` (in beats, relative
    to its context) and wraps an underlying client object it delegates to. The
    concrete onset of a material typically comes from its *placement* inside a
    `clausters.model.group.Group`, not from the material itself, so a standalone
    leaf commonly has a duration but no onset (a ``relative`` character).

    Args:
        wraps: the underlying object realization delegates to (or ``None`` for a
            pure container like a `Group`).
        onset: start in beats relative to the context, or ``None``.
        duration: length in beats, or ``None``.
    """

    def __init__(self, wraps=None, onset=None, duration=None):
        self.wraps = wraps
        self.onset = None if onset is None else float(onset)
        self.duration = None if duration is None else float(duration)

    @property
    def temporal_character(self) -> str:
        """This material's character (`SEGMENT`/`PUNCTUAL`/`RELATIVE`/`ABSTRACT`),
        derived from the presence of ``onset`` and ``duration``."""
        return temporal_character(self.onset, self.duration)

    def play(self, destination):
        """Delegate realization to the wrapped item's ``play(destination)`` — the
        double-dispatch seam shared by `clausters.seq.Event`,
        `clausters.seq.timeline.OscEvent`/`MidiEvent` and
        `clausters.seq.Automation`.

        Container and pattern-backed materials (`Group`, `Track`, a `Sequence`
        wrapping a `Pattern`) are **not** directly playable this way — they are
        realized by ``realize()`` (Fase 1B). Delegating here requires the wrapped
        object to follow the ``play(destination)`` protocol.
        """
        play = getattr(self.wraps, "play", None)
        if play is None:
            raise NotImplementedError(
                f"{type(self).__name__} is not directly playable; use realize()"
            )
        return play(destination)

    def to_timeline(self, base: float = 0.0):
        """Flatten this material to a flat `clausters.seq.Timeline` in absolute
        beats (accumulating nested placement offsets). See
        `clausters.model.realize`."""
        from .realize import to_timeline

        return to_timeline(self, base)

    def realize(self, destination, clock=None, *, at: float = 0.0, quant=None,
                ports=None):
        """Realize this material onto ``destination`` — the change of state to
        sound. A compositional material flattens and plays through a
        `clausters.seq.Playhead` over ``clock`` (returns the playhead); a logical
        `Group` sends and instances a `GraphDef` on the server (returns the
        instance). See `clausters.model.realize.realize`."""
        from .realize import realize

        return realize(self, destination, clock, at=at, quant=quant, ports=ports)


class Event(Material):
    """*event/clip*: parameters grouped into one action, internally simultaneous.

    Wraps a `clausters.seq.Event` (or a plain ``dict`` of parameters). Its
    ``duration`` defaults to the event's ``dur`` when not given explicitly; its
    ``onset`` usually comes from its placement in a `Group`.
    """

    def __init__(self, event, onset=None, duration=None):
        from ..seq.event import Event as SeqEvent

        wrapped = event if isinstance(event, SeqEvent) else SeqEvent(event)
        if duration is None:
            dur = wrapped.get("dur")
            if dur is not None:
                duration = float(dur)
        super().__init__(wraps=wrapped, onset=onset, duration=duration)


class Sequence(Material):
    """*List*: strict order with no concrete time — only sequence.

    Wraps a Python list or a `clausters.seq.pattern.Pattern`. The elements can be
    numbers, events, notes or whole materials; the structure fixes only their
    successive order. Realized in Fase 1B (a pattern-backed sequence is bounced;
    a list is interpreted by its content).
    """

    def __init__(self, items, onset=None, duration=None):
        super().__init__(wraps=items, onset=onset, duration=duration)


class Buffer(Material):
    """*Buffer*: a list at constant time — audio or control samples.

    Wraps a `clausters.defs.Buffer`. An automation sampled at a constant interval
    is a control buffer (the List/Buffer duality of the model).
    """

    def __init__(self, buffer, onset=None, duration=None):
        super().__init__(wraps=buffer, onset=onset, duration=duration)


class Track(Material):
    """*Set*: mixed placement of materials — a DAW track.

    Wraps a `clausters.seq.Timeline` (free placement of items by beat). A fresh
    empty `Timeline` is created when none is given.
    """

    def __init__(self, timeline=None, onset=None, duration=None):
        if timeline is None:
            from ..seq.timeline import Timeline

            timeline = Timeline()
        super().__init__(wraps=timeline, onset=onset, duration=duration)


class Generator(Material):
    """*Function*: logical/generator material.

    Wraps either server DSP (a `SynthDef`/`FaustDef`/`GraphDef`, or a def name)
    or a sequence generator (a `Pbind`/`Routine`). Its *change of state* —
    evaluating the generator into concrete material — happens at realization: a
    contained event pattern is bounced to a timeline (Fase 1B); a def member of a
    logical `Group` becomes a wired GraphDef member (Fase 1C).

    Args:
        generator: the wrapped def (name or object) or sequence generator.
        controls: control values for a logical-graph member — numbers, an
            internal-bus name (a ``str`` matching a `Group` bus), or ``"OUT"``
            (hardware). Used by `Group.to_graphdef`.
        maps: control-bus bindings for a logical-graph member (``/n_map``),
            ``{control: bus_name}``.
    """

    def __init__(self, generator, onset=None, duration=None, *, controls=None, maps=None):
        super().__init__(wraps=generator, onset=onset, duration=duration)
        self.controls = controls
        self.maps = maps

    @property
    def def_name(self) -> str:
        """The member def name — the wrapped string itself, or the def object's
        ``name``."""
        return self.wraps if isinstance(self.wraps, str) else self.wraps.name
