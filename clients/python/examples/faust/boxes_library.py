#!/usr/bin/env python3
"""The box API: Faust library DSP glued together from Python.

Runs from the *installed* package, offline, like ``buffers/offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/faust/boxes_library.py out.wav

``clausters.defs.boxes`` reuses the Faust libraries **without transcribing
them**: ``box.faust(...)`` compiles any Faust expression into a ``Box`` that
composes like a primitive. This example builds an instrument out of three
library pieces — an oscillator (``os.osc``), a lowpass (``fi.lowpass``) and a
stereo reverb (``re.stereo_freeverb``) — wired to sliders and arithmetic
built in Python, then renders a short phrase offline and writes a WAV.

The two application stages at work, kept apart in the syntax:

- arguments to ``box.faust`` are **evaluation-stage**, spliced into the Faust
  source text (the filter order ``3``, the reverb's structural parameters);
- arguments to *calling* the resulting box are **composition-stage**, boxes
  wired to its signal inputs (the sliders, the previous stage).

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change a reverb parameter in the def cell and
re-render in the next one.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import FaustDef
from clausters.defs import boxes as box
from clausters.defs import Synth

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0

# %% [markdown]
# ## The def
# osc -> lowpass -> stereo reverb, all from the Faust libraries.

# %%
def soft_voice(name: str = "soft_voice") -> FaustDef:
    freq = box.hslider("freq", 220.0, 20.0, 2000.0, 0.1)
    cutoff = box.hslider("cutoff", 900.0, 50.0, 8000.0, 1.0)
    amp = box.hslider("amp", 0.2, 0.0, 1.0, 0.001)

    # box.faust("os.osc") is the unapplied oscillator: one input (the
    # frequency), one output. Calling it wires the slider in; the result is
    # an ordinary Box, so `* amp` composes arithmetic around it.
    tone = box.faust("os.osc", ins=1, outs=1)(freq) * amp

    # fi.lowpass(3): the order is structural, so it is an eval-arg (spliced
    # into the source); the cutoff and the signal are its two inputs.
    dry = box.faust("fi.lowpass", 3, ins=2, outs=1)(cutoff, tone)

    # The reverb's feedback/damp/spread bake into the source too. It takes a
    # stereo pair: reusing the `dry` VALUE twice is fine (and deliberate) --
    # a repeated subexpression is computed once, this is not two filters.
    wet = box.faust("re.stereo_freeverb", 0.80, 0.70, 0.55, 23,
                    ins=2, outs=2)(dry, dry)

    # Channel selection needs the arity, which only the Faust compiler knows
    # for fragments -- hence outs=2 above. A gentle dry/wet per side (the
    # reverb has gain; keep the sum comfortably below full scale).
    left, right = wet.outs()
    return FaustDef.from_box(name, box.par(dry * 0.5 + left * 0.15,
                                           dry * 0.5 + right * 0.15))


# %% [markdown]
# ## The score
# The fragment sliders are ordinary controls: ``/node_set`` by label.

# %%
session = Session.nrt(tempo=2.0).activate()
soft_voice().send()   # NRT: scored at time 0
voice = Synth("soft_voice", {"freq": 220.0, "amp": 0.25})


def sequence():
    for step, midi in enumerate([57, 60, 64, 67, 64, 60, 57, 52]):
        hz = 440.0 * 2.0 ** ((midi - 69) / 12.0)
        session.server.send_bundle(("/node_set", voice.id, "freq", hz,
                                    "cutoff", 600.0 + 400.0 * step))
        yield 0.5
    yield 2.0                                # let the reverb tail ring
    session.server.send_bundle(("/node_free", voice.id))


Routine(sequence).play()


# %%
def run(path: str = str(OUT / "boxes_library.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "boxes_library.wav")))
else:
    print("score ready - run('out.wav') to render it")
