# pyright: basic
"""The session file: what every application's file holds, and its source table.

Private: the names are `clausters.document`'s.
"""

import json
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from ._format import SESSION_FORMAT, _unknown_fields

if TYPE_CHECKING:
    from ..seq.sequence import EventSequence



@dataclass
class Source:
    """One entry in a session's source table: where samples are, how long they
    live, and what shape they have.

    The two fields a naive format leaves out and then cannot add are here:
    `provenance`, a reference to whatever produced the samples, carried opaquely
    so re-generating stays possible *without the document knowing how*; and
    `editing`, a destructive edit that has not been confirmed -- a save never
    blocks on a confirmation, so a saved session has to be able to say *this is
    a working copy of that, and the person has not decided yet*.
    """

    #: ``{"at": "file", "path": ...}``, ``{"at": "volatile"}`` or ``{"at":
    #: "events"}`` (the events are `sequence`). A relative path
    #: is resolved against the session's own folder, which is what makes a
    #: session directory movable; an absolute one names the user's own file,
    #: which a session must never copy or rewrite.
    location: dict
    #: ``"external"`` (the user's own file), ``"session"`` (saved beside the
    #: document) or ``"temporary"`` (a working copy that dies with the edit).
    lifetime: str = "session"
    #: Which generation of the content this is -- bumped by a destructive edit,
    #: so a reader holding an older copy knows to re-read.
    generation: int = 0
    channels: "int | None" = None
    frames: "int | None" = None
    #: Carried, never acted on: resampling is an edit.
    sample_rate: "float | None" = None
    provenance: "dict | None" = None
    #: ``{"from": source_id, "confirmed": bool}`` while a destructive edit is
    #: open over these samples.
    editing: "dict | None" = None
    extra: dict = field(default_factory=dict)
    #: **The events, when the source is a sequence of them**: the
    #: `clausters.seq.EventSequence` itself, held rather than copied, so what an
    #: editor does to it is what the next save writes. Left out of equality:
    #: a handle is compared by what it holds, which `write` says.
    sequence: "EventSequence | None" = field(default=None, compare=False)

    @classmethod
    def file(cls, path: str, lifetime: str = "session") -> "Source":
        """Samples in a file."""
        return cls(location={"at": "file", "path": path}, lifetime=lifetime)

    @classmethod
    def volatile(cls, lifetime: str = "session") -> "Source":
        """Samples that have not been written down -- a buffer never exported, a
        result never saved. A session may hold one, because saving must not be
        blocked by it, but a reader that finds one knows the samples are not
        there and opens that element unresolved rather than pretending."""
        return cls(location={"at": "volatile"}, lifetime=lifetime)

    @classmethod
    def events(cls, sequence, lifetime: str = "session") -> "Source":
        """A sequence of events -- a `clausters.seq.EventSequence` -- held in
        the session file itself. A region over it is a window onto its beats
        (in seconds, through its tempo map), and it draws the notes."""
        return cls(location={"at": "events"}, lifetime=lifetime, sequence=sequence)

    def shaped(self, channels: int, frames: int, sample_rate: float) -> "Source":
        """Its shape, for a caller that knows it."""
        self.channels, self.frames, self.sample_rate = channels, frames, sample_rate
        return self

    @property
    def path(self) -> "str | None":
        """Where the file is, when the samples are in one."""
        if self.location.get("at") == "file":
            return self.location.get("path") or None
        return None

    @property
    def is_resolvable(self) -> bool:
        """Whether the samples are somewhere a reader could find them. A
        sequence is in the file itself, so it always is."""
        return self.sequence is not None or bool(self.path)

    @property
    def is_being_edited(self) -> bool:
        """Whether a destructive edit is open and undecided over these
        samples."""
        return bool(self.editing) and not self.editing.get("confirmed", False)

    def write(self) -> dict:
        location = self.location
        if self.sequence is not None:
            location = {"at": "events", "sequence": self.sequence.data()}
        out: dict = {"location": location, "lifetime": self.lifetime,
                     "generation": self.generation}
        for name in ("channels", "frames", "sample_rate", "provenance", "editing"):
            value = getattr(self, name)
            if value is not None:
                out[name] = value
        out.update(self.extra)
        return out

    @classmethod
    def read(cls, written: dict) -> "Source":
        known = ("location", "lifetime", "generation", "channels", "frames",
                 "sample_rate", "provenance", "editing")
        location = dict(written.get("location") or {"at": "volatile"})
        sequence = None
        if location.get("at") == "events":
            from ..seq.sequence import EventSequence

            sequence = EventSequence.from_data(location.pop("sequence", {}))
        return cls(
            location=location,
            sequence=sequence,
            lifetime=str(written.get("lifetime", "session")),
            generation=int(written.get("generation", 0) or 0),
            channels=written.get("channels"),
            frames=written.get("frames"),
            sample_rate=written.get("sample_rate"),
            provenance=written.get("provenance"),
            editing=written.get("editing"),
            extra=_unknown_fields(written, *known),
        )


