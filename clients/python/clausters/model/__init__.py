"""The compositional model (*the model*) — the client-side conceptual layer.

A recursive algebra of materials for composing music: the five primitives
(`Event`, `Sequence`, `Buffer`, `Track`, `Generator`) as thin adornments over
the objects the client already has, and `Group` — the one new structure —
placing materials recursively with an offset and deriving their temporal
relation. Pure and transport-agnostic; realization onto the server/NRT is a
later phase.

See `clausters.model.material` for the primitives and the temporal *character*,
and `clausters.model.group` for grouping and the temporal *relation*.
"""

from .material import (
    ABSTRACT,
    PUNCTUAL,
    RELATIVE,
    SEGMENT,
    Buffer,
    Event,
    Generator,
    Material,
    Sequence,
    Track,
    temporal_character,
)
from .group import (
    COMPOSITIONAL,
    LOGICAL,
    MIXED,
    SIMULTANEOUS,
    SUCCESSIVE,
    Group,
)

__all__ = [
    "Material",
    "Event",
    "Sequence",
    "Buffer",
    "Track",
    "Generator",
    "Group",
    "temporal_character",
    # temporal character
    "SEGMENT",
    "PUNCTUAL",
    "RELATIVE",
    "ABSTRACT",
    # group kind
    "COMPOSITIONAL",
    "LOGICAL",
    # temporal relation
    "SUCCESSIVE",
    "SIMULTANEOUS",
    "MIXED",
]
