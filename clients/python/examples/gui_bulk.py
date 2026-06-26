#!/usr/bin/env python3
"""Bulk data the right way: a multi-megabyte buffer rendered from a shared file.

The G7 example. Large payloads do **not** ride OSC: a UDP datagram caps near
64 KB, and chunking a buffer over ``/b_getn`` re-traverses the network for data
that already sits in local RAM. Instead the data lands in a **local file** the
GUI host memory-maps and reads zero-copy. This shows the three shared-resource
forms a ``waveform`` accepts, none of which re-send the samples per frame:

- ``cache=`` — a prebuilt **peak-pyramid** file (`peaks_cache_file`, built by the
  shared native core via the FFI). The most compact: the host maps just the
  overview, never the raw buffer.
- ``path=`` — a file of raw little-endian ``f32`` (`samples_to_file`). The host
  maps a multi-megabyte buffer with no OSC and no re-send.
- a **server buffer exported** to a file with ``/b_export`` — the audio server
  dumps its RT buffer to a local file the host maps, the shared-resource
  counterpart of pulling it over ``/b_getn``.

Three processes cooperate, as in ``gui_meters.py``: the **audio server**, the
**GUI host**, and this **script**. Files are passed by absolute path, so the
host (a separate process) resolves them.

Start the audio server (from the repo root)::

    cargo run

Start the windowed GUI host, attached to that server (from ``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_bulk.py

A window opens with three waveforms: a big client-side sweep shown from its peak
cache and from its raw file, and a server buffer the host mapped after
``/b_export``. Close the window to stop. Needs a display and a GPU adapter.
"""

import math
import os
import struct
import sys
import tempfile
import time
import wave

from clausters import Session
from clausters.gui import GuiHost, peaks_cache_file, samples_to_file, waveform, window

SR = 48_000


def big_sweep(seconds: float = 10.0) -> list[float]:
    """A long log sine sweep — ~480k samples at 10 s, a couple of megabytes as
    raw f32: too big for an OSC blob, the case the bulk path exists for."""
    n = int(seconds * SR)
    out = []
    for i in range(n):
        t = i / SR
        freq = 80.0 * (2.0 ** (3.0 * t / seconds))  # 80 Hz up three octaves
        out.append(0.8 * math.sin(2 * math.pi * freq * t))
    return out


def write_sine_wav(freq: float = 220.0, secs: float = 1.0) -> str:
    fd, path = tempfile.mkstemp(prefix="clausters_bulk_", suffix=".wav")
    os.close(fd)
    with wave.open(path, "w") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        frames = bytearray()
        for i in range(int(secs * SR)):
            frames += struct.pack("<h", int(32767 * 0.8 * math.sin(2 * math.pi * freq * i / SR)))
        w.writeframes(bytes(frames))
    return path


def scene(cache_path: str, raw_path: str, exported_path: str) -> dict:
    """A column of three waveforms, one per shared-resource form."""
    return window(
        waveform(10, cache=cache_path),                  # prebuilt peak cache
        waveform(11, path=raw_path),                     # raw f32, host maps it
        waveform(12, path=exported_path, channels=1),    # a server buffer export
        title="Bulk: mapped files, no OSC", w=900, h=600, layout="col",
    )


def main():
    tmp = tempfile.mkdtemp(prefix="clausters_bulk_")
    raw_path = os.path.join(tmp, "sweep.f32")
    cache_path = os.path.join(tmp, "sweep.peaks")
    exported_path = os.path.join(tmp, "exported.f32")
    wav = write_sine_wav()
    try:
        # Client-origin: a big buffer written as a raw file and as a peak cache.
        sweep = big_sweep()
        samples_to_file(sweep, raw_path)
        peaks_cache_file(sweep, cache_path, base_bucket=256)
        print(f"wrote {len(sweep)} samples: {os.path.getsize(raw_path)} B raw, "
              f"{os.path.getsize(cache_path)} B peak cache")

        with Session.live() as session:  # UDP to 127.0.0.1:57110
            server = session.server
            bufnum = server.buffers.alloc()
            server.send_msg("/b_allocRead", bufnum, wav)
            server.sync()
            # Server-origin: export the RT buffer to a local file the host maps.
            server.request("/b_export", bufnum, exported_path, expect=("/done", "/fail"))
            print(f"server exported buffer {bufnum} -> {os.path.getsize(exported_path)} B")

            with GuiHost() as gui:  # 127.0.0.1:57210 by default
                gui.define(1, scene(cache_path, raw_path, exported_path))
                print("three waveforms mapped from files (zero OSC for the samples); "
                      "zoom/pan with wheel/drag, close the window to stop")
                start = time.monotonic()
                while time.monotonic() - start < 30.0:
                    msg = gui.poll(timeout=0.1)
                    if msg is not None and msg[0] == "/gui_closed":
                        print("window closed")
                        break
    finally:
        os.remove(wav)
        for p in (raw_path, cache_path, exported_path):
            if os.path.exists(p):
                os.remove(p)
        os.rmdir(tmp)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
