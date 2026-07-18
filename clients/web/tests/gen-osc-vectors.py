#!/usr/bin/env python3
"""Generate osc-vectors.json from the Python client's reference codec.

The Python client (`clausters.base._osclib`) is the reference OSC encoder;
this script freezes a set of encode vectors so the TS client's core-backed
codec (`base/osc.ts` over crates/clausters-core-web) can assert byte-for-byte
parity in `tests/osc-parity.test.ts`. The JSON is committed; regenerate with:

    PYTHONPATH=../../python python3 gen-osc-vectors.py

Arguments are recorded in the TS client's tagged form ([tag, value]) with
blobs as hex strings (JSON has no bytes).
"""

import json
import pathlib
import sys

sys.path.insert(
    0, str(pathlib.Path(__file__).resolve().parents[2] / "python")
)
from clausters.base import _osclib as osclib  # noqa: E402


def blob_hex(value):
    return bytes.fromhex(value)


CASES = [
    # (name, addr, [tagged args])  — the tags mirror base/osc.ts OscArg.
    ("no_args", "/status", []),
    ("s_new", "/s_new", [
        ("s", "sine"), ("i", 1000), ("i", 0), ("i", 1),
        ("s", "freq"), ("f", 440.0),
    ]),
    ("negative_int", "/n_free", [("i", -1)]),
    ("int64", "/late", [("h", 4294967296)]),
    ("blob", "/d_recv", [("b", "0001feff")]),
    ("blob_pad", "/d_recv", [("b", "deadbeef0a")]),
    ("utf8_string", "/label", [("s", "árbol ñandú")]),
    ("mixed", "/n_set", [("i", 1000), ("s", "freq"), ("f", 220.5)]),
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


if __name__ == "__main__":
    main()