class FrozenSource:
    """A source a session names and this process does not hold.

    Reading a session written elsewhere -- or written here before a buffer was
    allocated -- gives a reference and not an object. Rather than losing it, a
    region's window holds this: the same ``bufnum`` a real buffer answers with,
    plus what the table said about where the samples are and what shape they
    have, so a re-save keeps every location it was given.

    Without it, a multitrack opened with no way to read its files would be written
    back with every source marked volatile, which is a format that loses its own
    contents on the second save.
    """

    def __init__(self, id: int, entry: "Source | None" = None):
        self.bufnum = int(id)
        self.lifetime = "session"
        self.generation = 0
        self.path: "str | None" = None
        self.frames = 0
        self.channels = 0
        self.sample_rate = 0.0
        self.locate(entry)

    def locate(self, entry: "Source | None") -> None:
        """Take where and what this source is from a session's table entry."""
        if entry is None:
            return
        self.path = entry.path
        self.lifetime = entry.lifetime
        self.generation = entry.generation
        self.frames = int(entry.frames or 0)
        self.channels = int(entry.channels or 0)
        self.sample_rate = float(entry.sample_rate or 0.0)

    def __repr__(self) -> str:
        where = self.path or "volatile"
        return f"<FrozenSource {self.bufnum} {where}>"




#: The application a file written before ``metadata["application"]`` existed
#: belongs to: the multitrack, the one application that had a session file then.
_UNNAMED_APPLICATION = "multitrack"

#: What the envelope names; every other top-level field is the contents'.
_ENVELOPE = ("format", "metadata", "document", "sources", "provenance")


