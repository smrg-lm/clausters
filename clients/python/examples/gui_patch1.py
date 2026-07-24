#!/usr/bin/env python3
"""The patcher, **level 1**: whole defs wired by buses — built, viewed, heard.

This is the level-1 half of the patcher; `examples/gui_patch2.py` is level 2 (a
single def's internal UGen graph). The two are one directed, typed grammar apart
by what a box *is*: here a box is a **whole def** and a cord *is* a **server
bus**; there a box is a **UGen** and a cord is an **internal wire**.

A `clausters.defs.GraphPatch` is the **programmatic** level-1 patcher: boxes
(whole defs) with typed **inlets** (top) and **outlets** (bottom), and a **cord**
per ``outlet -> inlet``. A cord *is* a bus, but you never number one: `compile`
runs the shared cord->bus pass (`clausters_core::patch`, in Rust) that names one
bus per connected net — its writers **sum** — and `to_graphdef` hands back a
`GraphDef` ready to send. That is the whole model, and it needs no GUI:

    p = GraphPatch()
    osc = p.add(osc_def)                  # ports derived from the SynthDef's graph
    dac = p.add(dac_def)                  # a terminal sink: an inlet, no outlet
    p.connect(osc, "out", dac, "in")      # osc -> dac -> speakers
    server.add_graphdef(p.to_graphdef("patch")); server.graph("patch")   # sounds

The GUI is a **view** of that same model. The `patch` widget draws the boxes and
cords and is a full **canvas**:

- **Auto-layout** — the host lays every box out on its own: a layered graph
  drawing (sources on top, sinks at the bottom, signal flowing downward), the
  cords ordered to cross as little as possible and the whole graph centred in the
  window. No box is placed by hand — the same layout the Def-view (level 2) uses.
- **Dragging** — grab a box and move it; the edit flows back as
  ``/gui_event <id> "move" <index> <x> <y>`` and prints here. Moving a box in the
  selection moves the whole selection.
- **Selection** — click a box to select it; drag the empty canvas to sweep a
  **marquee** over several; click empty canvas to clear.
- **Cording** — drag an outlet's pin onto an inlet (either grab order) to draw a
  cord; a rate mismatch is refused at the gesture. The edit flows back as
  ``/gui_event <id> "wire" <src> <outlet> <dst> <inlet>``, which is just
  `GraphPatch.connect` by name — so the picture and the object stay one thing.
- **Navigation** — the patch sits in a `scroll` workspace: **Shift+drag** the
  empty canvas to pan, the wheel zooms anchored at the cursor; boxes, cords and
  text scale together.

Press **render** to compile the patch you drew and hear it, **stop** to free it.
The direction reads top to bottom, and the buses are never on screen: an unwired
outlet keeps its def's default, so these defs default their bus controls to
``SILENT`` (a spare bus nobody reads) — a box is silent until a cord reaches it —
and the hardware output is reached through a **terminal def** (``dac``: an inlet,
no outlet, its ``Out.ar(0, …)`` baked in), a box like any other, not an ``OUT``.

The canvas, its scroll workspace and the two transport buttons are *named*, so
the script wires each by name and never matches a widget id.

Run it as a script (``python gui_patch1.py``) or cell by cell (``# %%``). Needs
a live audio server (it starts its own) and a display with a GPU adapter; the
install bundles both binaries. **The ABI bumped to 11** for the cord->bus pass,
so refresh the bundled native copy first: ``scripts/refresh-bin.sh``.
"""

# %%
import sys

from clausters import Session
from clausters.defs import GraphPatch, SynthDef, control, in_, lag, out, sine
from clausters.gui import button, panel, patch, scroll, window

TEMPO = 2.0
SILENT = 64  # a spare audio bus (0..127) nothing reads: the "unconnected" default.


# %% [markdown]
# ## The member defs (the boxes' building blocks)
# Four SynthDefs forming an effect chain. A control that feeds an ``Out`` is an
# **outlet**, one that feeds an ``In`` an **inlet** — that is where a box's ports
# come from, and it is structural (not a guess). `osc` is a source; `filt` and
# `trem` each read ``in`` and write ``out``; `dac` is the **terminal** stage — it
# reads ``in`` and writes hardware bus 0 itself, so it has an inlet and no outlet
# (the speakers are reached by a cord *into* it).

# %%
def osc(name: str = "osc") -> SynthDef:
    """A sine source: writes ``sine(freq) * amp`` to its ``out`` bus."""
    return SynthDef(name, out(control("out", SILENT),
                              sine(control("freq", 110.0)) * control("amp", 0.2)))


def filt(name: str = "filt") -> SynthDef:
    """A crude tone control: lags (smooths) its input — a one-pole low-pass — and
    scales it by ``gain``."""
    return SynthDef(name, out(control("out", SILENT),
                              lag(in_(control("in", SILENT)), 0.002) * control("gain", 1.0)))


def trem(name: str = "trem") -> SynthDef:
    """A tremolo: reads ``in``, rings it with a slow LFO, writes ``out``."""
    lfo = sine(control("rate", 5.0)) * 0.5 + 0.5
    return SynthDef(name, out(control("out", SILENT),
                              in_(control("in", SILENT)) * lfo))


