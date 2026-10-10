#!/usr/bin/env python3
"""Patterns: descriptions you walk, compose with operators, and play together
-- and a voice whose notes go on into one another.

Runs from the *installed* package, offline, like ``basics/sc_vocabulary.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/patterns.py out.wav

**A pattern is a description, walked by a loop.** ``pseq(steps) + 67`` is a
pattern of MIDI notes made from a pattern of steps and an operator, and a plain
``for`` loop over it prints its values -- no clock, no server. The steps come
from ``pyramid``, a list function: it reshapes the whole list before it becomes
a pattern (a list function reads a whole list, an operator one value at a time).

**A voice is articulated by its legato.** The melody is a ``pvoice``: each
note's ``legato`` says how it goes on to the next. Where it is 1 the note is
joined to the next -- the server keeps one synth and sets its ``freq``, which
has a lag, so the line glides -- and where it is less the next note
re-attacks. Rendered into an event sequence, the same joins are a page's ties
and slurs, printed below.

**Patterns play together.** A counter-line from a ``pbind`` -- ``pxrand``
never repeating a note, ``pstutter`` doubling its durations, ``pmeanrand``
keeping its level near the middle -- runs beside the voice under ``ppar``,
merged in time. The session is seeded, so the draws are the same on every run
and in both clients.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Change a legato or a pyramid type and re-render in the next cell.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base.builtins import pyramid
from clausters.defs import DoneAction, Env, SynthDef, control, env_gen, out, sine
from clausters.seq import (INF, pbind, pmeanrand, ppar, pseq, pstutter, pvoice,
                           pxrand)

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0


# %% [markdown]
# ## The def
# A gated tone whose `freq` has a lag: a joined note of a voice glides to its
# pitch instead of jumping.

# %%
def glider(name: str = "glider") -> SynthDef:
    freq = control("freq", 440.0, lag=0.08)
    amp = control("amp", 0.1)
    gate = control("gate", 1.0)
    fade = env_gen(Env.asr(0.01, 1.0, 0.2), gate=gate,
                   done_action=DoneAction.FREE_SELF)
    tone = sine(freq) + sine(freq * 2.0) * 0.25
    sig = tone * fade * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The patterns
# A list function first, then patterns made of it -- and a loop to see them.

# %%
steps = pyramid([0, 2, 4, 7], 1)          # 0  0 2  0 2 4  0 2 4 7
notes = pseq(steps) + 67                  # an operator: one value at a time
print("melody", [int(n) for n in notes])

melody = pvoice("glider",
                freq=notes.midicps(),
                dur=0.5,
                legato=pseq([1.0, 1.0, 0.6], INF),
                amp=0.12,
                has_gate=True,
                artic=True)

counter = pbind(instrument="glider",
                freq=(pxrand([0, 3, 5, 7], 10) + 62).midicps(),
                dur=pstutter(pseq([1.0, 0.5], INF), 2),
                amp=pmeanrand(0.04, 0.08),
                has_gate=True)


# %% [markdown]
# ## The page
# The voice's joins written as ties and slurs: the same data the server played.

# %%
written = melody.render_events().data()
ties = [e["id"] for e in written["events"] if e["data"].get("tie")]
print("ties on notes", ties)
print("slurs", written.get("notation", {}).get("spanners", []))


# %% [markdown]
# ## The score
# Both lines together, rendered offline.

# %%
session = Session.nrt().activate()
session.clock.set_tempo(2.0)
session.seed(70)
glider().send()
ppar([melody, counter]).play()


# %%
def run(path: str = str(OUT / "patterns.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "patterns.wav")))
else:
    print("score ready - run('out.wav') to render it")
