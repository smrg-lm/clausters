#!/usr/bin/env python3
"""Two-chain spectral processing: cross-synthesis and a magnitude freeze.

Runs from the *installed* package, offline, like ``chain.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/spectral/cross.py out.wav

Where ``chain.py`` filters one chain, this def runs **two** FFT chains and
combines them — the two-chain PV family:

- `pv_mag_mul(chain_a, chain_b)` is the classic **cross-synthesis**: chain A's
  bins (a white-noise carrier) are scaled by chain B's magnitudes (a small
  harmonic stack playing a melody). The noise comes out *wearing the
  modulator's spectral envelope* — a vocoder in two UGens. Both chains must
  share the same ``fft_size``; the result travels on in chain A, so the rest
  of the chain (`pv_mag_freeze`, `ifft`) just follows the combiner's wire.
- `pv_mag_freeze(chain, freeze)` holds the last spectral envelope while
  ``freeze > 0``: the melody stops driving the noise, but the phases keep
  running — a frozen, breathing chord. The routine flips the ``freeze``
  control for the final beats, so the ending is the last note held as a
  texture.

Left channel: the dry modulator (quiet, for reference). Right channel: the
cross-synthesized noise. The combined chain is one def — no buffers, no buses
between the stages; the spectral frames are synth-private scratch.

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
    Synth,
    SynthDef,
    control,
    fft,
    ifft,
    out,
    pv_mag_freeze,
    pv_mag_mul,
    sine,
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
# ## The def
# Two FFT chains crossed: one signal's magnitudes on the other's phases.

# %%
def cross(name: str = "cross") -> SynthDef:
    """White noise wearing the spectral envelope of a harmonic stack."""
    freq = control("freq", 220.0)
    freeze = control("freeze", 0.0)
    # The modulator: a 3-partial stack — enough spectral shape to hear the
    # melody inside the noise.
    mod = sine(freq) + sine(freq * 2.0) * 0.5 + sine(freq * 3.0) * 0.25
    # Two chains, same window size (mandatory for a combiner).
    chain_a = fft(white_noise(), fft_size=1024)
    chain_b = fft(mod, fft_size=1024)
    # A's bins (noise) scaled by B's magnitudes (the stack): cross-synthesis.
    combined = pv_mag_mul(chain_a, chain_b)
    # Freezable spectral envelope: transparent while freeze <= 0.
    frozen = pv_mag_freeze(combined, freeze)
    sig = ifft(frozen)
    # Bin magnitudes are raw transform sums (a unit sine peaks at roughly
    # fft_size/4 in its bin), so scaling A by B's magnitudes multiplies the
    # level by that factor — the cross-synthesis needs a small make-up gain.
    return SynthDef(name, out(0.0, mod * 0.05), out(1.0, sig * 0.01))


# %% [markdown]
# ## The score

# %%
session = Session.nrt(tempo=2.0).activate()
cross().send()


def sequence():
    voice = Synth("cross")
    # A little melody in the modulator: the noise follows it.
    for midi, dur in [(57, 2.0), (60, 2.0), (64, 2.0), (62, 2.0)]:
        freq = 440.0 * 2.0 ** ((midi - 69.0) / 12.0)
        session.server.send_bundle(("/node_set", voice.id, "freq", freq))
        yield dur
    # Freeze the last envelope: the melody stops, the texture holds.
    session.server.send_bundle(("/node_set", voice.id, "freeze", 1.0))
    yield 4.0
    session.server.send_bundle(("/node_free", voice.id))

Routine(sequence).play()


# %%
def run(path: str = str(OUT / "cross.wav")):
    """Render the score to ``path``."""
    stats = session.render(sample_rate=SR, channels=2, path=path)

    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")

    print(f"wrote {path} - listen with: pw-play {path}")


# %%
if __name__ == "__main__" and not hasattr(sys, "ps1"):
    run(next((a for a in sys.argv[1:] if not a.startswith("-")),
             str(OUT / "cross.wav")))
else:
    print("score ready - run('out.wav') to render it")
