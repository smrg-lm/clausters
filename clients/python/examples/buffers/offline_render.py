#!/usr/bin/env python3
"""Self-contained offline render with the *installed* package.

Unlike the examples under the repo-root ``examples/`` (which insert
``clients/python`` onto ``sys.path``), these ship with the wheel and import
``clausters`` straight from the installed package -- the bundled embed cdylib is
found automatically, so this runs from anywhere in a venv with no server, no
audio device and no ``target/`` directory in sight::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python            # or: pip install clausters-*.whl
    python clients/python/examples/buffers/offline_render.py out.wav

It renders a short arpeggio through the embedded NRT renderer and writes a WAV.
Because the synthesis (native ``Sine``) and the offline render both run inside
the bundled libraries, the result is bit-identical to the live server's.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: step through it with Shift+Enter, change the
pattern in one cell and re-render in the next.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base import Routine
from clausters.seq import Pbind, Pseq, Pwhite

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0

# %% [markdown]
# ## The phrase
# A one-bar arpeggio. ``degree`` walks a major scale (Event maps it to a midinote
# then a frequency in the shared native core); ``amp`` jitters.

# %%
phrase = Pbind(
    degree=Pseq([0, 2, 4, 7, 4, 2], repeats=2),
    dur=0.25,
    amp=Pwhite(0.1, 0.2),
)

# %% [markdown]
# ## The offline session
# Same `Pbind` API as live, but the clock drives a score the bundled embed
# renderer turns into samples -- no server, no audio device. The session is its
# own random context, so its seed reproduces every random draw (the `Pwhite`
# here) end to end, independently of any other session.

# %%
session = Session.nrt(tempo=2.0).activate()
session.seed(1)
session.play(phrase)

# A render ends at the score's **last event**, and the last one the phrase
# writes is the gate closing on its final note — so without a later event the
# file stops there and the built-in instrument's 0.3 s release is cut off,
# which is a click at the end of the take. `/node_free 0` after the tail is
# that closing event.
PHRASE_BEATS = 6 * 2 * 0.25     # six degrees, twice, a quarter beat each
TAIL = 1.0                      # beats: the release (0.3 s at 2 beats/s) and room


def close():
    yield PHRASE_BEATS + TAIL
    session.server.send_bundle(("/node_free", 0))


Routine(close).play()

# %% [markdown]
# ## Render
# The server's own seed is a separate one: it starts the render's stochastic
# UGens, and with none given the render draws a fresh one -- a score with noise
# in it is a new take every run. `stats.seed` is how you get a take back: pass it
# as ``seed=`` and the render repeats sample for sample.

# %%
def run(path: str = str(OUT / "offline_render.wav")):
    """Render the score to ``path`` and report what came out."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")
    print(f"seed {stats.seed} - pass seed={stats.seed} to render this take again")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "offline_render.wav")))
else:
    print("score ready - run('out.wav') to render it")
