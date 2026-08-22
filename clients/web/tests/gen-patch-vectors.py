#!/usr/bin/env python3
"""Generate patch-vectors.json from the Python client's patcher models.

The patcher is one model written twice — `clausters/defs/patch.py` and
`clients/web/src/defs/patch.ts` — and what has to agree is what leaves it: the
`{boxes, cords}` a def decodes into, and the widget schema the host is handed.
A Def view is a *reading* of a def, so a difference here is a picture that shows
one client a graph the other does not have.

So this script decodes a handful of defs with the Python surface and freezes
both forms for each; `tests/patch-parity.test.ts` rebuilds the same defs with
the TypeScript surface and asserts the same results.

The JSON is committed; regenerate with:

    python3 gen-patch-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's .venv
has it installed editable).
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters.defs import (  # noqa: E402
    DefPatch, FaustDef, GraphPatch, SynthDef, control, in_, out, sine,
)
from clausters.defs.signals import hslider, sin  # noqa: E402


def tremolo_sine() -> SynthDef:
    """Every cord weight in one graph: audio (heavy), control (thin) and the
    init (dashed) scalar — the def `examples/gui_patch2.py` is written around."""
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    detune = control("detune", 1.5, rate="ir")
    tremolo = sine(control("lfo", 5.0)).at_rate("kr") * 0.5 + 0.5
    carrier = sine(freq * detune)
    sig = carrier * amp * tremolo
    return SynthDef("tremolo_sine", out(0.0, sig), out(1.0, sig))


def shared_input() -> SynthDef:
    """One UGen feeding two inputs of the same box: the decode dedups by
    identity, so it is one box with two cords rather than two boxes."""
    osc = sine(control("freq", 110.0))
    return SynthDef("shared_input", out(0.0, osc * osc))


def fm_tone() -> FaustDef:
    """A Faust signal tree: every op a box, the sliders source boxes."""
    freq = hslider("freq", 220.0, 20.0, 2000.0, 0.1)
    gain = hslider("gain", 0.2, 0.0, 1.0, 0.01)
    modulator = sin(freq * 3.0) * 40.0
    return FaustDef.from_signals("fm_tone", sin(freq + modulator) * gain)


def opaque_faust() -> FaustDef:
    """A source def has no client-side internals: one box, no cords."""
    return FaustDef.from_source("opaque", "process = os.osc(440);")


def tone_and_dac() -> GraphPatch:
    """Level 1 beside level 2, so the shared widget rendering is covered too."""
    tone = SynthDef("tone", out(control("out", 0.0), sine(control("freq", 220.0))))
    dac = SynthDef("dac", out(0.0, in_(control("in", 0.0))))
    patch = GraphPatch()
    a = patch.add(tone)
    b = patch.add(dac)
    patch.connect(a, "out", b, "in")
    return patch


def case(model, *, spec=None) -> dict:
    """One frozen reading: the model's own boxes and cords, and the widget
    schema the host is handed. A SynthDef case also freezes the spec its round
    trip must reproduce."""
    frozen = {
        "boxes": [
            {
                "def": b["def"],
                "kind": b.get("kind"),
                "role": b.get("role", "object"),
                "ports": b["ports"],
            }
            for b in model.boxes
        ],
        "cords": model.cords,
        "widget": model.to_widget(),
    }
    if spec is not None:
        frozen["spec"] = spec
    return frozen


def main():
    synth = tremolo_sine()
    shared = shared_input()
    cases = {
        "tremolo_sine": case(DefPatch.from_synthdef(synth), spec=synth.spec()),
        "shared_input": case(DefPatch.from_synthdef(shared), spec=shared.spec()),
        "fm_tone": case(DefPatch.from_faustdef(fm_tone())),
        "opaque": case(DefPatch.from_faustdef(opaque_faust())),
        "graph_level1": case(tone_and_dac()),
    }
    path = pathlib.Path(__file__).with_name("patch-vectors.json")
    path.write_text(json.dumps({"cases": cases}, indent=1) + "\n")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
