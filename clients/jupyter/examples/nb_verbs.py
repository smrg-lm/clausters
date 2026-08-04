#!/usr/bin/env python3
# %%
"""The verbs draw in the notebook: `plot` and `scope` in their own cells.

The shortest thing this package does. One import wires the session, and after
it the ordinary client's verbs draw where you are looking -- `clausters.plot`
in the cell that produced it, `clausters.scope` watching a bus of the audio
engine running **in the page**. No display server, no GUI process, no audio
device on this machine: the wasm GUI host and the wasm engine both live in the
browser tab, and this kernel only authors.

What it shows: the ambient host a notebook installs, a window per cell, and the
in-page engine sounding a synth the kernel started.

What it needs: a browser with WebGPU and Web Audio (Chrome, or Firefox with
``dom.webgpu.enabled``). Nothing native -- this example never launches a
binary.

How to run it: install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python -e ./clients/jupyter \
        jupyterlab jupytext

then start JupyterLab in this directory and open it::

    cd clients/jupyter/examples && ../../../.venv/bin/jupyter lab

Either half opens: this ``.py`` and the ``.ipynb`` beside it are the same
notebook, paired by jupytext (``jupytext.toml`` here), so an edit to one
reaches the other on save. The ``.py`` is the authored half and the one in
git; the notebook is generated, and ``jupytext --sync nb_*.py`` rebuilds it.
Step through with Shift+Enter -- this is not a script.

Run from a terminal it wires nothing, because there is no cell to draw in.

A browser starts no audio until something in the page is clicked, so click
anywhere in the notebook once before the sounding cell.
"""
import clausters_jupyter                                # noqa: F401  (the wiring)

from clausters import plot, scope
from clausters.defs import Synth, SynthDef, control, out, sine

# %% [markdown]
# ## A plot is the cell's output
# `clausters.plot` renders the def offline and opens a window on the ambient
# host -- the same call as on the desktop. The window becomes this cell's
# canvas because it is the cell's **last expression**: displaying it is what
# makes the widget, so a plot assigned to a variable and never shown draws
# nothing (and is still displayable later, from another cell).

# %%
plot(sine(220) * 0.5, dur=0.02, label="220 Hz")

# %% [markdown]
# ## Every cell gets its own canvas, and they share one host
# The second plot is another window on the same wasm host -- one host per
# *notebook*, however many cells draw (the page may hold several, one per open
# notebook). Here a plain sequence, materialized by the client.

# %%
plot([(i / 200) % 1.0 - 0.5 for i in range(1200)], label="a ramp")

# %% [markdown]
# ## The engine is in the page, so this is heard, not rendered
# `session.server` is an ordinary `clausters.defs.Server`; its packets happen
# to travel over the kernel's comm to the engine in the tab. A def is sent and
# a synth started exactly as they would be against a native server.

# %%
session = clausters_jupyter.current()
server = session.server
# No cell for the engine here: the plot below displays a window, and any
# displayed window carries the same leg to the same engine. A notebook that
# only sounds -- nothing on screen -- is the one that needs
# `clausters_jupyter.audio()` in a cell of its own; see nb_widgets.


def tone(name: str = "notebook_tone") -> SynthDef:
    """A quiet stereo sine on the `freq` control (default 220 Hz)."""
    sig = sine(freq=control("freq", 220.0)) * 0.2
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


tone().send(server)
synth = Synth("notebook_tone", {"freq": 330.0}, server=server)
print("click anywhere in the page if you hear nothing -- a browser starts no "
      "audio without a gesture")

# %% [markdown]
# ## A scope watches the bus it is sounding on
# The scope reads the engine over the wire (the page has no shared memory), so
# it is a stream rather than a mapping -- and it moves, which is the point.

# %%
scope(bus=0, channels=2, label="out")

# %% [markdown]
# ## Retune it and watch the trace follow
# Ordinary client calls. Re-run this cell with other numbers; the scope above
# keeps drawing.

# %%
synth.set({"freq": 550.0})

# %% [markdown]
# ## Stop
# What was created is freed, in the order it was made: the node with `free`,
# the windows with `close_all`. Neither happens on its own -- the engine and
# the GUI host live in the page for as long as this kernel does, so a synth
# nobody freed keeps sounding through every later cell, and a window nobody
# closed keeps its canvas. Closing the notebook's tab does not end it either:
# that leaves the kernel running, which is what these belong to. Ending the
# kernel is what releases the rest.
#
# `quit` ends the server, as it does anywhere: this one is a live engine,
# not a rendering that can be asked again. What differs by backend is
# how you get another -- a native one is booted from a cell, and the
# page's comes with the page.

# %%
synth.free()
session.gui().close_all()          # the two plots and the scope
server.quit()                      # and the engine in the page
print("node freed, windows closed, engine stopped")

# %%
