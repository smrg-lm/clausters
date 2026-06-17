"""Minimal OSC wire encoding (stdlib only).

The low-level byte layer: build OSC messages and timetagged bundles, and frame
an NRT score. It is deliberately tiny and matches the helpers in the repo's
``examples/json_client.py`` so scores produced here render identically. The
higher-level destination abstraction (RT/NRT/MIDI interfaces, `NetAddr`) lands
on top of this in milestone C2 (`base/_oscinterface.py`); the timetag↔sample
math lives in the native core (`clausters._native`).
"""

import struct
import time

NTP_UNIX_OFFSET = 2_208_988_800


def _pad(data: bytes) -> bytes:
    return data + b"\x00" * (-len(data) % 4)


def _string(s: str) -> bytes:
    return _pad(s.encode() + b"\x00")


class Int64:
    """Marker for an OSC int64 (`h`) argument — e.g. `/sched` sample targets."""

    def __init__(self, value: int):
        self.value = int(value)


def message(addr: str, *args) -> bytes:
    """Encodes one OSC message. Supports int (`i`), :class:`Int64` (`h`), float
    (`f`), str (`s`) and bytes (`b`) arguments."""
    tags, data = ",", b""
    for a in args:
        if isinstance(a, bool):
            raise TypeError("OSC has no bool tag here; use int")
        if isinstance(a, Int64):
            tags, data = tags + "h", data + struct.pack(">q", a.value)
        elif isinstance(a, int):
            tags, data = tags + "i", data + struct.pack(">i", a)
        elif isinstance(a, float):
            tags, data = tags + "f", data + struct.pack(">f", a)
        elif isinstance(a, str):
            tags, data = tags + "s", data + _string(a)
        elif isinstance(a, bytes):
            tags, data = tags + "b", data + struct.pack(">i", len(a)) + _pad(a)
        else:
            raise TypeError(f"unsupported OSC argument: {a!r}")
    return _string(addr) + _string(tags) + data


def _timetag(ntp_seconds: float) -> bytes:
    return struct.pack(">II", int(ntp_seconds), int((ntp_seconds % 1.0) * 2**32))


def bundle(seconds_ahead: float, *packets: bytes) -> bytes:
    """An RT bundle timetagged `seconds_ahead` from now (wall clock)."""
    return bundle_at(time.time() + seconds_ahead, *packets)


def bundle_at(unix_seconds: float, *packets: bytes) -> bytes:
    """An RT bundle timetagged at an absolute Unix instant (wall clock)."""
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + _timetag(unix_seconds + NTP_UNIX_OFFSET) + body


def score_bundle(seconds: float, *packets: bytes) -> bytes:
    """A bundle for an NRT score: the timetag counts seconds from the start of
    the render, not wall-clock time."""
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + _timetag(seconds) + body


def score(*bundles: bytes) -> bytes:
    """Frames bundles into the binary NRT score (`[i32 len][packet]…`)."""
    return b"".join(struct.pack(">i", len(b)) + b for b in bundles)
