#!/usr/bin/env python3
"""One analysis, two uses: ``Group.auto_order`` and ``Group.parallel``.

The server reads which buses each node touches and infers who depends on
whom. The same reading answers two different questions, and each is a verb on
the group:

- `Group.auto_order` (``/group_sortMode``) -- *in what order must these run?*
  A node that reads a bus runs after the node that writes it. Add the members
  in any order and the chain comes out right, now and after every later change.
- `Group.parallel` (``/group_parallel``) -- *which of these can run at the same
  time?* Members that touch no bus in common cannot affect each other, so the
  server batches them into stages and the DSP worker threads take them. The
  samples are the same either way: this asks for them sooner, never for
  different ones.

The graph is built to show both. Four voices write four **private** buses and
one mixer sums them into the output -- so the four are independent of each
other (parallel can spread them) and all four come before the mixer
(auto-order must put them there). And it is added **backwards**, the mixer
first, so that before the sort the mixer reads buses nobody has written yet
and the output is silent.

What you hear: silence, then a chord the moment `auto_order` lands. What you
read: the tree before and after, which is the server's answer printed back.

`Group.parallel` is the second half, and this example is honest about what it
can show. Whether it *helps* is a measurement and belongs to one
(``cargo run --release --example bench``, whose parallel section sweeps worker
counts over a graph wide enough to see it); what an example can show is the
condition -- workers to run on, and members on disjoint buses -- which is why
the graph is shaped the way it is. Without ``workers`` the flag is accepted and
remembered and everything stays sequential.

`Session.live` boots an audio server if none is up, so in a venv where the
client is installed (``pip install ./clients/python``) this runs on its own::

    python clients/python/examples/basics/group_order.py

Needs an audio device. This file is organized as ``# %%`` cells (the VS Code /
Jupyter convention): step through it with Shift+Enter, or run it as a plain
script.
"""

# %%
import sys

from clausters import Session
from clausters.defs import (
    AddAction, Bus, Group, Synth, SynthDef, control, in_, out, sine,
)

VOICES = 4
CHORD = (220.0, 277.0, 330.0, 440.0)

# %% [markdown]
# ## The two defs
# A voice writes its own bus; the mixer reads four of them and sums them to the
# output. Nothing here says anything about order -- the buses are the whole
# statement, and the server reads them.

# %%
def voice(name: str = "voice") -> SynthDef:
    bus = control("bus", 16.0)
    freq = control("freq", 220.0)
    return SynthDef(name, out(bus, sine(freq) * 0.12))


def mixer(name: str = "mixer") -> SynthDef:
    # One control per bus, rather than one control plus arithmetic: the
    # analysis reads a bus index that is a constant **or a control** (it takes
    # the node's current value), and `first + n` is neither -- it is a signal,
    # which makes the node a barrier the server cannot place. That is the one
    # thing to get right when writing a def for an auto-ordered group.
    sig = sum((in_(control(f"b{n}", 16.0 + n)) for n in range(VOICES)), start=0.0)
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The session and the group
# `workers` is what gives `parallel` something to spread onto: without it the
# flag is remembered and every stage runs on the audio thread. Both verbs
# answer the group, so the group says how it runs in the expression that makes
# it.

# %%
session = Session.live(tempo=1.0, workers=2).activate()
voice().send()
mixer().send()
session.server.sync()

bank = Group().parallel()
buses = [Bus.audio() for _ in range(VOICES)]

# %% [markdown]
# ## Backwards on purpose
# The mixer is added first, so it runs before anything has written the buses it
# reads: silence. Then one `auto_order` and the server puts the voices in front
# of it.

# %%
def run(seconds: float = 4.0):
    """Sound the chord wrong, then let the server order it."""
    mix = Synth("mixer", {f"b{n}": bus.index for n, bus in enumerate(buses)},
                target=bank)
    voices = [
        Synth("voice", {"bus": bus.index, "freq": freq},
              target=bank, action=AddAction.TAIL)
        for bus, freq in zip(buses, CHORD)
    ]
    session.server.sync()
    print("added backwards -- the mixer reads buses nobody has written yet:")
    print(session.server.query_tree(bank))
    session.run(seconds / 2)      # silence

    bank.auto_order()
    session.server.sync()
    print("after auto_order -- the server put the voices in front of it:")
    print(session.server.query_tree(bank))
    session.run(seconds / 2)      # the chord

    for node in [*voices, mix]:
        node.free()
    for bus in buses:
        bus.free()


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    try:
        run()
    finally:
        session.close()
else:
    print("session up - run() to hear it, session.close() to end")
