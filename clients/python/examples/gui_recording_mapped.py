#!/usr/bin/env python3
"""The web page's window, natively — and **nothing about the audio crosses the
wire**.

`clients/web/examples/recording.html` draws four takes as they record, a
finished take too big to download, and one peer's edit landing in a lane. A
page can map nothing, so each of those three pictures is something it has to
**ask the server for**: the overview of a recording as it is written
(``/buffer_stream``), the overview of a take that stands still
(``/buffer_peaks``), and the span another peer edited, read back
(``/buffer_getRange``).

This is the same window with the same three things happening, on a host that
**maps the segment** — and every one of them is a local read:

- **A recording** is followed by the buffer's *write frontier*, one number the
  engine publishes in the directory row, with the samples already being the
  cells the host draws.
- **The finished take** is drawn from the **overview file beside its region**
  (``<segment>.buf<n>.<gen>.peaks``), which the server writes when it publishes
  the buffer and keeps current span by span. So a minute of stereo — 23 MB —
  opens with a few hundred kilobytes mapped and no pass over the samples at
  all. The lane prints both numbers when it opens.
- **A peer's edit** arrives as the span and nothing else (``/buffer_touch``):
  the samples under the host are already the new ones, so what it does is
  re-summarize those buckets. Here the edit is the server's own verb over the
  buffer (`clausters.Buffer.silence`), announced by this script; a local peer
  that maps the region can equally store into the cells and announce the same
  span, which is what ``examples/shm_samples.py`` shows.

So the two files are one experiment run twice: **the route differs and the
drawing does not.** Open them side by side.

The two lanes' groups are worth a look before anything else happens: the four
takes share one navigation group and one ruler, and the long take has its own,
because a group is one window in samples over every lane in it and this take is
eight times longer than those. Zoom (wheel) and pan (drag) work on whichever
ruler or lane the pointer is over.

Needs an audio device, a display and a GPU adapter. The buttons drive it, so
nothing has to be run cell by cell — though it is organized as ``# %%`` cells
too, so it steps under Shift+Enter with the window staying up between them.
With the client importable (``pip install -e ./clients/python``)::

    python clients/python/examples/gui_recording_mapped.py
"""

# %%
import os
import random
import sys
import time

from clausters import Buffer, Session, Synth, SynthDef
from clausters.defs import control
from clausters.defs.ugens import line, out, record_buf, sine
from clausters.gui import button, label, panel, timeruler, view, waveform
from clausters.ipc import ShmClient

#: The four takes, and the finished one beside them.
TAKES = 4
SECONDS = 8.0
LONG_SECONDS = 60.0
RATE = 48_000.0
#: The navigation groups: one axis for the takes, one for the long take.
LANES, LONG = 1, 2

# %% [markdown]
# ## The server, the host and the segment
# One server owning a segment and one host mapping it. Every buffer lives in a
# region beside that segment, and — since the server writes one — a summary
# beside the region.

# %%
session = Session.live()
server = session.server
gui = session.gui()
print(f"audio server on segment {server.shm}")

takes = [Buffer.alloc(int(SECONDS * RATE), 1, server=server) for _ in range(TAKES)]
long_take = Buffer.alloc(int(LONG_SECONDS * RATE), 2, server=server)

# %% [markdown]
# ## Fill the long one without sending a sample
# `clausters.Buffer.fill` says *how many* samples to write rather than carrying
# them, so a square wave across 23 MB is a dozen numbers on the wire. The runs
# land on whole buckets of the summary (256 frames): a boundary falling inside
# a bucket gives that bucket both values, and the picture a full-height column
# at every step — true, and a distraction here.

