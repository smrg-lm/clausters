#!/usr/bin/env python3
"""Bulk data the right way: a multi-megabyte buffer rendered from a shared file.

Large payloads do **not** ride OSC: a UDP datagram caps near 64 KB, and chunking
a buffer over ``/buffer_getRange`` re-traverses the network for data that already sits in
local RAM. Instead the data lands in a **local file** the GUI host memory-maps
and reads zero-copy. This shows the three shared-resource forms a ``waveform``
accepts, none of which re-send the samples per frame:

- ``cache=`` -- a prebuilt **peak-pyramid** file (`peaks_cache_file`, built by the
  shared native core via the FFI). The most compact: the host maps just the
  overview, never the raw buffer.
- ``path=`` -- a file of raw little-endian ``f32`` (`samples_to_file`). The host
  maps a multi-megabyte buffer with no OSC and no re-send.
- a **server buffer exported** to a file with ``/buffer_export`` -- the audio server
  dumps its RT buffer to a local file the host maps.

A fourth lane stacks two views of the *same* mapped cache -- the peak envelope
and the RMS body over it -- which is what a peak cache carrying a mean square
per bucket buys: the classic editor picture with no second pass over the samples
and no second file.

Files are passed by absolute path, so the host (a separate process) resolves
them; the buffer export needs the host's client leg pointed at the server, which
`Session.gui` wires up.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_bulk.py``. It self-launches the audio server
and the GUI host (`Session.live` + `Session.gui`); by hand that is ``clausters``
and ``clausters-gui --server 127.0.0.1:57110``. Run this with no server already
up on 57110, so the session boots its own. Needs a display and a GPU adapter.
"""

# %%
import math
import os
import shutil
import struct
import sys
import tempfile
import time
import wave

from clausters import Session
from clausters.base.bulk import blob_to_samples
from clausters.gui import (button, field, label, panel, peaks_cache_file,
                           samples_to_file, signal, waveform, window)

SR = 48_000

# %% [markdown]
# ## The client-origin bulk files
# A long log sine sweep (~480k samples, a couple of megabytes as raw f32: too big
# for an OSC blob) written both as a raw file and as a peak-pyramid cache.

# %%
def big_sweep(seconds: float = 10.0) -> list:
    """A long log sine sweep -- the case the bulk path exists for."""
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


tmp = tempfile.mkdtemp(prefix="clausters_bulk_")
raw_path = os.path.join(tmp, "sweep.f32")
cache_path = os.path.join(tmp, "sweep.peaks")
exported_path = os.path.join(tmp, "exported.f32")
wav = write_sine_wav()

sweep = big_sweep()
samples_to_file(sweep, raw_path)
peaks_cache_file(sweep, cache_path, base_bucket=256)
print(f"wrote {len(sweep)} samples: {os.path.getsize(raw_path)} B raw, "
      f"{os.path.getsize(cache_path)} B peak cache")

# %% [markdown]
# ## Launch the server and the GUI, and export a server buffer to a file
# `Session.gui()` points the host at this session's server; the server dumps its
# RT buffer to a local file the host will map.

# %%
session = Session.live()
server = session.server
gui = session.gui()

bufnum = server.buffers.alloc()
server.send_msg("/buffer_allocRead", bufnum, wav)
server.sync()
server.request("/buffer_export", bufnum, exported_path, expect=("/done", "/fail"))
print(f"server exported buffer {bufnum} -> {os.path.getsize(exported_path)} B")

# %% [markdown]
# ## The window
# Three waveforms, one per shared-resource form -- all mapped from files, zero
# OSC for the samples. Named, so `open` resolves them.
#
# The fourth lane is the same mapped cache drawn **twice**: what the sweep
# reached (the peak envelope) and what it held (the RMS body), stacked. That is
# the classic editor picture, and it is composition rather than a mode -- two
# `signal` elements measuring differently, laid over one `field` (a lane, so it
# carries its children; a *placed* field is a clip, whose bodies come from its
# own props). Neither layer knows the other is there, and the body is the same
# mapped pyramid the envelope reads: the mean square rides in the cache beside
# the min and max, so the second picture costs no second pass over the samples.

# %%
#: A short take, small enough that every sample is a disc on screen -- which is
#: the zoom at which a sample is a thing you can grab.
EDITABLE = [round(math.sin(i * math.tau / 32) * 0.8, 4) for i in range(64)]

