# pyright: basic
"""The multitrack's session file: the envelope every application's file shares,
with what the multitrack keeps in it.

What is the multitrack's is three things: the `clausters.document.Multitrack`
itself, how each window was looking at it (`MultitrackView`) and where a pass
over it ends. They are read out of the envelope's contents and written back
into it, so the JSON is the one the format has always had -- ``multitrack``,
``views`` and ``end`` beside ``sources`` at the top level -- and a file another
application writes over the same envelope is read by that application, not by
this one. The web client's twin is ``gui/editing/multitrack-session.ts``.
"""

from dataclasses import dataclass, field

from ...document import Multitrack, SessionFile
from .multitrack_view import MultitrackView

__all__ = ["MultitrackSession"]


@dataclass(eq=False)
class MultitrackSession(SessionFile):
    """**A multitrack, saved**: the `clausters.document.SessionFile` envelope,
    the multitrack's tracks, its windows' views and where a pass over it ends.

    A `clausters.document.Multitrack` says *what plays when* and deliberately
    does not say where a source lives, because inside a running system a
    source is a server buffer, a mapped file or a rendered result and the
    multitrack has no business knowing which. The envelope's source table is
    that missing half.

    The `document` field carries the general tree, beside the multitrack and
    never inside it: a multitrack's regions are windows onto sources, and no
    region places a tree.
    """

    #: The application a multitrack's session file belongs to, as its
    #: metadata names it.
    APPLICATION = "multitrack"

    metadata: dict = field(default_factory=lambda: {"application": "multitrack"})
    #: The multitrack. Always present, possibly empty -- which mirrors the crate,
    #: where an absent multitrack reads as an empty one rather than as nothing.
    multitrack: Multitrack = field(default_factory=lambda: Multitrack())
    #: How the multitrack was being **looked at**: one entry per window. Carried
    #: because reopening a multitrack into the window it was left in is what a
    #: person expects, and a reader that ignores it opens the same multitrack.
    views: list = field(default_factory=list)
    #: **Where a pass over the multitrack ends**, as a playback's ``end`` says
    #: it: ``None`` (it rolls on, the default), ``"contents"`` (where the last
    #: region ends) or a number of seconds, an end marker. The transport's own
    #: switch, kept so a session opened again stops where it stopped before --
    #: not in a view, since it changes what is heard. It bounds no axis: how
    #: far a window shows or zooms out reads nothing here.
    end: "str | float | None" = None

    def dangling(self) -> list:
        """Sources the multitrack names but the table does not hold -- what an
        opening reader reports rather than discovering one element at a time.

        **Every** take lane is walked and not only the ones that play: an
        alternate take names its source whether or not anyone has chosen it
        yet.
        """
        missing = []
        for region in self.multitrack.regions():
            named = (region.content.window or {}).get("source")
            id = named.get("source") if isinstance(named, dict) else None
            if id is not None and int(id) not in self.sources \
                    and int(id) not in missing:
                missing.append(int(id))
        return missing

    def _written_contents(self) -> dict:
        out: dict = {}
        written = self.multitrack.write()
        if written:
            out["multitrack"] = written
        if self.views:
            out["views"] = [v.write() for v in self.views]
        if self.end is not None:
            out["end"] = self.end if self.end == "contents" else float(self.end)
        return out

    def _read_contents(self) -> None:
        """Takes the multitrack's fields out of the contents read.

        Raises:
            ValueError: when the file's metadata names another application
                than the multitrack.
        """
        if not self.belongs_to(self.APPLICATION):
            raise ValueError(
                f"the file belongs to the {self.metadata.get('application')} "
                "application, not to the multitrack")
        contents = dict(self.contents)
        # `arrangement` is what the field was called before 2026-09-08, read
        # so a session written then still opens.
        written = contents.pop("multitrack", None)
        older = contents.pop("arrangement", None)
        self.multitrack = Multitrack.read(written or older or {})
        self.views = [MultitrackView.read(v) for v in contents.pop("views", [])]
        self.end = contents.pop("end", None)
        self.contents = contents
