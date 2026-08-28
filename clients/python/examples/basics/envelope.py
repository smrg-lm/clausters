#!/usr/bin/env python3
"""Amplitude envelopes with ``EnvGen`` -- a gated ADSR that frees itself.

Runs from the *installed* package, offline, like ``buffers/offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/envelope.py out.wav

The point of interest is the SynthDef: a sine shaped by an ``EnvGen`` playing an
`Env.adsr`. The envelope's `gate` is a control, and its `done_action` is
``FREE_SELF``, so the synth stays alive exactly as long as its sound and then
frees itself -- no ``/node_free`` bookkeeping from the client.

``Pbind(..., has_gate=True)`` is what closes the gate: for each note the player
sends ``gate 0`` after the note's ``sustain`` instead of freeing the node
outright, which starts the release segment; the synth disappears when that
release finishes. The built-in ``default`` instrument carries a gated envelope
of exactly this shape (a fixed ASR) and is gate-released for you; this example
shows how to build your **own** envelope -- a full ADSR with a chosen attack,
decay, sustain and release.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change an envelope segment in the def cell and
re-render in the next one.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    control,
    env_gen,
    out,
    sine,
)
from clausters.seq import Pbind, Pseq

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0

# %% [markdown]
# ## The def
# A sine whose amplitude follows an ADSR. ``gate`` sustains at half level while
# held; on release it fades out and frees the synth.

# %%
def adsr_pad(name: str = "adsr_pad") -> SynthDef:
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env = env_gen(
        Env.adsr(attack=0.02, decay=0.15, sustain=0.5, release=0.5),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sine(freq) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The phrase
# `has_gate` makes each note release its envelope (``gate 0``) after ``sustain``
# rather than being freed outright; ``sustain`` < ``dur`` leaves a gap so the
# release tail is audible before the next note.

# %%
phrase = Pbind(
    instrument="adsr_pad",
    has_gate=True,
    degree=Pseq([0, 2, 4, 7], repeats=2),
    dur=0.5,
    sustain=0.35,
    amp=0.2,
)

# %%
session = Session.nrt(tempo=2.0)
adsr_pad().send(session.server)  # /def_send synth at time 0 in the score
session.play(phrase)


# %%
def run(path: str = str(OUT / "envelope.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "envelope.wav")))
else:
    print("score ready - run('out.wav') to render it")
