#!/usr/bin/env python3
"""``edit(buffer)``: a take in the audio editor, edited as a list of parts.

A `clausters.defs.Buffer` opens in `clausters.gui.editing.AudioEditor`, which
edits a private copy of it and writes nothing it was handed until it is saved.
The window draws a **join** -- a buffer made of spans of other buffers -- and
every edit leaves a new list of those spans:

- a **cut** takes a span out of the list, and moves no samples;
- a **paste** writes the block into a take of its own and puts it in;
- a **mix** writes a new take over the frames the block lands on, the block
  added onto them;
- a **pencil stroke** writes a new take the size of the stroke and splices it
  over the frames it was drawn on.

So an undo is the list before, stitched again: it costs the list, not the
samples. The takes a stroke or a paste made are kept while an undo or a redo
can still reach them and freed when neither can -- ``history_bytes`` below caps
what only the history holds, and the oldest entries go first past it.

What to do in the window: **drag** to select, then **Ctrl+X** to cut,
**Ctrl+C** to copy, **Ctrl+V** to paste at the cursor and
**Ctrl+Shift+V** to mix there. **Wheel** to zoom in until each sample is a
disc, then **Alt+drag** to draw. **Ctrl+Z** and **Ctrl+Shift+Z** walk the
history. ``hear()`` plays the take as the edits have left it, and ``parts()``
prints what it is made of. ``editor.save()`` -- or Ctrl+S in the window --
writes the edit back into the buffer it was opened from, and
``editor.save(path)`` writes it as a file instead.

Run it as a script, or step through the cells::

    pip install -e clients/python

    python clients/python/examples/editors/edit_audio.py

It self-launches the audio server and the GUI host, and writes its own take --
nothing has to be found on disk.
"""

# %%
import math
import sys

from clausters import Session, play
from clausters.defs import Buffer
from clausters.gui import edit

SECONDS = 2.0

# %% [markdown]
# ## A take, made here
#
# Two seconds of a decaying tone, written into a server buffer with
# `clausters.defs.Buffer.from_samples`. A shape worth recognizing, so an edit
# over it is visibly an edit over *this*.

# %%
session = Session.live()
server = session.server
rate = server.query_info().nominal_sample_rate
frames = int(SECONDS * rate)
samples = [
    math.exp(-3.0 * i / frames) * 0.7 * math.sin(2 * math.pi * 440.0 * i / rate)
    for i in range(frames)
]
take = Buffer.from_samples(samples, 1, rate, server=server)

# %% [markdown]
# ## One verb
#
# A `Buffer` opens in the audio editor: one `waveform` over the join it owns.
# The buffer is not written while you edit -- every edit is a new list of parts
# over a copy of it and over the takes the edits made -- until you save.

# %%
session.gui()          # the host wired to this session's server
editor = edit(take, title="take", history_bytes=64 * 1024 * 1024)

# %% [markdown]
# ## Hear it, and read what it is made of
#
# `editor.buffer` is the join the window draws: the take as the edits have left
# it, which `clausters.play` reads like any other buffer.

# %%
def hear():
    """Play the take as the edits have left it."""
    play(editor.buffer, server=server)


def parts():
    """Print the spans the take is made of, in the order they play."""
    for part in editor.parts:
        span = part["source"]["range"]
        print(f"buffer {part['source']['source']}: frames {span['start']}..{span['end']}")


# %%
def run():
    """Keep the window open until it is closed."""
    print("drag to select, Ctrl+X / Ctrl+C / Ctrl+V / Ctrl+Shift+V, Alt+drag to draw.")
    print("Ctrl+Z walks back. Call hear() to play it, parts() to read it.")
    editor.wait()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run()
else:
    print("up -- edit the take, hear() to play it, parts() to read it")
