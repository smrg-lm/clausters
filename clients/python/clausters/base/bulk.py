"""Bulk samples at the boundary: the little-endian ``f32`` blob, both ways.

**The rule this module exists to keep in one place.** A payload whose length
scales with the *audio* — a buffer range, a scope window, a waveform to draw —
crosses as raw little-endian ``f32``; a payload whose length scales with the
*parameters* stays typed OSC arguments (``docs/schemas.md``). The reason is not
tidiness: N samples as N float arguments costs N type tags and N encode steps at
each end, which is thousands of times slower than one byte copy at the sizes an
editor works with, and wider on the wire besides.

So every path that carries samples — ``/buffer_setRange`` and
``/buffer_getRange.reply``, ``/bus_tapStream.reply``, a ``waveform``'s ``blob``
prop, ``/buffer_export``'s file — goes through these two functions rather than
looping in Python. The loop that stays is `array`'s, which is C.

The one thing worth centralizing beyond speed is **endianness**: ``array('f')``
is host-endian, so on a big-endian machine the naive ``tobytes()`` is silently
wrong. That check belongs to the convention, not to each caller of it.
"""

import array
import sys

__all__ = ["samples_to_blob", "blob_to_samples"]

#: Whether this host's `array` already matches the wire's byte order, so the
#: pack and unpack are a straight copy (every ordinary target; the check is what
#: keeps the two correct where it is not).
_NATIVE_IS_LITTLE = sys.byteorder == "little"


def samples_to_blob(samples) -> bytes:
    """Packs an iterable of numbers into a little-endian ``f32`` blob.

    ``samples`` may be anything `array` accepts — a list, another ``array('f')``,
    what `blob_to_samples` returned. The conversion happens in C; nothing here
    touches a sample one at a time.
    """
    buf = array.array("f", samples)
    if not _NATIVE_IS_LITTLE:
        buf.byteswap()
    return buf.tobytes()


def blob_to_samples(blob) -> "array.array":
    """Unpacks a little-endian ``f32`` blob into an ``array('f')`` — the inverse
    of `samples_to_blob`, and what every reply carrying samples is read with.

    Raises `ValueError` when the blob is not a whole number of ``f32``s, which
    is the only way it can be malformed.
    """
    data = bytes(blob)
    if len(data) % 4:
        raise ValueError(
            f"a sample blob is little-endian f32: {len(data)} bytes "
            "is not a multiple of 4")
    buf = array.array("f")
    buf.frombytes(data)
    if not _NATIVE_IS_LITTLE:
        buf.byteswap()
    return buf
