#!/usr/bin/env python3
"""Render one persistent native CLAP instrument without an audio device.

Build `cargo build --features plugin-clap` and
`cargo build -p clausters-plugin-host`, then run:

    python3 examples/plugin_render.py reference.json [out.wav]

The reference JSON is the /def_send plugin recipe: format, binary path, native
plugin id, every input/output port with id/channels/bus, optional initial state
and bridge depth. Use an instrument with an audible preset and a native note
port; --port selects that port's native id. Paths belong to this machine.
The bridge latency stays in the file; the last bundle leaves room for release.
Plugins need not be deterministic. Failures abort instead of writing silence.
Only the Python standard library is required. Output defaults to examples/out/.
"""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import tempfile

import json_client as osc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("output", nargs="?", type=Path,
                        default=Path(__file__).resolve().parent / "out" / "plugin.wav")
    parser.add_argument("--server", type=Path,
                        default=Path(__file__).resolve().parents[1] / "target/debug/clausters")
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    reference = json.loads(args.reference.read_text())
    packets = [osc.score_bundle(0.0,
        osc.message("/def_send", "plugin", "keys", json.dumps(reference)),
        osc.message("/synth_new", "keys", 42, 1, 0))]
    for time, kind in [(0.1, "note_on"), (1.1, "note_off")]:
        event = {"kind": kind, "source": 1, "note_id": 1, "port": args.port,
                 "channel": 0, "key": 60, "value": 0.15 if kind == "note_on" else 0.0}
        packets.append(osc.score_bundle(time, osc.message("/plugin_event", 42, json.dumps(event))))
    packets.append(osc.score_bundle(2.1, osc.message("/node_free", 42)))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="clausters-plugin-render-") as directory:
        score = Path(directory) / "score.osc"
        score.write_bytes(b"".join(struct.pack(">i", len(packet)) + packet for packet in packets))
        subprocess.run([str(args.server), "--nrt", str(score), str(args.output),
                        "--rate", "48000", "--channels", "2", "--stats"], check=True)
    print(f"wrote {args.output} -- listen with: pw-play {args.output}")


if __name__ == "__main__":
    main()
