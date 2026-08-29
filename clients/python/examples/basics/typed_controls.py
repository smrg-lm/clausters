#!/usr/bin/env python3
"""Typed controls: a `tr` trigger, a lagged control and an `ir` scalar.

Runs from the *installed* package, offline, like ``buffers/offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/typed_controls.py out.wav

The point of interest is the SynthDef and the control **types**:

- ``freq`` carries a ``lag`` (0.12 s), so when the routine sets a new note the
  pitch **glides** to it instead of jumping — a portamento lead.
- ``gate`` is a trigger (``rate="tr"``): a ``/node_set gate 1`` holds for one block
  and the server resets it, so each set **re-plucks** the percussive envelope.
  A plain ``kr`` gate would stay 1 and never re-trigger.
- ``detune`` is a scalar (``rate="ir"``): the value given at ``/synth_new`` is
  read once and then frozen, so the ``/node_set detune`` the routine sends
  halfway through is **ignored** — the pitch does not move. A plain ``kr``
  control would have taken it.

One persistent synth is driven by a `Routine` that sets ``freq``/``gate`` per
note; the lag and the trigger only make sense on a synth that outlives its
notes, which is exactly what a routine (not one ``/synth_new`` per note) gives.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change a control's rate or lag in the def cell
and re-render in the next one.
"""

# %%
import pathlib
import random
import sys

from clausters import Session
from clausters.base import Routine
from clausters.base.builtins import midicps
from clausters.defs import (
    DoneAction,
    Env,
    Synth,
    SynthDef,
    control,
    env_gen,
    out,
    sine,
)

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0

# %% [markdown]
# ## The def
# A sine lead: ``freq`` glides (lag), ``gate`` re-triggers a pluck (tr),
# ``detune`` is a fixed random offset (ir).

# %%
def glide_lead(name: str = "glide_lead") -> SynthDef:
    freq = control("freq", 220.0, lag=0.12)          # portamento
    gate = control("gate", 0.0, rate="tr")           # one-block re-trigger
    amp = control("amp", 0.2)
    detune = control("detune", 0.0, rate="ir")       # read at init, then frozen
    env = env_gen(Env.perc(attack=0.005, release=0.2), gate=gate,
                  done_action=DoneAction.NONE)
    sig = sine(freq + detune) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## One persistent synth, driven by a routine
# The lag and the trigger only make sense on a synth that outlives its notes,
# which is what a routine gives -- not one ``/synth_new`` per note. In an NRT
# score the control changes must be *timetagged* to spread over time, so they go
# out as `send_bundle` (stamped with the routine's logical beat) rather than the
# immediate `set`/`free`, which would collapse onto time 0 and free the synth
# before it ever sounded.

# %%
session = Session.nrt(tempo=2.0).activate()
glide_lead().send()        # /def_send synth at time 0
lead = Synth("glide_lead", {"amp": 0.2, "freq": midicps(48),
                            "detune": random.uniform(-4.0, 4.0)})


def sequence():
    for i, midi in enumerate([48, 55, 60, 63, 60, 55, 51, 48]):
        session.server.send_bundle(
            ("/node_set", lead.id, "freq", midicps(midi), "gate", 1.0))
        if i == 3:
            # An ir control is frozen at init: this set lands and does nothing.
            session.server.send_bundle(
                ("/node_set", lead.id, "detune", 200.0))
        yield 0.5                                 # beats between notes
    yield 1.0
    session.server.send_bundle(("/node_free", lead.id))


Routine(sequence).play()


# %%
def run(path: str = str(OUT / "typed_controls.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "typed_controls.wav")))
else:
    print("score ready - run('out.wav') to render it")
