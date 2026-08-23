#!/usr/bin/env python3
"""A bound panel drives a synth directly: the values bypass the script.

The low-latency interactive path: a control *bound* to a running synth sends its
value **straight to the audio server** on every turn, with no round-trip through
this Python process. An unbound control would instead emit a ``/gui_event`` back
here; binding swaps that for a direct ``/node_set`` to the server.

The controls are built from the def's own `clausters.defs.control` objects, so
each widget already knows which control it drives and the window binds in one
verb -- ``win.bind(synth)`` -- instead of one hand-typed name per widget.

The point of the binding is that it lives **in the GUI host, not in this
script**: ``/gui_bind`` registers ``knob -> /node_set <node> freq`` inside
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
from clausters.gui import knob, layout, slider
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


#: The controls the panel drives. A control is a **name and a default** -- what
#: `/node_set` addresses and what the synth starts at -- and a widget built from
#: one reads both, so the two cannot disagree about what "freq" is. The *range*
#: is not here: a control is a signal in the graph and says nothing about how a
#: knob should be drawn, so each widget below spells its own.
FREQ = control("freq", 220.0)
AMP = control("amp", 0.2)


def beep(name: str = "gui_bind_beep") -> SynthDef:
    """A quiet stereo sine whose frequency and level are the `freq` and `amp`
    controls -- what the bindings `/node_set <node> <control> <value>` drive."""
    sig = sine(freq=FREQ) * AMP
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


beep().send(server)
synth = Synth("gui_bind_beep", server=server)   # the def's own defaults

# %% [markdown]
# ## A panel of the def's controls, bound to the synth
# Each widget is built from the def's own control: its name and its default come
# from `FREQ`/`AMP`, and the range it is turned over is the widget's own. The
# name each takes is the control's, which is what the script addresses it by --
# it never picks an id.
#
# So the window knows what it drives, and `win.bind(synth)` wires the whole
# surface at once: one `/gui_bind` per control widget, each forwarding
# `/node_set <node> <control> <value>`. Binding one at a time is still there
# (`win["freq"].bind("/node_set", synth.id, "freq")`) and is what you reach for
# when the target is not a def control -- a bus, another widget, an arbitrary
# address.
#
# The view is the subject either way: `v.open()` rather than `host.open(v)`, on
# the host `session.gui()` already made ambient.

# %%
v = layout(knob(FREQ, min=110.0, max=880.0),
           slider(AMP, min=0.0, max=0.5),
           flow="col")

win = v.open()
win.bind(synth)
win.on_closed(lambda: globals().__setitem__("_closed", True))
print(f"bound to synth {synth.id}: {win.controls} -- turn them, the sound "
      "follows directly and nothing prints here (no script round-trip)")

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
