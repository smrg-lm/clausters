#!/usr/bin/env python3
# %%
"""The native backend: a real server on this machine, its GUI in the cell.

The default backend runs everything in the browser tab, which is what makes it
work with a remote kernel -- and what costs it Faust, shared memory and the
audio devices of the machine. `clausters_jupyter.notebook` with ``"native"``
trades that back: it boots the ordinary `clausters` server here, with all of
its capability, and keeps only the GUI in the page.

Two wires instead of one, and neither goes through this kernel twice: the
client reaches the server over its socket, as any script does, and the host in
the page opens its **own** WebSocket to the server's ``--ws`` port -- which is
why ``ws`` is forced on when this boots the server. A bound widget therefore
still drives the audio at frame rate.

This is local-only, for two separate reasons: the sound comes out of the
kernel's speakers, and the page opens that WebSocket from the *browser*. With a
remote kernel neither reaches you, and nothing here can detect that -- it just
draws, silently, with meters that never move. Use the default backend there.

What it shows: `notebook("native")`, a FaustDef compiled by the server's JIT
(the in-page engine has no libfaust), and a bound widget over the page's own
socket to that server.

What it needs: a browser with WebGPU, an audio device on this machine, and the
``clausters`` server binary -- the wheel bundles one, and in a source checkout
``scripts/refresh-bin.sh`` stages a fresh one over it (the bundled copy wins,
so it goes stale the moment a crate is rebuilt).

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

Note the first cell: importing the package wires the default (page) session,
so choosing a backend means calling `notebook` before any cell has drawn
anything. It replaces the auto-wired one, which has cost nothing yet.
"""
import clausters_jupyter

# The import already wired the default (page) session -- it does that in any
# IPython shell. An explicit call *replaces* it as long as no cell is showing
# anything yet, which is why this belongs in the first cell.
session = clausters_jupyter.notebook("native")
server = session.server
gui = session.gui()
print(f"server on {server.target.host}:{server.target.port}; "
      "the page reaches it on 57120")

# %% [markdown]
# ## Something only this backend can do
# A FaustDef is compiled by libfaust inside the server. The in-page engine is
# the ``synth,embed`` build -- no libfaust, no LLVM -- so this def exists on
# this backend and nowhere else.

# %%
from clausters.defs import FaustDef, Synth                          # noqa: E402

DSP = """
import("stdfaust.lib");
freq = hslider("freq", 220, 50, 2000, 0.01);
cutoff = hslider("cutoff", 800, 100, 8000, 1);
process = os.sawtooth(freq) : fi.lowpass(2, cutoff) * 0.2 <: _, _;
"""

FaustDef.from_source("notebook_faust", DSP).send(server)
synth = Synth("notebook_faust", {"freq": 110.0, "cutoff": 900.0}, server=server)
print("a filtered saw, compiled on this machine")

# %% [markdown]
# ## Plot it, and bind a control to it
# The plot is drawn by the wasm host in the cell, exactly as on the other
# backend -- the GUI does not know which server it is looking at. The bound
# knob goes from the page straight to the native server over that WebSocket.

# %%
from clausters.gui import knob, window                              # noqa: E402

win = gui.open(window(
    knob(name="cutoff", label="cutoff", min=100.0, max=8000.0, value=900.0,
         weight=1.0),
    title="cutoff -> native server", w=360, h=240, layout="col"))
win["cutoff"].bind("/node_set", synth.id, "cutoff")
win

# %% [markdown]
# ## A scope, reading the machine's own output bus
# Turn the knob above and watch the trace change while this kernel does
# nothing at all.

# %%
from clausters import scope                                         # noqa: E402

scope(bus=0, channels=2, label="out")

# %% [markdown]
# ## Stop
# Freed in the order it was made, and here the last step is a real one: this
# backend booted a server process, so `close` stops it. On the default backend
# there is no process to stop -- the engine lives in the page and ends with the
# kernel -- but everything above it is freed the same way either way.

# %%
gui.close_all()                    # the knob's window and the scope's
synth.free()
server.quit()                      # /server_quit, the graceful stop
server.close()                     # and this end of the socket
print("windows closed, node freed, server stopped")
