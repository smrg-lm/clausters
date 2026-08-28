#!/usr/bin/env python3
"""The table family: `osc` / `vosc` over `/buffer_gen` wavetables, and `shaper`.

Runs from the *installed* package, offline, like ``buffers/offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/wavetables.py out.wav

The point of interest is the buffer-backed oscillators:

- Two **wavetable-format** buffers are filled with ``/buffer_gen sine1`` (flag 7 =
  normalize | wavetable | clear): buffer 0 a pure sine, buffer 1 a bright
  sawtooth-like harmonic stack. The wavetable flag stores scsynth's
  offset/slope pairs, which is what the interpolating readers expect.
- ``vosc(pos, freq)`` reads the table at ``pos`` and crossfades toward the
  next one by its fractional part, so a lagged ``pos`` control gliding 0 -> 1
  **morphs** sine into saw while the note holds. (``osc(0, freq)`` is the same
  reader pinned to one table; ``oscn`` is the cheap non-interpolating one for
  *plain* buffers.)
- Buffer 2 holds a ``cheby`` transfer curve, and ``shaper(2, sine * drive)``
  waveshapes a pure sine through it: sweeping ``drive`` fades harmonics in —
  distortion with an exact, band-limited recipe instead of a clipped edge.

The morph plays first (two bars), the waveshaped note answers (one bar).

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change the def in one cell and re-render in the
next.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import Buffer
from clausters.defs import (
    DoneAction,
    Env,
    Synth,
    SynthDef,
    control,
    env_gen,
    out,
    shaper,
    sine,
    vosc,
)

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0

#: /buffer_gen flags: normalize (1) | wavetable (2) | clear (4).
WT = 7


# %% [markdown]
# ## Morphing between tables

# %%
def morph(name: str = "wt_morph") -> SynthDef:
    """`vosc` between adjacent tables; ``pos`` glides thanks to its lag."""
    pos = control("pos", 0.0, lag=2.0)               # the morph itself
    freq = control("freq", 110.0)
    gate = control("gate", 1.0)
    amp = control("amp", 0.1)
    env = env_gen(Env.asr(attack=0.05, release=0.5), gate=gate,
                  done_action=DoneAction.FREE_SELF)
    sig = vosc(pos, freq) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## Waveshaping through a table

# %%
def shaped(name: str = "wt_shaped") -> SynthDef:
    """A pure sine pushed through the cheby transfer table; ``drive`` (also
    lagged) is how far into the curve the input reaches."""
    drive = control("drive", 0.1, lag=1.5)
    freq = control("freq", 165.0)
    gate = control("gate", 1.0)
    amp = control("amp", 0.1)
    env = env_gen(Env.asr(attack=0.05, release=0.5), gate=gate,
                  done_action=DoneAction.FREE_SELF)
    sig = shaper(2.0, sine(freq) * drive) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The score

# %%
session = Session.nrt(tempo=2.0).activate()
server = session.server

# Three buffers, scored at time 0. 2048 frames hold a 1024-point table.
# vosc reads pos and pos+1, so the two wavetables must be adjacent and
# equally sized; the allocator hands out 0, 1, 2 in order.
sine_buf = Buffer.alloc(2048, 1, server=server)
sine_buf.gen("sine1", WT, 1.0)
saw_buf = Buffer.alloc(2048, 1, server=server)
saw_buf.gen("sine1", WT, *(1.0 / k for k in range(1, 9)))
cheby_buf = Buffer.alloc(2048, 1, server=server)
cheby_buf.gen("cheby", WT, 1.0, 0.0, 0.6, 0.0, 0.3)

morph().send(server)
shaped().send(server)


def sequence():
    # Timetagged bundles, as in every NRT routine: the beat stamps them.
    voice = Synth("wt_morph", {"freq": 110.0, "pos": 0.0}, server=server)
    yield 1.0
    server.send_bundle(("/node_set", voice.id, "pos", 1.0))  # glide to saw
    yield 3.0
    server.send_bundle(("/node_set", voice.id, "gate", 0.0))
    yield 0.5
    answer = Synth("wt_shaped", {"freq": 165.0, "drive": 0.1}, server=server)
    server.send_bundle(("/node_set", answer.id, "drive", 0.9))  # harmonics in
    yield 2.0
    server.send_bundle(("/node_set", answer.id, "gate", 0.0))
    # The score's closing event, a release after the last gate: a render ends
    # at its last event, so without this one the file would stop where that
    # gate closed and the release would be cut — a click at the end.
    yield 1.5
    server.send_bundle(("/node_free", 0))

Routine(sequence).play()


# %%
def run(path: str = str(OUT / "wavetables.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)

    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")

    print(f"wrote {path} - listen with: pw-play {path}")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "wavetables.wav")))
else:
    print("score ready - run('out.wav') to render it")
