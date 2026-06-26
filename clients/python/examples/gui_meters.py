#!/usr/bin/env python3
"""Meters, a scope and a server-buffer waveform: the GUI as a client of the server.

The G5 example. It shows the two ways the GUI host reaches into the audio server,
the third leg of the topology:

- a ``meter`` and a ``scope`` read a **control bus straight from the audio
  server's shared-memory segment**, every frame, with no OSC traffic at all — the
  script only writes the bus with ``/c_set``;
- a ``waveform`` references a **server buffer by number**; the host fetches its
  samples from the server (``/b_query`` then ``/b_getn``) and renders them.

So three processes cooperate: the **audio server**, the **GUI host**, and this
**script**. Because the meter path is shared memory, both the server and the host
must map the *same* segment file, and the host needs ``--server`` to fetch the
buffer.

Start the audio server with a shared segment (built from the repo root)::

    cargo run -- --shm /dev/shm/clausters_g5

Start the windowed GUI host, attached to that server and segment (from
``clients/gui``)::

    cargo run --bin clausters-gui -- --server 127.0.0.1:57110 --shm /dev/shm/clausters_g5 -v

Then, with the client importable (``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_meters.py

A window opens with a moving meter and scope (driven by the control bus this
script animates) and a waveform of the sine buffer the host pulled from the
server. Close the window, or wait, to end.
"""

import math
import os
import struct
import sys
import tempfile
import time
import wave

from clausters import Session
from clausters.gui import GuiHost, meter, panel, scope, waveform, window


def write_sine_wav(freq: float = 220.0, secs: float = 1.0, sr: int = 48_000) -> str:
    """Writes a short mono sine WAV to a temp file and returns its path."""
    fd, path = tempfile.mkstemp(prefix="clausters_gui_", suffix=".wav")
    os.close(fd)
    n = int(secs * sr)
    with wave.open(path, "w") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        frames = bytearray()
        for i in range(n):
            sample = int(32767 * 0.8 * math.sin(2 * math.pi * freq * i / sr))
            frames += struct.pack("<h", sample)
        w.writeframes(bytes(frames))
    return path


def scene(bus_index: int, bufnum: int) -> dict:
    """A window: a meter + scope on `bus_index`, over a server-buffer waveform."""
    return window(
        panel(2,
              meter(10, bus_index, min=-1.0, max=1.0, label="bus"),
              scope(11, bus_index, min=-1.0, max=1.0, label="bus"),
              layout="row"),
        waveform(12, buffer=bufnum),
        title="Meters + server buffer", w=640, h=440, layout="col",
    )


def main():
    wav = write_sine_wav()
    try:
        with Session.live() as session:  # UDP to 127.0.0.1:57110
            server = session.server
            # Load the sine WAV into a server buffer (async: barrier with /sync).
            bufnum = server.buffers.alloc()
            server.send_msg("/b_allocRead", bufnum, wav)
            server.sync()

            # A control bus the meter/scope will read from shared memory.
            bus = server.control_bus()

            with GuiHost() as gui:  # 127.0.0.1:57210 by default
                gui.define(1, scene(bus.index, bufnum))
                print("watch the meter/scope move and the buffer waveform render; "
                      "close the window to stop")

                start = time.monotonic()
                while time.monotonic() - start < 15.0:
                    # Animate the bus: a 0.5 Hz sine. The host reads this bus from
                    # shared memory each frame — these /c_set messages go only to
                    # the audio server, never to the GUI.
                    phase = time.monotonic() - start
                    server.set_bus(bus, math.sin(2 * math.pi * 0.5 * phase))
                    msg = gui.poll(timeout=0.03)
                    if msg is not None and msg[0] == "/gui_closed":
                        print("window closed")
                        break
    finally:
        os.remove(wav)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
