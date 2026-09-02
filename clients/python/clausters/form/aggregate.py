"""The arrangement — grouping, and the derived temporal relation.

A `Aggregate` is the one genuinely new structure of the arrangement: the recursive
placement of elements with an offset, and the temporal *relation* derived from
how the members sit in time. Everything else (the five primitives) already
exists and is merely adorned by `clausters.form.element.Element`.

Two kinds of grouping:

- **concrete** — the members relate in time (a section holding clips, a melody
  holding note-events), with no processing relation.
- **logical** — the members relate by processing or generation logic (a
  bus-wired signal chain on the server, or a generative dependency on the
  client).

Rendering lives in `clausters.form.render`; this module is pure structure plus the
temporal-relation derivation (a pure function over the members' placements).
"""

import math

#: The kind of an `Aggregate`.
CONCRETE = "concrete"
LOGICAL = "logical"

#: The temporal relation between an aggregate's members, derived from their placements
#: ``successive`` — duration-only, tiling contiguously; ``simultaneous``
#: — all starting and ending together (a container that can be reinterpreted,
#: enabling recursion); ``mixed`` — any other combination.
SUCCESSIVE = "successive"
SIMULTANEOUS = "simultaneous"
MIXED = "mixed"

from .element import BEATS, Element, to_beats  # noqa: E402  (constants first for the docstring)


class _Member:
    """One placed member of an `Aggregate`. A stable object so it can be ``remove``d
    or ``move``d by identity after other edits shift things.

    ``offset`` is the member's start in beats relative to the aggregate's context;
    ``dur`` is an explicit placement length that overrides the element's own
    ``duration`` when set, **in the element's own unit**
    (`clausters.form.element.Element.duration_unit`: seconds for a take, beats
    for a phrase of events) — trimming a recording states seconds, and placing
    it states beats.

    **A handle is what carries the node id**, which is what makes one element
    placeable twice: a clip is a window onto an element, so the thing an edit
    names is the window and not the element behind it. The conversion stamps
    it here (`clausters.form.document`), which is why this class has a slot for
    something no caller sets.
    """

    __slots__ = ("offset", "dur", "element", "_doc_id")

    def __init__(self, offset, dur, element):
        self.offset = float(offset)
        self.dur = None if dur is None else float(dur)
        self.element = element

    @property
    def length(self):
        """The effective length of this member, in the element's own unit: the
        placement ``dur`` if given, else the element's own ``duration`` (may be
        ``None``)."""
        return self.dur if self.dur is not None else self.element.duration

    @property
    def duration_unit(self) -> str:
        """The unit `length` is in — the placed element's."""
        return getattr(self.element, "duration_unit", BEATS)

    def end(self, tempo: float = 1.0):
        """Where this placement ends, **in the aggregate's beats**: its offset
        plus its length converted at ``tempo`` (beats per second). ``None`` when
        it has no length to end at."""
        length = self.length
        if length is None:
            return None
        return self.offset + to_beats(length, self.duration_unit, tempo)