def dac(name: str = "dac") -> SynthDef:
    """The terminal stage: reads ``in``, scales it, and writes **hardware bus 0**
    itself (baked ``Out.ar(0, …)``). It has an inlet and no outlet — a cord into
    it is the only path to the speakers."""
    return SynthDef(name, out(0, in_(control("in", SILENT)) * control("amp", 0.4)))


session = Session.live(tempo=TEMPO, latency=0.1)
server = session.server
defs = {"osc": osc(), "filt": filt(), "trem": trem(), "dac": dac()}
for sdef in defs.values():
    server.add_synthdef(sdef)


# %% [markdown]
# ## The patch, built in code
# The chain `osc -> filt -> trem -> dac` (the terminal sink that reaches the
# speakers itself). Passing the `SynthDef` to `add` **derives its ports from the
# def's graph** — the `out`/`in_` controls become outlets/inlets, no second list
# to keep in sync. This is already a complete, sendable program — the GUI below
# only edits the same object.

# %%
p = GraphPatch()
b = {name: p.add(sdef) for name, sdef in defs.items()}   # box index per def
p.connect(b["osc"], "out", b["filt"], "in")
p.connect(b["filt"], "out", b["trem"], "in")
p.connect(b["trem"], "out", b["dac"], "in")

# The cord->bus pass names one bus per net; print what the server will get.
print("compiled:", p.compile())

# Where a dragged box last landed (index -> (x, y)), for the printout only: the
# layout is fully automatic, so nothing here is placed by hand.
placed: dict[int, tuple[float, float]] = {}


# %% [markdown]
# ## The GUI: a view of the patch
# `patch` draws the boxes and cords; `to_widget` renders the model into it. The
# patch sits in a `scroll` workspace (**Shift+drag** pans, wheel zooms; a plain
# drag marquee-selects). A thin transport strip rides below. Nothing sounds until
# **render** compiles the patch and instances it. Every widget is *named*.

# %%
transport = panel(None, button(name="render", label="render"),
                  button(name="stop", label="stop"), layout="row", h=48)

gui = session.gui()
win = gui.open(window(
    scroll(None, patch(name="patch", **p.to_widget(), label="patch"), name="workspace"),
    transport, title="Patch — level 1", w=720, h=680, layout="col"))
session.start()

instance = None


def render() -> None:
    """Compile the patch you drew and (re)instance it — freeing the one in flight,
    so a re-render replaces rather than stacks. A bad cord is reported, not fatal."""
    global instance
    try:
        gdef = p.to_graphdef("patch")
    except ValueError as exc:              # a malformed cord from the pass
        print(f"  cannot compile: {exc}")
        return
    server.add_graphdef(gdef)
    if instance is not None:
        instance.free()
    instance = server.graph("patch")
    print("  rendered — the patch is sounding")


def stop() -> None:
    """Free the instance in flight, if any."""
    global instance
    if instance is not None:
        instance.free()
        instance = None
        print("  stopped")


print(f"opened window {win}")
print("drag a box to move it; drag empty canvas to marquee-select; click empty to clear.")
print("Shift+drag pans, the wheel zooms; drag an outlet pin onto an inlet to cord them.")
print("press render to hear the chain, stop to free it.")


# %% [markdown]
# ## Wire the widgets to the patch, by name
# A ``"wire"`` event on the canvas is `GraphPatch.connect` by name; a ``"move"``
# persists a box's canvas position (presentation only); a ``"view"`` from the
# scroll workspace reports the pan/zoom. **render** compiles the model and
# instances it, **stop** frees it. The GUI never owns the patch — it edits the
# object you built.

# %%
_closed = False


def on_patch(tag, *rest):
    """The canvas edits onto the patch model."""
    if tag == "wire" and len(rest) >= 4:
        src, outlet, dst, inlet = int(rest[0]), rest[1], int(rest[2]), rest[3]
        p.connect(src, outlet, dst, inlet)
        print(f"  wired {src}.{outlet} -> {dst}.{inlet} — press render to hear it")
    elif tag == "move" and len(rest) >= 3:
        index, x, y = int(rest[0]), float(rest[1]), float(rest[2])
        placed[index] = (x, y)
        print(f"  moved box {index} to ({x:.0f}, {y:.0f})")


def on_view(tag, *rest):
    if tag == "view" and len(rest) >= 3:
        print(f"  view x={rest[0]:.0f} y={rest[1]:.0f} zoom={rest[2]:.2f}")


win["patch"].on_event(on_patch)
win["workspace"].on_event(on_view)
win["render"].on_event(lambda value: render() if value == 1 else None)
win["stop"].on_event(lambda value: stop() if value == 1 else None)
win.on_closed(lambda: globals().__setitem__("_closed", True))


# %% [markdown]
# ## The loop: apply edits, re-audition on render
# Cell-run: `gui.pump()` between cells while you drag and cord. Script-run: the
# loop pumps the events onto the model until the window closes; **render**
# re-flattens and instances whatever you have drawn by then.

# %%
if __name__ == "__main__":
    try:
        while not _closed:
            gui.pump(timeout=0.05)
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
    finally:
        session.close()
