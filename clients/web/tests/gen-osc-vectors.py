#!/usr/bin/env python3
"""Generate osc-vectors.json from the Python client's reference codec.

The Python client (`clausters.base._osclib`) is the reference OSC encoder;
this script freezes a set of encode vectors so the TS client's core-backed
codec (`base/osc.ts` over crates/clausters-core-web) can assert byte-for-byte
parity in `tests/osc-parity.test.ts`. The JSON is committed; regenerate with:

    PYTHONPATH=../../python python3 gen-osc-vectors.py

Arguments are recorded in the TS client's tagged form ([tag, value]) with
blobs as hex strings (JSON has no bytes).

It also writes osc-decode-vectors.json, the other direction: packets the
Python encoder cannot build (a **timetag argument** — `/clock_query.reply`'s
anchor) paired with what the reference *decoder* reads out of them. Encode
parity is byte parity and says nothing about what a value means once it has
crossed; a timetag is where the two answers differed.
"""

import json
import pathlib
import struct
import sys

sys.path.insert(
    0, str(pathlib.Path(__file__).resolve().parents[2] / "python")
)
from clausters import _native  # noqa: E402
from clausters.base import _osclib as osclib  # noqa: E402


def blob_hex(value):
    return bytes.fromhex(value)


CASES = [
    # (name, addr, [tagged args])  — the tags mirror base/osc.ts OscArg.
    ("no_args", "/server_status", []),
    ("s_new", "/synth_new", [
        ("s", "sine"), ("i", 1000), ("i", 0), ("i", 1),
        ("s", "freq"), ("f", 440.0),
    ]),
    ("negative_int", "/node_free", [("i", -1)]),
    ("int64", "/late", [("h", 4294967296)]),
    ("blob", "/def_send", [("s", "synth"), ("b", "0001feff")]),
    ("blob_pad", "/def_send", [("s", "synth"), ("b", "deadbeef0a")]),
    ("utf8_string", "/label", [("s", "árbol ñandú")]),
    ("mixed", "/node_set", [("i", 1000), ("s", "freq"), ("f", 220.5)]),
]


def py_arg(tag, value):
    if tag == "i":
        return int(value)
    if tag == "h":
        return osclib.Int64(value)
    if tag == "f":
        return float(value)
    if tag == "s":
        return value
    if tag == "b":
        return blob_hex(value)
    raise KeyError(tag)


# Packets built here rather than by `osclib.message`, which encodes no
# timetag: the reference this half freezes is `osclib.decode`.
def clock_query_reply(sample: int, rate: float, unix_secs: float) -> bytes:
    ntp = _native.unix_to_ntp(unix_secs)
    return (osclib._string("/clock_query.reply") + osclib._string(",hdt")
            + struct.pack(">q", sample) + struct.pack(">d", rate)
            + struct.pack(">Q", ntp))


DECODE_CASES = [
    ("clock_anchor", clock_query_reply(480000, 48000.0, 1_000_000_000.0)),
    ("clock_anchor_fractional", clock_query_reply(1, 44100.0, 1_756_000_000.5)),
]


def main():
    vectors = []
    for name, addr, args in CASES:
        py_args = [py_arg(t, v) for t, v in args]
        packet = osclib.message(addr, *py_args)
        vectors.append({
            "name": name,
            "addr": addr,
            "args": [[t, v] for t, v in args],
            "hex": packet.hex(),
        })
    out = pathlib.Path(__file__).with_name("osc-vectors.json")
    out.write_text(json.dumps(vectors, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {out.name}: {len(vectors)} vectors")

    decodes = []
    for name, packet in DECODE_CASES:
        addr, args = osclib.decode(packet)
        decodes.append({"name": name, "addr": addr, "args": args,
                        "hex": packet.hex()})
    out = pathlib.Path(__file__).with_name("osc-decode-vectors.json")
    out.write_text(json.dumps(decodes, ensure_ascii=False, indent=2) + "\n")
    print(f"wrote {out.name}: {len(decodes)} vectors")


if __name__ == "__main__":
    main()
