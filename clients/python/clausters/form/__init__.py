"""The **arrangement** — the client-side layer under the multitrack editor.

A recursive algebra of elements for composing music: the five primitives
(`Clang`, `Sequence`, `Vector` — with `Segments`, the same primitive over
several windows — `Track`, `Generator`) as thin adornments over
the objects the client already has, and `Aggregate` — the one new structure —
placing elements recursively with an offset and deriving their temporal
relation. An element is *generated* (the rendered thing: random-access, editable)
or a *generator* (the algorithm that renders it: forward-only), and evaluating the
second into the first is the **change of state** rendering performs. Pure and
transport-agnostic; the multitrack view of it lives in `clausters.gui.editor`.

See `clausters.form.element` for the primitives and the temporal *character*,
`clausters.form.aggregate` for grouping and the temporal *relation*, and
`clausters.form.render` for the change of state to sound.
"""

from .element import (
    ABSTRACT,
    PUNCTUAL,
    RELATIVE,
    SEGMENT,
    Element,
    Generator,
    Clang,
    Segment,
    Segments,
    Sequence,
    Track,
    Vector,
    temporal_character,
)
from .aggregate import (
    CONCRETE,
    LOGICAL,
    MIXED,
    SIMULTANEOUS,
    SUCCESSIVE,
    Aggregate,
)
from .document import (FIRST_VERSION, SESSION_FORMAT, from_document,
                       from_session, to_document, to_session)
from .render import flatten, render, render_logical, to_timeline

__all__ = [
    "Element",
    "FIRST_VERSION",
    "to_document",
    "from_document",
    "SESSION_FORMAT",
    "to_session",
    "from_session",
    "Clang",
    "Sequence",
    "Vector",
    "Segment",
    "Segments",
    "Track",
    "Generator",
    "Aggregate",
    "temporal_character",
    "flatten",
    "to_timeline",
    "render",
    "render_logical",
    # temporal character
    "SEGMENT",
    "PUNCTUAL",
    "RELATIVE",
    "ABSTRACT",
    # aggregate kind
    "CONCRETE",
    "LOGICAL",
    # temporal relation
    "SUCCESSIVE",
    "SIMULTANEOUS",
    "MIXED",
]
