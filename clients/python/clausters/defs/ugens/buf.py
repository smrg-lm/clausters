"""Buffers: playing them, reading them as tables, asking about them.

The players (`play_buf`, `buf_rd`), the table oscillators and waveshaper that
read a `/buffer_gen` table, and the init-rate queries that report a buffer's
shape into the graph.
"""

from .graph import Ugen
from ._generated.buf import *  # noqa: F403 -- the builders that mirror the wire

def record_buf(bufnum, chan, signal, offset=0.0, rec_level=1.0, pre_level=0.0,
               run=1.0, loop=0.0, trigger=0.0, done_action=0) -> Ugen:
    """**Records** ``signal`` into a buffer, one frame per sample -- the
    self-advancing writer, as `play_buf` is the self-advancing reader.

    ``rec_level`` and ``pre_level`` are what make it a looper rather than a tape
    head: each frame becomes ``signal*rec_level + old*pre_level``, so ``(1, 0)``
    overwrites, ``(1, 1)`` overdubs onto what is there and ``(1, 0.5)`` overdubs
    with the older layers fading.

    ``run`` at 0 holds the position and writes nothing, so a recording can be
    gated without losing its place; a rising ``trigger`` re-cues to ``offset``;
    without ``loop``, reaching the end stops the recording and fires
    ``done_action`` (`DoneAction.FREE_SELF` frees the node). Passes ``signal``
    through.

    Recording into a buffer another node is playing is the ordinary case -- a
    buffer's contents are mutable and only its shape is fixed.
    """
    return Ugen("RecordBuf", [bufnum, chan, signal, offset, rec_level, pre_level,
                              run, loop, trigger, int(done_action)])


def t_windex(trig, weights, normalize=0.0) -> Ugen:
    """At each rising edge of `trig`, an index drawn with the probabilities of
    `weights` (summing to 1, or divided by their sum with `normalize` 1) --
    the weighted choice a `select` then reads a list by. A draw is made at the
    start too. What unnormalized weights leave over goes to the last index."""
    return Ugen("TWindex", [trig, normalize, *weights])
