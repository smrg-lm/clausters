#!/usr/bin/env python3
"""Author, with the Python client, a bundle whose GuiDef controls a GraphDef —
then mount it as a web component.

The point of this example is the **whole chain with no intermediary client at
run time**: the Python client only *authors* the files (it talks to nothing);
what runs afterwards is the persisted bundle — the same one everywhere — with
each control knob wired **straight to the synthesis** through the GraphDef's
named surface:

    knob turn -> /n_set <graph node> "<port>" <value>   (the widget's `bind`)
              -> the graph's surface port fans out to its member controls

The instrument is an FM voice feeding a tremolo through a private graph bus:

    fm_voice --[voice bus]--> tremolo --> OUT (stereo)
                                     `--> control bus 0 (the LFO, for the meter)

and the `GraphDef` exposes the musically meaningful **surface ports** (`freq`,
`ratio`, `bright`, `rate`, `depth`, `amp`) rather than the raw member controls
— `bright` shows a scaled port: the 0..1 knob maps to FM index 0..8. The GuiDef
binds one widget per port and carries a root `boot` list with the single
`/graph_new` that instantiates the graph, so the saved tree is self-driving.

Generate the bundle (from this directory; the client importable as usual —
``pip install ./clients/python`` or ``PYTHONPATH=../../../python``)::

    python3 make_bundle.py

Then the **same** bundle runs on every leg, no script attached to any of them:

- **Browser, as a web component** (the wasm engine in an AudioWorklet):
  build/stage the package once (``../../build.sh``), then serve **from
  ``clients/web`` — the package root, never this folder** (the page imports
  ``../../dist/...``, which must stay inside the served root; serving
  ``graph-controls/`` itself turns those imports into 404s)::

      cd clients/web && python3 -m http.server

  and open ``http://localhost:8000/examples/graph-controls/`` — `index.html`
  here is just ``<clausters-bundle src="bundle">``; its power button boots
  the whole instrument in the tab.
- **Desktop, self-contained** (the embedded server; from ``clients/gui``)::

      cargo run --features standalone --bin clausters-gui -- \\
          --standalone fm_trem --data-dir <this dir>/bundle

- **Desktop, loopback** (a running ``clausters`` + ``clausters-gui --server``
  pointing at it, the bundle's dir as ``--data-dir``): the same files again.

The layout it writes (the native persisted formats plus the one browser-only
file, the ``bundle.json`` manifest — HTTP cannot list directories)::

    bundle/defs/synthdefs/graph_fm_voice.json    the members (SynthDef specs,
    bundle/defs/synthdefs/graph_fm_trem.json      the /d_recv payloads)
    bundle/defs/graphdefs/fm_graph.json          the GraphDef (the /d_graph
                                                  payload: buses, members,
                                                  the surface)
    bundle/defs/guidefs/fm_trem.json             the GuiDef record
    bundle/bundle.json                           the manifest
"""

import json
import os

from clausters.defs import GraphDef, SynthDef, control, in_, out, out_ctl, sine
from clausters.gui import knob, label, meter, panel, scope, window

#: The member def names — the file stems under ``defs/synthdefs`` and the
#: names the GraphDef's members reference.
VOICE_NAME = "graph_fm_voice"
TREM_NAME = "graph_fm_trem"
#: The GraphDef name: the file stem under ``defs/graphdefs`` and the name the
#: boot ``/graph_new`` instantiates.
GRAPH_NAME = "fm_graph"
#: The GuiDef (bundle) name; the file stem under ``defs/guidefs``.
GUI_NAME = "fm_trem"
#: The node id the boot ``/graph_new`` creates — the graph's group. Every
#: widget binds to it: an ``/n_set`` on a graph group goes through the named
#: surface, not to any member directly.
GRAPH_NODE = 1000


def fm_voice() -> SynthDef:
    """A two-operator FM voice. ``out`` is the control the graph wires to its
    private bus; ``fm`` is the modulation index (driven scaled, see the
    ``bright`` port)."""
    freq = control("freq", 220.0)
    ratio = control("ratio", 2.0)
    index = control("fm", 3.0)
    modulator = sine(freq * ratio) * freq * index
    voice = sine(freq + modulator) * 0.5
    return SynthDef(VOICE_NAME, out(control("out", 0.0), voice))


def tremolo() -> SynthDef:
    """A tremolo reading the voice bus (the ``in`` control the graph wires) to
    the hardware outputs, and publishing its LFO on control bus 0 so the
    GuiDef's meter/scope have something to watch."""
    lfo = sine(control("rate", 4.0)) * 0.5 + 0.5  # 0..1
    gain = (1.0 - lfo * control("depth", 0.5)) * control("amp", 0.25)
    sig = in_(control("in", 0.0)) * gain
    return SynthDef(TREM_NAME, out(0.0, sig), out(1.0, sig), out_ctl(0.0, lfo))


