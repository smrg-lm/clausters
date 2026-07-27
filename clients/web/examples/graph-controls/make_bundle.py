#!/usr/bin/env python3
"""Author, with the Python client, a bundle whose GuiDef controls a GraphDef —
then mount it as a web component, twice on one page if you like.

The point of this example is the **whole chain with no intermediary client at
run time**: the Python client only *authors* the files (it talks to nothing);
what runs afterwards is the persisted bundle — the same one everywhere — with
each control knob wired **straight to the synthesis** through the GraphDef's
named surface:

    knob turn -> /n_set <graph node> "<port>" <value>   (the widget's `bind`)
              -> the graph's surface port fans out to its member controls

The instrument is an FM voice feeding a tremolo through a private graph bus:

    fm.voice --[voice bus]--> fm.trem --> OUT (stereo)
                                     `--> the LFO bus (for the meter/scope)

and the `GraphDef` exposes the musically meaningful **surface ports** (`freq`,
`ratio`, `bright`, `rate`, `depth`, `amp`) rather than the raw member controls
— `bright` shows a scaled port: the 0..1 knob maps to FM index 0..8.

**What makes it a component.** Two things the mount allocates per instance,
declared rather than picked::

    graph = b.node("graph")      # -> "@graph": the node the boot /graph_new
                                 #    creates, and every knob's bind target
    lfo   = b.bus("lfo")         # -> "@lfo":  where the tremolo publishes,
                                 #    reaching the def through a surface port

so two instances instantiate two graphs at two node ids, each watching its own
LFO bus. `freq` and `amp` are declared parameters, so the markup can tune each
instance:

    <fm-trem></fm-trem>
    <fm-trem freq="330" amp="0.15"></fm-trem>
    <fm-trem preset="bright"></fm-trem>

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
          --standalone fm-trem --data-dir <this dir>/bundle

- **Desktop, loopback** (a running ``clausters`` + ``clausters-gui --server``
  pointing at it, the bundle's dir as ``--data-dir``): the same files again.

The layout it writes::

    bundle/defs/synthdefs/fm-trem.voice.json    the members (SynthDef specs,
    bundle/defs/synthdefs/fm-trem.trem.json      the /d_recv payloads)
    bundle/defs/graphdefs/fm-trem.graph.json    the GraphDef (the /d_graph
                                                 payload: buses, members,
                                                 the surface)
    bundle/defs/guidefs/fm-trem.json            the GuiDef record — a template
    bundle/presets/bright.json                  a named parameter bundle
    bundle/bundle.json                          the manifest
    bundle/index.js                             the generated ES module
"""

import os

from clausters.bundle import Bundle
from clausters.defs import GraphDef, SynthDef, control, in_, out, out_ctl, sine
from clausters.gui import knob, label, meter, panel, scope, toggle, window

#: The bundle's name — the tag ``index.js`` registers, and the prefix its def
#: names carry (``fm-trem.voice``, ``fm-trem.graph``).
BUNDLE = "fm-trem"


def fm_voice() -> SynthDef:
    """A two-operator FM voice. ``out`` is the control the graph wires to its
    private bus; ``fm`` is the modulation index (driven scaled, see the
    ``bright`` port)."""
    freq = control("freq", 220.0)
    ratio = control("ratio", 2.0)
    index = control("fm", 3.0)
    modulator = sine(freq * ratio) * freq * index
    voice = sine(freq + modulator) * 0.5
    return SynthDef("voice", out(control("out", 0.0), voice))


def tremolo() -> SynthDef:
    """A tremolo reading the voice bus (the ``in`` control the graph wires) to
    the hardware outputs, and publishing its LFO on ``lfo_bus`` — **a
    control**, so each mounted instance watches the bus it was allocated
    instead of every instance writing the same one."""
    lfo = sine(control("rate", 4.0)) * 0.5 + 0.5  # 0..1
    gain = (1.0 - lfo * control("depth", 0.5)) * control("amp", 0.25)
    sig = in_(control("in", 0.0)) * gain
    return SynthDef("trem", out(0.0, sig), out(1.0, sig),
                    out_ctl(control("lfo_bus", 0.0), lfo))


