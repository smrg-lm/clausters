"""Splitting a client-side id space between clients of one server.

The server partitions node ids into a **client range**, its own auto range
(``/synth_new -1``, GraphDef members) and its MIDI voice range, all scaled from
``--max-nodes``, and every client allocates from that one client range. That is
exact while a server has one client, and a fiction the moment it has two: both
registries start at the same base, hand out the same first id, and the second
``/synth_new`` of the pair is refused as a duplicate — or, worse, accepted
against the other client's node. The same holds for buses, buffers and, on the
GUI host, widget ids.

An `IdShare` is how two clients that cannot talk to each other still agree,
and it works because there is nothing to negotiate: the shares are **equal
slices in a fixed order**, so ``IdShare(0, 2)`` and ``IdShare(1, 2)`` are
disjoint by arithmetic. Whoever arranges the two hands each its index.

It costs range, not capability: a share of two halves what either client may
hold live at once, which is why the default everywhere is `WHOLE`, the whole
space.
"""

__all__ = ["IdShare", "WHOLE", "share_of"]


class IdShare:
    """Which slice of a client-side id space a client takes.

    Args:
        index: this client's slice, from ``0`` to ``of - 1``.
        of: how many clients the space is split between.
    """

    __slots__ = ("index", "of")

    def __init__(self, index: int = 0, of: int = 1):
        if not isinstance(of, int) or of < 1:
            raise ValueError(
                f"an id share is split between 1 or more clients, not {of}")
        if not isinstance(index, int) or not 0 <= index < of:
            raise ValueError(f"id share {index} is outside a split of {of}")
        self.index = index
        self.of = of

    def __eq__(self, other):
        if not isinstance(other, IdShare):
            return NotImplemented
        return (self.index, self.of) == (other.index, other.of)

    def __hash__(self):
        return hash((self.index, self.of))

    def __repr__(self):
        return f"IdShare(index={self.index}, of={self.of})"


#: The whole space: what a server's only client takes.
WHOLE = IdShare(0, 1)


def share_of(base: int, span: int, share: "IdShare | None" = None):
    """The ``(base, span)`` of ``share`` within ``span`` ids at ``base``.

    The **last share takes the remainder**, so the slices tile the range
    exactly rather than leaving a few ids nobody may allocate. A share of a
    range too small to split yields an empty span, and an empty registry
    reports exhaustion from its first call — a client that cannot allocate says
    so, which is the failure this whole mechanism exists to make loud.
    """
    share = WHOLE if share is None else share
    each = span // share.of
    if share.index == share.of - 1:
        return base + share.index * each, span - share.index * each
    return base + share.index * each, each
