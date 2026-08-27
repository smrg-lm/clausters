#!/usr/bin/env python3
"""Generate bundle-vectors.json from the Python bundle writer and the core pass.

The Python client is the reference authoring client, and the web client now
writes bundles too, so this vector holds both halves of the agreement:

- **the files** the Python writer emits, byte for byte, which the TypeScript
  writer must emit from the same authoring calls. The format is canonical JSON
  precisely so that this comparison can be made on bytes rather than on shape;
- **the resolution** of what was written, for a given allocation, which the
  browser's wasm door must reproduce exactly.

Both sides call one pass (`clausters_core::bundle`), so a mismatch in the
second means a binding drifted, which is the only thing that can drift there;
a mismatch in the first means the two writers have grown apart, which is what
the standing non-divergence rule forbids.

The JSON is committed; regenerate with:

    python3 gen-bundle-vectors.py

(from clients/web/tests/, with the Python client importable — the repo's .venv
has it installed editable — and libclausters_ffi built.)
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "python"))

from clausters import _native  # noqa: E402
from clausters.bundle import Bundle  # noqa: E402
from clausters.defs import (  # noqa: E402
    DoneAction, Env, SynthDef, control, env_gen, out, out_ctl, sine,
)
from clausters.gui import knob, meter, view  # noqa: E402


def voice() -> SynthDef:
    """The bus reaches the def as a **control**, never baked in — the rule that
    lets two instances share the one def that was sent."""
    freq = control("freq", 220.0)
    env_bus = control("env_bus", 0.0)
    env = env_gen(Env.perc(), done_action=DoneAction.FREE_SELF)
    return SynthDef("voice", out(0.0, sine(freq) * env), out_ctl(env_bus, env))


def reference() -> Bundle:
    """The reference bundle: both kinds of hole, in props and in the boot list,
    plus a preset and a nested tree."""
    b = Bundle("fm-voice")
    freq = b.param("freq", float, default=220.0, min=60.0, max=700.0)
    title = b.param("title", str, default="FM voice")
    lfo = b.bus("lfo")
    node = b.node("voice")
    b.synthdef(voice())
    b.gui(view(
        knob(label="freq", value=freq, min=60.0, max=700.0,
             bind=["/node_set", node, "freq"], id=2),
        meter(lfo, rate="control", label="env", id=3),
        title=title, layout="col", w=320, h=200,
    ))
    b.boot(["/synth_new", "fm-voice.voice", node, 0, 0, "freq", freq, "env_bus", lfo])
    b.preset("bright", freq=660.0, title="bright voice")
    return b


#: One mount each: the defaults, an attribute override, and a preset with an
#: attribute over it — the whole resolution order, frozen.
MOUNTS = [
    ("defaults", {}, {}),
    ("attribute", {"freq": "440"}, {}),
    ("preset_under_attribute", {"freq": "330"}, {"freq": 660.0, "title": "bright voice"}),
]


def main():
    bundle = reference()
    bundle.validate()
    manifest = bundle.manifest()
    template = bundle.record()

    requirements = _native.bundle_requirements(manifest, template)
    cases = []
    for i, (name, attributes, preset) in enumerate(MOUNTS):
        # A distinct allocation per case, as a page would hand out: the point
        # is that two instances share nothing.
        allocation = {
            "widget_base": 1000 + i * requirements["widgets"],
            "nodes": {n: 2000 + i for n in requirements["nodes"]},
            "buses": {b["name"]: 300 + i for b in requirements["buses"]},
            "buffers": {b: 40 + i for b in requirements["buffers"]},
        }
        cases.append({
            "name": name,
            "attributes": attributes,
            "preset": preset,
            "allocation": allocation,
            "resolved": _native.bundle_resolve(
                manifest, template, allocation, attributes, preset
            ),
        })

    out = {
        "manifest": manifest,
        "template": template,
        # Every file the writer emits, by path relative to the bundle
        # directory -- the TypeScript writer's own output is compared with
        # this text, not with a re-parsed shape.
        "files": bundle.files(),
        "requirements": requirements,
        "cases": cases,
    }
    path = pathlib.Path(__file__).with_name("bundle-vectors.json")
    path.write_text(json.dumps(out, indent=1) + "\n")
    print(f"wrote {path} ({len(cases)} mount(s))")


if __name__ == "__main__":
    main()
