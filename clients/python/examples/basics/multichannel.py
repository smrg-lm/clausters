#!/usr/bin/env python3
"""Multichannel without expansion: `dup`, the channel list, and `mix`.

Runs from the *installed* package, offline, like ``buffers/offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/multichannel.py out.wav

The point of interest is the two `dup` semantics and the fold:

- ``dup(lambda: sine(rand(...)), 12)`` **evaluates** the callable 12 times:
  twelve *distinct* sines, each with its own frozen `rand` detune — a thick
  unison bank. (``dup(node, n)`` would repeat one sine **by reference**:
  cheap, but twelve identical channels.)
- Operators **broadcast** over the list (``bank * env`` scales every channel
  by the same envelope, shared by reference) — no loop written.
- ``mix(bank)`` folds the twelve channels through the fused sums
  (`Sum4`/`Sum3`, not an `Add` chain), back to one signal.
- ``out(0, dup(sig))`` is the reference dup at its best: the mixed signal
  fanned to buses 0 and 1 — stereo out, one graph.

And the container is an **expression** like any other, so it needs no def of
its own to be heard or rendered: ``render(chans(a, b), channels=2)`` bounces it
and ``play(sine(440).dup())`` sounds it in stereo on a live server. The channels
land on buses 0, 1, … in order, which is why the render must have at least as
many outputs as the expression writes — asking for fewer raises rather than
dropping half the take.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change the voice count in the def cell and
re-render in the next one.
"""

# %%
import pathlib
import sys

from clausters import Event, Session, render
from clausters.base import Routine
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    chans,
    control,
    dup,
    env_gen,
    mix,
    out,
    rand,
    sine,
)

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0
VOICES = 12

# %% [markdown]
# ## The def
# A detuned unison bank around ``freq``: each voice adds its own ``rand``
# cents-scale offset, drawn once at synth init.

# %%
def unison(name: str = "unison") -> SynthDef:
    freq = control("freq", 110.0)
    gate = control("gate", 1.0)
    amp = control("amp", 0.1)
    env = env_gen(Env.asr(attack=0.4, release=1.2), gate=gate,
                  done_action=DoneAction.FREE_SELF)
    bank = dup(lambda: sine(freq + rand(-3.0, 3.0)), VOICES)
    sig = mix(bank) * (env * amp / VOICES)
    return SynthDef(name, out(0.0, dup(sig)))


# %% [markdown]
# ## The score
# Notes go out as events, not as `server.synth`. A message has no time --
# `server.synth` sends one, so from inside a routine it lands *now* and every
# voice would start at 0 no matter what the yields say. An event rides the
# bundle path: ``/synth_new`` at the routine's exact logical beat and the gate
# release a ``sustain`` later, which is what makes this a sequence rather than a
# chord. `has_gate` releases by closing the gate, which is what this def's
# `env_gen` waits for.

# %%
session = Session.nrt(tempo=2.0).activate()
unison().send()


def sequence():
    for midi, dur in [(45, 3.0), (52, 3.0), (50, 4.0)]:
        freq = 440.0 * 2.0 ** ((midi - 69.0) / 12.0)
        Event(instrument="unison", freq=freq, amp=0.4,
              dur=dur, sustain=dur - 0.5, has_gate=True).play()
        yield dur
    # The score's closing event, a release after the last gate: a render ends
    # at its last event, so without this one the file would stop where that
    # gate closed and the 1.2 s release would be cut — a click at the end.
    yield 3.0
    session.server.send_bundle(("/node_free", 0))


Routine(sequence).play()


# %%
def run(path: str = str(OUT / "multichannel.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %% [markdown]
# ## A channel list needs no def at all
# No def, no session: a channel list is an expression, so the verbs take it
# directly. Two different signals, one per channel.

# %%
pair = render(chans(sine(220.0) * 0.2, sine(330.0) * 0.2),
              dur=0.5, sample_rate=SR, channels=2)
print(f"bare channel list: {pair.channels} channels, "
      f"peak {max(pair.peak, default=0.0):.3f}")

# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "multichannel.wav")))
else:
    print("score ready - run('out.wav') to render it")
