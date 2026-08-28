#!/usr/bin/env python3
"""Automate a synth control with a break-point curve -- an **automation lane**.

An `Automation` is a control curve placed on the timeline that drives one or
more ``(node, control)`` targets. It is rendered as a **control vector**: the
break-point curve is discretized on the server into a control buffer
(``/buffer_gen "env"``, evaluated through the same envelope-shape math the ``EnvGen``
UGen plays), and a small control synth reads that buffer onto a control bus which
the target follows via ``/node_map``. The curve is the same `Env` the ``bpf`` editor
round-trips (`env_to_points`/`points_to_env`), so a drawn envelope and a played
automation are one object.

Here a single sustained sine has its pitch swept by a lane: 220 Hz up to 880
(exponential), then down to 330 (linear), over four beats -- an audible glissando
with no per-note retriggering.

Runs from the *installed* package, offline (no server process, no display),
like ``buffers/offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install -e ./clients/python
    python clients/python/examples/transport/automation_lane.py out.wav

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention); run
it cell by cell (Shift+Enter) or as a plain script.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base.stream import Routine
from clausters.defs import SynthDef, control, out, sine
from clausters.seq import Automation
from clausters.defs import Synth

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000


# %%
def tone(name: str = "tone") -> SynthDef:
    """A sine whose ``freq`` and ``amp`` are ``kr`` controls -- ``kr`` so they
    can be ``/node_map``-ed to a control bus and tracked per block."""
    freq = control("freq", 220.0, "kr")
    amp = control("amp", 0.2, "kr")
    sig = sine(freq) * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


# %% Build the offline session and register the instrument.
session = Session.nrt(tempo=1.0)
server = session.server
tone().send(server)

# %% A sustained voice, and an automation lane sweeping its pitch.
voice = Synth("tone", server=server)           # one held voice
gliss = Automation.from_points(
    [(0, 220.0, 2, 0.0),                            # 220 Hz ...
     (2, 880.0, 2, 0.0),                            # ... up to 880 (exponential) ...
     (4, 330.0, 1, 0.0)],                           # ... down to 330 (linear)
    target=(voice, "freq"))
gliss.prepare(server)                               # alloc + fill the control buffer

# %% Play the lane in a routine (it schedules the lane synth + the /node_map),
# then free the voice, and render the score to interleaved samples.
def score():
    gliss.play(server)
    yield gliss.duration()
    server.send_bundle(("/node_free", voice.id))


session.clock.play(Routine(score))

# %% Render it -- the server writes the WAV, we keep the stats.
out_path = next((a for a in sys.argv[1:] if not a.startswith("-")),
                str(OUT / "automation_lane.wav"))
stats = session.render(sample_rate=SR, channels=2, path=out_path)
peak = max(stats.peak, default=0.0)
print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f} -> {out_path}")