win = gui.open(window(
    waveform(name="cache", cache=cache_path),                 # prebuilt peak cache
    waveform(name="raw", path=raw_path),                      # raw f32, host maps it
    waveform(name="exported", path=exported_path, channels=1),  # a server buffer export
    field(                                                    # the layer stack
        signal(cache=cache_path, navigable=False),            # what it reached
        signal(cache=cache_path, navigable=False, measure="rms"),  # what it held
        label="peak + rms"),
    # The editable lane: drag a sample, or draw a run of them.
    waveform(name="edit", data=EDITABLE, gestures={"drag": "sample"}),
    panel(button(name="mode", label="mode: grab a sample"),
          label(name="log", text="drag a sample in the lane above"),
          layout="row", h=40),
    title="Bulk: mapped files, no OSC", w=900, h=800, layout="col"))
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("three waveforms mapped from files (zero OSC for the samples), and a "
      "fourth lane stacking the RMS body over the peak envelope of the same "
      "cache; zoom/pan with wheel/drag, close the window to stop")

# %% [markdown]
# ## The edit round trip
# The bottom lane is the other direction: the **host owns no data**, so dragging
# a sample changes nothing by itself. What leaves is an *intent* --
# `"sample" channel frame value previous` -- absolute and carrying its own
# inverse, so whoever owns the material can apply it and undo it without having
# remembered anything.
#
# While it is in flight the host draws the value the hand is holding, marked (a
# ring tethered to the value it replaces), over a picture that has not changed.
# This handler is the owner: it applies the edit to its own copy, pushes the
# samples that now hold, and **acknowledges** -- and the acknowledgement is what
# lets the host drop the pending drawing, the edit having become the material.
# Doing it in the other order would blink the old value back.

# %%
edits: list[str] = []


#: Which gesture the editable lane is bound to. Both are steps of the same
#: plan table, so switching is a `/gui_set` and not a mode inside the host.
_mode = "sample"


def toggle_mode(*_) -> None:
    """Swap the lane's drag between grabbing one sample and drawing a run."""
    global _mode
    _mode = "draw" if _mode == "sample" else "sample"
    win["edit"].set(gestures={"drag": _mode})
    win["mode"].set(label=f"mode: {'draw a run' if _mode == 'draw' else 'grab a sample'}")
    print(f"drag is now {_mode!r}")


def on_edit(tag: str, *values) -> None:
    """Apply one dragged sample, then say so.

    The payload is the event's tag followed by its flat values, so the tag is
    what says which gesture this was — this lane emits only one, and the check
    is what keeps that true when it grows a second.
    """
    if tag == "refused":
        # A stroke where a pixel is more than one sample: the host says so
        # rather than doing nothing, and the lane says it back.
        win["log"].set(text=f"refused: {values[1] if len(values) > 1 else values}")
        print(f"refused: {values}")
        return
    if tag == "sample":
        channel, frame, value, previous = values
        EDITABLE[int(frame)] = round(float(value), 4)
        edits.append(
            f"sample {int(frame)} of channel {int(channel)}: {previous:+.3f} -> {value:+.3f}")
    elif tag == "draw":
        # One intent for the whole stroke, the run as a blob -- the same
        # convention every bulk payload here uses.
        channel, start, new_run, _previous = values
        run = blob_to_samples(new_run)
        for i, v in enumerate(run):
            EDITABLE[int(start) + i] = round(float(v), 4)
        edits.append(f"stroke over {len(run)} samples from {int(start)}")
    else:
        return
    win["edit"].set(data=EDITABLE)      # the material the picture is now drawn from
    gui.ack(gui.last_seq)               # ...and only now does the hand let go
    win["log"].set(text=edits[-1])
    print(edits[-1])


win["edit"].on_event(on_edit)
win["mode"].on_event(toggle_mode)

# %% [markdown]
# ## Wait, then clean up
# Nothing to drive -- the views are static. Pump events until the window closes.

# %%
_closed = False


def run(seconds: float | None = None) -> None:
    """Pumps events for ``seconds`` (three lanes are static; the last one edits).

    Script-run there is no bound and the window is what ends it; the
    ``seconds`` argument is for a cell run, where a notebook wants the loop to
    give the prompt back.
    """
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.1)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
        # Sweep the whole temp dir: besides the files written here, the host
        # leaves a sibling peaks cache next to each mapped resource. The one-off
        # source WAV lives in the system temp dir, so it goes on its own.
        os.remove(wav)
        shutil.rmtree(tmp, ignore_errors=True)
else:
    print("bulk up - run(10) to keep it open, session.close() to end")
