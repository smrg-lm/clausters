"""Grouping — composite materials and the derived temporal relation.

A `Group` is the one genuinely new structure of the model: the recursive
placement of materials with an offset, and the temporal *relation* derived from
how the members sit in time. Everything else (the five primitives) already
exists and is merely adorned by `clausters.model.material.Material`.

Two kinds of grouping (§2.5):

- **compositional** — a structural/temporal relation between the contents (a
  section holding clips, a melody holding note-events), with no processing
  relation.
- **logical** — the contents relate by processing or generation logic (a
  bus-wired signal chain on the server, or a generative dependency on the
  client).

Realization is a Fase 1B/1C concern; this module is pure structure plus the
temporal-relation derivation (a pure function over the members' placements).
"""

import math

#: The kind of a `Group` (§2.5).
COMPOSITIONAL = "compositional"
LOGICAL = "logical"

#: The temporal relation between a group's members, derived from their placements
#: (§2.3). ``successive`` — duration-only, tiling contiguously; ``simultaneous``
#: — all starting and ending together (a container that can be reinterpreted,
#: enabling recursion); ``mixed`` — any other combination.
SUCCESSIVE = "successive"
SIMULTANEOUS = "simultaneous"
MIXED = "mixed"

from .material import Material  # noqa: E402  (constants first for the docstring)


class _Member:
    """One placed member of a `Group`. A stable object so it can be ``remove``d
    or ``move``d by identity after other edits shift things.

    ``offset`` is the member's start in beats relative to the group's context;
    ``dur`` is an explicit placement length that overrides the material's own
    ``duration`` when set.
    """

    __slots__ = ("offset", "dur", "material")

    def __init__(self, offset, dur, material):
        self.offset = float(offset)
        self.dur = None if dur is None else float(dur)
        self.material = material

    @property
    def length(self):
        """The effective length of this member: the placement ``dur`` if given,
        else the material's own ``duration`` (may be ``None``)."""
        return self.dur if self.dur is not None else self.material.duration


