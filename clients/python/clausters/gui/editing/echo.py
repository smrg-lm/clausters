"""The acknowledgement protocol: what a view is told about the edit it made.

A host draws what the hand did and then waits to be told what actually
happened — the edit as applied, snapped, or refused — and every editor owes it
the same three things: the **version** the composition is at, the
**corrections** its own gesture did not survive intact, and the **reason** when
one is owed. That triple is the whole of this module, and it knows nothing
about what was edited: a stamp, a floor and a list of props.

It is separate because it is the one part of an editor with no data behind it.
`Echo` is exercised by a test that never builds a structure, which is what a
protocol should cost to check.
"""


class Echo:
    """One view's end of the acknowledgement protocol.

    Args:
        host: the `clausters.gui.host.GuiHost` to answer, or ``None`` for an
            editor with no window — which answers by doing nothing, since there
            is nobody to tell.
        version: a zero-argument callable answering the composition's current
            version. A callable rather than a number because the version
            belongs to the **editing context** and moves under this object:
            two windows over one composition read one counter, and a copy kept
            here would be a second answer to a question with one.
    """

    def __init__(self, host=None, version=None):
        self.host = host
        self._version = version if version is not None else (lambda: 0)
        #: The **oldest version an incoming edit may name**: raised whenever the
        #: composition moves by a route that is not a host event, and by nothing
        #: else. See `stale`, the only thing that reads it.
        self.floor = int(self._version())
        #: What the host should be drawing instead of what it drew, collected
        #: while one event is routed and sent with its acknowledgement.
        self.corrections: list = []
        #: Why the last routed event did not do what it asked, if it did not. It
        #: rides with the acknowledgement, because a refusal with no reason
        #: teaches "sometimes it does not work" -- the one answer worse than no.
        self.reason: "str | None" = None

    @property
    def version(self) -> int:
        """The version an acknowledgement carries — the context's, read now."""
        return int(self._version())

    def announce(self):
        """Tell the host which version it is drawing, before any edit.

        A stamp of zero retires nothing -- the host's own numbering starts at
        one -- so this is purely the version, and it is what keeps the *first*
        gesture checked like every later one. Without it the host would name
        zero until the first acknowledgement came back, and the opening edit
        would be the one edit nobody could tell was stale."""
        if self.host is not None:
            self.host.ack(0, doc_version=self.version)

    def stale(self, against: int) -> bool:
        """Whether an edit made against version ``against`` has been overtaken.

        Zero is *unstated* rather than a version -- an older host, or one no
        owner has reported a version to -- and unstated applies unchecked, which
        is the behavior there was before there were versions at all.

        Overtaken means *by a route the host never saw*. Every version an editor
        makes while answering the host's own events is one the host is either
        about to be told or has been told already, so an edit naming one of them
        is an answer that had not arrived yet -- a drag's later frames, a second
        gesture begun inside one round trip. What raises the floor is a script's
        edit, a second editor's, a redefine, an undo: the cases where the picture
        the gesture was made against is gone."""
        return against != 0 and against < self.floor

    def raise_floor(self):
        """The composition moved by a route no gesture took, so what is in
        flight was made against a picture that is gone."""
        self.floor = self.version

    def correct(self, widget_id: int, **props):
        """What the host should be drawing instead of what it drew.

        Called while routing, when the editor did not do what the gesture asked
        -- snapped it to the grid, or refused it outright. The value travels
        with the acknowledgement in one bundle, which is what lets the host
        adopt it without a redefine."""
        self.corrections.append((int(widget_id), props))

    def clear(self):
        """Drop what has not been sent: one event's corrections are that
        event's."""
        self.corrections = []

    def acknowledge(self, seq: int, reason: "str | None" = None):
        """Answer the host for everything up to ``seq``.

        An editor snaps a placement to the musical grid and refuses an edit to a
        generator, and without this the host could learn neither -- so a note
        dragged onto read-only samples stayed drawn where the hand put it, and a
        clip landed half a grid step from where it was released. The stamp
        closes both, because it lets the host retire what it drew and adopt what
        actually happened.

        Every acknowledgement carries the composition's version, which is what
        the host names back on its next gesture -- that round trip is the whole
        of the staleness check, and it costs one integer."""
        if self.host is None:
            return
        if not seq and not self.corrections:
            return
        # A stamp of zero retires nothing, which is exactly what an **unasked**
        # push needs: an undo answers no gesture, so it carries values and a
        # version and takes no pending edit with it.
        if self.corrections:
            self.host.push(seq, *self.corrections, doc_version=self.version,
                           reason=reason)
        else:
            self.host.ack(seq, doc_version=self.version, reason=reason)
