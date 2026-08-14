"""The arrangement to and from the **document** — the shared model in
``clausters-document``.

The document is the single authoritative model of a composition, and it lives
in a Rust crate so that every deployment mode binds one of it: this client, the
web client, and a ``standalone`` GUI host with no language attached at all. This
module is the bridge, and it is a **round trip through the format** rather than
a binding: the tree here is converted to the document's JSON and back, so the
crate stays the normative shape without this client giving up its own objects.

What crosses, and what cannot
-----------------------------

The document holds **where things are** — the tree, the placements, the
grouping. It holds a leaf's configuration as an **opaque payload it never
interprets**, which is not a limitation to work around but the reason one
document can serve three languages: a generator *is code*, in the language of
whoever wrote it, and no format owns that.

So the conversion is lossless for **concrete material** (events, placements,
sets, buffers by reference) and carries a **generator by reference**, exactly as
a project file references a plugin rather than serializing it. Coming back, a
leaf whose configuration names an object this process no longer has resolves
through ``resolve``; without one it comes back as the reference itself, which a
`clausters.form.Generator` already accepts (it wraps a def *name* as readily as
a def object). That is the frozen case, and it is the floor rather than a
failure: a host with no interpreter shows what was rendered.

Identity
--------

The document addresses nodes by id and the arrangement does not, so `to_document`
assigns one per element and **stamps it on the element**, reusing whatever is
already there. Converting the same tree twice therefore yields the same ids,
which is what lets an edit made against one conversion still name the right node
in the next.
"""

from .element import Buffer, Element, Event, Generator, Sequence, Track
from .group import CONCRETE, LOGICAL, Group

#: The attribute `to_document` stamps a node id onto. Private by name because it
#: is bookkeeping for the bridge, not part of the arrangement's own surface.
ID_ATTR = "_doc_id"


FIRST_VERSION = 1
"""The version an unedited document carries.

One rather than zero, because zero is what an edit means by *unstated* when it
names the state it was made against — the same reservation the GUI host's
sequence numbers make. An unedited document is a real state an editor must be
able to name, so it cannot share a number with "I cannot say".
"""


def to_document(element, *, version: int = FIRST_VERSION) -> dict:
    """The whole arrangement as a document, ready for ``serde``.

    Args:
        element: the root `clausters.form.Element` (usually a `Group`).
        version: the document version to stamp (see the crate: the document's
            half of the two counters). Defaults to `FIRST_VERSION`; zero means
            *unstated* and is never a document's own version.

    Returns:
        The document as plain JSON-able Python — ``{"version": …, "root": …}``.
    """
    return {"version": int(version), "root": _node(element, _Ids(element))}


def from_document(document: dict, *, resolve=None):
    """Rebuild an arrangement from a document.

    Args:
        document: what `to_document` produces (or what the crate wrote).
        resolve: optional ``resolve(kind, config) -> object`` for leaves whose
            configuration *names* something this process must supply — a
            generator's def, a pattern. Returning ``None`` (or passing no
            resolver) leaves the reference itself in place, which is the frozen
            case rather than an error.

    Returns:
        The root `clausters.form.Element`.
    """
    return _element(document["root"], resolve)


# ---- arrangement -> document ----


class _Ids:
    """Node ids for one conversion: whatever an element already carries, and a
    fresh number past all of them for one that does not.

    Allocating past the maximum already stamped is what keeps a second
    conversion stable — a new element added between two conversions cannot take
    an id an existing element is still using."""

    def __init__(self, root):
        self.next = 1
        self._scan(root)

    def _scan(self, element):
        existing = getattr(element, ID_ATTR, None)
        if existing is not None:
            self.next = max(self.next, int(existing) + 1)
        for child in _children(element):
            self._scan(child)

    def of(self, element) -> int:
        existing = getattr(element, ID_ATTR, None)
        if existing is not None:
            return int(existing)
        assigned = self.next
        self.next += 1
        setattr(element, ID_ATTR, assigned)
        return assigned