def graph(voice_name: str, trem_name: str) -> GraphDef:
    """The composition: voice -> bus -> tremolo, and the **surface** — the
    named ports the outside world sets. ``bright`` maps a 0..1 knob to FM
    index 0..8 (``.scaled``); the rest pass through 1:1.

    ``lfo_bus`` is a port like any other: that is how a per-instance bus
    reaches a member's control without being baked into either def.
    """
    g = GraphDef("graph")
    voice_bus = g.bus("voice", rate="audio")
    v = g.add(voice_name, {"out": voice_bus})
    t = g.add(trem_name, {"in": voice_bus})
    g.port("freq", v["freq"], default=220.0)
    g.port("ratio", v["ratio"], default=2.0)
    g.port("bright", v["fm"].scaled(8.0), default=0.4)
    g.port("rate", t["rate"], default=4.0)
    g.port("depth", t["depth"], default=0.5)
    g.port("amp", t["amp"], default=0.25)
    g.port("lfo_bus", t["lfo_bus"], default=0.0)
    return g


def build() -> Bundle:
    """The bundle: two declared symbols (the graph's node, the LFO bus), two
    declared parameters, the three defs, and the GuiDef that drives them.

    Widget ids are **local** — the root is 1, so the children start at 2 — and
    the mount offsets the whole block per instance.
    """
    b = Bundle(BUNDLE)
    freq = b.param("freq", float, default=220.0, min=60.0, max=700.0)
    amp = b.param("amp", float, default=0.25, min=0.0, max=0.5)
    node = b.node("graph")
    lfo = b.bus("lfo")

    voice_name = b.synthdef(fm_voice())
    trem_name = b.synthdef(tremolo())
    graph_name = b.graphdef(graph(voice_name, trem_name))

    def port_knob(id, port, lo, hi, value):
        return knob(id, label=port, min=lo, max=hi, value=value,
                    bind=["/n_set", node, port])

    b.gui(window(
        # The header row: the note, and this instance's own play/stop. A page
        # holding several instruments has them all sounding at once otherwise,
        # and each needs to be silenced on its own — which is what the toggle
        # is for, bound to `/n_run` on *this* instance's graph node. Pausing a
        # group skips its whole subtree on the audio thread, so a stopped
        # instrument costs nothing rather than merely going quiet. `weight`
        # splits the row 3:1.
        panel(
            2,
            label(3, "every knob sets a surface port of the running GraphDef",
                  weight=3),
            toggle(4, label="play", value=True, bind=["/n_run", node], weight=1),
            layout="row", h=30,
        ),
        panel(
            5,
            port_knob(6, "freq", 60.0, 700.0, freq),
            port_knob(7, "ratio", 0.5, 8.0, 2.0),
            port_knob(8, "bright", 0.0, 1.0, 0.4),
            port_knob(9, "rate", 0.2, 12.0, 4.0),
            port_knob(10, "depth", 0.0, 1.0, 0.5),
            port_knob(11, "amp", 0.0, 0.5, amp),
            layout="row",
        ),
        panel(
            12,
            meter(13, lfo, min=0.0, max=1.0, label="lfo"),
            scope(14, lfo, min=0.0, max=1.0, label="lfo"),
            layout="row",
        ),
        title="FM + tremolo (a GraphDef's surface)", w=680, h=400,
        layout="col",
    ))
    # One message brings the instance up: its own node id, its own LFO bus and
    # its tag's parameters, as initial port values.
    #
    # The bus rides *in* the `/graph_new` rather than in an `/n_set` after it,
    # because a def latches its output bus when the synth starts — a later
    # value would arrive after the member had already chosen where to write.
    b.boot([
        "/graph_new", graph_name, node, 0, 0,
        "lfo_bus", lfo, "freq", freq, "amp", amp,
    ])
    b.preset("bright", freq=110.0, amp=0.3)
    return b


def main():
    data_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bundle")
    build().write(data_dir)
    print(f"bundle written to {data_dir}")
    print("\nserve the PACKAGE ROOT (clients/web) — not this folder — and "
          "open the component page:\n")
    print("    cd ../..   # clients/web")
    print("    ./build.sh && python3 -m http.server")
    print("    http://localhost:8000/examples/graph-controls/\n")
    print("or run the same bundle self-contained on the desktop "
          "(from clients/gui):\n")
    print(f"    cargo run --features standalone --bin clausters-gui -- "
          f"--standalone {BUNDLE} --data-dir {data_dir}")


if __name__ == "__main__":
    main()
