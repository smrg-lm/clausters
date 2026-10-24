"""What a take's picture measures, and the wire's samples as numbers.

**What the picture measures is the view's.** A waveform is drawn as a stack of
measures over one field -- what the signal reached (``peak``) with what it held
inside that (``rms``) -- and that is a prop of the one widget rather than a pile
of widgets: every view of a signal paints its own field before it draws, so two
of them on one rectangle are not layers, the second hides the first. Measuring
twice into one body is also what makes the rest of it one thing: one axis, one
ruler, one selection, one playhead, one upload of the samples.

The editor whose picture these measure is `clausters.gui.editing.AudioEditor`.
"""

import struct
from array import array

from ... import _native


#: The editor's default stack: what the signal reached and held inside it.
#: Other visual layers are validated by the shared Rust view vocabulary.
MEASURES = ("peak", "rms")


def measures(stack) -> tuple:
    """A visual layer stack as a tuple, or a `ValueError` naming what is wrong.

    A stack is written by hand, so a silent typo is a layer that quietly does
    not appear, and an empty one is a picture that measures nothing. The check
    is the crate's (`clausters._native.samples_measures`).
    """
    return _native.samples_measures(stack)


def _plain(value):
    """An event's arguments as JSON carries them: a blob is the run a stroke
    wrote or replaced, read into its numbers -- the wire's framing is this
    client's."""
    if isinstance(value, (bytes, bytearray, memoryview, array)):
        return _floats(value)
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    return value


def _floats(blob) -> list:
    """A run of samples as floats, from the little-endian ``f32`` blob the wire
    carries (or from a list, which is what a hand-written test sends)."""
    if isinstance(blob, (bytes, bytearray, memoryview)):
        raw = bytes(blob)
        return list(struct.unpack(f"<{len(raw) // 4}f", raw[:len(raw) // 4 * 4]))
    if isinstance(blob, array):
        return list(blob)
    if isinstance(blob, (list, tuple)):
        return [float(v) for v in blob]
    return []
