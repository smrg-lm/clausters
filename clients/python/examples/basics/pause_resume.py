#!/usr/bin/env python3
"""Pausing and resuming a node with ``/node_run`` -- pause is not terminal.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/basics/pause_resume.py out.wav

The point of interest is ``Server.pause`` / ``Server.resume`` (the ``/node_run``
command). A paused node stays in the tree and keeps its state, but is skipped
during processing -- silent and free of CPU -- and resumes *exactly* where it
left off. This is what makes ``DoneAction.PAUSE_SELF`` non-terminal: a synth
parked by its envelope can be brought back with ``/node_run 1``.

The render is a steady drone that is paused for one beat and then resumed, so
the WAV has an audible gap of silence in the middle with the tone continuing
unchanged on either side. In an NRT score the toggles must be *timetagged*, so
they go out through ``send_bundle`` (stamped with the routine's logical beat)
rather than the immediate ``pause``/``resume``, which would collapse onto time
0. A live RT session would call ``node.pause()`` /
``.resume(node)`` directly instead.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: step through it with Shift+Enter, change the
score in one cell and re-render in the next.
"""

# %%
import sys

from clausters import Session
from clausters.render import read_soundfile
from clausters.base import Routine
from clausters.defs import SynthDef, control, out, sine
from clausters.defs import Synth

SR = 48000.0

# %% [markdown]
# ## The def
# A plain sustained sine -- no envelope, so it runs until paused or freed. Its
# phase is what we watch survive a pause and resume.

# %%
def drone(name: str = "drone") -> SynthDef:
    freq = control("freq", 220.0)
    amp = control("amp", 0.2)
    sig = sine(freq) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% [markdown]
# ## The score
# Tone, pause, tone. In an NRT score the toggles must be timetagged, so they go
# out through `send_bundle` (stamped with the routine's logical beat).

# %%
session = Session.nrt(tempo=2.0)
drone().send(session.server)             # /def_send synth at time 0
node = Synth("drone", {"freq": 220.0, "amp": 0.2}, server=session.server)


def sequence():
    yield 1.0                                    # a beat of tone
    session.server.send_bundle(("/node_run", node.id, 0))   # pause: goes silent
    yield 1.0                                    # a beat of silence
    session.server.send_bundle(("/node_run", node.id, 1))   # resume: tone returns
    yield 1.0                                    # a beat of tone again
    session.server.send_bundle(("/node_free", node.id))


Routine(sequence).play(session.clock)


# %% [markdown]
# ## Render, and check the gap by the numbers
# The middle beat is silent and the outer beats are not. That needs the samples
# per beat rather than the whole-render RMS the stats carry, so the file the
# server wrote is read back for them.

# %%
def run(path: str = "pause_resume.wav"):
    stats = session.render(sample_rate=SR, channels=2, path=path)
    audio = read_soundfile(path)
    third = audio.frames // 3
    rms = lambda a: (sum(s * s for s in a) / max(1, len(a))) ** 0.5
    mono = audio.channel(0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s)")
    print(f"beat RMS: {rms(mono[:third]):.3f} (on) "
          f"{rms(mono[third:2 * third]):.3f} (paused) "
          f"{rms(mono[2 * third:]):.3f} (resumed)")
    print(f"wrote {path} - listen with: pw-play {path}")
    return stats


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")), "pause_resume.wav"))
else:
    print("score ready - run('out.wav') to render it")
