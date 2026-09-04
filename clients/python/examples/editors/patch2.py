#!/usr/bin/env python3
"""The patcher, **level 2**: a single def's internal UGen graph, drawn.

This is the level-2 half of the patcher; `examples/editors/patch1.py` is level 1
(whole defs wired by server buses). The two are one directed, typed grammar apart
by what a box *is*: at level 1 a box is a **whole def** and a cord *is* a
**server bus**; here a box is a **UGen** (or a Faust signal op) and a cord is an
**internal wire** of the one def — never an allocated bus. Level 2 adds a third
cord weight over level 1's audio (`ar`, heavy) / control (`kr`, thin): **init**
(`ir`, dashed) — a scalar read once at init time.

The Def-view is a **read-only representation**, decoded from the def's own
structure — no server, no sound. Two verbs open it, distinct on purpose:

- ``some_def.plot_def()`` draws the def's **structure** — its UGen boxes and the
  cords between them — the picture this example is about.
- ``clausters.plot(some_def)`` renders the def's **sound** — its output waveform.

Both a `SynthDef` and a `FaustDef` decode: ``SynthDef.plot_def`` reads the
in-memory UGen graph (every UGen a box, every input a cord; a constant input
stays the box's value, uncorded), and ``FaustDef.plot_def`` reads a signal-tree
def the same way (a box-tree or source def is opaque, so it draws as one box).
The decode is faithful — ``DefPatch.from_synthdef(sdef).to_synthdef(name)``
reproduces the original spec — so what you see is exactly what the def is.

Run it as a script (``python patch2.py``) or cell by cell (``# %%``). It
needs **no audio server** (the Def-view rides no server), only a display with a
GPU adapter; the install bundles the GUI binary. Refresh the bundled native copy
first if you have rebuilt anything: ``scripts/refresh-bin.sh``.
"""

# %%
import sys

from clausters.defs import (
    DefPatch,
    FaustDef,
    SynthDef,
    control,
    out,
    sine,
)
from clausters.gui import GuiHost


# %% [markdown]
# ## A SynthDef with all three cord weights
# A small SynthDef, built to show every cord type in one picture: audio cords
# (heavy) carry the sound, a control cord (thin) is the tremolo LFO, and an init
# cord (dashed) is the scalar ``detune`` read once at note start. Each UGen
# becomes a box — inlets named from the builder's own signature (``out(bus,
# signal)`` -> ``bus``/``signal``), the arithmetic and op UGens named by their
# operation (``Mul``, ``Add``, ``midicps`` …) — and every input a cord, unless it
# is a constant (then it stays the box's value, drawn as a plain inlet).

# %%
def tremolo_sine() -> SynthDef:
    """A detuned sine with a control-rate tremolo — audio, control and init cords
    in one graph, so the Def-view draws all three cord weights."""
    freq = control("freq", 220.0)                       # kr control
    amp = control("amp", 0.2)                           # kr control
    detune = control("detune", 1.5, rate="ir")          # ir (scalar) control
    # A control-rate tremolo: a kr sine, unipolar. Its cords are thin (control).
    tremolo = sine(control("lfo", 5.0)).at_rate("kr") * 0.5 + 0.5
    carrier = sine(freq * detune)                       # detune feeds an ir cord
    sig = carrier * amp * tremolo                       # audio cords (heavy)
    return SynthDef("tremolo_sine", out(0.0, sig), out(1.0, sig))


synth_def = tremolo_sine()

# The model behind the picture — usable headless. Print its structure so the
# view can be corroborated against it: every box carries a layout `role` (a
# `source` control, a `const` value box, or a plain `object` UGen) that the host
# uses to lay the graph out as an inverted tree.
patch = DefPatch.from_synthdef(synth_def)
print(f"{synth_def.name} decoded into {len(patch.boxes)} boxes, {len(patch.cords)} cords")
for i, box in enumerate(patch.boxes):
    inlets = [p["name"] or "-" for p in box["ports"] if p["dir"] == "in"]
    print(f"  box {i}: {box['def']:<10} [{box.get('role', 'object'):>7}] inlets={inlets}")

# The decode is faithful: rebuilding the SynthDef reproduces the original spec.
rebuilt = patch.to_synthdef(synth_def.name)
print("round trip reproduces the spec:", rebuilt.spec() == synth_def.spec())


# %% [markdown]
# ## A FaustDef signal tree, decoded the same way
# A Faust signal graph decodes node for node: every op a box, every operand a
# cord, the ``hslider`` a source box. The grammar is the same as the SynthDef's.

# %%
def fm_tone() -> FaustDef:
    """A Faust FM tone: a sine modulated in frequency by another, gained by a
    slider — a signal tree the Def-view decodes node for node."""
    from clausters.defs.signals import hslider, sin

    freq = hslider("freq", 220.0, 20.0, 2000.0, 0.1)
    gain = hslider("gain", 0.2, 0.0, 1.0, 0.01)
    modulator = sin(freq * 3.0) * 40.0
    return FaustDef.from_signals("fm_tone", sin(freq + modulator) * gain)


faust_def = fm_tone()
faust_patch = DefPatch.from_faustdef(faust_def)
print(f"\n{faust_def.name} decoded into {len(faust_patch.boxes)} boxes, "
      f"{len(faust_patch.cords)} cords")


# %% [markdown]
# ## The windows: one Def-view at a time
# ``plot_def()`` opens each def's structure in its own window on the given host.
# We open the SynthDef view first; close it (the window's X) to open the Faust
# one. Pan with a drag on the empty canvas, zoom with the wheel — the patch sits
# in a scroll workspace, so a graph bigger than the window stays reachable.

# %%
def show(host, a_def, kind: str) -> None:
    """Open a def's Def-view and block until its window is closed. The window and
    the panel are captioned by the def *kind* (``SynthDef`` / ``FaustDef``) — the
    visualization this example is about — not by the def's own name.

    The window handle answers its own close: `PatchWindow.wait` holds the
    script until the ``/gui_closed`` arrives, on the host's own event loop — no
    widget id matched by hand, and no loop written here."""
    print(f"\nthe {kind} (level 2) — close the window to continue")
    win = a_def.plot_def(host=host, title=f"{kind} — level 2")
    win.wait()


if __name__ == "__main__":
    try:
        with GuiHost().boot() as gui:      # no server: the Def-view needs none
            show(gui, synth_def, "SynthDef")
            show(gui, faust_def, "FaustDef")
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