def _children(element) -> list:
    """What below this element carries an id of its own.

    A `Track`'s timeline items are not `Element`s, but they *are* nodes in the
    document (decision A: a note is addressable, or no edit could name it and no
    log could invert it), so they take ids the same way — and the scan has to see
    them, or a second conversion would hand them numbers the first did not."""
    if isinstance(element, Group):
        return [m.element for m in element.handles]
    if isinstance(element, Track):
        return [item for _, item in _timeline_items(element.wraps)]
    if isinstance(element, Sequence) and isinstance(element.wraps, (list, tuple)):
        return [item for item in element.wraps if isinstance(item, Element)]
    return []


def _node(element, ids: _Ids) -> dict:
    """One element as a document node: the temporal metadata every node has,
    plus the body that says what it is."""
    node = {"id": ids.of(element)}
    if element.onset is not None:
        node["onset"] = float(element.onset)
    if element.duration is not None:
        node["duration"] = float(element.duration)
    if element.resident:
        node["resident"] = True
    node.update(_body(element, ids))
    return node


#: The keys `_node` writes itself; a preserved body must not restate them.
_TEMPORAL = ("id", "onset", "duration", "resident")


def _body(element, ids: _Ids) -> dict:
    preserved = _preserved(element)
    if preserved is not None:
        # A body this build does not know, on its way back out untouched.
        return {k: v for k, v in preserved.items() if k not in _TEMPORAL}
    if isinstance(element, Group):
        return {
            "kind": "set",
            "grouping": LOGICAL if element.kind == LOGICAL else CONCRETE,
            "members": [
                _member(offset, dur, child, ids)
                for offset, dur, child in element.members
            ],
        }
    if isinstance(element, Track):
        # A Set with the restrictions of a multitrack view, and its items are
        # placed elements like any others -- which is what makes a note in a
        # roll addressable, and therefore editable and undoable.
        return {
            "kind": "set",
            "grouping": CONCRETE,
            "members": [
                _timeline_member(beat, item, ids)
                for beat, item in _timeline_items(element.wraps)
            ],
        }
    if isinstance(element, Event):
        return _with_config({"kind": "event"}, _plain(element.wraps))
    if isinstance(element, Sequence):
        items = element.wraps
        if isinstance(items, (list, tuple)) and all(
            isinstance(i, Element) for i in items
        ):
            return {
                "kind": "sequence",
                "members": [_member(0.0, None, i, ids) for i in items],
            }
        # A pattern, or a list of values the client owns: a reference, not a
        # serialization.
        return _with_config({"kind": "sequence"}, {"sequence": _reference(items)})
    if isinstance(element, Buffer):
        body = {"kind": "buffer", "source": _source(element.wraps)}
        config = {}
        if element.instrument is not None:
            config["instrument"] = _reference(element.instrument)
        if element.controls:
            config["controls"] = _plain(element.controls)
        return _with_config(body, config or None)
    if isinstance(element, Generator):
        config = {"generator": _reference(element.wraps)}
        if element.controls:
            config["controls"] = _plain(element.controls)
        if element.maps:
            config["maps"] = _plain(element.maps)
        return _with_config({"kind": "generator"}, config)
    raise TypeError(f"no document body for {type(element).__name__}")


def _preserved(element):
    """The raw node a `from_document` kept for a body this build cannot name, or
    ``None`` for an element it understands."""
    if type(element) is Element and isinstance(element.wraps, dict):
        return element.wraps
    return None


def _member(offset, dur, element, ids: _Ids) -> dict:
    member = {"offset": float(offset), "node": _node(element, ids)}
    if dur is not None:
        member["dur"] = float(dur)
    return member


def _timeline_member(beat, item, ids: _Ids) -> dict:
    """A timeline item as a placed event, with an id stamped on the item itself
    so it survives to the next conversion."""
    node = {"id": ids.of(item)}
    node.update(_with_config({"kind": "event"}, _plain(item)))
    return {"offset": float(beat), "node": node}


def _timeline_items(timeline) -> list:
    """``(beat, item)`` pairs from a `clausters.seq.Timeline`, or nothing when a
    `Track` wraps something else."""
    if timeline is None:
        return []
    # A `Timeline` iterates as `(beat, item)`; anything else a `Track` wraps
    # has nothing placed to convert.
    try:
        return [(beat, item) for beat, item in timeline]
    except TypeError:
        return []


