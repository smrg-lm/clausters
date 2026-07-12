"""The **arrangement model** — the client-side layer under the multitrack editor.

A recursive algebra of materials for composing music: the five primitives
(`Event`, `Sequence`, `Buffer`, `Track`, `Generator`) as thin adornments over
the objects the client already has, and `Group` — the one new structure —
placing materials recursively with an offset and deriving their temporal
relation. A material is *generated* (the rendered thing: random-access, editable)
or a *generator* (the algorithm that renders it: forward-only), and evaluating the
second into the first is the **change of state** realization performs. Pure and
transport-agnostic; the multitrack view of it lives in `clausters.gui.editor`.

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
from .realize import flatten, realize, realize_logical, to_timeline

__all__ = [
    "Material",
    "Event",
    "Sequence",
    "Buffer",
    "Track",
    "Generator",
    "Group",
    "temporal_character",
    "flatten",
    "to_timeline",
    "realize",
    "realize_logical",
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
