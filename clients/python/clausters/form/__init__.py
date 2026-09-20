"""The **arrangement** -- the client-side layer under the multitrack editor.

A recursive algebra of elements for composing music: the five primitives
(`Clang`, `Sequence`, `Vector` -- with `Segments`, the same primitive over
several windows -- `Track`, `Generator`) as thin adornments over
the objects the client already has, and `Aggregate` -- the one new structure --
placing elements recursively with an offset and deriving their temporal
relation. An element is *generated* (the rendered thing: random-access, editable)
or a *generator* (the algorithm that renders it: forward-only), and evaluating the
second into the first is the **change of state** rendering performs. Pure and
transport-agnostic; the multitrack view of it lives in `clausters.gui.editing`.

See `clausters.form.element` for the primitives and the temporal *character*,
`clausters.form.aggregate` for grouping and the temporal *relation*, and
`clausters.form.render` for the change of state to sound.

**This module has no door to the shared document.** It had one -- a bridge that
converted these elements to the crate's JSON -- and it was removed on 2026-09-06
with the turn that made the arrangement a model of its own. What a multitrack is
written with now is `clausters.multitrack`, and the crate is reached through
`clausters.document`. Nothing here converts, and nothing here is designed
around: this is a frozen, secondary module of data structures.
"""

from .element import (
    ABSTRACT,
    BEATS,
    SECONDS,
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
    take,
    temporal_character,
    to_beats,
)
from .aggregate import (
    CONCRETE,
    LOGICAL,
    MIXED,
    SIMULTANEOUS,
    SUCCESSIVE,
    Aggregate,
)
from .render import flatten, render, render_logical, to_timeline

__all__ = [
    "Element",
    "Clang",
    "Sequence",
    "Vector",
    "take",
    "Segment",
    "Segments",
    "Track",
    "Generator",
    "Aggregate",
    "temporal_character",
    "to_beats",
    "flatten",
    "to_timeline",
    "render",
    "render_logical",
    # the unit a length is in
    "BEATS",
    "SECONDS",
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