class FrozenSource:
    """Material a document names and this process does not hold.

    A `Buffer` element wraps `clausters.defs.Buffer`; reading a document written
    elsewhere (or written here before the buffer was allocated) gives the
    reference and not the object. Rather than losing it, the element wraps this:
    the same `bufnum` a real buffer answers with, plus the lifetime and
    generation the document carried, so a re-conversion is faithful and a caller
    that *can* resolve it does so through ``resolve``."""

    def __init__(self, source: dict):
        self.bufnum = int(source.get("source", 0))
        self.lifetime = source.get("lifetime", "session")
        self.generation = int(source.get("generation", 0))


def _source(buffer) -> dict:
    """A buffer element's material. A server buffer the user allocated is
    **session** material -- neither the external-file rule nor a scratch copy --
    and a `FrozenSource` reports whatever the document said instead."""
    return {
        "source": int(getattr(buffer, "bufnum", 0) or 0),
        "lifetime": getattr(buffer, "lifetime", "session"),
        "generation": int(getattr(buffer, "generation", 0)),
    }


def _with_config(body: dict, config) -> dict:
    if config:
        body["config"] = config
    return body


def _reference(obj):
    """What names an object the document does not own: its name when it has
    one, otherwise its own string form. Never the object."""
    if isinstance(obj, str):
        return obj
    name = getattr(obj, "name", None)
    return name if isinstance(name, str) else repr(obj)


def _plain(value):
    """A value as plain JSON-able data, leaving anything else as its reference.
    A `clausters.seq.Event` is a ``dict`` and travels as one."""
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    return _reference(value)


# ---- document -> arrangement ----


def _element(node: dict, resolve):
    kind = node.get("kind")
    config = node.get("config") or {}
    onset = node.get("onset")
    duration = node.get("duration")

    if kind == "set":
        group = Group(
            kind=LOGICAL if node.get("grouping") == LOGICAL else CONCRETE,
            onset=onset,
            duration=duration,
        )
        for member in node.get("members", []):
            group.add(
                _element(member["node"], resolve),
                offset=member.get("offset", 0.0),
                dur=member.get("dur"),
            )
        built = group
    elif kind == "event":
        from ..seq import Event as SeqEvent

        built = Event(SeqEvent(config), onset=onset, duration=duration)
    elif kind == "sequence":
        members = node.get("members")
        if members:
            items = [_element(m["node"], resolve) for m in members]
        else:
            items = _resolved(resolve, "sequence", config) or config.get("sequence")
        built = Sequence(items, onset=onset, duration=duration)
    elif kind == "buffer":
        built = Buffer(
            _resolved(resolve, "buffer", node.get("source"))
            or FrozenSource(node.get("source") or {}),
            onset=onset,
            duration=duration,
            instrument=config.get("instrument"),
            controls=config.get("controls"),
        )
    elif kind == "generator":
        built = Generator(
            _resolved(resolve, "generator", config) or config.get("generator"),
            onset=onset,
            duration=duration,
            controls=config.get("controls"),
            maps=config.get("maps"),
        )
    else:
        # A body this build does not know. The document preserves it whole and
        # so does this side: it comes back as an abstract element carrying the
        # payload, so a round trip through an older client does not lose it.
        built = Element(dict(node), onset=onset, duration=duration)

    # The document is the authority on temporal metadata: an element's own
    # constructor may derive a duration (a `form.Event` takes the event's `dur`
    # when none is given), and letting that win would make a document say
    # something the document did not say.
    built.onset = None if onset is None else float(onset)
    built.duration = None if duration is None else float(duration)
    if node.get("resident"):
        built.resident = True
    if "id" in node:
        setattr(built, ID_ATTR, int(node["id"]))
    return built


def _resolved(resolve, kind, config):
    """Whatever the caller supplies for a leaf the document only names, or
    ``None`` when nobody can supply it — the frozen case."""
    return None if resolve is None else resolve(kind, config)
