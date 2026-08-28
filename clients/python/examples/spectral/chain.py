#!/usr/bin/env python3
"""Frequency-domain processing: an FFT -> PV_* -> IFFT chain.

Runs from the *installed* package, offline, like ``basics/typed_controls.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/spectral/chain.py out.wav

The def analyses a bright source with `fft`, low-passes it **in the spectral
domain** with `pv_brick_wall` (zeroing the top bins), and resynthesises audio
with `ifft` (overlap-add). No buffer is allocated — the spectral frame is
synth-private scratch on the server (SuperCollider's ``LocalBuf`` model), so the
chain is just wired UGen-to-UGen. Only `fft` names the window size; the server
propagates it to the rest of the chain.

Two synths render side by side so the effect is audible: the raw noise on the
left, the spectrally low-passed noise on the right. A running server would also
let you swap the FFT window live with ``synth.u_cmd(fft_index, "window", 4)``
(see the docs); here we render offline.

The smoothing windows themselves are shared with the server through the native
core (``clausters._native.window``), so a client that pre-windows audio matches
the server's FFT bit for bit.

This file is organized as ``# %%`` cells (the VS Code / Jupyter convention).
Offline does not mean run-once: change the def in one cell and re-render in the
next.
"""

# %%
import pathlib
import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import (
    DoneAction,
    Env,
    Synth,
    SynthDef,
    control,
    env_gen,
    fft,
    ifft,
    out,
    pv_brick_wall,
    white_noise,
)

#: Where a run leaves its file when no path is given: ``examples/out/``, the
#: git-ignored directory every generator in this tree writes to — beside the
#: examples rather than in whatever directory you ran from. Made here so that
#: rendering is one call and not two.
OUT = pathlib.Path(__file__).resolve().parents[1] / "out"
OUT.mkdir(exist_ok=True)

SR = 48000.0


# %% [markdown]
# ## The source
# A band of noise -- the thing the filter has to act on.

# %%
def fade():
    """A short attack and release, opened at birth and closed by the gate.

    Noise switched on at full amplitude starts the take on a step, which is a
    click — audible on the first sample of a file meant to be listened to. Two
    hundredths of a second of ramp costs nothing and is not the subject here;
    the same envelope closes the take when the routine drops the gate."""
    gate = control("gate", 1.0)
    return env_gen(Env.asr(0.02, 1.0, 0.05), gate=gate,
                   done_action=DoneAction.FREE_SELF)


def noisy(name: str = "noisy") -> SynthDef:
    """Plain band-unlimited noise on the left channel (the reference)."""
    return SynthDef(name, out(0.0, white_noise() * 0.25 * fade()))


# %% [markdown]
# ## The spectral filter
# An FFT chain: analyse, zero the bins above a cutoff, resynthesise.

# %%
def spectral_lowpass(name: str = "spectral_lp") -> SynthDef:
    """White noise -> FFT -> PV_BrickWall (low pass) -> IFFT, on the right."""
    chain = fft(white_noise() * 0.25, fft_size=1024, hop=0.5, wintype=0)
    chain = pv_brick_wall(chain, 0.75)  # keep the bottom 25% of bins
    return SynthDef(name, out(1.0, ifft(chain) * fade()))


# %% [markdown]
# ## The score

# %%
session = Session.nrt(tempo=1.0).activate()
noisy().send()
spectral_lowpass().send()

# Both play for the whole render; a routine frees them at t = 2 beats
# (tempo 1 -> 2 s) so the offline render has a defined duration.
raw = Synth("noisy")
lp = Synth("spectral_lp")


def stop():
    yield 2.0
    # Closed, not freed: the gate lets each envelope fall to zero instead of
    # cutting the noise mid-sample.
    session.server.send_bundle(("/node_set", raw.id, "gate", 0.0))
    session.server.send_bundle(("/node_set", lp.id, "gate", 0.0))
    yield 0.25
    # The score's closing event, after the release: a render ends at its last
    # event, so without this one the file would stop where the gate closed and
    # the tail would be cut off — the click again, at the other end.
    session.server.send_bundle(("/node_free", 0))

Routine(stop).play()


# %%
def run(path: str = str(OUT / "chain.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")

    print(f"wrote {path} - left = raw noise, right = spectral low-pass")
    print(f"listen with: pw-play {path}")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "chain.wav")))
else:
    print("score ready - run('out.wav') to render it")