class Aggregate(Element):
    """A composite element: a set of placed members with a grouping ``kind``.

    Members are placed by an ``offset`` (beats relative to the aggregate's context)
    and an optional placement ``dur``. Edit freely — `add`, `remove`, `move`; a
    handle returned by `add` stays valid across other edits (like
    `clausters.seq.Timeline`).

    A `LOGICAL` aggregate additionally names the composition and may declare internal
    buses; `to_graphdef` translates it into a `clausters.defs.GraphDef` (the
    bus-wired configuration the server already expresses).

    Args:
        children: optional iterable seeding the aggregate. Each item is a
            ``(offset, element)`` pair, a ``(offset, dur, element)`` triple, or
            a bare `Element` (placed at offset 0).
        kind: `CONCRETE` (default) or `LOGICAL`.
        name: the composition's name — the GraphDef name for a logical aggregate.
        buses: internal buses for a logical aggregate — each a ``name`` (audio,
            1 channel) or a ``(name, rate)`` / ``(name, rate, channels)`` tuple.
        onset: the aggregate's own onset in its parent context, or ``None``.
        duration: the aggregate's own duration, or ``None``.
    """

    def __init__(self, children=None, kind=CONCRETE, *, name=None,
                 buses=None, onset=None, duration=None):
        super().__init__(wraps=None, onset=onset, duration=duration, name=name)
        if kind not in (CONCRETE, LOGICAL):
            raise ValueError(f"unknown aggregate kind: {kind!r}")
        self.kind = kind
        self._bus_specs = [_bus_spec(b) for b in (buses or [])]
        self._members = []
        if children is not None:
            for child in children:
                self._add_child(child)

    # ---- editing ----


    @property
    def locatable(self) -> bool:
        """An aggregate is locatable only when every member is.

        One resident generator inside it makes the whole placement unlocatable:
        a position on the aggregate would be a position on that member too, and it
        has none. See `clausters.form.element.Element.locatable`."""
        return all(handle.element.locatable for handle in self.handles)

    def _add_child(self, child):
        if isinstance(child, Element):
            self.add(child)
        elif len(child) == 2:
            offset, element = child
            self.add(element, offset)
        elif len(child) == 3:
            offset, dur, element = child
            self.add(element, offset, dur)
        else:
            raise ValueError(f"invalid child spec: {child!r}")

    def add(self, element, offset=0.0, dur=None):
        """Place ``element`` at ``offset`` (beats), optionally overriding its
        length with ``dur`` (in the element's own unit — see `_Member`).
        Returns a member handle for `remove`/`move`."""
        member = _Member(offset, dur, element)
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
        """The members as ``(offset, dur, element)`` triples, insertion order."""
        return [(m.offset, m.dur, m.element) for m in self._members]

    @property
    def handles(self) -> list:
        """The member **handles** (the objects `add` returns), insertion order —
        the stable identities `remove` and `move` take. Reading a placement is
        `members`; holding on to one across edits (as an editor keying its
        widgets by member does) needs these."""
        return list(self._members)

    def __len__(self):
        return len(self._members)

    def __iter__(self):
        return iter(self.members)

    # ---- the derived temporal relation ----

    def temporal_relation(self, tempo: float = 1.0):
        """Derive this aggregate's temporal relation (`SUCCESSIVE`/`SIMULTANEOUS`/
        `MIXED`) from its members' placements, or ``None`` when empty.

        - `SIMULTANEOUS`: every member starts and ends together (a single member
          trivially qualifies).
        - `SUCCESSIVE`: members tile contiguously in time — sorted by start, each
          member begins exactly where the previous ends (requires known lengths).
        - `MIXED`: anything else.

        ``tempo`` (beats per second) is what puts an end on the same axis as an
        offset: an offset is in beats and a length is in the unit of its
        data it measures, so a take beside a phrase cannot be compared without it. An
        aggregate whose members are all measured in beats ignores it.
        """
        members = self._members
        if not members:
            return None

        starts = [m.offset for m in members]
        lengths = [
            None if m.length is None
            else to_beats(m.length, m.duration_unit, tempo)
            for m in members
        ]
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

    # ---- the internal buses (a logical aggregate's private wires) ----

    @property
    def bus_names(self) -> list:
        """The names of the internal buses this (logical) aggregate declares."""
        return [spec["name"] for spec in self._bus_specs]

    @property
    def bus_specs(self) -> list:
        """The bus declarations themselves — ``name``, ``rate``, ``channels``.

        What `to_document` carries in the body's opaque config, and what a cord
        drawn in the patcher edits: the wiring is the aggregate's, so it is the
        aggregate's configuration that states it.
        """
        return [dict(spec) for spec in self._bus_specs]

    def set_buses(self, buses) -> "Aggregate":
        """Replace the bus declarations, whole.

        The absolute form of `declare_bus`, and what a configuration written
        onto this aggregate means: what the list does not carry is not declared.
        A cord undone takes its bus with it that way, without anyone tracking
        which declaration a gesture happened to add.
        """
        self._bus_specs = [_bus_spec(b) for b in (buses or [])]
        return self

    def declare_bus(self, name, rate: str = "audio", channels: int = 1):
        """Declare an internal bus — a logical aggregate's private wire between
        members. Idempotent by name: re-declaring an existing bus updates its
        ``rate``/``channels``. This is what a patcher edit (a cord drawn between
        two members) calls to name the bus the connection implies."""
        spec = _bus_spec((str(name), rate, int(channels)))
        for i, existing in enumerate(self._bus_specs):
            if existing["name"] == spec["name"]:
                self._bus_specs[i] = spec
                return self
        self._bus_specs.append(spec)
        return self

    # ---- the logical rendering: a GraphDef ----

    def to_graphdef(self, name=None):
        """Translate this **logical** aggregate into a `clausters.defs.GraphDef` — the
        1:1 mapping of the arrangement's logical grouping (nodes wired by sender/
        receiver buses) onto the configuration the server already expresses.

        Each member must be a `clausters.form.element.Generator` (its
        ``def_name`` is the member def; its ``controls`` — numbers, an internal
        bus name, or ``"OUT"`` — and ``maps`` wire it). The aggregate's `buses` become
        the private internal buses. Placement offsets are ignored (a logical aggregate
        is a signal graph, not a timeline). Returns the `GraphDef`; sending and
        instancing it is `clausters.form.render`.
        """
        from ..defs.graphdef import GraphDef
        from .element import Generator

        gname = name or self.name
        if gname is None:
            raise ValueError("a logical Aggregate needs a name to become a GraphDef")
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
                    "a logical Aggregate member must be a Generator, "
                    f"got {type(child).__name__}"
                )
            controls = {
                key: (refs.get(value, value) if isinstance(value, str) else value)
                for key, value in (child.controls or {}).items()
            }
            gdef.add(child.def_name, controls, maps=child.maps)
        return gdef


def _bus_spec(bus) -> dict:
    """Normalize an `Aggregate` bus declaration into the dict `to_graphdef`
    consumes.

    Takes a bare name, a ``(name, rate[, channels])`` tuple, or **a spec that is
    already one** — which is what comes back out of a document, since that is
    the form the body's config carries.
    """
    if isinstance(bus, dict):
        name = bus.get("name")
        rate, channels = bus.get("rate", "audio"), bus.get("channels", 1)
    elif isinstance(bus, str):
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
