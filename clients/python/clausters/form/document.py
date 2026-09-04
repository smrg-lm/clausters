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

So the conversion is lossless for **concrete data** (events, placements,
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
means. Give each appearance its own element over the same source — two
`Vector` leaves over one server buffer — until the addressing settles.
"""

import os

from .element import (SECONDS, Vector, Element, Clang, Generator, Segments,
                      Sequence, Track)
from .aggregate import CONCRETE, LOGICAL, Aggregate
from ..segments import NoteSegments

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
        element: the root `clausters.form.Element` (usually an `Aggregate`).
        version: the document version to stamp (see the crate: the document's
            half of the two counters). Defaults to `FIRST_VERSION`; zero means
            *unstated* and is never a document's own version.

    Returns:
        The document as plain JSON-able Python — ``{"version": …, "root": …}``.
    """
    ids = _Ids(element)
    shared = _shared_content(element, ids)
    document = {"version": int(version), "root": _node(element, ids, shared=shared)}
    if shared:
        # **Content is written once, and the tree names it.** Its nodes are in
        # the same id space as the tree's, which is what lets a window and an
        # intent name the same thing.
        document["content"] = [entry["node"] for entry in shared.values()]
    return document


def _content_id(timeline, shared, ids) -> int:
    """The node a window names for this timeline.

    The conversion's own answer when it built one (`_shared_content`), else the
    id already stamped on the timeline, else a fresh one -- which is the same
    order every id question in this module takes, and what keeps a subtree
    converted on its own (a join writing the node its member now holds) naming
    the node the whole tree names.
    """
    entry = (shared or {}).get(id(timeline))
    if entry is not None:
        return int(entry["id"])
    return ids.of(timeline)


def _shared_content(root, ids: "_Ids") -> dict:
    """The contents **more than one element reads**, as content nodes.

    A window onto samples costs nothing to repeat because the samples are not in
    the document; a window onto a timeline of notes is not so lucky -- the notes
    are nodes, so writing two windows as two tracks writes every note twice,
    with the same ids in each, and a reopened piece gets two timelines that
    drift apart from the first edit. So a timeline **two elements hold** is
    written once, here, and each of its readers becomes a window naming it
    (`clausters_document::SegmentSource::Node`).

    A timeline only one element holds is written exactly as it always was: the
    table exists for sharing, not for tracks.

    Returns ``{id(timeline): {"id": node id, "node": the content node}}``, built
    before the tree so the tree can name what is in it.
    """
    #: ``id(timeline) -> (timeline, how many elements hold it)``, plus the ones a
    #: window **names**: those are content however few read them, since there is
    #: no other way to write "this clip plays a stretch of that timeline" down.
    holders: dict = {}
    named: set = set()

    def scan(element):
        if isinstance(element, Track):
            held, readers = holders.get(id(element.wraps), (element.wraps, 0))
            holders[id(element.wraps)] = (held, readers + 1)
        if isinstance(element, Segments) and element.duration_unit != SECONDS:
            for seg in element.segments:
                holders.setdefault(id(seg.source), (seg.source, 0))
                named.add(id(seg.source))
        for child in _children(element):
            scan(child)

    scan(root)
    shared = {}
    for key, (timeline, readers) in holders.items():
        if readers < 2 and key not in named:
            # **Not content, and it has to stop being it.** A join makes two
            # windows one element again, and a timeline still carrying the id it
            # had as content would send the next note edit to a node this
            # document no longer has.
            if getattr(timeline, ID_ATTR, None) is not None:
                setattr(timeline, ID_ATTR, None)
            continue
        # The content node carries the notes, and the id is the **timeline's**
        # own -- stamped on it, so a second conversion names the same node and
        # every intent recorded against a note still lands.
        node = {"id": ids.of(timeline), "kind": "aggregate", "grouping": CONCRETE,
                "members": [_timeline_member(beat, item, ids)
                            for beat, item in _timeline_items(timeline)],
                "config": {"form": FORM_TRACK}}
        shared[key] = {"id": node["id"], "node": node}
    return shared


#: The session format this client writes (the crate's `session::FORMAT`).
SESSION_FORMAT = 1


def to_session(element, *, sources=None, version: int = FIRST_VERSION,
               provenance=None) -> dict:
    """The arrangement as a **session**: the document, plus the table that says
    where its source is.

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
            ``editing``). A source with an **open destructive edit** carries
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
        # that source unresolved -- the take draws nothing and nothing says
        # why, which is a defect found the only way it can be: by looking at a
        # window two saves later. The table is caller data (what a location
        # *means* is the caller's), but whether it covers the tree is checkable
        # here, and it is the difference between an error now and a silent hole
        # later. It bites hardest where the ids move under you: reopening
        # resolves source into new buffers, so a table built once at startup
        # stops matching the composition it is saved with.
        raise ValueError(
            f"the source table does not cover this document: no entry for "
            f"{', '.join(str(m) for m in missing)}. Build it from the "
            f"arrangement being saved (each buffer element's current source), "
            f"not from the source the script started with."
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
        # A `segments` node names one source per segment, and a session whose
        # table covered only the first would reopen with the rest of the
        # source missing.
        for seg in current.get("segments") or ():
            if isinstance(seg, dict):
                stack.append(seg)
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
    element = from_document(session["document"], resolve=resolve)
    # A source nothing resolved comes back as a reference, and the table is what
    # says where it is -- so the reference is given it. Otherwise opening a
    # session and saving it again wrote every unresolved take as volatile, and
    # the format lost its own contents on the second save.
    for source in _source_objects(element):
        if isinstance(source, FrozenSource):
            source.locate(sources.get(int(source.bufnum)))
    return element, sources


def session_resolver(session: dict, *, server=None, folder=None, defs=None):
    """A ``resolve`` for `from_session`, **over the session's own table**.

    `from_session` rebuilds the tree; this is what makes the tree hold
    something. A document names a source by number and says nothing about where
    it is — the table says that — so reopening a piece into a running system is
    two steps, and this is the second one:

    - **A take** (a `Vector` or a `Segments` window) whose source the table
      locates in a **file** is read onto the server, once per source id no
      matter how many windows name it: two clips over one take are two windows
      onto **one** buffer, and reading it twice would give them two buffers
      that drift apart on the first edit. A source the table calls *volatile*
      existed only in the run that wrote it, so it comes back as a
      `FrozenSource` — drawn, placed, silent — rather than as a lie.
    - **A generator** (or a pattern) is code, and a document carries a
      *reference* to code and never the code. So it is looked up in ``defs``,
      and a name nothing supplies is left frozen with whatever it last
      **rendered** as its floor — which is already the format's contract and is
      the whole of what a host with no language attached can show.

    Args:
        session: the session as read (`from_session` takes the same object).
        server: the `clausters.defs.Server` the takes are read onto; ``None``
            takes the ambient one, as every other buffer call does.
        folder: the session file's own folder. A **relative** path in the table
            is resolved against it, which is what makes a session directory
            movable; an absolute one names the user's own file and is left
            exactly as written.
        defs: what supplies the code a leaf names — a mapping from reference to
            object, or a callable ``defs(kind, reference)``. Anything it does
            not have is frozen rather than an error.

    Returns:
        The resolver, to hand to `from_session`::

            with open(path) as f:
                data = json.load(f)
            piece, sources = from_session(
                data, resolve=session_resolver(data, folder=os.path.dirname(path)))
    """
    table = {int(k): v for k, v in (session.get("sources") or {}).items()}
    # **Read once per source id, before the tree asks.** Two clips over one
    # take are two windows onto *one* buffer, and resolving each window on its
    # own would give them two buffers that drift apart on the first edit. The
    # table covers exactly what the document names (`to_session` refuses one
    # that does not), so there is nothing here that the piece does not use.
    buffers = {}
    for source_id, entry in table.items():
        location = (entry or {}).get("location") or {}
        path = location.get("path") if location.get("at") == "file" else None
        buffers[source_id] = None if not path else _read_take(path, folder, server)

    def resolve(kind, config):
        config = config or {}
        if kind == "vector":
            return buffers.get(int(config.get("source", -1)))
        reference = config.get(kind) or config.get("generator")
        if defs is None or not isinstance(reference, str):
            return None
        if callable(defs):
            return defs(kind, reference)
        return defs.get(reference)

    return resolve


def _read_take(path: str, folder, server):
    """One source's file onto the server, or ``None`` when it cannot be read.

    A missing file is **not** an error here. Half a session is worth opening —
    the piece still draws, the other lanes still sound, and the element that
    could not be resolved comes back frozen the way an unresolved generator
    does. Raising instead would make one moved file the difference between a
    piece and nothing.
    """
    from ..defs.buffer import Buffer

    if folder is not None and not os.path.isabs(path):
        path = os.path.join(str(folder), path)
    try:
        return Buffer.read(path, server=server)
    except Exception:
        return None


def sources_of(element, *, folder=None) -> dict:
    """The **source table** for an arrangement, built from what its takes
    actually hold — the table `to_session` demands and refuses to guess.

    Its error message says to build the table "from the arrangement being
    saved, each buffer element's current source", and this is that sentence as
    a function, in one place rather than in every script. Each take's buffer is
    asked where it is: a `clausters.defs.Buffer` read from a file knows its
    ``path`` and is written as that file, and one allocated in this run is
    written as **volatile** — it existed only while the process did, and a
    session that claimed otherwise would reopen with silence where it promised
    samples. A `FrozenSource` reports what the document it came from said, so a
    session opened and saved again keeps every location it was given.

    Args:
        element: the root being saved.
        folder: the session file's own folder. A path inside it is written
            **relative**, which is what makes the pair of files movable
            together; one outside it stays absolute, because a session must
            never claim to own the user's own file.

    Returns:
        ``{source_id: dict}``, ready to hand to `to_session`.
    """
    table = {}
    for source in _source_objects(element):
        bufnum = int(getattr(source, "bufnum", 0) or 0)
        entry = {
            "location": _location_of(source, folder),
            "lifetime": getattr(source, "lifetime", "session"),
            "generation": int(getattr(source, "generation", 0) or 0),
        }
        for key, attr in (("channels", "channels"), ("frames", "frames"),
                          ("sample_rate", "sample_rate")):
            value = getattr(source, attr, None)
            if value:
                entry[key] = float(value) if key == "sample_rate" else int(value)
        table[bufnum] = entry
    return table


def _location_of(source, folder) -> dict:
    """Where one source's samples are, as the crate's `session::Location`."""
    path = getattr(source, "path", None)
    if not isinstance(path, str) or not path:
        return {"at": "volatile"}
    if folder:
        try:
            inside = os.path.relpath(path, str(folder))
        except ValueError:
            inside = None       # a different drive: nothing relative to say
        if inside is not None and not inside.startswith(os.pardir):
            path = inside
    return {"at": "file", "path": path}


def _source_objects(element) -> list:
    """Every take's source in this arrangement, in the order the walk meets
    them, one entry per source id — a take placed twice is one source."""
    found: dict = {}
    stack = [element]
    while stack:
        current = stack.pop()
        for source in _sources_in(current):
            found.setdefault(int(getattr(source, "bufnum", 0) or 0), source)
        if isinstance(current, Aggregate):
            stack += [handle.element for handle in current.handles]
        elif isinstance(current, Sequence) and isinstance(current.wraps, (list, tuple)):
            stack += [i for i in current.wraps if isinstance(i, Element)]
        elif isinstance(current, Generator) and current.rendered is not None:
            stack.append(current.rendered)
    return list(found.values())


def _sources_in(element) -> list:
    """The sources one element names itself — a take's buffer, or one per
    window of a `Segments`."""
    if isinstance(element, Vector):
        return [element.wraps] if element.wraps is not None else []
    if isinstance(element, Segments):
        return [seg.buffer for seg in element.segments if seg.buffer is not None]
    return []


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
    # **Content first, because the tree names it.** A window onto a node reads
    # contents the document holds once and several windows share, so it is built
    # before the tree and handed down -- and every window over one node gets the
    # *same* object, which is the whole point: two halves of a cut edit one
    # timeline, and reopening a piece must not hand them two.
    content = {}
    for node in document.get("content") or ():
        built = _element(node, resolve)
        content[int(node["id"])] = built
        setattr(getattr(built, "wraps", built), ID_ATTR, int(node["id"]))
    return _element(document["root"], resolve, content=content)


# ---- arrangement -> document ----


class _Ids:
    """Node ids for one conversion: whatever an element already carries, and a
    fresh number past all of them for one that does not.

    Allocating past the maximum already stamped is what keeps a second
    conversion stable — a new element added between two conversions cannot take
    an id an existing element is still using.

    **An id names one element, and this is where that is enforced.** The number
    is stamped on the element object, and numbering starts at 1 for every root,
    so two arrangements built in one script both hold 1, 2, 3 — and source
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
    happens only when source crosses between trees, it stamps a number nothing
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
        if isinstance(element, Aggregate):
            for handle in element.handles:
                self._scan(handle.element, handle)
            return
        for child in _children(element):
            self._scan(child)

    def of(self, element, member=None) -> int:
        """The id of the node this element occupies — **the placement's** when
        it is placed, since a clip is a window onto source and what an edit
        names is the window.

        An element reached any other way (the root, a rendered subtree, an item
        of a sequence) carries its own, which is the same rule read where there
        is no placement to name."""
        holder = element if member is None else member
        existing = getattr(holder, ID_ATTR, None)
        if existing is not None and id(holder) not in self._renumber:
            # **Ownership is checked here and not only at the scan**, because
            # not everything numbered is something the scan walks: a timeline is
            # content, so it carries an id and is reached through a window
            # rather than through the tree. One that arrives holding a number
            # another object in this conversion already claimed is renumbered,
            # which is the rule this class states -- it was simply only applied
            # to what the scan had seen.
            owner = self._owner.setdefault(int(existing), id(holder))
            if owner == id(holder):
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
    if isinstance(element, Aggregate):
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


def _node(element, ids: _Ids, member=None, *, shared=None) -> dict:
    """One element as a document node: the temporal metadata every node has,
    plus the body that says what it is."""
    node = {"id": ids.of(element, member)}
    name = getattr(element, "name", None)
    if isinstance(name, str) and name:
        # A referenceable label, never a second identity -- the server's own
        # rule for an aggregate's name, and the reason a reopened piece can still
        # label its lanes the way it was authored.
        node["name"] = name
    if element.onset is not None:
        node["onset"] = float(element.onset)
    if element.duration is not None:
        node["duration"] = float(element.duration)
    if element.resident:
        node["resident"] = True
    node.update(_body(element, ids, shared))
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


def _kind_body(element, ids: _Ids, shared=None) -> dict:
    preserved = _preserved(element)
    if preserved is not None:
        # A body this build does not know, on its way back out untouched.
        return {k: v for k, v in preserved.items() if k not in _TEMPORAL}
    if isinstance(element, Aggregate):
        # A logical aggregate's **declared buses** ride in the body's opaque
        # config, the same door a `Track`'s restrictions use: they are the
        # writer's own wiring, carried and never read. Without this a patch lost
        # its buses on every round trip -- the cords survived (a member's
        # controls are in its own config) while the buses they name did not, so
        # a reopened patcher drew the connections and could render none of them.
        # And an edit no format carries is an edit no history can invert, which
        # is why a cord was undoable by nothing.
        return _with_config({
            "kind": "aggregate",
            "grouping": LOGICAL if element.kind == LOGICAL else CONCRETE,
            "members": [_member(handle, ids, shared) for handle in element.handles],
        }, {"buses": element.bus_specs} if element.bus_specs else None)
    if isinstance(element, Track):
        entry = (shared or {}).get(id(element.wraps))
        if entry is not None:
            # **This timeline is content, so this element is a window onto it.**
            # More than one element reads it, and writing the notes once per
            # reader would write one identity twice -- so the notes are in
            # `content` and each reader names the node
            # (`SegmentSource::Node`). The *element's* length stays the node's
            # own, absent when nobody stated one; the window's length is how
            # much of the notes it can show, which is what a reader that
            # does not resolve the content lays the clip out with.
            start = float(element.start)
            length = (float(element.duration) if element.duration is not None
                      else max(0.0, float(element.wraps.duration()) - start))
            return _with_config({
                "kind": "segments",
                "segments": [{"source": {"node": entry["id"]},
                              "start": start, "duration": length}],
            }, {"form": FORM_TRACK})
        # A Set with the restrictions of a multitrack view, and its items are
        # placed elements like any others -- which is what makes a note in a
        # roll addressable, and therefore editable and undoable. Which
        # restrictions those are is the client's own business, so it rides in
        # the body's opaque config and the document never reads it.
        return _with_config({
            "kind": "aggregate",
            "grouping": CONCRETE,
            "members": [
                _timeline_member(beat, item, ids)
                for beat, item in _timeline_items(element.wraps)
            ],
        }, _named({"form": FORM_TRACK,
                   # The **window** onto the timeline, written only when there
                   # is one -- the beats counterpart of a vector's `start`, and
                   # through the same door: the config carries what the document
                   # does not interpret. A track saying nothing about a window
                   # reads its timeline from the beginning, which is every track
                   # written before windows existed.
                   "start": float(element.start) if element.start else None}))
    if isinstance(element, Clang):
        return _with_config({"kind": "clang"}, _plain(_item_data(element.wraps)))
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
    if isinstance(element, Segments):
        # Several windows read as one: the source is the **list**, each entry
        # naming its own source and its own window into it. One node, because
        # what this element is is one thing to play.
        body = {
            "kind": "segments",
            "segments": [
                {
                    # A window is onto samples, which the document names by
                    # reference, or onto **content** -- a timeline it holds --
                    # which it names by node. The run says which; the shape on
                    # the wire says it back.
                    "source": (_source(seg.source) if element.duration_unit == SECONDS
                               else {"node": _content_id(seg.source, shared, ids)}),
                    "start": float(seg.start),
                    "duration": float(seg.duration),
                }
                for seg in element.segments
            ],
        }
        config = {}
        if element.instrument is not None:
            instrument = _reference(element.instrument)
            if instrument is not None:
                config["instrument"] = instrument
        if element.controls:
            config["controls"] = _plain(element.controls)
        return _with_config(body, config or None)
    if isinstance(element, Vector):
        body = {"kind": "vector", "source": _source(element.wraps)}
        config = {}
        if element.instrument is not None:
            instrument = _reference(element.instrument)
            if instrument is not None:
                config["instrument"] = instrument
        if element.controls:
            config["controls"] = _plain(element.controls)
        # The **window** onto the source, written only when it is not the
        # whole of it: a document saying nothing about a window means one that
        # reads the buffer from its first frame, which is every take written
        # before windows existed.
        if element.start:
            config["start"] = float(element.start)
        if element.loop:
            config["loop"] = True
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
    # came back), so a resolver that recognized the source once stopped
    # recognizing it on the second open.
    return _with_config({"kind": "generator"},
                        _named({"generator": _reference(element.wraps, element),
                                "points": _points_of(element.wraps)}))


#: The mixing keys a node's configuration carries, and their defaults. A
#: configuration is written **whole**, so a key that is not there is the
#: default -- audible, unsoloed, at unit gain.
MIXING = {"mute": False, "solo": False, "level": 1.0}


def _body(element, ids: _Ids, shared=None) -> dict:
    """One element's body — what kind of thing it is — with the **mixing** the
    composition holds over it laid into its configuration.

    Mute, solo and level go through the same opaque door a leaf's code and a
    track's restrictions use: the document carries them and never reads them,
    because what a level *means* is the client's. They ride in the config
    rather than beside the temporal keys so that `leaf_config` picks them up —
    a `Configure` intent replaces a configuration whole, and one that started
    from a config without them would silence-then-unsilence a lane on every
    curve edit.
    """
    body = _kind_body(element, ids, shared)
    mixing = mixing_of(element)
    if mixing:
        config = dict(body.get("config") or {})
        config.update(mixing)
        body["config"] = config
    return body


def mixing_of(element) -> dict:
    """What of `MIXING` this element states — only what differs from the
    default, so an ordinary element writes no mixing at all and a file written
    before mixing existed reads back identical."""
    stated = {}
    for key, default in MIXING.items():
        value = getattr(element, key, default)
        value = float(value) if key == "level" else bool(value)
        if value != default:
            stated[key] = value
    return stated


def set_mixing(element, config: dict) -> None:
    """Write a node's mixing onto the element, **whole**: a key the
    configuration does not carry is the default, which is the same rule every
    other `Configure` follows."""
    element.mute = bool(config.get("mute", False))
    element.solo = bool(config.get("solo", False))
    element.level = float(config.get("level", 1.0))


def _preserved(element):
    """The raw node a `from_document` kept for a body this build cannot name, or
    ``None`` for an element it understands."""
    if type(element) is Element and isinstance(element.wraps, dict):
        return element.wraps
    return None


def _member(handle, ids: _Ids, shared=None) -> dict:
    """One placement: where it sits, and the node it holds — whose id is the
    **handle's**, so one element placed twice is two windows and not one
    ambiguous name."""
    _placeable_twice(handle, ids)
    member = {"offset": float(handle.offset),
              "node": _node(handle.element, ids, handle, shared=shared)}
    if handle.dur is not None:
        member["dur"] = float(handle.dur)
    return member


def _placeable_twice(handle, ids: _Ids):
    """Refuse a *second* placement of an element whose source is in the node.

    Two windows share source only when the node **references** it — a buffer
    names a source, a generator names a recipe, and both placements point at the
    one thing. A clang, a track or an aggregate carries its source *inside* the
    node, so a second placement is a second **copy**: they diverge on the first
    edit, which is the answer the open decision rejected. Refused with the
    distinction rather than copied in silence.
    """
    element = handle.element
    if id(element) not in ids.placed:
        ids.placed.add(id(element))
        return
    if isinstance(element, (Vector, Generator)) or (
            isinstance(element, Sequence) and not isinstance(element.wraps, (list, tuple))):
        return  # a window onto source the node only names
    raise ValueError(
        f"{type(element).__name__} is placed more than once, and its source is "
        "in the node rather than named by it — two placements would be two "
        "copies that diverge on the first edit. Place a leaf that *references* "
        "its source (a Vector over one server buffer, a Generator over one "
        "recipe), or give each placement its own element."
    )


def _timeline_member(beat, item, ids: _Ids) -> dict:
    """A timeline item as a placed clang, with an id stamped on the item itself
    so it survives to the next conversion.

    **Whatever the item is.** A clang is "parameters or actions that happen
    together" and its configuration is the client's own terms, which is what an
    OSC marker and a raw MIDI message are as much as a note — so all three
    travel as the one description `clausters.seq.item_data` writes, and come
    back as themselves. Handing the item over raw wrote a marker as the *name*
    that answered for it, which reopened as a note with no parameters: a lane a
    piece could draw and not save."""
    node = {"id": ids.of(item)}
    node.update(_with_config({"kind": "clang"}, _plain(_item_data(item))))
    return {"offset": float(beat), "node": node}


def _item_data(item):
    """One timeline item as the config a clang carries — the shared
    description, falling back to the item itself for anything it has none of
    (which `_plain` then writes as the reference it always did)."""
    from ..seq.timeline import item_data

    data = item_data(item)
    return item if data is None else data


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
    """A source a document names and this process does not hold.

    A `Vector` element wraps `clausters.defs.Buffer`; reading a document written
    elsewhere (or written here before the buffer was allocated) gives the
    reference and not the object. Rather than losing it, the element wraps this:
    the same `bufnum` a real buffer answers with, plus the lifetime and
    generation the document carried, so a re-conversion is faithful and a caller
    that *can* resolve it does so through ``resolve``."""

    def __init__(self, source: dict, entry: "dict | None" = None):
        self.bufnum = int(source.get("source", 0))
        self.lifetime = source.get("lifetime", "session")
        self.generation = int(source.get("generation", 0))
        #: What the **session's table** said about this source, when it was read
        #: from one: where the samples are, and their shape. It is what makes
        #: opening a session and saving it again keep every location it was
        #: given -- without it, a piece opened with no resolver (or with one
        #: that could not read a file) would be written back with every take
        #: marked volatile, which is a format that loses its own contents on the
        #: second save.
        self.path = None
        self.frames = 0
        self.channels = 0
        self.sample_rate = 0.0
        self.locate(entry)

    def locate(self, entry: "dict | None") -> None:
        """Take where and what this source is from a session table entry."""
        if not entry:
            return
        location = entry.get("location") or {}
        if location.get("at") == "file" and location.get("path"):
            self.path = str(location["path"])
        self.lifetime = entry.get("lifetime", self.lifetime)
        self.generation = int(entry.get("generation", self.generation) or 0)
        self.frames = int(entry.get("frames", 0) or 0)
        self.channels = int(entry.get("channels", 0) or 0)
        self.sample_rate = float(entry.get("sample_rate", 0.0) or 0.0)


def _source(buffer) -> dict:
    """A buffer element's source. A server buffer the user allocated is
    **session** source -- neither the external-file rule nor a scratch copy --
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


def leaf_node(element) -> dict:
    """A leaf's **whole node body**, exactly as `to_document` writes it — its
    kind, its source and its configuration, with no id (the id belongs to the
    placement that holds it).

    Public for the same reason `leaf_config` is, one step further out: an edit
    that replaces *what a placement holds* — a run of clips joined into one
    element — states the result as a member list, and a member carries the node.
    Re-deriving that in the editor would be a second description of what a leaf
    is written as.
    """
    body = dict(_body(element, _Ids(element)))
    body.pop("id", None)
    return body


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
    author writes for source that has none of its own — a `Pbind` is code and
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


def _element(node: dict, resolve, *, placed: bool = False, content=None):
    kind = node.get("kind")
    config = node.get("config") or {}
    onset = node.get("onset")
    duration = node.get("duration")

    if kind == "aggregate" and config.get("form") == FORM_TRACK:
        # A set the author wrote as a `Track`, said by the body's own config.
        # Rebuilding it as an `Aggregate` is what made a reopened piece grow a level
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
        built = Track(timeline, onset=onset, duration=duration,
                      start=config.get("start", 0.0) or 0.0)
    elif kind == "aggregate":
        aggregate = Aggregate(
            kind=LOGICAL if node.get("grouping") == LOGICAL else CONCRETE,
            onset=onset,
            duration=duration,
            buses=config.get("buses") or None,
        )
        for member in node.get("members", []):
            child = member["node"]
            handle = aggregate.add(
                _element(child, resolve, placed=True, content=content),
                offset=member.get("offset", 0.0),
                dur=member.get("dur"),
            )
            if "id" in child:
                # The placement's id, on the placement: a second window onto the
                # same source is a second handle with a number of its own.
                setattr(handle, ID_ATTR, int(child["id"]))
        built = aggregate
    elif kind == "clang":
        from ..seq.timeline import item_from_data

        # An OSC marker and a raw MIDI message are clangs too, and each names
        # itself in its config -- see `_timeline_member`.
        built = Clang(item_from_data(config), onset=onset, duration=duration)
    elif kind == "sequence":
        members = node.get("members")
        if members:
            items = [_element(m["node"], resolve, content=content) for m in members]
        else:
            items = _resolved(resolve, "sequence", config) or config.get("sequence")
        built = Sequence(items, onset=onset, duration=duration)
    elif kind == "segments":
        windows = list(node.get("segments") or ())
        over_nodes = [w for w in windows if isinstance(w.get("source"), dict)
                      and "node" in (w.get("source") or {})]
        if over_nodes:
            # **A window onto content**: the notes are a node of this
            # document, built once and shared by every window that names it, so
            # this element is a `Track` reading that timeline from `start`. Its
            # own length is the node's -- absent when nobody stated one -- and
            # the window's is how much of the notes it can show, which is
            # what a reader that does not resolve the content lays it out with.
            def timeline_of(window):
                held = (content or {}).get(int(window["source"]["node"]))
                found = getattr(held, "wraps", None)
                if found is None:
                    raise ValueError(
                        "a window names content node "
                        f"{window['source']['node']}, which this document does "
                        "not hold: a window and the notes it reads are written "
                        "together"
                    )
                return found

            if len(over_nodes) == 1:
                window = over_nodes[0]
                built = Track(timeline_of(window), onset=onset,
                              duration=duration,
                              start=float(window.get("start", 0.0)))
            else:
                # Several windows onto timelines, read back to back: what a join
                # across them makes. The element places the run; the run is what
                # knows the windows.
                built = Segments(
                    NoteSegments([(timeline_of(w), float(w.get("start", 0.0)),
                                   float(w.get("duration", 0.0)))
                                  for w in over_nodes]),
                    onset=onset, duration=duration)
        else:
            built = Segments(
                [
                    (
                        _resolved(resolve, "vector", seg.get("source"))
                        or FrozenSource(seg.get("source") or {}),
                        seg.get("start", 0.0),
                        seg.get("duration", 0.0),
                    )
                    for seg in windows
                ],
                onset=onset,
                duration=duration,
                instrument=config.get("instrument"),
                controls=config.get("controls"),
            )
    elif kind == "vector":
        built = Vector(
            _resolved(resolve, "vector", node.get("source"))
            or FrozenSource(node.get("source") or {}),
            onset=onset,
            duration=duration,
            instrument=config.get("instrument"),
            controls=config.get("controls"),
            start=config.get("start", 0.0),
            loop=config.get("loop", False),
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
    # constructor may derive a duration (a `form.Clang` takes the event's `dur`
    # when none is given), and letting that win would make a document say
    # something the document did not say.
    built.onset = None if onset is None else float(onset)
    built.duration = None if duration is None else float(duration)
    if node.get("resident"):
        built.resident = True
    # The composition's mixing, restored the way it was written: whole, so a
    # document that says nothing says the audible default.
    set_mixing(built, config)
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
    """Put a carried curve back onto the source that was handed to us.

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
