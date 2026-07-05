"""Minimal OSC wire encoding.

The low-level byte layer: build OSC messages and timetagged bundles, and frame
an NRT score. It is deliberately tiny and matches the helpers in the repo's
``examples/json_client.py`` so scores produced here render identically. The
higher-level destination abstraction (RT/NRT/MIDI interfaces, `NetAddr`) sits
on top of this in `base/_oscinterface.py`.

The byte codec itself stays per-language (structured arguments cannot cross the
flat C ABI — the documented seam exception), but **every time value** goes
through the native core: timetag packing (`clausters._native.ntp_timetag` /
``unix_to_ntp``, fraction *rounded*) and the timetag↔sample math, so identical
instants produce identical bits in every client.
"""

import struct
import time

from .. import _native

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
    """Encodes one OSC message. Supports int (`i`), `Int64` (`h`), float
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
    # Packed by the shared core (fraction rounded, not truncated), so the bits
    # match any other client stamping the same instant.
    return struct.pack(">Q", _native.ntp_timetag(ntp_seconds))


def bundle(seconds_ahead: float, *packets: bytes) -> bytes:
    """An RT bundle timetagged `seconds_ahead` from now (wall clock)."""
    return bundle_at(time.time() + seconds_ahead, *packets)


def bundle_at(unix_seconds: float, *packets: bytes) -> bytes:
    """An RT bundle timetagged at an absolute Unix instant (wall clock)."""
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + struct.pack(">Q", _native.unix_to_ntp(unix_seconds)) + body


def immediate_bundle(*packets: bytes) -> bytes:
    """A bundle with the immediate timetag ``{0, 1}`` — used as the ``/sched``
    payload, where the server ignores the inner timetag and fires it at the
    scheduled sample."""
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + struct.pack(">II", 0, 1) + body


def score_bundle(seconds: float, *packets: bytes) -> bytes:
    """A bundle for an NRT score: the timetag counts seconds from the start of
    the render, not wall-clock time."""
    body = b"".join(struct.pack(">i", len(p)) + p for p in packets)
    return _string("#bundle") + _timetag(seconds) + body


def score(*bundles: bytes) -> bytes:
    """Frames bundles into the binary NRT score (`[i32 len][packet]…`)."""
    return b"".join(struct.pack(">i", len(b)) + b for b in bundles)


def _read_string(data: bytes) -> tuple[str, bytes]:
    end = data.index(b"\x00")
    n = (end + 4) // 4 * 4
    return data[:end].decode(), data[n:]


def decode(packet: bytes) -> tuple[str, list]:
    """Decodes a single OSC message into ``(addr, args)``. Enough for the
    server's replies (`/done`, `/fail`, `/status.reply`, …); bundles are not
    expected as replies."""
    addr, rest = _read_string(packet)
    tags, rest = _read_string(rest)
    args = []
    for t in tags[1:]:  # skip the leading ','
        if t == "i":
            args.append(struct.unpack(">i", rest[:4])[0]); rest = rest[4:]
        elif t == "h":
            args.append(struct.unpack(">q", rest[:8])[0]); rest = rest[8:]
        elif t == "f":
            args.append(struct.unpack(">f", rest[:4])[0]); rest = rest[4:]
        elif t == "d":
            args.append(struct.unpack(">d", rest[:8])[0]); rest = rest[8:]
        elif t == "t":  # OSC timetag (NTP): decode to Unix seconds
            secs, frac = struct.unpack(">II", rest[:8]); rest = rest[8:]
            args.append(secs - 2_208_988_800 + frac / 2 ** 32)
        elif t == "s":
            s, rest = _read_string(rest); args.append(s)
        elif t == "b":
            size = struct.unpack(">i", rest[:4])[0]
            args.append(rest[4:4 + size]); rest = rest[4 + (size + 3) // 4 * 4:]
    return addr, args


def decode_packet(packet: bytes, time=None) -> list:
    """Decodes an incoming OSC packet into a flat list of ``(addr, args,
    time)`` messages, unwrapping bundles recursively. A bundle's NTP timetag is
    decoded to Unix seconds and carried as each contained message's ``time``
    (``None`` for the immediate timetag ``{0,1}``); a bare message carries the
    ``time`` passed in. The single entry point for received packets — the
    counterpart of the server's ``osc::decode_packet`` — so the responder
    layer handles bundles transparently."""
    if packet[:8] == b"#bundle\x00":
        secs, frac = struct.unpack(">II", packet[8:16])
        btime = None if (secs, frac) == (0, 1) else secs - NTP_UNIX_OFFSET + frac / 2 ** 32
        out = []
        rest = packet[16:]
        while len(rest) >= 4:
            size = struct.unpack(">i", rest[:4])[0]
            out += decode_packet(rest[4:4 + size], btime)
            rest = rest[4 + size:]
        return out
    addr, args = decode(packet)
    return [(addr, args, time)]