class Group(Material):
    """A composite material: a set of placed members with a grouping ``kind``.

    Members are placed by an ``offset`` (beats relative to the group's context)
    and an optional placement ``dur``. Edit freely — `add`, `remove`, `move`; a
    handle returned by `add` stays valid across other edits (like
    `clausters.seq.Timeline`).

    A `LOGICAL` group additionally names the composition and may declare internal
    buses; `to_graphdef` translates it into a `clausters.defs.GraphDef` (the
    bus-wired configuration the server already expresses).

    Args:
        children: optional iterable seeding the group. Each item is a
            ``(offset, material)`` pair, a ``(offset, dur, material)`` triple, or
            a bare `Material` (placed at offset 0).
        kind: `COMPOSITIONAL` (default) or `LOGICAL`.
        name: the composition's name — the GraphDef name for a logical group.
        buses: internal buses for a logical group — each a ``name`` (audio,
            1 channel) or a ``(name, rate)`` / ``(name, rate, channels)`` tuple.
        onset: the group's own onset in its parent context, or ``None``.
        duration: the group's own duration, or ``None``.
    """

    def __init__(self, children=None, kind=COMPOSITIONAL, *, name=None,
                 buses=None, onset=None, duration=None):
        super().__init__(wraps=None, onset=onset, duration=duration)
        if kind not in (COMPOSITIONAL, LOGICAL):
            raise ValueError(f"unknown group kind: {kind!r}")
        self.kind = kind
        self.name = name
        self._bus_specs = [_bus_spec(b) for b in (buses or [])]
        self._members = []
        if children is not None:
            for child in children:
                self._add_child(child)

    # ---- editing ----

    def _add_child(self, child):
        if isinstance(child, Material):
            self.add(child)
        elif len(child) == 2:
            offset, material = child
            self.add(material, offset)
        elif len(child) == 3:
            offset, dur, material = child
            self.add(material, offset, dur)
        else:
            raise ValueError(f"invalid child spec: {child!r}")

    def add(self, material, offset=0.0, dur=None):
        """Place ``material`` at ``offset`` (beats), optionally overriding its
        length with ``dur``. Returns a member handle for `remove`/`move`."""
        member = _Member(offset, dur, material)
        self._members.append(member)
        return member

    def remove(self, member):
        """Remove a member returned by `add` (by identity)."""
        self._members.remove(member)
        return self

    def move(self, member, offset, dur=None):
        """Reposition ``member`` to ``offset`` (and optionally set ``dur``)."""
        member.offset = float(offset)
        if dur is not None:
            member.dur = float(dur)
        return member

    def clear(self):
        """Drop every member."""
        self._members.clear()
        return self

    # ---- reading ----

    @property
    def members(self) -> list:
        """The members as ``(offset, dur, material)`` triples, insertion order."""
        return [(m.offset, m.dur, m.material) for m in self._members]

    def __len__(self):
        return len(self._members)

    def __iter__(self):
        return iter(self.members)

    # ---- the derived temporal relation ----

    def temporal_relation(self):
        """Derive this group's temporal relation (`SUCCESSIVE`/`SIMULTANEOUS`/
        `MIXED`) from its members' placements, or ``None`` when empty.

        - `SIMULTANEOUS`: every member starts and ends together (a single member
          trivially qualifies).
        - `SUCCESSIVE`: members tile contiguously in time — sorted by start, each
          member begins exactly where the previous ends (requires known lengths).
        - `MIXED`: anything else.
        """
        members = self._members
        if not members:
            return None

        starts = [m.offset for m in members]
        lengths = [m.length for m in members]
        ends = [
            s + length if length is not None else None
            for s, length in zip(starts, lengths)
        ]

        if _all_close(starts) and _ends_all_close(ends):
            return SIMULTANEOUS

        if all(length is not None for length in lengths):
            ordered = sorted(zip(starts, lengths))
            if all(
                math.isclose(ordered[i][0], ordered[i - 1][0] + ordered[i - 1][1])
                for i in range(1, len(ordered))
            ):
                return SUCCESSIVE

        return MIXED

    # ---- the logical realization: a GraphDef (Fase 1C) ----

    def to_graphdef(self, name=None):
        """Translate this **logical** group into a `clausters.defs.GraphDef` — the
        1:1 mapping of the model's logical grouping (nodes wired by sender/
        receiver buses) onto the configuration the server already expresses.

        Each member must be a `clausters.model.material.Generator` (its
        ``def_name`` is the member def; its ``controls`` — numbers, an internal
        bus name, or ``"OUT"`` — and ``maps`` wire it). The group's `buses` become
        the private internal buses. Placement offsets are ignored (a logical group
        is a signal graph, not a timeline). Returns the `GraphDef`; sending and
        instancing it is `clausters.model.realize`.
        """
        from ..defs.graphdef import GraphDef
        from .material import Generator

        gname = name or self.name
        if gname is None:
            raise ValueError("a logical Group needs a name to become a GraphDef")
        gdef = GraphDef(gname)
        refs = {
            spec["name"]: gdef.bus(
                spec["name"], rate=spec["rate"], channels=spec["channels"]
            )
            for spec in self._bus_specs
        }
        for _offset, _dur, child in self.members:
            if not isinstance(child, Generator):
                raise TypeError(
                    "a logical Group member must be a Generator, "
                    f"got {type(child).__name__}"
                )
            controls = {
                key: (refs.get(value, value) if isinstance(value, str) else value)
                for key, value in (child.controls or {}).items()
            }
            gdef.add(child.def_name, controls, maps=child.maps)
        return gdef


def _bus_spec(bus) -> dict:
    """Normalize a `Group` bus declaration (a name, or a ``(name, rate[,
    channels])`` tuple) into the dict `to_graphdef` consumes."""
    if isinstance(bus, str):
        name, rate, channels = bus, "audio", 1
    elif len(bus) == 2:
        (name, rate), channels = bus, 1
    else:
        name, rate, channels = bus
    return {"name": str(name), "rate": rate, "channels": int(channels)}


def _all_close(values) -> bool:
    """True when every value is close to the first (a float-safe all-equal)."""
    return all(math.isclose(v, values[0]) for v in values)


def _ends_all_close(ends) -> bool:
    """All-equal for member ends where a ``None`` end (unknown length) counts as
    equal only when every end is ``None``."""
    if all(e is None for e in ends):
        return True
    if any(e is None for e in ends):
        return False
    return _all_close(ends)
