"""The data adapter: one structure's own vocabulary, on both sides of an edit.

An editor orchestrates; a **domain** is what it orchestrates over. Given a
gesture it says what that gesture needs read with it, and given an applied
payload it writes it onto the client object. Two answers, one per structure
kind — a break-point curve, a buffer's samples, a timeline of events, a
multitrack — and they are the two halves a language genuinely owns.

Three things it deliberately does not do, and all of them are boundaries rather
than omissions:

- **It does not read a gesture itself.** What a tag and a flat list of values
  *mean* is `clausters._native.editing_intake`, in the shared crate, for the
  same reason the inverse is: sixteen small readers written twice, once per
  language, are sixteen chances for two clients to disagree about what a
  septuple says. What a domain adds is the **request** — what that vocabulary
  needs beside the report, which is the multitrack, the timeline, an axis or nothing
  at all.
- **It does not know how an edit inverts.** That is `history::Editable` in the
  shared crate (`apply`, `current`, `coalesce_key`), because an inverse written
  once per language is an inverse that disagrees with itself. What a domain
  asks the crate for is `current` — the state a payload is about to replace,
  which is the inverse — and hands the pair to the history.
- **It does not draw.** A picture of a curve is a `clausters.gui.editing.View`,
  and the two are separate because one structure is drawn several ways (a curve
  is a `bpf` on its own and a body inside a clip) while its vocabulary is one.
"""

from ... import _native


class Domain:
    """What one kind of structure is, to an editor.

    Subclass it per structure kind; `name` is the vocabulary its payloads are
    written in, which is what `clausters.gui.editing.Editor` registers with the
    history and what routes a leg coming back out of one.
    """

    #: The crate's own name for this vocabulary — ``"points"``, ``"samples"``,
    #: ``"events"``, ``"multitrack"``. It is carried by the history, and it is
    #: what `clausters._native.editing_intake` answers in.
    name = ""

    #: Whether the crate reads this vocabulary's gestures.
    #:
    #: True for the four structures it knows, and **false for a domain written
    #: outside it** — a script's own `Domain` over its own object, which the
    #: editing surface has always accepted. Such a domain answers with `payload`
    #: and `label` as it always did, and `read` assembles the same shape out of
    #: them, so nothing downstream can tell the two apart.
    ingested = False

    def __init__(self):
        #: What the last `read` came to, held for the length of one gesture.
        #:
        #: The editor asks once and then wants three things off the answer —
        #: the payloads, the label, and whether the run carried its own
        #: inverse — and asking the crate again for each would be three reads
        #: of one gesture.
        self._taken: dict = {}

    def request(self, structure, tag: str, values) -> dict:
        """What this vocabulary needs beside the report, as
        `clausters._native.editing_intake` reads it.

        The default is the report alone, which is what the two stateless
        vocabularies take. A domain over a structure the reading depends on —
        the multitrack, the timeline — states it here, and so does one whose axis is
        the view's.
        """
        return {"values": list(values)}

    def read(self, structure, tag: str, values) -> dict:
        """**What a gesture means**, in this vocabulary — the one door.

        ``{"payloads": [...], "label": str}``, with ``inverse`` where the
        gesture carried one and ``refusal`` where the gesture *is* this
        domain's and cannot be written. No payloads and no refusal is "nothing
        to say", which is the ordinary answer rather than a failure: a view
        emits tags for everything it can do and a domain answers for the ones
        that are edits of *its* structure.
        """
        if self.ingested and self.name:
            self._taken = _native.editing_intake(
                self.name, tag, **self.request(structure, tag, values))
            return self._taken
        payloads = self.payloads(structure, tag, values)
        out = {"payloads": payloads,
               "label": self.label(payloads[0]) if payloads else "edit"}
        reason = self.refusal(structure, tag, values)
        if reason is not None:
            out["refusal"] = reason
        self._taken = out
        return out

    def payload(self, structure, tag: str, values) -> "dict | None":
        """The gesture as a payload in this vocabulary, or ``None`` when the
        tag is not this domain's.

        The singular door, and what a domain written outside the crate
        implements. `read` is what an editor actually goes through — a report
        is the whole structure for two of the four vocabularies, so one message
        is however many edits it takes.
        """
        if self.ingested:
            found = self.payloads(structure, tag, values)
            return found[0] if len(found) == 1 else None
        raise NotImplementedError

    def payloads(self, structure, tag: str, values) -> list:
        """The gesture as **however many payloads it takes**, in order.

        The plural door, and the default is the singular one wrapped: most
        gestures are one edit, and a domain that never needs more never mentions
        this. What needs it is a report that states the *whole structure* — a
        multitrack's boxes after a block drag, where one message says a move, a
        trim and a lane's new contents at once — and those are one entry in the
        history, because they are one thing a hand did.
        """
        if self.ingested:
            return self.read(structure, tag, values)["payloads"]
        payload = self.payload(structure, tag, values)
        return [] if payload is None else [payload]

    def refusal(self, structure, tag: str, values) -> "str | None":
        """Why a gesture this domain *does* understand cannot be written —
        ``None`` when there is no such case.

        The difference from `payload` answering ``None`` is the whole of it: a
        tag that is not this domain's is nothing, and the host goes on drawing
        what it drew because nothing here disagrees. A tag that *is* this
        domain's and cannot be honoured is a **refusal**, and a refusal the
        host is not told about leaves the picture and the data disagreeing
        silently — the one failure the acknowledgement exists to make
        impossible. What comes back is the sentence the user is shown.
        """
        return self.read(structure, tag, values).get("refusal") \
            if self.ingested else None

    def current(self, structure, payload: dict) -> "dict | None":
        """The state ``payload`` is about to replace — **the inverse**.

        Read before the edit lands, which is why it is a method here rather
        than something an editor derives afterwards: after the write there is
        nothing left to read.
        """
        raise NotImplementedError

    def project(self, structure, payload: dict) -> bool:
        """Write a payload onto the client object, and say whether it changed
        anything.

        The one door, so an edit, the projection of an inverse and the adoption
        of a redone state cannot disagree about which of the three happened —
        the rule the arrangement's editor already follows for a curve.
        """
        raise NotImplementedError

    def label(self, payload: dict) -> str:
        """What an undo menu calls this edit.

        **It travels with the gesture and not with the payload**, because for
        one vocabulary it is not a function of the payload at all: both of a
        roll's lanes state the same whole-list intent, so only the gesture knows
        whether a hand edited the notes or the markers. `read` is what states
        it; this reads it back off the last one.
        """
        return str(self._taken.get("label") or "edit") if self.ingested else "edit"

    def coalesce_key(self, payload: dict) -> "str | None":
        """What makes two edits *the same thing done the same way*, so a run of
        small adjustments becomes one undo.

        The crate's answer by default: one vocabulary, one key, in the shared
        implementation both clients bind. A domain with no key never coalesces,
        which is the safe end of the trade.
        """
        if not self.name:
            return None
        key = _native.domain_coalesce_key(self.name, payload)
        return key or None
