#!/usr/bin/env python3
"""Four takes drawn while they are recorded, natively.

The Python twin of ``clients/web/examples/views/recording.html``.

**Record 4 takes** plays four glissandi and records each into its own buffer.
This host shares memory with the server, so it reads the buffers directly as
they are written; nothing is sent over the wire. A page cannot share that
memory, so there the server sends a summary of what was just recorded
(``/buffer_stream``). The picture is the same either way.

**Zoom** with the wheel and **pan** by dragging, over the ruler or any lane.

The second button stands for another client editing a take: it silences the
middle of the first one and says which span changed (``/buffer_touch``). The
host re-summarizes only that span.

Needs an audio device, a display and a GPU adapter. With the client importable
(``pip install -e ./clients/python``), run it as a script, or step through its
``# %%`` cells::

    python clients/python/examples/views/recording_mapped.py
"""

# %%
import random
import sys

from clausters import Buffer, Session, Synth, SynthDef
from clausters.defs import control
from clausters.defs.ugens import line, out, record_buf, sine
from clausters.gui import button, label, panel, timeruler, view, waveform

#: The four takes.
TAKES = 4
SECONDS = 8.0
RATE = 48_000.0
#: The navigation group the four lanes and their ruler share.
LANES = 1

# %% [markdown]
# ## The server, the host and the segment
# One server owning a segment and one host mapping it: every buffer lives in a
# region beside that segment, and the host reads it there.

# %%
session = Session.live()
server = session.server
gui = session.gui()
print(f"audio server on segment {server.shm}")

takes = [Buffer.alloc(int(SECONDS * RATE), 1, server=server) for _ in range(TAKES)]

# %% [markdown]
# ## The window
# The page's, lane for lane, plus the buttons that drive it. `fills` is the one
# thing the host cannot work out for itself: it says *these samples are being
# written*, so a lane stops at the frontier and leaves the axis past it empty
# rather than inking the buffer's own zeros.

# %%
win = view(
    panel(button(name="record", label="record 4 takes"),
          button(name="edit", label="another peer silences the middle of take 1"),
          label(name="log", text="press record"),
          layout="row", gap=8.0, h=34),
    *[waveform(name=f"take{i}", buffer=t.bufnum, sample_rate=RATE,
               fills=True, ruler="off", ruler_y="off", link=LANES)
      for i, t in enumerate(takes)],
    # Under the lanes, so its marks point **up** at them.
    timeruler(ruler="time", dir="up", sample_rate=RATE, link=LANES, h=22),
    title="the page's window, mapped", w=980, h=560, layout="col").open()

# %% [markdown]
# ## What the buttons do
# **Record** plays four glissandi and records them at once; `record_buf` passes
# its input through, so the same signal reaches the buffers and the speakers,
# and `done_action=2` frees each node at the end of its take -- which is what
# stops the pictures growing. **Silence** is the edit: the server writes the
# span in place, and this script announces it with
# `clausters.Buffer.touch`, which the server broadcasts to every other client --
# the host among them -- as the span and never the samples.

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
    takes[0].fill((start, frames, 0.0))   # a mono take: a frame is a sample
    takes[0].touch(0, start, frames)
    say(f"another peer silenced frames {start}..{start + frames} of take 1 -- "
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
# lane goes back to drawing the whole of its samples -- the same lane, a
# different claim about what it holds.

# %%
def finished() -> None:
    """Tells the four lanes their takes are written, whenever you are done."""
    for i in range(TAKES):
        win[f"take{i}"].set(fills=False)
    say("the lanes now draw the whole of what they hold")


# %% [markdown]
# ## Run it
# The buttons work with nothing driving them: the host's event loop delivers
# every message on a thread of its own, so all a script has left to do is stay
# alive until the window closes. Cell by cell there is nothing to call.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        session.close()
else:
    print("up -- press the buttons, or call record(), peer_edit(), finished(); "
          "win.wait(10) to hold for ten seconds, session.close() to end")
