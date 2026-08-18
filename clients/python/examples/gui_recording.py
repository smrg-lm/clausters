#!/usr/bin/env python3
"""A take drawn **while it records**: the picture follows the write frontier.

Every other way material reaches a picture announces itself — a client sends
samples, a peer edits a span and says so. A **recording** does not: a
`record_buf` fills a buffer block by block from the audio thread, which is the
one place that must never send a message. So the engine publishes a single
number instead — how far the material now goes, the buffer's *write frontier* —
into the shared segment's directory row, and a host that maps the same segment
draws the rest for itself.

What that buys, and it is the whole example: **nothing about the audio crosses
the wire**. The samples are already the cells the engine is writing (the host
maps the region), so the picture is not fetched, not streamed and not copied —
what the frame tick re-reads is the *summary* of the frames that appeared since
the last one, and only those.

Two things worth watching for:

- The trace fills from the left as the recorder advances, and the part it has
  not reached yet draws as the silence an allocated buffer is. The host cannot
  tell "not written yet" from "recorded silence" — the client is what knows,
  since it allocated the empty buffer — so that distinction is a prop the
  widget does not have yet.
- Zoom into the part already recorded (**wheel**) and it is sample-exact
  immediately: the zoomed-in regimes read the cells themselves, so they are
  current with nothing told to them at all.

Needs an audio device, a display and a GPU adapter, and a **server with a
shared-memory segment** — which is what `Session.live` boots (``shm="auto"``)
and what `Session.gui` points the host at. By hand that is
``clausters --shm <path>`` and ``clausters-gui --server 127.0.0.1:57110 --shm
<path>``. With the client importable (``pip install -e ./clients/python``)::

    python clients/python/examples/gui_recording.py

Organized as ``# %%`` cells: step through it with Shift+Enter and the window
stays up between cells, or run it as a plain script.
"""

# %%
import time

from clausters import Buffer, Session, Synth, SynthDef
from clausters.defs.ugens import line, out, record_buf, sin_osc
from clausters.gui import waveform, window

# %% [markdown]
# ## The server, the host and the segment
# One server owning a segment and one GUI host mapping it. The take lives in a
# region beside that segment, so the host draws the very memory the engine
# records into.

# %%
session = Session.live()
server = session.server
gui = session.gui()
print(f"audio server on segment {server.shm}")

#: Ten seconds of mono at the engine's rate: long enough that filling it is
#: something to watch rather than a flicker.
SECONDS = 10.0
rate = 48_000.0
take = Buffer.alloc(int(SECONDS * rate), 1, server=server)
print(f"take: buffer {take.bufnum}, {take.frames} frames, empty")

# %% [markdown]
# ## The window
# A plain waveform over the buffer number. Nothing here says "recording": the
# view is the same one that draws a take read from a file, and what makes it
# live is that the material is mapped and its frontier moves.

# %%
win = gui.open(window(
    waveform(name="take", buffer=take.bufnum, sample_rate=rate,
             ruler="time", ruler_y="norm"),
    title="A take, while it records", w=900, h=420, layout="col"))
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("the window is empty: nothing has been recorded into the take yet")

# %% [markdown]
# ## Record into it
# A sweep, heard and recorded at once — `record_buf` passes its input through,
# so the same signal reaches the buffer and the speakers. `done_action=2` frees
# the node when the recorder reaches the end of the buffer, which is how the
# picture stops filling.

# %%
SynthDef(
    "recorder",
    out(0.0, record_buf(
        float(take.bufnum), 0.0,
        sin_osc(line(120.0, 900.0, SECONDS)) * line(0.05, 0.3, SECONDS),
        done_action=2,
    )),
).send(server)
server.sync()

Synth("recorder", server=server)
print("recording: the trace fills from the left, one frame tick at a time")

# %% [markdown]
# ## Watch it fill
# Nothing in this loop touches the picture. The host is reading the frontier on
# its own frame tick and re-summarizing what appeared; all this does is keep
# the script alive while it happens. A client that wants the same news — a
# headless capture, or a page, which can map nothing — asks for it over the
# wire instead: ``server.stream_buffers(50, take)`` and an `OscFunc` on
# ``/buffer_stream.reply``, which carries the summary and not the samples.

# %%
started = time.monotonic()
while not _closed and time.monotonic() - started < SECONDS + 2.0:
    time.sleep(0.5)
print("done recording" if not _closed else "window closed")

# %% [markdown]
# ## And it is ordinary material afterwards
# The frontier stops moving when the recorder frees itself, and what is left is
# a take like any other — zoom it, sweep a selection, play it. Closing the
# session stops everything it started.

# %%
if not _closed:
    print("zoom with the wheel, sweep a selection, then close the window")
    while not _closed:
        time.sleep(0.2)
# The recorder freed itself when it reached the end of the buffer
# (``done_action=2``), which is what stopped the picture from filling; nothing
# here has to free it.
take.free()
gui.close_all()
session.close()
