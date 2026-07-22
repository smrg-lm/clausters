#!/usr/bin/env python3
"""Build a directed patch — in code and on screen — and hear it.

A `clausters.defs.GraphPatch` is the **programmatic** level-1 patcher: boxes
(whole defs) with typed **inlets** (top) and **outlets** (bottom), and a **cord**
per ``outlet -> inlet``. A cord *is* a bus, but you never number one: `compile`
runs the shared cord->bus pass (`clausters_core::patch`, in Rust) that names one
bus per connected net — its writers **sum** — and `to_graphdef` hands back a
`GraphDef` ready to send. That is the whole model, and it needs no GUI:

    p = GraphPatch()
    tone = p.add("tone", outlets=["out"])
    dac = p.add("dac", inlets=["in"], outlets=["out"])
    out = p.sink()
    p.connect(tone, "out", dac, "in"); p.connect(dac, "out", out, "in")
    server.add_graphdef(p.to_graphdef("patch")); server.graph("patch")   # sounds

The GUI is a **view** of that same model. The `graph` widget draws the boxes and
cords; dragging an outlet onto an inlet (either grab order, a rate mismatch
refused at the gesture) flows back as ``/gui_event <id> "wire" <src> <outlet>
<dst> <inlet>``, which is just `GraphPatch.connect` by name — so the picture and
the object stay one thing. Press **render** to compile the patch you drew and
hear it; the direction reads top-to-bottom, and the buses are never on screen.

An unwired outlet keeps its def's default, so these defs default their bus
controls to ``SILENT`` (a spare bus nobody reads): a box is silent until a cord
reaches it, and the only path to the speakers is a cord into the ``OUT`` box.

Run it as a script (``python gui_patcher.py``) or cell by cell (``# %%``). Needs
a live audio server (it starts its own) and a display with a GPU adapter; the
install bundles both binaries. **The ABI bumped to 11** for the cord->bus pass,
so refresh the bundled native copy first: ``scripts/refresh-bin.sh``.
"""

# %%
import sys

from clausters import Session
from clausters.defs import GraphPatch, SynthDef, control, in_, out, sine
from clausters.gui import button, graph, panel, scroll, window

TEMPO = 2.0
SILENT = 64  # a spare audio bus (0..127) nothing reads: the "unconnected" default.


# %% [markdown]
# ## The member defs (the boxes' building blocks)
# Three SynthDefs. A control that feeds an ``Out`` is an **outlet**, one that
# feeds an ``In`` an **inlet** — that is where the box's ports come from, and it
# is structural (not a guess). `tone` is a source, `trem` a tremolo, `dac` the
# output stage a cord to ``OUT`` sends to the speakers.

# %%
def tone(name: str = "tone") -> SynthDef:
    """A sine source: writes ``sine(freq) * amp`` to its ``out`` bus."""
    return SynthDef(name, out(control("out", SILENT),
                              sine(control("freq", 220.0)) * control("amp", 0.2)))


def trem(name: str = "trem") -> SynthDef:
    """A tremolo: reads ``in``, rings it with a slow LFO, writes ``out``."""
    lfo = sine(control("rate", 4.0)) * 0.5 + 0.5
    return SynthDef(name, out(control("out", SILENT),
                              in_(control("in", SILENT)) * lfo))


def dac(name: str = "dac") -> SynthDef:
    """The output stage: reads ``in``, scales it, writes ``out`` — a cord to the
    ``OUT`` box reaches the speakers (hardware bus 0, so this sounds on the left)."""
    return SynthDef(name, out(control("out", SILENT),
                              in_(control("in", SILENT)) * control("amp", 0.4)))


session = Session.live(tempo=TEMPO, latency=0.1)
server = session.server
for sdef in (tone(), trem(), dac()):
    server.add_synthdef(sdef)


# %% [markdown]
# ## The patch, built in code
# The seed: `tone` -> `dac` -> the hardware. Each box declares its typed ports;
# a cord connects an outlet to an inlet. This is already a complete, sendable
# program — the GUI below only edits the same object.

