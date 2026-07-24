#!/usr/bin/env python3
"""Author a standalone bundle, then launch it from ``clausters-gui`` alone.

A *bundle* is a data directory that holds a named GuiDef beside the
SynthDefs/GraphDefs it needs. ``clausters-gui --standalone <name>`` boots such a
bundle against an **embedded** audio server (loaded in-process), runs the
GuiDef's ``boot`` messages to bring the instrument up, then opens its window — a
self-contained instrument with **no separate audio server and no language client
running**. This is the GUI counterpart of the server's MIDI-standalone preset:
the saved definitions are enough to launch a working program on their own.

This script does the *authoring* half. It writes a bundle to disk and prints the
single command that launches it; it talks to nothing. A bundle is just files, so
the layout is the whole story:

    <data>/defs/synthdefs/gui_standalone_drone.json   the instrument (a SynthDef
                                                       spec, the /d_recv payload)
    <data>/defs/guidefs/drone.json                    the GuiDef record

Two GuiDef features make a saved tree self-driving, so it needs no live script:

- a root ``boot`` list — OSC messages the standalone host sends right after the
  defs load, to instantiate the instrument (here one ``/s_new`` creating node
  1000 from the drone SynthDef);
- a widget ``bind`` prop — the declarative form of ``/gui_bind``, wiring the
  knob's value **straight to the embedded server** (here ``/n_set 1000 freq``),
  so turning it changes the pitch with no round-trip through any script.

The int/float distinction is preserved end to end: node ids are written as
integers (``1000``) and stay integers on the wire; control values are floats.

Run it (the client importable as usual — ``pip install ./clients/python`` or
``PYTHONPATH=clients/python``)::

    python clients/python/examples/gui_standalone.py /tmp/clausters-bundle

It writes the two files and prints, e.g.::

    cargo run --features standalone --bin clausters-gui -- --standalone drone --data-dir /tmp/clausters-bundle

The ``standalone`` feature links the embedded audio server into ``clausters-gui``
(off by default, since it pulls the engine + audio backend). Run that command
(from ``clients/gui``) and a window opens whose knob drives the drone's frequency
on the embedded server — no other process involved. Close the window to stop.
Needs a display and a Vulkan/Metal/DX12/GL adapter.

The same bundle also boots **in a browser tab** — the engine in an
AudioWorklet, the GUI host on a canvas, still no server process. That path
needs one extra file (a ``bundle.json`` manifest, since HTTP cannot list
directories) and the served page; the script prints those steps too. See "A
standalone bundle in a tab" in ``docs/clients.md``.
"""

import json
import os
import sys

from clausters.defs import SynthDef, control, out, sine
from clausters.gui import knob, window

#: The instrument's def name; the GuiDef's ``boot`` /s_new references it, and it
#: is the file stem under ``defs/synthdefs``.
SYNTH_NAME = "gui_standalone_drone"
#: The GuiDef (bundle) name; the file stem under ``defs/guidefs`` and the name
#: passed to ``--standalone``.
GUI_NAME = "drone"
#: The node id the boot message creates and the knob binds to. A fixed,
#: script-allocated id, so the saved GuiDef can name it directly.
DRONE_NODE = 1000


def drone() -> SynthDef:
    """A quiet stereo sine drone whose pitch is the ``freq`` control (default
    160 Hz) — the boot ``/s_new`` instantiates it and the knob's binding drives
    its ``freq``."""
    sig = sine(freq=control("freq", 160.0)) * 0.2
    return SynthDef(SYNTH_NAME, out(0.0, sig), out(1.0, sig))


def scene() -> dict:
    """The GuiDef: one knob over a low range, made self-driving by ``boot`` and
    ``bind`` so the standalone host needs no script.

    - ``boot`` runs once after the defs load: create node ``DRONE_NODE`` from the
      drone SynthDef in the root group.
    - the knob's ``bind`` forwards its value as ``/n_set <DRONE_NODE> freq
      <value>`` straight to the embedded server on every turn.
    - ``name`` lets a *live* ``clausters-gui --data-dir`` auto-persist this same
      tree on ``/gui_def``; here we write the file ourselves, so it is only for
      symmetry with that path.
    """
    return window(
        knob(10, label="freq", min=80.0, max=400.0, value=160.0,
             bind=["/n_set", DRONE_NODE, "freq"]),
        title="Standalone drone", w=420, h=260, layout="col",
        name=GUI_NAME,
        boot=[["/s_new", SYNTH_NAME, DRONE_NODE, 0, 0]],
    )


def write_bundle(data_dir: str):
    """Writes the two bundle files under ``data_dir`` and returns their paths.

    A SynthDef file is exactly the ``/d_recv`` spec JSON (``SynthDef.dump_def``);
    a GuiDef record wraps the tree with the id it is defined under,
    ``{"id": <int>, "gui": <tree>}`` — the standalone host replays it as
    ``/gui_def <id> <tree>``.
    """
    synthdefs = os.path.join(data_dir, "defs", "synthdefs")
    guidefs = os.path.join(data_dir, "defs", "guidefs")
    os.makedirs(synthdefs, exist_ok=True)
    os.makedirs(guidefs, exist_ok=True)

    synth_path = os.path.join(synthdefs, f"{SYNTH_NAME}.json")
    with open(synth_path, "w") as f:
        f.write(drone().dump_def())

    gui_path = os.path.join(guidefs, f"{GUI_NAME}.json")
    with open(gui_path, "w") as f:
        json.dump({"id": 1, "gui": scene()}, f)

    return synth_path, gui_path


def main(argv):
    data_dir = os.path.abspath(argv[1]) if len(argv) > 1 else "/tmp/clausters-bundle"
    synth_path, gui_path = write_bundle(data_dir)
    print(f"wrote {synth_path}")
    print(f"wrote {gui_path}")
    print("\nlaunch the bundle as a self-contained instrument (the GUI host is "
          "its own workspace, so point cargo at its manifest -- run this from the "
          "repo root):\n")
    print(f"    cargo run --manifest-path clients/gui/Cargo.toml "
          f"--features standalone --bin clausters-gui -- "
          f"--standalone {GUI_NAME} --data-dir {data_dir}\n")
    print("a window opens; turning the knob drives the drone's freq on the "
          "embedded server (no other process).")
    print("\nthis Python script only WRITES the bundle once. Re-launching it "
          "needs no interpreter: the line above (or, with [standalone].gui set "
          "in your config, just `clausters-gui --standalone`) runs the app "
          "directly. The embedded server loads the data-dir's defs and "
          "boot.json itself.")
    print("\nor boot the same bundle in a browser tab (from clients/web, after "
          "./build.sh):\n")
    print(f"    python3 tools/bundle-manifest.py {data_dir}")
    print(f"    ln -s {data_dir} my-bundle")
    print("    python3 -m http.server  # then open")
    print("    http://localhost:8000/examples/standalone.html?bundle=/my-bundle\n")
    print("the engine runs in an AudioWorklet, the GUI on a canvas — no server "
          "process anywhere.")


if __name__ == "__main__":
    main(sys.argv)
