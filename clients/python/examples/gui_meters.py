#!/usr/bin/env python3
"""Meters, a scope and a server-buffer waveform: the GUI as a client of the server.

It shows the two ways the GUI host reaches into the audio server:

- a ``meter`` and a ``scope`` read a bus **straight from the audio server's
  shared-memory segment**, every frame, with no OSC traffic at all. Both name a
  bus and a rate; here they say ``rate="control"`` (their default is audio, the
  console case) and the script only writes the bus with ``/c_set``;
- a ``waveform`` references a **server buffer by number**; the host fetches its
  samples from the server (``/b_query`` then ``/b_getn``) and renders them.

Because the meter path is shared memory, the server and the host must map the
*same* segment, and the host needs its client leg pointed at the server to fetch
the buffer -- which is exactly what `Session.live` + `Session.gui` wire up.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_meters.py``. It self-launches the audio
server (with a shared-memory segment, ``shm="auto"``) and the GUI host mapping
that same segment; by hand that is ``clausters --shm <path>`` and
``clausters-gui --server 127.0.0.1:57110 --shm <path>``. Run this with no server
already up on 57110, so the session boots its own. Needs a display and a GPU
adapter.
"""

# %%
import math
import os
import struct
import sys
import tempfile
import time
import wave

from clausters import Session
from clausters.defs import Bus
from clausters.gui import meter, panel, scope, waveform, window

# %% [markdown]
# ## Launch the server and the GUI
# `Session.live()` boots the server with a shared-memory segment (`shm="auto"`),
# and `session.gui()` maps the same segment -- the meter and scope read the
# control bus straight from it.

# %%
session = Session.live()
server = session.server
gui = session.gui()

# %% [markdown]
# ## A sine buffer on the server, and a control bus to animate
# The WAV is loaded into a server buffer (async: barrier with `/sync`), which the
# host fetches over its client leg; the control bus is what the meter/scope read
# from shared memory.

# %%
def write_sine_wav(freq: float = 220.0, secs: float = 1.0, sr: int = 48_000) -> str:
    """Writes a short mono sine WAV to a temp file and returns its path."""
    fd, path = tempfile.mkstemp(prefix="clausters_gui_", suffix=".wav")
    os.close(fd)
    with wave.open(path, "w") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        frames = bytearray()
        for i in range(int(secs * sr)):
            frames += struct.pack("<h", int(32767 * 0.8 * math.sin(2 * math.pi * freq * i / sr)))
        w.writeframes(bytes(frames))
    return path


wav = write_sine_wav()
bufnum = server.buffers.alloc()
server.send_msg("/b_allocRead", bufnum, wav)
server.sync()
bus = Bus.control(server=server)

# %% [markdown]
# ## The window
# A meter + scope on the control bus, over a waveform of the server buffer.
# All named, not numbered -- `open` hands back a handle that resolves the
# names.

# %%
win = gui.open(window(
    panel(meter(bus.index, rate="control", name="level",
                min=-1.0, max=1.0, label="bus"),
          scope(bus.index, rate="control", name="trace",
                min=-1.0, max=1.0, label="bus"),
          layout="row"),
    waveform(name="buffer", buffer=bufnum),
    title="Meters + server buffer", w=640, h=440, layout="col"))
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("watch the meter/scope move and the buffer waveform render; "
      "close the window to stop")

# %% [markdown]
# ## Drive it
# Animate the bus with a 0.5 Hz sine. The host reads this bus from shared memory
# each frame -- these `/c_set` messages go only to the audio server, never to the
# GUI.

# %%
_closed = False


def run(seconds: float) -> None:
    """Animates the control bus for ``seconds``."""
    start = time.monotonic()
    while time.monotonic() - start < seconds and not _closed:
        bus.set(math.sin(2 * math.pi * 0.5 * (time.monotonic() - start)))
        gui.pump(timeout=0.03)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run(15.0)
    finally:
        os.remove(wav)
        session.close()
else:
    print("meters up - run(10) to animate the bus, session.close() to end")
