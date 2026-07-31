#!/usr/bin/env python3
"""Author, with the Python client, a bundle whose GuiDef is a playable piano —
then mount it as a web component, twice on one page if you like.

The point of this example is the ``piano`` widget's **host-voice mode with no
script at run time**: the Python client only *authors* the files (it talks to
nothing); what runs afterwards is the persisted bundle, and the *host* manages
one server voice per held key:

    key press   -> /s_new piano.voice <id> 0 0 freq <hz> amp <vel/127> gate 1
    key release -> /n_set <id> gate 0        (the envelope releases and the
                                              def frees the node itself)

so the keyboard plays the wasm engine in the tab with zero page JS — the same
posture as ``examples/graph-controls``, whose knobs bind ``/n_set``s. The other
mapping path (the widget unbound, the script programming voices from the
``"note"`` events) is the Python example ``clients/python/examples/gui_piano.py``.

**What makes it a component.** The voice publishes its envelope on a control
bus, and the bus is *declared* rather than picked::

    env = b.bus("env")                       # -> "@env"
    ...  out_ctl(control("env_bus"), env)    # the def takes the bus it is given

The def payload holds no bus number, so mounting the bundle twice gives each
instance its own bus and each meter reads its own keyboard. Written the old way
— ``out_ctl(0.0, env)``, the number compiled in — both instances would write
bus 0 and the page would show one signal twice. That is the authoring rule the
whole format rests on: *a bus, a node or a buffer reaches a def as a control,
never as a baked constant.*

``title`` is a declared parameter, so the markup can name each instance:

    <piano-keys title="left hand"></piano-keys>
    <piano-keys title="right hand"></piano-keys>

The keyboard itself: real piano proportions (it resizes with the element), the
overview strip above the keys pans/zooms the visible MIDI range, and the keys
outside the 88-key piano range draw grayed (``active_min``/``active_max``).

Generate the bundle (from this directory; the client importable as usual —
``pip install ./clients/python`` or ``PYTHONPATH=../../../python``)::

    python3 make_bundle.py

Then the **same** bundle runs on every leg, no script attached to any of them:

- **Browser, as a web component** (the wasm engine in an AudioWorklet):
  build/stage the package once (``../../build.sh``), then serve **from
  ``clients/web`` — the package root, never this folder** (the page imports
  ``../../dist/...``, which must stay inside the served root)::

      cd clients/web && python3 -m http.server

  and open ``http://localhost:8000/examples/piano/`` — `index.html` here
  imports ``bundle/index.js``, which registers the ``<piano-keys>`` tag; its
  power button boots the whole instrument in the tab.
- **Desktop, self-contained** (the embedded server; from ``clients/gui``)::

      cargo run --features standalone --bin clausters-gui -- \\
          --standalone piano --data-dir <this dir>/bundle

- **Desktop, loopback** (a running ``clausters`` + ``clausters-gui --server``
  pointing at it, the bundle's dir as ``--data-dir``): the same files again.

The layout it writes (the native persisted formats plus the manifest, which
both the browser and the desktop read)::

    bundle/defs/synthdefs/piano.voice.json    the voice (the /d_recv payload)
    bundle/defs/guidefs/piano.json            the GuiDef record — a template
    bundle/bundle.json                        the manifest
    bundle/index.js                           the generated ES module
"""

import os

from clausters.bundle import Bundle
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, out_ctl, sine
from clausters.gui import label, meter, piano, window

#: The bundle's name — the tag ``index.js`` registers, and the prefix its def
#: names carry (``piano.voice``), since a def name is a global namespace on the
#: server.
BUNDLE = "piano"
#: The custom element the generated module registers. HTML wants a hyphen in a
#: custom element name, and "piano" -- a perfectly good GuiDef name on the
#: desktop -- has none.
TAG = "piano-keys"


def voice() -> SynthDef:
    """The gated voice a key plays: the conventional ``freq``/``amp``/``gate``
    surface the piano's host-voice mode drives — the note-on opens the gate,
    the note-off closes it, and the release tail frees the synth
    (``FREE_SELF``).

    The envelope goes out on ``env_bus``, **a control**: the mount passes each
    instance the bus it allocated, so two keyboards on a page do not write over
    each other.
    """
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env_bus = control("env_bus", 0.0)
    env = env_gen(
        Env.adsr(attack=0.005, decay=0.1, sustain=0.7, release=0.4),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sine(freq) * env * amp
    return SynthDef("voice", out(0.0, sig), out(1.0, sig), out_ctl(env_bus, env))


def build() -> Bundle:
    """The bundle: the declared bus and title, the voice, and the GuiDef that
    plays it.

    Widget ids are **local** — the root is 1, so the children start at 2 — and
    the mount offsets the whole block per instance.
    """
    b = Bundle(BUNDLE)
    title = b.param("title", str, default="Piano (host voices)")
    env = b.bus("env")
    voice_name = b.synthdef(voice())

    b.gui(window(
        label("click/drag plays; drag the strip to pan, wheel to zoom", id=2),
        # `voice` names the def the host spawns per held key; `voice_args`
        # rides along with every /s_new, which is how this instance's own bus
        # reaches its voices.
        piano(min=48, max=84, active_min=21, active_max=108,
              voice=voice_name, voice_args=[("env_bus", env)], label="keys",
              id=3),
        meter(env, rate="control", min=0.0, max=1.0, label="env", id=4),
        title=title, w=820, h=300, layout="col",
    ))
    return b


def main():
    data_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bundle")
    build().write(data_dir, tag=TAG)
    print(f"bundle written to {data_dir}")
    print("\nserve the PACKAGE ROOT (clients/web) — not this folder — and "
          "open the component page:\n")
    print("    cd ../..   # clients/web")
    print("    ./build.sh && python3 -m http.server")
    print("    http://localhost:8000/examples/piano/\n")
    print("or run the same bundle self-contained on the desktop "
          "(from clients/gui):\n")
    print(f"    cargo run --features standalone --bin clausters-gui -- "
          f"--standalone {BUNDLE} --data-dir {data_dir}")


if __name__ == "__main__":
    main()