@dataclass
class SessionFile:
    """**A session file**: what every application's file holds, whatever the
    application keeps in it (the crate's `session::SessionFile`).

    The format, the application the file belongs to, the general tree, the
    table that says where each source's samples are and what produced the
    whole are the same for every application; what one application keeps
    beside them is its own and stays in `contents`, as written, for that
    application to read. The multitrack's is
    `clausters.gui.editing.MultitrackSession`, which adds its tracks, its
    windows' views and where a pass ends over this envelope; another
    application with a file adds its own the same way, rather than a second
    format.

    It is not `clausters.Session`, which holds a running server, a clock and a
    GUI host. This one is a **file**, and its name says so.
    """

    #: The format version this build writes -- `clausters.document.SESSION_FORMAT`,
    #: which is the crate's `session::FORMAT`. A session **read** in an older
    #: format is migrated to this one first (`read`), since its numbers are
    #: read differently: format 2 placed the multitrack in beats.
    format: int = SESSION_FORMAT
    #: What the file says about itself: ``{"application": ...}``, the
    #: application it belongs to. A file written before it was named belongs
    #: to the multitrack.
    metadata: dict = field(default_factory=dict)
    #: The general tree, for what no application's contents hold.
    document: "dict | None" = None
    #: Where each source is, keyed by source id.
    sources: dict = field(default_factory=dict)
    #: What produced the session as a whole -- the scripts behind it -- carried
    #: opaquely. The document never knows how to re-run them; it only has to
    #: not lose the reference.
    provenance: "dict | None" = None
    #: **What the application keeps**, and the fields a newer writer wrote:
    #: every top-level field the envelope does not name, as written. The
    #: application the file belongs to reads its own out of it; the rest is
    #: carried.
    contents: dict = field(default_factory=dict)
    #: The file this session was opened from or last saved to, or ``None``.
    #: Not written into the file: where a session is is not what it says.
    path: "str | None" = field(default=None, compare=False, repr=False)

    def __eq__(self, other) -> bool:
        """Two sessions are equal when they write the same file: a multitrack
        is a handle, and a handle is compared by what it holds."""
        if not isinstance(other, SessionFile):
            return NotImplemented
        return type(self) is type(other) and self.write() == other.write()

    def belongs_to(self, application: str) -> bool:
        """Whether the file belongs to ``application``."""
        return self.metadata.get("application") == application

    @classmethod
    def open(cls, path):
        """A session read from the file at ``path``, remembering where it came
        from: an application reads relative paths against that file's folder
        (`clausters.gui.edit`), and `save` writes back to it. Called on a
        subclass, it reads that application's file."""
        with open(path) as f:
            session = cls.read(json.load(f))
        session.path = str(path)
        return session

    def save(self, path=None) -> str:
        """Writes the session to ``path``, or back to the file it was opened
        from or last saved to, and answers where it went.

        Raises:
            ValueError: when there is no ``path`` and the session was never
                opened or saved.
        """
        target = str(path) if path is not None else self.path
        if target is None:
            raise ValueError("this session has nowhere to save to: give it a path")
        with open(target, "w") as f:
            f.write(json.dumps(self.write(), indent=1))
        self.path = target
        return target

    def source(self, id: int) -> "Source | None":
        """The source a reference names, if the table has it."""
        return self.sources.get(int(id))

    def sequences(self) -> dict:
        """The sources that are sequences of events: source id ->
        `clausters.seq.EventSequence`, the handles the table holds."""
        return {int(id): s.sequence for id, s in self.sources.items()
                if s.sequence is not None}

    def volatile(self) -> list:
        """Sources whose samples are not written down anywhere -- what a save
        consults before promising the file is complete."""
        return sorted(id for id, s in self.sources.items() if not s.is_resolvable)

    def open_edits(self) -> list:
        """Sources with a destructive edit still open and undecided."""
        return sorted(id for id, s in self.sources.items() if s.is_being_edited)

    def promote(self, id: int) -> bool:
        """Promotes a temporary working copy to one saved beside the document,
        **leaving the edit open**. What a save mid-edit does: auto-confirming
        would turn a save into an edit, and refusing until the edit is settled
        would make the safest habit in the program the one that is blocked."""
        source = self.sources.get(int(id))
        if source is None or source.lifetime != "temporary":
            return False
        source.lifetime = "session"
        return True

    def confirm(self, id: int) -> bool:
        """Confirms the edit open over a source: the working copy becomes the
        samples, and there is nothing left undecided about it."""
        source = self.sources.get(int(id))
        if source is None or not source.editing:
            return False
        source.editing = dict(source.editing, confirmed=True)
        return True

    def _written_contents(self) -> dict:
        """What the application writes into the envelope: nothing, for the
        envelope alone. An application's file answers its own fields."""
        return {}

    def _read_contents(self) -> None:
        """Takes the application's own fields out of `contents` after a read.
        Nothing, for the envelope alone."""

    def write(self) -> dict:
        """The session as the crate's JSON."""
        out: dict = {"format": self.format, "metadata": dict(self.metadata)}
        if self.document is not None:
            out["document"] = self.document
        if self.sources:
            out["sources"] = {str(id): s.write()
                              for id, s in sorted(self.sources.items())}
        if self.provenance is not None:
            out["provenance"] = self.provenance
        out.update(self._written_contents())
        out.update(self.contents)
        return out

    @classmethod
    def read(cls, written: dict):
        """A session file from the crate's JSON, migrated first when it was
        written in an older format. Called on a subclass, it reads that
        application's file, which refuses one another application's metadata
        names.

        The migration is the crate's (`clausters._native.session_migrate`), so
        an old file opens the same here, in the web client and in the GUI host.
        """
        if int(written.get("format", 1)) < SESSION_FORMAT:
            from .._native import session_migrate

            written = session_migrate(written)
        metadata = dict(written.get("metadata") or {})
        metadata.setdefault("application", _UNNAMED_APPLICATION)
        session = cls(
            format=int(written.get("format", 1)),
            metadata=metadata,
            document=written.get("document"),
            sources={int(id): Source.read(entry)
                     for id, entry in (written.get("sources") or {}).items()},
            provenance=written.get("provenance"),
            contents=_unknown_fields(written, *_ENVELOPE),
        )
        session._read_contents()
        return session