# %%
patch = GraphPatch()
b_tone = patch.add("tone", outlets=["out"])
b_dac = patch.add("dac", inlets=["in"], outlets=["out"])
b_out = patch.sink()                        # the hardware output box
patch.connect(b_tone, "out", b_dac, "in")   # tone -> dac
patch.connect(b_dac, "out", b_out, "in")    # dac -> speakers

# The cord->bus pass names one bus per net; print what the server will get.
print("compiled:", patch.compile())

# A place for each box, so the patch reads top-to-bottom (the GUI persists moves
# from here). Box index -> (x, y) in canvas units.
geometry = {b_tone: (200.0, 40.0), b_dac: (200.0, 230.0), b_out: (200.0, 420.0)}


# %% [markdown]
# ## The GUI: a view of the patch
# `graph` draws the boxes and cords; `to_widget` renders the model into it. The
# patch sits in a `scroll` workspace — **Shift+drag to pan, wheel to zoom** (a
# plain drag marquee-selects) — so a patch bigger than the window stays
# reachable. A thin transport
# strip (a fixed ``h``, so it does not eat the canvas) rides below. Nothing sounds
# until **render** compiles the patch and instances it.

# %%
PATCH, WORKSPACE, RENDER, STOP = 7, 6, 1, 2
CONTENT = (900.0, 760.0)
transport = panel(5, button(RENDER, label="render"), button(STOP, label="stop"),
                  layout="row", h=48)

gui = session.gui()
win = gui.open(window(
    scroll(WORKSPACE,
           graph(PATCH, **patch.to_widget(geometry), label="patch",
                 x=0.0, y=0.0, w=CONTENT[0], h=CONTENT[1]),
           content_w=CONTENT[0], content_h=CONTENT[1]),
    transport, title="Patcher", w=560, h=620, layout="col"))
session.start()

instance = None


def render() -> None:
    """Compile the patch you drew and (re)instance it — freeing the one in flight,
    so a re-render replaces rather than stacks. A bad cord is reported, not fatal."""
    global instance
    try:
        gdef = patch.to_graphdef("patch")
    except ValueError as exc:              # a malformed cord from the pass
        print(f"  cannot compile: {exc}")
        return
    server.add_graphdef(gdef)
    if instance is not None:
        instance.free()
    instance = server.graph("patch")
    print("  rendered — the patch is sounding")


print(f"opened window {win}")
print("drag empty canvas to marquee-select; Shift+drag to pan, wheel to zoom; drag a box to move it.")
print("drag an outlet pin onto an inlet to cord them; press render to hear.")


# %% [markdown]
# ## The loop: apply edits, re-audition on render
# A ``"wire"`` event is `GraphPatch.connect` by name; a ``"move"`` persists a
# box's canvas position (presentation only). **render** compiles the model and
# instances it. The GUI never owns the patch — it edits the object you built.

# %%
def step(addr, args) -> None:
    """One host event onto the patch model. A ``"wire"`` is `connect` by name; a
    ``"move"`` persists a box's position; the transport buttons render/stop."""
    if addr != "/gui_event" or len(args) < 2:
        return
    tag = args[1]
    if tag == 1 and args[0] == RENDER:
        render()
    elif tag == 1 and args[0] == STOP:
        global instance
        if instance is not None:
            instance.free()
            instance = None
            print("  stopped")
    elif tag == "wire":
        src, outlet, dst, inlet = int(args[2]), args[3], int(args[4]), args[5]
        patch.connect(src, outlet, dst, inlet)
        print(f"  wired {src}.{outlet} -> {dst}.{inlet} — press render to hear it")
    elif tag == "move":
        geometry[int(args[2])] = (float(args[3]), float(args[4]))


if __name__ == "__main__":
    try:
        running = True
        while running:
            msg = gui.poll(0.05)
            if msg is None:
                continue
            addr, args = msg
            if addr == "/gui_closed":
                running = False
            else:
                step(addr, args)
    except (OSError, RuntimeError, ConnectionError) as e:
        sys.exit(str(e))
    finally:
        session.close()
