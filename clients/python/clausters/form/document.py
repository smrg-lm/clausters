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

The id is on the *element*, so placing one element at two offsets writes two
nodes with one id, and an edit naming that id cannot say which of the two it
means. Give each appearance its own element over the same material — two
`Buffer` leaves over one server buffer — until the addressing settles.
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


#: The session format this client writes (the crate's `session::FORMAT`).
SESSION_FORMAT = 1


def to_session(element, *, sources=None, version: int = FIRST_VERSION,
               provenance=None) -> dict:
    """The arrangement as a **session**: the document, plus the table that says
    where its material is.

    A document says *what plays when* and deliberately not where a source lives,
    because inside a running system a source is a server buffer, a mapped file
    or a rendered result and the tree has no business knowing which. A session
    is the document plus exactly that missing half, so the thing can be closed
    and opened again — by this client, or by a ``standalone`` host with no
    language attached, which is why the format lives in the crate and not here.

    Args:
        element: the root `clausters.form.Element`.
        sources: ``{source_id: dict}`` — each entry as the crate's
            ``session::Source`` (``location``, ``lifetime``, ``generation``, and
            optionally ``channels``/``frames``/``sample_rate``/``provenance``/
            ``editing``). Material with an **open destructive edit** carries
            ``editing`` and reopens that way: a save never blocks on a
            confirmation.
        version: the document version to stamp.
        provenance: an opaque reference to whatever produced the session — the
            scripts behind it. Carried and never interpreted, which is what
            makes re-generating possible without the format knowing how.

    Returns:
        The session as plain JSON-able Python.
    """
    document = to_document(element, version=version)
    table = {str(k): v for k, v in (sources or {}).items()}
    missing = sorted(_source_ids(document["root"]) - {int(k) for k in table})
    if missing:
        # A session whose table does not cover its own document reopens with
        # that material unresolved -- the take draws nothing and nothing says
        # why, which is a defect found the only way it can be: by looking at a
        # window two saves later. The table is caller data (what a location
        # *means* is the caller's), but whether it covers the tree is checkable
        # here, and it is the difference between an error now and a silent hole
        # later. It bites hardest where the ids move under you: reopening
        # resolves material into new buffers, so a table built once at startup
        # stops matching the composition it is saved with.
        raise ValueError(
            f"the source table does not cover this document: no entry for "
            f"{', '.join(str(m) for m in missing)}. Build it from the "
            f"arrangement being saved (each buffer element's current source), "
            f"not from the material the script started with."
        )
    session = {
        "format": SESSION_FORMAT,
        "document": document,
        "sources": table,
    }
    if provenance is not None:
        session["provenance"] = provenance
    return session


def _source_ids(node: dict) -> set:
    """Every source id the document names, so a session can be checked against
    its own table before it is written."""
    found = set()
    stack = [node]
    while stack:
        current = stack.pop()
        if not isinstance(current, dict):
            continue
        source = current.get("source")
        if isinstance(source, dict) and "source" in source:
            found.add(int(source["source"]))
        for member in current.get("members") or ():
            if isinstance(member, dict):
                stack.append(member.get("node"))
        stack.append(current.get("rendered"))
    return found


