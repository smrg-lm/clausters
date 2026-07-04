#!/usr/bin/env python3
"""Frequency-domain processing (S8): an FFT -> PV_* -> IFFT chain.

Runs from the *installed* package, offline, like ``typed_controls.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/spectral.py out.wav

The def analyses a bright source with `fft`, low-passes it **in the spectral
domain** with `pv_brick_wall` (zeroing the top bins), and resynthesises audio
with `ifft` (overlap-add). No buffer is allocated — the spectral frame is
synth-private scratch on the server (SuperCollider's ``LocalBuf`` model), so the
chain is just wired UGen-to-UGen. Only `fft` names the window size; the server
propagates it to the rest of the chain.

Two synths render side by side so the effect is audible: the raw noise on the
left, the spectrally low-passed noise on the right. A running server would also
let you swap the FFT window live with ``server.u_cmd(synth, fft_index,
"window", 4)`` (see the docs); here we render offline.

The smoothing windows themselves are shared with the server through the native
core (``clausters._native.window``), so a client that pre-windows audio matches
the server's FFT bit for bit.
"""

import struct
import sys
import wave

from clausters import Session
from clausters.base import Routine
from clausters.defs import (
    SynthDef,
    fft,
    ifft,
    out,
    pv_brick_wall,
    white_noise,
)

SR = 48000.0


def noisy(name: str = "noisy") -> SynthDef:
    """Plain band-unlimited noise on the left channel (the reference)."""
    return SynthDef(name, out(0.0, white_noise() * 0.25))


def spectral_lowpass(name: str = "spectral_lp") -> SynthDef:
    """White noise -> FFT -> PV_BrickWall (low pass) -> IFFT, on the right."""
    chain = fft(white_noise() * 0.25, fft_size=1024, hop=0.5, wintype=0)
    chain = pv_brick_wall(chain, 0.75)  # keep the bottom 25% of bins
    return SynthDef(name, out(1.0, ifft(chain)))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "spectral.wav")

    session = Session.nrt(tempo=1.0)
    session.server.add_synthdef(noisy())
    session.server.add_synthdef(spectral_lowpass())

    # Both play for the whole render; a routine frees them at t = 2 beats
    # (tempo 1 -> 2 s) so the offline render has a defined duration.
    raw = session.server.synth("noisy")
    lp = session.server.synth("spectral_lp")

    def stop():
        yield 2.0
        session.server.send_bundle(("/n_free", raw.id))
        session.server.send_bundle(("/n_free", lp.id))

    Routine(stop).play(session.clock)
    samples, frames = session.render(sample_rate=SR, channels=2)
    peak = max((abs(s) for s in samples), default=0.0)
    print(f"rendered {frames} frames ({frames / SR:.2f} s) | peak {peak:.3f}")

    with wave.open(out_path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(int(SR))
        w.writeframes(b"".join(
            struct.pack("<h", int(max(-1.0, min(1.0, s)) * 32767)) for s in samples
        ))
    print(f"wrote {out_path} - left = raw noise, right = spectral low-pass")
    print(f"listen with: ffplay -autoexit {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
