#!/usr/bin/env python3
"""A bound knob drives a synth directly: the value bypasses the script.

The low-latency interactive path: a knob *bound* to a running synth's control
(`clausters.gui.host.GuiHost.bind`) sends its value **straight to the audio
server** on every turn, with no round-trip through this Python process. An
unbound knob would instead emit a ``/gui_event`` back here; binding swaps that
for a direct ``/node_set`` to the server.

The point of the binding is that it lives **in the GUI host, not in this
script**: ``/gui_bind`` registers ``knob "freq" -> /node_set <node> freq`` inside
the host, and the host forwards every change to the audio server on its own. So
while the host runs, the knob drives the pitch with nothing going through Python
-- turn it and nothing prints here. (A binding baked into a *saved standalone*
bundle keeps working with no client at all, even after every script exits --
that is ``gui_standalone.py``.)

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/gui_bind.py``. It self-launches the audio
server and the GUI host (`Session.live` + `Session.gui`); by hand that is
``clausters`` and ``clausters-gui --server 127.0.0.1:57110``. Run this with no
server already up on 57110, so the session boots its own. Needs a display and a
GPU adapter.
"""

# %%
import sys
import time

from clausters import Session
from clausters.defs import SynthDef, control, out, sine
from clausters.gui import knob, window
from clausters.defs import Synth

# %% [markdown]
# ## Launch the server and the GUI, and a synth to drive
# `Session.live()` boots the audio server; `session.gui()` boots the GUI host
# with its client leg pointed at that server, which is what lets `/gui_bind`
# forward straight to it.

# %%
session = Session.live()
server = session.server
gui = session.gui()


def beep(name: str = "gui_bind_beep") -> SynthDef:
    """A quiet stereo sine whose frequency is the `freq` control (default
    220 Hz) -- the binding target `/node_set <node> freq <value>` drives."""
    sig = sine(freq=control("freq", 220.0)) * 0.2
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


beep().send(server)
synth = Synth("gui_bind_beep", {"freq": 220.0}, server=server)

# %% [markdown]
# ## A named knob, bound to the synth's freq
# The knob is *named*, not numbered -- the script addresses it by that name and
# never picks an id. `bind` registers the forward in the host.
#
# A control knows how big it wants to be, so on its own it would be a strip at
# the top of the window; `weight` overrides that and stretches it over the whole
# pane -- one knob is all this window has to show.

# %%
win = gui.open(window(
    knob(name="freq", label="freq", min=110.0, max=880.0, value=220.0,
         weight=1.0),
    title="Bound knob -> synth freq", w=420, h=260, layout="col"))
win["freq"].bind("/node_set", synth.id, "freq")
win.on_closed(lambda: globals().__setitem__("_closed", True))
print(f"knob bound to synth {synth.id} freq; turn it -- the pitch follows "
      "directly and nothing prints here (no script round-trip)")

# %% [markdown]
# ## Drive it
# Nothing to do but wait: the bound knob sends its value to the server, not here,
# so this loop only pumps events to notice the window closing. Turn the knob and
# hear the pitch follow with no Python in the path.

# %%
_closed = False


def run(seconds: float | None = None) -> None:
    """Pumps events for ``seconds`` (a bound knob sends none back).

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
        synth.free()
        session.close()
else:
    print("bind up - run(10) to pump events, session.close() to end")