def from_session(session: dict, *, resolve=None):
    """Open a session: the root element, and its source table.

    Returns:
        ``(element, sources)`` — the arrangement, and ``{source_id: dict}`` as
        written. The table is handed back as data rather than resolved, because
        what a source *is* (a server buffer to allocate, a file to map) is the
        caller's to decide and depends on what is running.

    Raises:
        ValueError: if the file was written in a format this build cannot read.
            A newer *field* is not a version change — it is ignored on the way
            through, the way an unknown body is carried rather than dropped —
            so this only fires when reading it wrongly is the alternative.
    """
    format_ = int(session.get("format", SESSION_FORMAT))
    if format_ > SESSION_FORMAT:
        raise ValueError(
            f"session format {format_} is newer than this build reads "
            f"({SESSION_FORMAT})"
        )
    sources = {int(k): v for k, v in (session.get("sources") or {}).items()}
    return from_document(session["document"], resolve=resolve), sources


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
    an id an existing element is still using.

    **An id names one element, and this is where that is enforced.** The number
    is stamped on the element object, and numbering starts at 1 for every root,
    so two arrangements built in one script both hold 1, 2, 3 — and material
    authored in one and used in the other arrives carrying a number a different
    element here already holds. Nothing downstream survives that: an intent
    naming the id reaches whichever node the crate's lookup finds first while
    the editor's index keeps the last, so one gesture writes two places. The
    walk therefore **claims** each id for the object it first meets carrying it,
    and an object that turns up with an id already claimed by another is
    renumbered.

    Two things it deliberately does not do. It does not touch the *same* object
    appearing twice — two placements of one element are one node with one id,
    which is a question about what an id identifies and is open in the document
    crate's plan, not something to settle by accident here. And it does not
    renumber the first claimant, so a tree converted on its own is numbered
    exactly as it always was.

    The cost of renumbering, stated because it is real: a log entry recorded
    earlier against the moved element's old number no longer names it. It
    happens only when material crosses between trees, it stamps a number nothing
    else in this tree holds, and the editor re-derives its index from the
    document on every edit — so what is at risk is undo of an edit made before
    the crossing, not the current one."""

    def __init__(self, root):
        self.next = 1
        self._owner: "dict[int, int]" = {}
        self._renumber: "set[int]" = set()
        #: Elements already met as a placement, so a second one is checked
        #: against what may be placed twice at all.
        self.placed: "set[int]" = set()
        self._scan(root)

    def _scan(self, element, member=None):
        holder = element if member is None else member
        existing = getattr(holder, ID_ATTR, None)
        if existing is not None:
            existing = int(existing)
            owner = self._owner.setdefault(existing, id(holder))
            if owner == id(holder):
                self.next = max(self.next, existing + 1)
            else:
                # Another object in this tree claimed the number first, so this
                # one was numbered against a tree that is not this one.
                self._renumber.add(id(holder))
        if isinstance(element, Group):
            for handle in element.handles:
                self._scan(handle.element, handle)
            return
        for child in _children(element):
            self._scan(child)

    def of(self, element, member=None) -> int:
        """The id of the node this element occupies — **the placement's** when
        it is placed, since a clip is a window onto material and what an edit
        names is the window.

        An element reached any other way (the root, a rendered subtree, an item
        of a sequence) carries its own, which is the same rule read where there
        is no placement to name."""
        holder = element if member is None else member
        existing = getattr(holder, ID_ATTR, None)
        if existing is not None and id(holder) not in self._renumber:
            return int(existing)
        assigned = self.next
        self.next += 1
        self._owner[assigned] = id(holder)
        self._renumber.discard(id(holder))
        setattr(holder, ID_ATTR, assigned)
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
    if isinstance(element, Generator) and element.rendered is not None:
        # The last rendered result is ordinary tree, so its nodes take ids like
        # any others -- and the scan has to see them, or a second conversion
        # would renumber a subtree the first had already stamped.
        return [element.rendered]
    return []


def _node(element, ids: _Ids, member=None) -> dict:
    """One element as a document node: the temporal metadata every node has,
    plus the body that says what it is."""
    node = {"id": ids.of(element, member)}
    name = getattr(element, "name", None)
    if isinstance(name, str) and name:
        # A referenceable label, never a second identity -- the server's own
        # rule for a group's name, and the reason a reopened piece can still
        # label its lanes the way it was authored.
        node["name"] = name
    if element.onset is not None:
        node["onset"] = float(element.onset)
    if element.duration is not None:
        node["duration"] = float(element.duration)
    if element.resident:
        node["resident"] = True
    node.update(_body(element, ids))
    return node


#: The keys `_node` writes itself; a preserved body must not restate them.
_TEMPORAL = ("id", "name", "onset", "duration", "resident")

#: What a `Track` is, in the set body's opaque config. The document has one set
#: kind and goes on having one -- a track is *a set with the restrictions of a
#: multitrack view*, and the tree deliberately carries no view. But a writer
#: that has such a set must get it back, or a round trip turns every track into
#: a plain set and the piece reopens with a level of nesting nobody wrote. So
#: the restriction travels the way a leaf's code does: carried, uninterpreted.
FORM_TRACK = "track"


def _body(element, ids: _Ids) -> dict:
    preserved = _preserved(element)
    if preserved is not None:
        # A body this build does not know, on its way back out untouched.
        return {k: v for k, v in preserved.items() if k not in _TEMPORAL}
    if isinstance(element, Group):
        return {
            "kind": "set",
            "grouping": LOGICAL if element.kind == LOGICAL else CONCRETE,
            "members": [_member(handle, ids) for handle in element.handles],
        }
    if isinstance(element, Track):
        # A Set with the restrictions of a multitrack view, and its items are
        # placed elements like any others -- which is what makes a note in a
        # roll addressable, and therefore editable and undoable. Which
        # restrictions those are is the client's own business, so it rides in
        # the body's opaque config and the document never reads it.
        return _with_config({
            "kind": "set",
            "grouping": CONCRETE,
            "members": [
                _timeline_member(beat, item, ids)
                for beat, item in _timeline_items(element.wraps)
            ],
        }, {"form": FORM_TRACK})
    if isinstance(element, Event):
        return _with_config({"kind": "event"}, _plain(element.wraps))
    if isinstance(element, Sequence):
        items = element.wraps
        if isinstance(items, (list, tuple)) and all(
            isinstance(i, Element) for i in items
        ):
            return {
                "kind": "sequence",
                # A sequence's items are *elements in order*, not placements —
                # there is no handle to name, so each node's id is its own.
                "members": [{"offset": 0.0, "node": _node(i, ids)} for i in items],
            }
        # A pattern, or a list of values the client owns: a reference, not a
        # serialization.
        # A leaf with no name is written with no reference: frozen, and the
        # same bytes on every run of the same script.
        return _with_config({"kind": "sequence"},
                            _named({"sequence": _reference(items, element)}))
    if isinstance(element, Buffer):
        body = {"kind": "buffer", "source": _source(element.wraps)}
        config = {}
        if element.instrument is not None:
            instrument = _reference(element.instrument)
            if instrument is not None:
                config["instrument"] = instrument
        if element.controls:
            config["controls"] = _plain(element.controls)
        return _with_config(body, config or None)
    if isinstance(element, Generator):
        config = _named({"generator": _reference(element.wraps, element)})
        if element.controls:
            config["controls"] = _plain(element.controls)
        if element.maps:
            config["maps"] = _plain(element.maps)
        body = {"kind": "generator"}
        if getattr(element, "rendered", None) is not None:
            # What the generator last produced, as ordinary tree. A host with
            # no language attached has nothing to run the generator with, so
            # this is the whole of what it can show.
            body["rendered"] = _node(element.rendered, ids)
        return _with_config(body, config)
    # A base `Element` wrapping something this module has no body for. It
    # becomes an opaque leaf rather than an error, which is the format's own
    # rule read from this side: **what a writer does not understand, it
    # preserves**. The alternative was found by routing the editor's own edits
    # through the document -- an arrangement is free to hold an element kind the
    # conversion predates, and refusing to convert would make the whole
    # composition unde-editable because one leaf in it is unfamiliar.
    # Under the **same config key** a `Generator` writes, because this is the
    # same body kind and the key is what a reader resolves on: writing a second
    # name for it made a round trip change the leaf's key (`element` on the way
    # out of a hand-written tree, `generator` on the way out of the one that
    # came back), so a resolver that recognized the material once stopped
    # recognizing it on the second open.
    return _with_config({"kind": "generator"},
                        _named({"generator": _reference(element.wraps, element),
                                "points": _points_of(element.wraps)}))


def _preserved(element):
    """The raw node a `from_document` kept for a body this build cannot name, or
    ``None`` for an element it understands."""
    if type(element) is Element and isinstance(element.wraps, dict):
        return element.wraps
    return None


def _member(handle, ids: _Ids) -> dict:
    """One placement: where it sits, and the node it holds — whose id is the
    **handle's**, so one element placed twice is two windows and not one
    ambiguous name."""
    _placeable_twice(handle, ids)
    member = {"offset": float(handle.offset), "node": _node(handle.element, ids, handle)}
    if handle.dur is not None:
        member["dur"] = float(handle.dur)
    return member


def _placeable_twice(handle, ids: _Ids):
    """Refuse a *second* placement of an element whose material is in the node.

    Two windows share material only when the node **references** it — a buffer
    names a source, a generator names a recipe, and both placements point at the
    one thing. An event, a track or a group carries its material *inside* the
    node, so a second placement is a second **copy**: they diverge on the first
    edit, which is the answer the open decision rejected. Refused with the
    distinction rather than copied in silence.
    """
    element = handle.element
    if id(element) not in ids.placed:
        ids.placed.add(id(element))
        return
    if isinstance(element, (Buffer, Generator)) or (
            isinstance(element, Sequence) and not isinstance(element.wraps, (list, tuple))):
        return  # a window onto material the node only names
    raise ValueError(
        f"{type(element).__name__} is placed more than once, and its material is "
        "in the node rather than named by it — two placements would be two "
        "copies that diverge on the first edit. Place a leaf that *references* "
        "its material (a Buffer over one server buffer, a Generator over one "
        "recipe), or give each placement its own element."
    )


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


def leaf_config(element) -> dict:
    """The configuration a leaf's node carries, exactly as `to_document` writes
    it.

    Public because an **editor** needs it: a `Configure` intent replaces a
    leaf's configuration *whole*, so an editor that wants to change one field of
    it has to start from the rest — and re-deriving that here rather than in the
    editor is what keeps one description of what a leaf's config is.
    """
    return dict((_body(element, _Ids(element)).get("config") or {}))


def next_node_id(element) -> int:
    """The first node id no element in this arrangement holds.

    What an editor mints from when it has to name a node the conversion has not
    seen yet — a note added by a gesture. It follows the conversion's own rule
    (past the maximum already stamped), so a minted id and a converted one
    cannot collide.
    """
    return _Ids(element).next


def _points_of(wrapped):
    """A curve's break-points, when the leaf is one — or ``None``.

    **The document has to carry these, and not only draw them.** A curve is a
    leaf like any other and its configuration is opaque, but an edit to it is a
    `Configure` intent, and an intent's inverse is *the previous value read out
    of the document*: with nothing there, a dragged break-point had nothing to
    invert and could not be undone. Carrying them also makes an edited curve
    survive a save, which it did not — reopening resolved the automation by name
    and took whatever envelope that object happened to hold.
    """
    to_points = getattr(wrapped, "to_points", None)
    if not callable(to_points):
        return None
    try:
        points = [float(v) for v in to_points()]
    except (AttributeError, TypeError, ValueError):
        # A leaf is opaque, and reading one must never be able to take a save
        # down: an object that answers to the name and not to the shape is
        # carried by reference like any other, with no points.
        return None
    return points or None


def _named(config: dict) -> dict:
    """A config with the keys whose value is `None` dropped — a reference
    nothing could name is left out rather than written as null, so an unnamed
    leaf and a leaf named nothing are the same file."""
    return {k: v for k, v in config.items() if v is not None}


def _with_config(body: dict, config) -> dict:
    if config:
        body["config"] = config
    return body


def _reference(obj, element=None):
    """What names an object the document does not own — or ``None`` when nothing
    does, which is the honest answer and used to be `repr`.

    A leaf is opaque by decision: the document carries a *reference* to an
    algorithm and never the algorithm, so reopening hands the reference to a
    resolver and takes back whatever that resolver has. The reference therefore
    has to be something a caller **can produce**. Three sources, in order: the
    object's own name (a def, an `Automation`), the element's `name` (what an
    author writes for material that has none of its own — a `Pbind` is code and
    carries no name), and nothing.

    **Nothing is better than `repr`**, which is what this wrote before: a
    memory address is unresolvable by construction *and* different between two
    runs of the same script, so it broke the format's determinism to hand a
    resolver a key that could never match. An unnamed leaf is written with no
    reference at all and comes back frozen — drawn, placed, silent — which is
    what a composition means where its language is not running.
    """
    if isinstance(obj, str):
        return obj
    name = getattr(obj, "name", None)
    if isinstance(name, str) and name:
        return name
    name = getattr(element, "name", None)
    return name if isinstance(name, str) and name else None


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


def _element(node: dict, resolve, *, placed: bool = False):
    kind = node.get("kind")
    config = node.get("config") or {}
    onset = node.get("onset")
    duration = node.get("duration")

    if kind == "set" and config.get("form") == FORM_TRACK:
        # A set the author wrote as a `Track`, said by the body's own config.
        # Rebuilding it as a `Group` is what made a reopened piece grow a level
        # of nesting nobody wrote, and left the editor drawing a lane of clips
        # where there had been a roll.
        from ..seq.timeline import Timeline

        timeline = Timeline()
        for member in node.get("members", []):
            child = member["node"]
            item = _element(child, resolve)
            # A timeline holds the client's own sequencing items, not elements:
            # what went out as a placed event comes back as the event itself.
            item = getattr(item, "wraps", item)
            if "id" in child:
                # The id belongs to the item, which is what the conversion
                # stamped on the way out -- so a note keeps its number across a
                # save, and an intent recorded against it still names it.
                setattr(item, ID_ATTR, int(child["id"]))
            timeline.add(member.get("offset", 0.0), item)
        built = Track(timeline, onset=onset, duration=duration)
    elif kind == "set":
        group = Group(
            kind=LOGICAL if node.get("grouping") == LOGICAL else CONCRETE,
            onset=onset,
            duration=duration,
        )
        for member in node.get("members", []):
            child = member["node"]
            handle = group.add(
                _element(child, resolve, placed=True),
                offset=member.get("offset", 0.0),
                dur=member.get("dur"),
            )
            if "id" in child:
                # The placement's id, on the placement: a second window onto the
                # same material is a second handle with a number of its own.
                setattr(handle, ID_ATTR, int(child["id"]))
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
        rendered = node.get("rendered")
        resolved = _resolved(resolve, "generator", config)
        _apply_points(resolved, config.get("points"))
        built = Generator(
            resolved
            # `element` is what this client wrote for a leaf it had no body for
            # before the two keys became one; a file carrying it still opens.
            or config.get("generator") or config.get("element"),
            onset=onset,
            duration=duration,
            controls=config.get("controls"),
            maps=config.get("maps"),
            rendered=None if rendered is None else _element(rendered, resolve),
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
    name = node.get("name")
    if isinstance(name, str) and name:
        # A label, not an identity: it says what the node is and nothing
        # addresses by it, so restoring it is what lets a reopened piece label
        # its lanes the way it was authored.
        built.name = name
    if "id" in node and not placed:
        # An element reached as a placement takes no id of its own: the number
        # is the window's, and its handle is what carries it.
        setattr(built, ID_ATTR, int(node["id"]))
    return built


def _apply_points(resolved, points):
    """Put a carried curve back onto the material that was handed to us.

    The document is the authority for what it holds: a resolver returns the
    `clausters.seq.Automation` this process has, and the envelope *in the file*
    is the one that was saved — without this, reopening a session showed the
    curve the script last built rather than the curve the piece was left with.
    """
    if not points or resolved is None or not hasattr(resolved, "env"):
        return
    from ..defs.ugens.env import points_to_env

    resolved.env = points_to_env(list(points))


def _resolved(resolve, kind, config):
    """Whatever the caller supplies for a leaf the document only names, or
    ``None`` when nobody can supply it — the frozen case."""
    return None if resolve is None else resolve(kind, config)
