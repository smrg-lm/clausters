#!/usr/bin/env python3
"""Maths on a UGen graph: the full operator set, not just ``+ - * /``.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/graph_maths.py out.wav

Every operator and math method beyond `+ - * /` — ``%``, ``min``/``max``, the
comparisons, ``.midicps()``, ``.distort()``, ``.clip2()`` … — composes the
server's generic ``BinaryOpUGen``/``UnaryOpUGen``, computed by the same
``clausters-core`` code the client uses off the RT path (so a value you compute
ahead of time and the UGen on the audio thread agree bit-for-bit). The point of
interest is the `SynthDef`: it does real per-sample maths, no Faust needed.

The **range maps** are part of that surface and are shown here too:
``.linexp()`` and its five siblings map a signal off one range onto another
through the very function the script's own ``linexp`` computes with. The
vibrato below is the case worth reading — a vibrato is a *ratio*, not an
offset — and its bounds are themselves signals, which a map allows.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change an operator in the def cell and re-render
in the next one.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import SynthDef, control, out, sine
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
# A lead whose pitch, timbre and tremolo are all built with graph maths.

# %%
def maths_lead(name: str = "maths_lead") -> SynthDef:
    note = control("note", 60.0)
    amp = control("amp", 0.3)

    freq = note.midicps()                       # UnaryOpUGen: MIDI note -> Hz
    # A 1% vibrato as a *ratio*: the LFO is mapped exponentially onto
    # freq*0.99..freq*1.01, so the bend is the same interval at every pitch.
    # Both bounds are signals here -- a map's ranges are ordinary inputs.
    tone = sine(sine(5.0).linexp(-1.0, 1.0, freq * 0.99, freq * 1.01))

    shaped = tone.distort()                      # UnaryOpUGen: soft saturation
    # A unipolar LFO clipped to 0.8 -> a gentle tremolo (>=, clip2 both compose).
    trem = (sine(3.0) * 0.5 + 0.5).clip2(0.8)

    sig = shaped * trem * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The score
# One sustained voice, its note set from a routine -- the def turns each MIDI
# number into Hz with `.midicps()`, so the score never computes a frequency.

# %%
session = Session.nrt(tempo=2.0)
maths_lead().send(session.server)
lead = Synth("maths_lead", {"note": 48.0, "amp": 0.3}, server=session.server)


def sequence():
    for midi in [48, 52, 55, 59, 60, 59, 55, 52]:
        session.server.send_bundle(("/node_set", lead.id, "note", float(midi)))
        yield 0.5
    yield 1.0
    session.server.send_bundle(("/node_free", lead.id))


Routine(sequence).play(session.clock)


# %%
def run(path: str = str(OUT / "graph_maths.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "graph_maths.wav")))
else:
    print("score ready - run('out.wav') to render it")
