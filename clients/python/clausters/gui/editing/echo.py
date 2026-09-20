"""The acknowledgement protocol: what a view is told about the edit it made.

A host draws what the hand did and then waits to be told what actually
happened -- the edit as applied, snapped, or refused -- and every editor owes it
the same three things: the **version** the data is at, the
**corrections** its own gesture did not survive intact, and the **reason** when
one is owed. That triple is the whole of this module, and it knows nothing
about what was edited: a stamp, a floor and a list of props.

**The rules are the shared crate's** (`clausters._native.conversation_read`,
`clausters._native.conversation_answer`): what makes an edit stale, what moves
the floor, and whether an answer is an ack, a push or nothing at all. What is
here is the half a language owns -- holding the two integers between messages
and putting the answer on this client's socket.

It is separate because it is the one part of an editor with no data behind it.
`Echo` is exercised by a test that never builds a structure, which is what a
protocol should cost to check.
"""


from ... import _native
from .trace import log


class Echo:
    """One view's end of the acknowledgement protocol.

    Args:
        host: the `clausters.gui.host.GuiHost` to answer, or ``None`` for an
            editor with no window -- which answers by doing nothing, since there
            is nobody to tell.
        version: a zero-argument callable answering the data's current
            version. A callable rather than a number because the version
            belongs to the **editing context** and moves under this object:
            two windows over one structure read one counter, and a copy kept
            here would be a second answer to a question with one.
    """

    def __init__(self, host=None, version=None):
        self.host = host
        self._version = version if version is not None else (lambda: 0)
        #: The conversation's whole state, as the crate holds it: the **floor**
        #: (the oldest version an incoming edit may name) and the version the
        #: last answered event left behind. Two integers, kept here because
        #: something has to keep them between messages and handed back to the
        #: crate on every one.
        self.state = {"floor": int(self._version()),
                      "applied": int(self._version())}
        #: What the host should be drawing instead of what it drew, collected
        #: while one event is routed and sent with its acknowledgement.
        self.corrections: list = []
        #: Why the last routed event did not do what it asked, if it did not. It
        #: rides with the acknowledgement, because a refusal with no reason
        #: teaches "sometimes it does not work" -- the one answer worse than no.
        self.reason: "str | None" = None

    @property
    def version(self) -> int:
        """The version an acknowledgement carries -- the context's, read now."""
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

    @property
    def floor(self) -> int:
        """The **oldest version an incoming edit may name**.

        Raised whenever the data moves by a route that is not a host
        event, and by nothing else -- which is what makes staleness a monotone
        test rather than a race.
        """
        return int(self.state.get("floor", 0))

    def read(self, message: dict) -> dict:
        """**What one message from the host is**, and the two integers as they
        now stand (`clausters._native.conversation_read`).

        A close, a history step, an edit made against a picture that is gone, or
        an edit to route. The rules are the crate's, so a page and a script
        cannot disagree about which gestures are refused.
        """
        answer = _native.conversation_read(self.state, message)
        self.state = answer.get("state") or self.state
        return answer.get("turn") or {"turn": "nothing"}

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

        Every acknowledgement carries the data's version, which is what
        the host names back on its next gesture -- that round trip is the whole
        of the staleness check, and it costs one integer."""
        if self.host is None:
            return
        # **What to send is the crate's decision**, including that an unasked
        # push with nothing to say is one message the wire does not carry.
        self.send(_native.conversation_answer(seq, self.version, reason,
                                              self.corrections))

    def send(self, answer: "dict | None"):
        """Put an answer the crate decided on this client's socket: ``ack``,
        ``push``, or nothing for ``silent``.

        The half of `acknowledge` a language owns, and the whole of what an
        editor whose turns are the crate's needs from this object."""
        if self.host is None or not answer:
            return
        kind = answer.get("answer")
        if kind not in ("ack", "push"):
            return
        seq = int(answer.get("seq", 0))
        corrections = [(int(c["widget"]), c["props"])
                       for c in answer.get("corrections") or ()]
        version = int(answer.get("docVersion", self.version))
        why = answer.get("reason")
        log.debug("ack    seq=%s version=%s%s%s", seq, version,
                  "" if not corrections else " correcting " + ", ".join(
                      f"{wid}({' '.join(sorted(props))})"
                      for wid, props in corrections),
                  "" if why is None else f" reason={why!r}")
        if kind == "push":
            self.host.push(seq, *corrections, doc_version=version, reason=why)
        else:
            self.host.ack(seq, doc_version=version, reason=why)
