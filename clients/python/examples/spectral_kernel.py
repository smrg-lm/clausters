#!/usr/bin/env python3
"""User-written spectral operations with `pv_kernel` (bin expressions).

Runs from the *installed* package, offline, like ``spectral.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/spectral_kernel.py out.wav

The curated ``pv_*`` set covers the common spectral operations; everything
that is a **pure per-bin map** — a rule deciding each bin from that bin's own
magnitude, phase, index or frequency — needs no server UGen at all: you write
it as an *expression* and `pv_kernel` interprets it on every bin of each
fresh frame. The symbolic terms (``mag``, ``phase``, ``bin_index``, ``nbins``,
``binfreq``, ``param(i)``) compose with ordinary Python operators — the same
maths vocabulary the rest of the client uses — and serialize to a tiny
postfix program the server validates at ``/d_recv`` and runs allocation-free.

This example renders a **tilted spectral gate**, an operation in no catalog:
the gate threshold rises with frequency, so the noise floor is swept away
progressively harder toward the highs, leaving a dark, sparse residue. The
left channel is the raw source; the right is the gated one. The threshold is
an ordinary control (``param(0)``), so a running server could sweep it live
with ``/n_set`` — a kernel stays fully modulatable.

What an expression can NOT do — state across frames (freeze), moving energy
between bins (shift), reading another chain (combiners) — stays with the
dedicated ``pv_*`` filters; see the composition docs ("Writing your own
spectral operation").
"""

import struct
import sys
import wave

from clausters import Session
from clausters.base import Routine
from clausters.defs import SynthDef, control, fft, ifft, out, pv_kernel, white_noise
from clausters.defs.pv_expr import bin_index, mag, nbins, param

SR = 48000.0


def raw(name: str = "raw") -> SynthDef:
    """The unprocessed reference: plain noise on the left channel."""
    return SynthDef(name, out(0.0, white_noise() * 0.25))


def tilted_gate(name: str = "tiltgate") -> SynthDef:
    """Noise -> FFT -> a user-written tilted gate -> IFFT, on the right.

    The expression reads like the rule it implements: keep a bin only when its
    magnitude clears a threshold that grows with the bin index — `thresh` at
    DC, `4 * thresh` at Nyquist. `mag >= t` evaluates to 1 or 0 per bin, so
    multiplying by it *is* the gate; the phase is untouched (identity), which
    keeps the kernel on the exact, cheap magnitude-scaling path.
    """
    chain = fft(white_noise() * 0.25, fft_size=1024)
    tilt = param(0) * (1 + 3 * bin_index / nbins)  # rising threshold
    chain = pv_kernel(chain, mag=mag * (mag >= tilt),
                      params=[control("thresh", 0.4)])
    return SynthDef(name, out(1.0, ifft(chain)))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")),
                    "spectral_kernel.wav")

    session = Session.nrt(tempo=1.0)
    session.server.add_synthdef(raw())
    session.server.add_synthdef(tilted_gate())

    reference = session.server.synth("raw")
    gated = session.server.synth("tiltgate")

    def stop():
        yield 2.0
        session.server.send_bundle(("/n_free", reference.id))
        session.server.send_bundle(("/n_free", gated.id))

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
    print(f"wrote {out_path} - left = raw noise, right = tilted spectral gate")
    print(f"listen with: ffplay -autoexit {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
