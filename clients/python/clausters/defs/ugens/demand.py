"""Demand rate: streams that have a next value, not samples.

A demand UGen is pulled rather than run -- `demand` (or `duty`) asks for the next
value on a trigger and the stream advances only then -- and nesting them is how a
sequence is built.
"""

from .graph import ChannelList, Ugen
from .pan import _sources
from ._generated.demand import *  # noqa: F403 -- the builders that mirror the wire

# ---- demand rate (dr) ----


# A demand UGen is a *stream*: it has no samples, only a next value, and it
# yields one each time a driver asks. Its inputs may be streams too -- that is
# what makes a sequence of phrases, rather than of numbers, expressible -- so
# every builder below accepts another `d*` anywhere it accepts a number.
#
# ``repeats`` is how many the stream yields before it ends: **0 means
# endlessly**. sclang writes ``inf`` there, which a def cannot carry (the wire
# rejects a non-finite constant, and JSON has no spelling for one), so the count
# of none is the endless one. For a list source it counts *passes over the
# list*; for a random pick it counts *items* -- scsynth's own asymmetry, and the
# useful reading of each.


def _values(values):
    """The value list of a demand source, as a list or a `ChannelList`."""
    items = list(values.items if isinstance(values, ChannelList) else values)
    if not items:
        raise ValueError("a demand source needs at least one value")
    return items


def dseq(values, repeats=0.0) -> Ugen:
    """A demand sequence: yields ``values`` in order, ``repeats`` times
    (``0`` endlessly), then ends.

    A value may be another demand stream, and then it is *drained* rather than
    taken once -- ``dseq([dseries(3, 0, 1), 100])`` is four items -- and restarted
    when the sequence comes round to it again."""
    return Ugen("Dseq", [repeats, *_values(values)], rate="dr")


def drand(values, repeats=0.0) -> Ugen:
    """``repeats`` items drawn at random from ``values``, each pick independent
    of the last. Unlike `dseq`, the count is of items, not passes."""
    return Ugen("Drand", [repeats, *_values(values)], rate="dr")


def dxrand(values, repeats=0.0) -> Ugen:
    """`drand` that never picks the value it just used -- the same list without
    immediate repetition."""
    return Ugen("Dxrand", [repeats, *_values(values)], rate="dr")


def dshuf(values, repeats=0.0) -> Ugen:
    """``values`` shuffled **once** and then replayed in that order,
    ``repeats`` times. The shuffle is redrawn on a reset, not on each pass --
    that is what separates it from `drand`."""
    return Ugen("Dshuf", [repeats, *_values(values)], rate="dr")


def dswitch1(which, *sources) -> Ugen:
    """Takes **one** item from the stream ``which`` picks, then picks again.

    Unlike `dseq`, an unselected stream is not advanced and the selected one is
    not drained -- the ``1`` is the count. The index wraps into range. Accepts
    the sources as arguments or as one list."""
    return Ugen("Dswitch1", [which, *_sources(sources)], rate="dr")




def dser(values, repeats=0.0) -> Ugen:
    """``repeats`` items read from ``values`` in order, round and round (``0``
    endlessly): where `dseq` counts passes over the list, this counts items --
    ``dser([1, 2, 3], 5)`` is ``1 2 3 1 2``."""
    return Ugen("Dser", [repeats, *_values(values)], rate="dr")


def dwrand(values, weights, repeats=0.0) -> Ugen:
    """``repeats`` items drawn at random from ``values``, each as likely as its
    share of ``weights``. The weights need not sum to one; a value with no
    weight is never drawn, and a weight with no value is dropped."""
    items = _values(values)
    w = list(weights.items if isinstance(weights, ChannelList) else weights)
    w = (w + [0.0] * len(items))[: len(items)]
    return Ugen("Dwrand", [repeats, len(items), *w, *items], rate="dr")


def dswitch(which, *sources) -> Ugen:
    """Plays the **whole** stream ``which`` picks, then picks again: where
    `dswitch1` takes one item, this one drains. A plain number is a stream of
    one item, and a stream picked again plays again from its start. The index
    wraps into range. Accepts the sources as arguments or as one list."""
    return Ugen("Dswitch", [which, *_sources(sources)], rate="dr")


def dpoll(signal, trig_id=-1, run=1, *, label="dpoll") -> Ugen:
    """Passes the ``signal`` stream through and, while ``run`` is above zero,
    posts ``label: value`` for every item to the server console and, when
    ``trig_id >= 0``, also sends ``/node_trigger nodeID trig_id value`` --
    `poll` for a stream. ``run`` may be a stream too.

    ``label`` is a **static** field and is keyword-only, so the positional
    parameters are the wire's three inputs in the wire's order."""
    return Ugen("Dpoll", [signal, trig_id, run], rate="dr", label=label)
