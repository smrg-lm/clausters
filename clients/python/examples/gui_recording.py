#!/usr/bin/env python3
"""Takes drawn **while they record**: the picture follows the write frontier.

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
what the host re-reads is the *summary* of the frames that appeared since last
time, and only those.

It records **several glissandi at once**, on internal buses, because the
interesting number is how the drawing scales: one recording is free on any
machine, and sixteen is where a design shows what it costs. They all start on
one pitch and fan out to a random one, up or down, so what is heard is a
unison opening rather than a stack of neighbours beating against each other.
Pass a track count::

    python clients/python/examples/gui_recording.py 16

**The knob it is here to show.** Re-reading a summary costs a copy of it, so
doing it every frame is a copy per frame per view — which is what scales badly
when many tracks record at once. The host waits for a **block** of material
instead and redraws in steps: ``--follow-block <seconds>``, one second by
default. Pass it as a second argument here and watch the host's CPU with
``top`` while it records::

    python clients/python/examples/gui_recording.py 16 0     # every frame
    python clients/python/examples/gui_recording.py 16 1     # one-second blocks

Neither the sound nor a playhead over it reads that number: what changes is how
often the *picture* catches up with the material.

Two things worth watching for:

- The traces grow in steps, and the part they have not reached yet draws as the
  silence an allocated buffer is. The host cannot tell "not written yet" from
  "recorded silence" — the client is what knows, since it allocated the empty
  buffer — so that distinction is a prop the widget does not have yet.
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
import random
import sys
import time

from clausters import Buffer, Session, Synth, SynthDef
from clausters.defs import control
from clausters.defs.ugens import line, out, record_buf, sine
from clausters.gui import timeruler, waveform, window

#: How many glissandi record at once, and how long they run. The count is a
#: command-line argument so the same file is a demonstration and a stress test.
TRACKS = int(sys.argv[1]) if len(sys.argv) > 1 else 8
SECONDS = 10.0
#: Seconds of material a picture waits for before catching up — the host's
#: ``--follow-block``. ``None`` leaves the host's own default (1 s).
BLOCK = float(sys.argv[2]) if len(sys.argv) > 2 else None

# %% [markdown]
# ## The server, the host and the segment
# One server owning a segment and one GUI host mapping it. Every take lives in
# a region beside that segment, so the host draws the very memory the engine
# records into.

# %%
session = Session.live()
server = session.server
gui = session.gui(extra_args=() if BLOCK is None else ("--follow-block", str(BLOCK)))
print(f"audio server on segment {server.shm}")
print(f"{TRACKS} tracks, {SECONDS:g} s each, follow block "
      f"{'the host default' if BLOCK is None else f'{BLOCK:g} s'}")

rate = 48_000.0
takes = [Buffer.alloc(int(SECONDS * rate), 1, server=server) for _ in range(TRACKS)]
print(f"takes: buffers {takes[0].bufnum}..{takes[-1].bufnum}, "
      f"{takes[0].frames} frames each, empty")

# %% [markdown]
# ## The window
# One waveform per take, stacked, all in one **navigation group** (``link``) so
# they zoom and pan together and one ruler labels them all. The ruler is a
# `clausters.gui.timeruler` with a box of its own rather than a lane's own
# ``ruler`` strip, which would come out of *that* lane's height and leave it
# shorter than the rest. Nothing here says "recording": these are the same
# views that draw a take read from a file, and what makes them live is that the
# material is mapped and its frontier moves.

# %%
LANES = 1  # the navigation group every lane and the ruler share
win = gui.open(window(
    *[waveform(name=f"take{i}", buffer=t.bufnum, sample_rate=rate,
               ruler="off", ruler_y="off", link=LANES)
      for i, t in enumerate(takes)],
    timeruler(ruler="time", sample_rate=rate, link=LANES),
    title=f"{TRACKS} takes, while they record",
    # A lane gets what is left over after the ruler, so the window is sized to
    # give every one of them the same room whatever the count — and capped, so
    # thirty-two lanes stay on a screen rather than growing off it.
    w=900, h=min(40 + 24 * TRACKS, 760), layout="col"))
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))
print("the window is empty: nothing has been recorded into the takes yet")

# %% [markdown]
# ## Record into them
# One def, one node per take: a glissando, heard and recorded at once —
# `record_buf` passes its input through, so the same signal reaches the buffer
# and the speakers. `done_action=2` frees the node when the recorder reaches
# the end of its buffer, which is how the pictures stop growing.

# %%
SynthDef(
    "glissando",
    out(0.0, record_buf(
        control("buf", 0.0, "ir"), 0.0,
        sine(line(control("from", 220.0, "ir"), control("to", 880.0, "ir"), SECONDS))
        * line(0.02, 0.25 / max(TRACKS, 1) ** 0.5, SECONDS),
        done_action=2,
    )),
).send(server)
server.sync()

#: They all start on one pitch and fan out to a random one, up or down —
#: a unison that opens rather than a stack of neighbouring sweeps, which would
#: spend the whole take beating against each other. Random per run, so no two
#: are the same picture.
START = 220.0
for take in takes:
    octaves = random.uniform(-2.0, 2.0)
    while abs(octaves) < 0.25:  # nothing that just sits on the start
        octaves = random.uniform(-2.0, 2.0)
    Synth(
        "glissando",
        {"buf": float(take.bufnum), "from": START, "to": START * 2.0**octaves},
        server=server,
    )
print("recording: the traces grow in steps, one block of material at a time")

# %% [markdown]
# ## Watch them fill
# Nothing in this loop touches the pictures. The host is reading the frontiers
# on its own tick and re-summarizing what appeared; all this does is keep the
# script alive while it happens. A client that wants the same news — a headless
# capture, or a page, which can map nothing — asks for it over the wire
# instead: ``server.stream_buffers(50, *takes)`` and an `OscFunc` on
# ``/buffer_stream.reply``, which carries the summary and not the samples.

# %%
def watch(seconds: "float | None" = None) -> None:
    """Keeps the script alive while the pictures fill, until the window is
    closed (or ``seconds`` pass, which is what a notebook wants so the prompt
    comes back).

    `clausters.gui.GuiHost.pump` is what makes closing the window end this:
    the host's messages — a widget's events, and the ``/gui_closed`` a closed
    window sends — are dispatched to the handlers from **the script's own
    loop**, never from a thread of their own. A loop that only sleeps is a
    script that never hears anything.
    """
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.05)


watch(SECONDS + 2.0)
print("window closed" if _closed else "done recording")

# %% [markdown]
# ## And they are ordinary material afterwards
# The frontiers stop moving when the recorders free themselves, and what is
# left is takes like any others — zoom them, sweep a selection, play them.
# Closing the session stops everything it started.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        if not _closed:
            print("zoom with the wheel, sweep a selection, then close the window")
            watch()
    finally:
        # The recorders freed themselves at the end of their buffers
        # (``done_action=2``), which is what stopped the pictures from growing;
        # nothing here has to free them. Closing the session stops the server
        # and the GUI host it started.
        session.close()
else:
    print("the takes are up - watch() to hold them open, session.close() to end")
