"""The data adapter: one structure's own vocabulary, on both sides of an edit.

An editor orchestrates; a **domain** is what it orchestrates over. Given a
gesture it says what payload that gesture is in the structure's vocabulary,
given a payload it writes it onto the client object, and it names the entry an
undo menu shows. Three answers, one per structure kind — a break-point curve,
a buffer's samples, a timeline of events.

Two things it deliberately does not do, and both are boundaries rather than
omissions:

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
    #: ``"events"``. It is carried by the history and read by nothing in the
    #: crate; what reads it is whoever routes a leg the pile hands back.
    name = ""

    def payload(self, structure, tag: str, values) -> "dict | None":
        """The gesture as a payload in this vocabulary, or ``None`` when the
        tag is not this domain's.

        ``None`` is the ordinary answer, not a failure: a view emits tags for
        everything it can do and a domain answers for the ones that are edits
        of *its* structure.
        """
        raise NotImplementedError

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
        """What an undo menu calls this edit."""
        return "edit"

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
