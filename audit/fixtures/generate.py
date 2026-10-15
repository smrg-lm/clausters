#!/usr/bin/env python3
"""Generate deterministic mono PCM16 test inputs, with no project dependency."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import wave


def generate(destination):
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=False)
    entries = []
    # Integer-only waveforms avoid platform-dependent transcendental rounding.
    for name, period in (("square-1000hz", 48), ("square-125hz", 384)):
        path = destination / (name + ".wav")
        samples = [8192 if i % period < period // 2 else -8192 for i in range(48000)]
        with wave.open(str(path), "wb") as stream:
            stream.setparams((1, 2, 48000, 0, "NONE", "not compressed"))
            stream.writeframes(struct.pack("<48000h", *samples))
        entries.append({"path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                        "frames": 48000, "sample_rate": 48000, "channels": 1,
                        "purpose": "Loading/scaling fixture, not an antialiasing reference"})
    (destination / "manifest.json").write_text(json.dumps({"format_version": 1, "generator": "integer-square-v1", "files": entries}, indent=2) + "\n")
    return entries


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination")
    print(json.dumps(generate(parser.parse_args().destination), indent=2))
