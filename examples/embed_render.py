#!/usr/bin/env python3
"""Synchronous offline render through the embed C ABI (M14).

The "scientific workflow" call: hand a binary score to the library, block
until it is rendered, get the samples back as flat float32 — no server, no
OSC sockets, no asynchrony, ready to analyze or plot. Build the library
once:

    cargo build --release --features embed,realtime

then:

    python3 examples/embed_render.py [out.wav]

The binding returns a stdlib ``array('f')``; a numpy user would wrap it
with ``numpy.frombuffer(samples, dtype=numpy.float32)`` — their choice, not
a dependency of the binding.
"""

import os
import struct
import sys
import wave

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "clients", "python"))
import json_client as osc  # score helpers (stdlib OSC)
import clausters


def arpeggio_score() -> bytes:
    """Five notes, 0.4 s apart, closed by the duration-setting bundle."""
    packets = []
    for i, freq in enumerate([262.0, 330.0, 392.0, 523.0, 659.0]):
        node = 1000 + i
        packets.append(osc.score_bundle(
            i * 0.4, osc.message("/s_new", "default", node, 1, 0,
                                 "freq", freq, "amp", 0.25)))
        packets.append(osc.score_bundle(i * 0.4 + 0.35,
                                        osc.message("/n_free", node)))
    packets.append(osc.score_bundle(2.1, osc.message("/n_free", 0)))
    return b"".join(struct.pack(">i", len(p)) + p for p in packets)


def main():
    samples, frames = clausters.render(arpeggio_score(), sample_rate=48000.0,
                                       channels=2)
    peak = max(abs(s) for s in samples)
    rms = (sum(s * s for s in samples) / len(samples)) ** 0.5
    print(f"rendered {frames} frames ({frames / 48000:.3f} s), "
          f"{len(samples)} samples | peak {peak:.3f} | rms {rms:.3f}")

    if len(sys.argv) > 1:
        path = sys.argv[1]
        with wave.open(path, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(48000)
            ints = array_to_int16(samples)
            w.writeframes(ints)
        print(f"wrote {path} — listen with: ffplay -autoexit {path}")


def array_to_int16(samples) -> bytes:
    out = bytearray()
    for s in samples:
        v = max(-1.0, min(1.0, s))
        out += struct.pack("<h", int(v * 32767))
    return bytes(out)


if __name__ == "__main__":
    try:
        main()
    except OSError as e:
        sys.exit(str(e))
