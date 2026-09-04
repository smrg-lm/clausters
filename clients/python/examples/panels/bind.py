#!/usr/bin/env python3
"""A bound panel drives a synth directly: the values bypass the script.

The low-latency interactive path: a control *bound* to a running synth sends its
value **straight to the audio server** on every turn, with no round-trip through
this Python process. An unbound control would instead emit a ``/gui_event`` back
here; binding swaps that for a direct ``/node_set`` to the server.

The controls are built from the def's own `clausters.defs.control` objects, so
each widget already knows which control it drives and the window binds in one
verb -- ``win.bind(synth)`` -- instead of one hand-typed name per widget.

**The two shapes a control signal comes in** are the two buttons. A button's
press *is* the event, and its ``mode`` says which of the two pointer primitives
reaches the server: the default ``"gate"`` sends its ``on`` at the press and its
``off`` at the release, so the value lasts exactly as long as the button is
held; ``"press"`` sends one message and nothing after it -- the bang. A widget
cannot make a value instantaneous, so the bang is only a bang against a control
that returns to zero on its own: a trigger (``rate="tr"``), which the server
resets after one block. Hold one button and the note sustains; hit the other and
a blip fires, both with no Python in the path.

The point of the binding is that it lives **in the GUI host, not in this
script**: ``/gui_bind`` registers ``knob -> /node_set <node> freq`` inside
the host, and the host forwards every change to the audio server on its own. So
while the host runs, the knob drives the pitch with nothing going through Python
-- turn it and nothing prints here. (A binding baked into a *saved standalone*
bundle keeps working with no client at all, even after every script exits --
that is ``standalone.py``.)

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention) and
**runs out of the box**. Install once, from the repo root::

    python -m venv .venv
    .venv/bin/pip install -e ./clients/python      # bundles the server + GUI binaries

Run it cell by cell (Shift+Enter), or as a plain script --
``python clients/python/examples/panels/bind.py``. It self-launches the audio
server and the GUI host (`Session.live` + `Session.gui`); by hand that is
``clausters`` and ``clausters-gui --server 127.0.0.1:57110``. Run this with no
server already up on 57110, so the session boots its own. Needs a display and a
GPU adapter.
"""

# %%
import sys

from clausters import Session
from clausters.defs import Env, SynthDef, control, env_gen, out, sine
from clausters.gui import button, knob, layout, slider
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

#: The two shapes a control signal comes in, and the reason a button has a mode.
#: `GATE` is an ordinary control the graph reads as a gate: the envelope
#: sustains while it is held and releases when it falls, so a button must send
#: both edges. `FIRE` is a **trigger** (`rate="tr"`), which the server resets to
#: zero after one block -- so a button that sends only the press is a bang
#: against it, and could not be one against `GATE`, where the value would stand
#: forever.
GATE = control("gate", 0.0)
FIRE = control("fire", 0.0, rate="tr")


def beep(name: str = "bind_beep") -> SynthDef:
    """A quiet stereo sine whose frequency and level are the `freq` and `amp`
    controls -- what the bindings `/node_set <node> <control> <value>` drive.

    Two envelopes over the same tone say what the two buttons do: a sustaining
    one the `gate` holds open, and a percussive one the `fire` trigger restarts
    from the top each time it arrives."""
    held = env_gen(Env.asr(0.02, 1.0, 0.4), gate=GATE)
    blip = env_gen(Env.perc(0.005, 0.35), gate=FIRE)
    sig = sine(freq=FREQ) * (held + blip) * AMP
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


beep().send(server)
synth = Synth("bind_beep", server=server)   # the def's own defaults

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
#
# The buttons bind by the same verb and the same rule as the knob: each one was
# built from a control, so `win.bind(synth)` wires all four.

# %%
v = layout(knob(FREQ, min=110.0, max=880.0),
           slider(AMP, min=0.0, max=0.5),
           # A gate needs no range: the button sends its two values, `1`/`0`
           # unless another pair is named. Held, so both edges reach the server.
           button(GATE, label="hold"),
           # And the bang: `on` at the press and nothing after it. Written
           # against `GATE` instead, this would raise -- the press would leave
           # the gate standing open forever.
           button(FIRE, mode="press", label="fire"),
           flow="col")

win = v.open()
win.bind(synth)
print(f"bound to synth {synth.id}: {win.controls} -- turn them, the sound "
      "follows directly and nothing prints here (no script round-trip). "
      "Hold `hold` and the note sustains for as long as you hold it; hit "
      "`fire` and a blip sounds once per press.")

# %% [markdown]
# ## Drive it
# Nothing to do but wait: every bound widget sends its value to the server, not
# here, so the script has nothing left but to stay alive until the window is
# closed. Turn the knob and hear the pitch follow with no Python in the path;
# hold `hold` for a sustained note and tap `fire` for a blip, and notice that
# the difference between them is one prop.

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        win.wait()
    finally:
        synth.free()
        session.close()
else:
    print("bind up - win.wait(10) to hold it, session.close() to end")