# %%
STEPS = 12
step_frames = (long_take.frames // STEPS // 256) * 256
long_take.fill(*[
    (i * step_frames * long_take.channels,
     step_frames * long_take.channels,
     (1 if i % 2 == 0 else -1) * (0.2 + 0.7 * (i / (STEPS - 1))))
    for i in range(STEPS)
])


def overview_bytes(buf: Buffer) -> "tuple[int, int]":
    """``(samples, overview)`` in bytes for `buf`, read off the filesystem.

    The region is the samples and its sibling is the summary the host maps
    instead of computing one — the whole point of the lane below, in two
    numbers.
    """
    shm = ShmClient(server.shm)
    try:
        region = shm.region_path(buf.bufnum)
    finally:
        shm.close()
    if region is None:
        return (0, 0)
    def size(path: str) -> int:
        return os.path.getsize(path) if os.path.exists(path) else 0

    return (size(region), size(region + ".peaks"))


_samples, _peaks = overview_bytes(long_take)
print(f"the finished take: {_samples / 1e6:.1f} MB of samples, "
      f"summarized beside it in {_peaks / 1e3:.0f} kB — "
      f"which is what the lane opens from")

# %% [markdown]
# ## The window
# The page's, lane for lane, plus the buttons that drive it. `fills` is the one
# thing the host cannot work out for itself: it says *these samples are being
# written*, so a lane stops at the frontier and leaves the axis past it empty
# rather than inking the buffer's own zeros. The finished take carries none —
# it is written everywhere, and that is a different picture.

# %%
win = gui.open(view(
    panel(button(name="record", label="record 4 takes"),
          button(name="edit", label="another peer silences the middle of take 1"),
          label(name="log", text="press record"),
          layout="row", gap=8.0, h=34),
    *[waveform(name=f"take{i}", buffer=t.bufnum, sample_rate=RATE,
               fills=True, ruler="off", ruler_y="off", link=LANES)
      for i, t in enumerate(takes)],
    timeruler(ruler="time", sample_rate=RATE, link=LANES, h=22),
    waveform(name="long", buffer=long_take.bufnum, sample_rate=RATE,
             ruler="off", ruler_y="off", link=LONG),
    timeruler(ruler="time", sample_rate=RATE, link=LONG, h=22),
    title="the page's window, mapped", w=980, h=560, layout="col"))
_closed = False
win.on_closed(lambda: globals().__setitem__("_closed", True))

# %% [markdown]
# ## What the buttons do
# **Record** plays four glissandi and records them at once; `record_buf` passes
# its input through, so the same signal reaches the buffers and the speakers,
# and `done_action=2` frees each node at the end of its take — which is what
# stops the pictures growing. **Silence** is the edit: the server writes the
# span in place, and this script announces it with
# `clausters.Buffer.touch`, which the server broadcasts to every other client —
# the host among them — as the span and never the samples.

# %%
SynthDef(
    "mapped_glissando",
    out(0.0, record_buf(
        control("buf", 0.0, "ir"), 0.0,
        sine(line(control("from", 220.0, "ir"), control("to", 880.0, "ir"), SECONDS))
        * line(0.02, 0.2 / TAKES**0.5, SECONDS),
        done_action=2,
    )),
).send(server)
server.sync()


def say(text: str) -> None:
    """Puts a line in the window's own log and on the terminal."""
    win["log"].set(text=text)
    print(text)


def record() -> None:
    """Four takes at once, heard and recorded."""
    for take in takes:
        octaves = random.uniform(-2.0, 2.0)
        while abs(octaves) < 0.25:      # nothing that just sits on the start
            octaves = random.uniform(-2.0, 2.0)
        Synth("mapped_glissando",
              {"buf": float(take.bufnum), "from": 220.0, "to": 220.0 * 2.0**octaves},
              server=server)
    say("recording: the traces grow with the sound, and nothing is sent")


def peer_edit() -> None:
    """Silence the middle third of the first take, and say where."""
    start = takes[0].frames // 3
    frames = takes[0].frames // 3
    takes[0].silence(start, frames)
    takes[0].touch(0, start, frames)
    say(f"another peer silenced frames {start}..{start + frames} of take 1 — "
        "the lane re-summarizes that span and nothing else")


# Each button acts on its **click**: the press completed on the button, so a
# press slid off before letting go cancels rather than recording a second pass.
win["record"].on_click(record)
win["edit"].on_click(peer_edit)
print("press the buttons in the window; zoom with the wheel over any lane or ruler")

# %% [markdown]
# ## Clear `fills` when a take is finished
# A recorder frees itself at the end of its buffer, so the frontier stops
# moving and what was written is all there is. `fills` comes off then, and the
# lane goes back to drawing the whole of its samples — the same lane, a
# different claim about what it holds.

# %%
def finished() -> None:
    """Tells the four lanes their takes are written, whenever you are done."""
    for i in range(TAKES):
        win[f"take{i}"].set(fills=False)
    say("the lanes now draw the whole of what they hold")


# %% [markdown]
# ## Run it
# `clausters.gui.GuiHost.pump` is what makes the buttons work *and* what makes
# closing the window end this: the host's messages are dispatched to the
# handlers from the script's own loop, never from a thread of their own.

# %%
def run(seconds: "float | None" = None) -> None:
    """Pumps host events until the window is closed (or ``seconds`` pass, which
    is what a cell run wants so the prompt comes back)."""
    start = time.monotonic()
    while not _closed and (seconds is None or time.monotonic() - start < seconds):
        gui.pump(timeout=0.05)


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("up — press the buttons, or call record(), peer_edit(), finished(); "
          "run(10) to pump for ten seconds, session.close() to end")
