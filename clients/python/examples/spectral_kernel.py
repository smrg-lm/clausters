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
postfix program the server validates at ``/def_send synth`` and runs allocation-free.

This example renders a **tilted spectral gate**, an operation in no catalog:
the gate threshold rises with frequency, so the noise floor is swept away
progressively harder toward the highs, leaving a dark, sparse residue. The
left channel is the raw source; the right is the gated one. The threshold is
an ordinary control (``param(0)``), so a running server could sweep it live
with ``/node_set`` — a kernel stays fully modulatable.

What an expression can NOT do — state across frames (freeze), moving energy
between bins (shift), reading another chain (combiners) — stays with the
dedicated ``pv_*`` filters; see the composition docs ("Writing your own
spectral operation").
"""

import sys

from clausters import Session
from clausters.base import Routine
from clausters.defs import SynthDef, control, fft, ifft, out, pv_kernel, white_noise
from clausters.defs.pv_expr import bin_index, mag, nbins, param
from clausters.defs import Synth

SR = 48000.0


def raw(name: str = "raw") -> SynthDef:
    """The unprocessed reference: plain noise on the left channel."""
    return SynthDef(name, out(0.0, white_noise() * 0.25))


def tilted_gate(name: str = "tiltgate") -> SynthDef:
    """Noise -> FFT -> a user-written tilted gate -> IFFT, on the right.

    The expression reads like the rule it implements: keep a bin only when its
    magnitude clears a threshold that grows with the bin index — `thresh` at
    DC, `5 * thresh` at Nyquist. `mag >= t` evaluates to 1 or 0 per bin, so
    multiplying by it *is* the gate; the phase is untouched (identity), which
    keeps the kernel on the exact, cheap magnitude-scaling path.

    **Calibrating the threshold**: bin magnitudes are on the FFT's scale, not
    0..1 — for this source (noise at amplitude 0.25, 1024-point Hann) they
    spread over roughly 0.5..5 with a median near 2.3. The default `thresh`
    of 2.0 puts the gate right at that median at DC and far above the loudest
    bins up high, so the lows survive sparsely and the highs are wiped — the
    audible result is a dark, crackly residue, unmistakable next to the raw
    noise. A threshold well below the magnitude spread would gate almost
    nothing (the output then only *sounds* like decorrelated noise, because
    the chain also delays by one window). Sweep it live with
    `/node_set thresh ...` on a running server to hear the gate open and close.
    """
    chain = fft(white_noise() * 0.25, fft_size=1024)
    tilt = param(0) * (1 + 4 * bin_index / nbins)  # rising threshold
    chain = pv_kernel(chain, mag=mag * (mag >= tilt),
                      params=[control("thresh", 2.0)])
    return SynthDef(name, out(1.0, ifft(chain)))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")),
                    "spectral_kernel.wav")

    session = Session.nrt(tempo=1.0)
    raw().send(session.server)
    tilted_gate().send(session.server)

    reference = Synth("raw", server=session.server)
    gated = Synth("tiltgate", server=session.server)

    def stop():
        yield 2.0
        session.server.send_bundle(("/node_free", reference.id))
        session.server.send_bundle(("/node_free", gated.id))

    Routine(stop).play(session.clock)
    stats = session.render(sample_rate=SR, channels=2, path=out_path)
    peak = max(stats.peak, default=0.0)
    print(f"rendered {stats.frames} frames ({stats.duration:.2f} s) | peak {peak:.3f}")

    print(f"wrote {out_path} - left = raw noise, right = tilted spectral gate")
    print(f"listen with: pw-play {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
