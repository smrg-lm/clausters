#!/usr/bin/env python3
"""SuperCollider's vocabulary on a graph and on values: what changed in meaning,
and what is new.

Runs from the *installed* package, offline, like ``basics/graph_maths.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/sc_vocabulary.py out.wav

Two halves, each named with the operators' own words.

**On the signal**, the def. ``.pow(0.35)`` of a sine is a **signed** power,
scsynth's: a bipolar signal stays bipolar and comes out squarer, where C's
``pow`` gave a NaN for every negative half. ``.fold(-0.6, 0.6)`` folds the
driven tone between two bounds -- the general form of ``fold2`` -- and an LFO
moves the drive, so the folds come and go. ``.hanwindow()`` reads a ramp as a
Hann window, shaping a tremolo into soft pulses; and ``.rand2()`` draws a new
value every sample from the node's own seeded stream, a breath of noise that
follows the envelope.

**On the values**, the score. The degrees are a ``scramble`` of a major
triad's and sixth's, turned into keys with ``degkey``; each note's length is an
``exprand`` (equal chances per octave of duration), and its level a
``wchoose``. All of it draws from the session's random context, so
``session.seed`` replays the same phrase on every run and in both clients.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change a bound or a seed and re-render in the
next cell.
"""

# %%
import pathlib
import sys

from clausters import Session, builtins, exprand, scramble, wchoose
from clausters.base import Routine
from clausters.defs import (DoneAction, Env, SynthDef, Synth, control, env_gen,
                            out, sine)
from clausters.defs.ugens import lf_saw

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0
MAJOR = [0, 2, 4, 5, 7, 9, 11]

# %% [markdown]
# ## The def
# A folded, pulsing lead: every shaping step is an operator taken from
# scsynth's table.

# %%
def folded(name: str = "folded") -> SynthDef:
    note = control("note", 69.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)

    tone = sine(note.midicps())
    # A signed power: the negative half is raised too, so the sine squares off
    # instead of turning into NaN.
    squared_off = tone.pow(0.35)
    # Driven by a slow LFO and folded back between -0.6 and 0.6.
    drive = sine(0.25).linlin(-1.0, 1.0, 1.0, 3.0)
    bright = (squared_off * drive).fold(-0.6, 0.6)
    # A rising ramp read as a Hann window: four soft pulses a second.
    pulses = lf_saw(4.0).linlin(-1.0, 1.0, 0.0, 1.0).hanwindow()

    fade = env_gen(Env.asr(0.02, 1.0, 0.3), gate=gate,
                   done_action=DoneAction.FREE_SELF)
    # A breath that follows the envelope: a new draw in -0.02..0.02 of the
    # fade every sample.
    breath = (fade * 0.02).rand2()

    sig = (bright * pulses * amp + breath) * fade
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The score
# The phrase is drawn, not written: seed the session and it is the same draw
# every time.

# %%
session = Session.nrt().activate()
session.clock.set_tempo(2.0)
session.seed(2026)
folded().send()
lead = Synth("folded", {"note": 69.0, "amp": 0.2})

degrees = scramble([0, 2, 4, 5, 7, 9])
notes = [69.0 + builtins.degkey(d, MAJOR) for d in degrees]
lengths = [exprand(0.5, 2.0) for _ in notes]
levels = [wchoose([0.12, 0.2, 0.28], [1.0, 2.0, 1.0]) for _ in notes]
print("notes", notes)


def sequence():
    for note, length, level in zip(notes, lengths, levels):
        session.server.send_bundle(
            ("/node_set", lead.id, "note", note, "amp", level))
        yield length
    # Closed, not freed: the gate lets the envelope fall to zero.
    session.server.send_bundle(("/node_set", lead.id, "gate", 0.0))
    yield 1.0
    # The score's closing event, after the release.
    session.server.send_bundle(("/node_free", 0))


Routine(sequence).play()


# %%
def run(path: str = str(OUT / "sc_vocabulary.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "sc_vocabulary.wav")))
else:
    print("score ready - run('out.wav') to render it")
