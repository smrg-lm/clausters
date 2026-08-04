#!/usr/bin/env python3
# %%
"""Widgets in a cell: bound to the engine, and read back from the next cell.

Two things a notebook does differently, and both are about *where the loop is*.

A **bound** widget never involves this kernel: `/gui_bind` registers the
forward inside the GUI host, and the host talks to the engine directly -- both
of them in the page, so a knob turned during a long cell still drives the pitch
at frame rate. That is the same wire a served page and the desktop use; the
kernel is an author, never a relay.

An **unbound** widget sends a ``/gui_event`` back here, and that event cannot
arrive while a cell is running: ipykernel holds the shell channel until the
cell ends, so the answer queues behind it. Hence the shape of this example --
interact, then run the next cell to drain. `clausters.gui.host.GuiHost.pump`
dispatches what has already arrived, which is exactly what a notebook can do.

What it shows: `bind` against the in-page engine, `pump` between cells, and
the refusal (`clausters_jupyter.RoundTripInCell`) you get for asking a running
cell to wait for an answer.

What it needs: a browser with WebGPU and Web Audio. Nothing native.

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
"""
import clausters_jupyter

from clausters.defs import Synth, SynthDef, control, out, sine
from clausters.gui import knob, slider, window

session = clausters_jupyter.current()
server = session.server
gui = session.gui()      # the in-page host the import installed

# The engine is in the browser tab, so starting it is not launching a process:
# it is giving it a cell to run in, since the page executes nothing until some
# cell has an output. `audio()` is that cell -- an empty box that draws nothing
# and only has to exist -- and it is *displayed* here, as the last expression,
# the way every widget library hands you an object and lets the cell show it.
# From here a synth sounds when it is created, as it does against any other
# server.
clausters_jupyter.audio()

# %% [markdown]
# ## Something to drive
# An ordinary def and an ordinary synth, sent to the engine in the page.

# %%
def voice(name: str = "notebook_voice") -> SynthDef:
    """A stereo sine on `freq`, with `amp` for the bound slider to move."""
    sig = sine(freq=control("freq", 220.0)) * control("amp", 0.2)
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


voice().send(server)
synth = Synth("notebook_voice", {"freq": 220.0, "amp": 0.2}, server=server)
print("sounding")

# %% [markdown]
# ## It sounds from here
# The synth above is running on the engine booted in the first cell. Click
# anywhere once if you hear nothing -- no browser starts audio without a
# gesture; the console says `clausters: engine up` and whether the
# `AudioContext` is running.

# %% [markdown]
# ## Two bound controls and one unbound
# `bind` points a widget at a server command; the host forwards every change
# on its own. The third knob is left unbound, so it reports back here instead.

# %%
win = gui.open(window(
    knob(name="freq", label="freq", min=110.0, max=880.0, value=220.0),
    slider(name="amp", label="amp", min=0.0, max=0.5, value=0.2),
    knob(name="watch", label="reported", min=0.0, max=1.0, value=0.0),
    title="bound / unbound", w=460, h=220, layout="row"))
win["freq"].bind("/node_set", synth.id, "freq")
win["amp"].bind("/node_set", synth.id, "amp")

reported = []
win["watch"].on_event(lambda value, *_: reported.append(value))
win

# %% [markdown]
# ## Turn the knobs, then run the cell below
# A knob is turned by **dragging up and down** on it, not by clicking; the
# slider goes sideways. `freq` and `amp` move the sound while you turn them --
# nothing prints, and nothing has to be running here. `reported` only fills
# when this kernel gets a turn, which is what the next cell gives it.

# %%
print(f"{gui.pump()} event(s) drained; last reported value: "
      f"{reported[-1] if reported else 'none yet'}")

# %% [markdown]
# ## What a round trip inside one cell does
# Asking to *wait* for an answer from the cell that is running cannot work --
# the answer is queued behind that very cell. It raises rather than hanging
# until the timeout, and the message says how to split the work.

# %%
try:
    gui.query(win)
except clausters_jupyter.RoundTripInCell as exc:
    print(f"refused, as it should be:\n  {exc}")

# %% [markdown]
# ## Stop
# Everything this made is freed here. `close` frees the window, and the cell
# showing it goes empty -- the canvas belongs to the window, so it leaves with
# it. The node is freed separately: nothing about a window owns what it was
# driving. Neither is automatic, and neither follows the notebook's tab: the
# host and the engine are the page's and live as long as this *kernel*, so a
# synth left running keeps sounding until the kernel ends.
#
# `quit` ends the server, as it does anywhere: this one is a live engine,
# not a rendering that can be asked again. What differs by backend is
# how you get another -- a native one is booted from a cell, and the
# page's comes with the page.

# %%
win.close()
synth.free()
server.quit()                      # the engine in the page, booted with it
print("window closed, node freed, engine stopped")

# %%

# %%

# %%