def graph() -> GraphDef:
    """The composition: voice -> bus -> tremolo, and the **surface** — the
    named ports the outside world sets. ``bright`` maps a 0..1 knob to FM
    index 0..8 (``.scaled``); the rest pass through 1:1. This surface is the
    graph's whole control contract: the GuiDef below names only these."""
    g = GraphDef(GRAPH_NAME)
    voice_bus = g.bus("voice", rate="audio")
    v = g.add(VOICE_NAME, {"out": voice_bus})
    t = g.add(TREM_NAME, {"in": voice_bus})
    g.port("freq", v["freq"], default=220.0)
    g.port("ratio", v["ratio"], default=2.0)
    g.port("bright", v["fm"].scaled(8.0), default=0.4)
    g.port("rate", t["rate"], default=4.0)
    g.port("depth", t["depth"], default=0.5)
    g.port("amp", t["amp"], default=0.25)
    return g


def scene() -> dict:
    """The GuiDef: one bound widget per surface port, plus the LFO views.
    ``boot`` instantiates the graph; each ``bind`` forwards the widget's value
    as ``/n_set <GRAPH_NODE> "<port>" <value>`` straight to the server —
    identical on the wasm engine in the page, the embedded desktop server and
    a loopback server."""

    def port_knob(id, port, lo, hi, value):
        return knob(id, label=port, min=lo, max=hi, value=value,
                    bind=["/n_set", GRAPH_NODE, port])

    return window(
        label(1, "every knob sets a surface port of the running GraphDef"),
        panel(
            20,
            port_knob(10, "freq", 60.0, 700.0, 220.0),
            port_knob(11, "ratio", 0.5, 8.0, 2.0),
            port_knob(12, "bright", 0.0, 1.0, 0.4),
            port_knob(13, "rate", 0.2, 12.0, 4.0),
            port_knob(14, "depth", 0.0, 1.0, 0.5),
            port_knob(15, "amp", 0.0, 0.5, 0.25),
            layout="row",
        ),
        panel(
            21,
            meter(16, 0, min=0.0, max=1.0, label="lfo"),
            scope(17, 0, min=0.0, max=1.0, label="lfo"),
            layout="row",
        ),
        title="FM + tremolo (a GraphDef's surface)", w=680, h=400,
        layout="col", name=GUI_NAME,
        boot=[["/graph_new", GRAPH_NAME, GRAPH_NODE, 0, 0]],
    )


def write_bundle(data_dir: str):
    """Writes the persisted files: each def is exactly its wire payload
    (``dump_def``), the GuiDef record is ``{"id": <int>, "gui": <tree>}``, and
    ``bundle.json`` is the manifest the browser boot needs (the same file
    ``tools/bundle-manifest.py`` generates for an existing directory)."""
    synthdefs = os.path.join(data_dir, "defs", "synthdefs")
    graphdefs = os.path.join(data_dir, "defs", "graphdefs")
    guidefs = os.path.join(data_dir, "defs", "guidefs")
    for d in (synthdefs, graphdefs, guidefs):
        os.makedirs(d, exist_ok=True)

    for sdef in (fm_voice(), tremolo()):
        with open(os.path.join(synthdefs, f"{sdef.name}.json"), "w") as f:
            f.write(sdef.dump_def())
    with open(os.path.join(graphdefs, f"{GRAPH_NAME}.json"), "w") as f:
        f.write(graph().dump_def())
    with open(os.path.join(guidefs, f"{GUI_NAME}.json"), "w") as f:
        json.dump({"id": 1, "gui": scene()}, f)
    with open(os.path.join(data_dir, "bundle.json"), "w") as f:
        json.dump({
            "gui": GUI_NAME,
            "synthdefs": [VOICE_NAME, TREM_NAME],
            "graphdefs": [GRAPH_NAME],
        }, f, indent=2)
        f.write("\n")


def main():
    data_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bundle")
    write_bundle(data_dir)
    print(f"bundle written to {data_dir}")
    print("\nserve the PACKAGE ROOT (clients/web) — not this folder — and "
          "open the component page:\n")
    print("    cd ../..   # clients/web")
    print("    ./build.sh && python3 -m http.server")
    print("    http://localhost:8000/examples/graph-controls/\n")
    print("or run the same bundle self-contained on the desktop "
          "(from clients/gui):\n")
    print(f"    cargo run --features standalone --bin clausters-gui -- "
          f"--standalone {GUI_NAME} --data-dir {data_dir}")


if __name__ == "__main__":
    main()
