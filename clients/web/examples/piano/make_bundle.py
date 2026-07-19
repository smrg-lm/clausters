#!/usr/bin/env python3
"""Author, with the Python client, a bundle whose GuiDef is a playable piano —
then mount it as a web component.

The point of this example is the ``piano`` widget's **host-voice mode with no
script at run time**: the Python client only *authors* the files (it talks to
nothing); what runs afterwards is the persisted bundle, and the *host* manages
one server voice per held key:

    key press   -> /s_new piano_voice <id> 0 0 freq <hz> amp <vel/127> gate 1
    key release -> /n_set <id> gate 0        (the envelope releases and the
                                              def frees the node itself)

so the keyboard plays the wasm engine in the tab with zero page JS — the same
posture as ``examples/graph-controls``, whose knobs bind ``/n_set``s. The other
mapping path (the widget unbound, the script programming voices from the
``"note"`` events) is the Python example ``clients/python/examples/gui_piano.py``.

The keyboard itself: real piano proportions (it resizes with the element), the
overview strip above the keys pans/zooms the visible MIDI range, and the keys
outside the 88-key piano range draw grayed (``active_min``/``active_max``). The
voice also publishes its envelope on control bus 0, watched by a meter under
the keys (and by the smoke assert).

Generate the bundle (from this directory; the client importable as usual —
``pip install ./clients/python`` or ``PYTHONPATH=../../../python``)::

    python3 make_bundle.py

Then the **same** bundle runs on every leg, no script attached to any of them:

- **Browser, as a web component** (the wasm engine in an AudioWorklet):
  build/stage the package once (``../../build.sh``), then serve **from
  ``clients/web`` — the package root, never this folder** (the page imports
  ``../../dist/...``, which must stay inside the served root)::

      cd clients/web && python3 -m http.server

  and open ``http://localhost:8000/examples/piano/`` — `index.html` here is
  just ``<clausters-bundle src="bundle">``; its power button boots the whole
  instrument in the tab.
- **Desktop, self-contained** (the embedded server; from ``clients/gui``)::

      cargo run --features standalone --bin clausters-gui -- \\
          --standalone piano --data-dir <this dir>/bundle

- **Desktop, loopback** (a running ``clausters`` + ``clausters-gui --server``
  pointing at it, the bundle's dir as ``--data-dir``): the same files again.

The layout it writes (the native persisted formats plus the one browser-only
file, the ``bundle.json`` manifest — HTTP cannot list directories)::

    bundle/defs/synthdefs/piano_voice.json    the voice (the /d_recv payload)
    bundle/defs/guidefs/piano.json            the GuiDef record
    bundle/bundle.json                        the manifest
"""

import json
import os

from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, out_ctl, sine
from clausters.gui import label, meter, piano, window

#: The voice def name — the file stem under ``defs/synthdefs`` and the name the
#: piano's ``voice`` prop spawns per held key.
VOICE_NAME = "piano_voice"
#: The GuiDef (bundle) name; the file stem under ``defs/guidefs``.
GUI_NAME = "piano"


def voice() -> SynthDef:
    """The gated voice a key plays: the conventional ``freq``/``amp``/``gate``
    surface the piano's host-voice mode drives — the note-on opens the gate,
    the note-off closes it, and the release tail frees the synth
    (``FREE_SELF``). The envelope is also published on control bus 0 for the
    meter (and the smoke assert)."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env = env_gen(
        Env.adsr(attack=0.005, decay=0.1, sustain=0.7, release=0.4),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sine(freq) * env * amp
    return SynthDef(VOICE_NAME, out(0.0, sig), out(1.0, sig), out_ctl(0.0, env))


def scene() -> dict:
    """The GuiDef: the piano in host-voice mode (``voice=`` names the def; the
    host sends the ``/s_new``/``gate 0`` pair per held key, so the saved tree
    is self-driving — no ``boot`` list needed), plus a meter watching the
    voice envelope on control bus 0."""
    return window(
        label(1, "click/drag plays; drag the strip to pan, wheel to zoom"),
        piano(10, min=48, max=84, active_min=21, active_max=108,
              voice=VOICE_NAME, label="keys"),
        meter(20, 0, min=0.0, max=1.0, label="env"),
        title="Piano (host voices)", w=820, h=300,
        layout="col", name=GUI_NAME,
    )


def write_bundle(data_dir: str):
    """Writes the persisted files: the def is exactly its wire payload
    (``dump_def``), the GuiDef record is ``{"id": <int>, "gui": <tree>}``, and
    ``bundle.json`` is the manifest the browser boot needs."""
    synthdefs = os.path.join(data_dir, "defs", "synthdefs")
    guidefs = os.path.join(data_dir, "defs", "guidefs")
    for d in (synthdefs, guidefs):
        os.makedirs(d, exist_ok=True)

    sdef = voice()
    with open(os.path.join(synthdefs, f"{sdef.name}.json"), "w") as f:
        f.write(sdef.dump_def())
    with open(os.path.join(guidefs, f"{GUI_NAME}.json"), "w") as f:
        json.dump({"id": 1, "gui": scene()}, f)
    with open(os.path.join(data_dir, "bundle.json"), "w") as f:
        json.dump({
            "gui": GUI_NAME,
            "synthdefs": [VOICE_NAME],
            "graphdefs": [],
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
    print("    http://localhost:8000/examples/piano/\n")
    print("or run the same bundle self-contained on the desktop "
          "(from clients/gui):\n")
    print(f"    cargo run --features standalone --bin clausters-gui -- "
          f"--standalone {GUI_NAME} --data-dir {data_dir}")


if __name__ == "__main__":
    main()
