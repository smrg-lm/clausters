#!/usr/bin/env python3
"""Amplitude envelopes with ``EnvGen`` -- a gated ADSR that frees itself.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/envelope.py out.wav

The point of interest is the SynthDef: a sine shaped by an ``EnvGen`` playing an
`Env.adsr`. The envelope's `gate` is a control, and its `done_action` is
``FREE_SELF``, so the synth stays alive exactly as long as its sound and then
frees itself -- no ``/n_free`` bookkeeping from the client.

``Pbind(..., has_gate=True)`` is what closes the gate: for each note the player
sends ``gate 0`` after the note's ``sustain`` instead of freeing the node
outright, which starts the release segment; the synth disappears when that
release finishes. Compare with ``offline_render.py``, whose built-in ``default``
instrument has no envelope and is freed directly.
"""

import struct
import sys
import wave

from clausters import Session
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    control,
    env_gen,
    out,
    sin_osc,
)
from clausters.seq import Pbind, Pseq

SR = 48000.0


def adsr_pad(name: str = "adsr_pad") -> SynthDef:
    """A sine whose amplitude follows an ADSR. ``gate`` sustains at half level
    while held; on release it fades out and frees the synth."""
    freq = control("freq", 440.0)
    amp = control("amp", 0.2)
    gate = control("gate", 1.0)
    env = env_gen(
        Env.adsr(attack=0.02, decay=0.15, sustain=0.5, release=0.5),
        gate=gate,
        done_action=DoneAction.FREE_SELF,
    )
    sig = sin_osc(freq) * env * amp
    return SynthDef(name, out(0.0, sig), out(1.0, sig))


def phrase() -> Pbind:
    # `has_gate` makes each note release its envelope (gate 0) after `sustain`
    # rather than being freed outright; `sustain` < `dur` leaves a gap so the
    # release tail is audible before the next note.
    return Pbind(
        instrument="adsr_pad",
        has_gate=True,
        degree=Pseq([0, 2, 4, 7], repeats=2),
        dur=0.5,
        sustain=0.35,
        amp=0.2,
    )


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "envelope.wav")

    session = Session.nrt(tempo=2.0)
    session.server.add_synthdef(adsr_pad())  # /d_recv at time 0 in the score
    session.play(phrase())
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
    print(f"wrote {out_path} - listen with: ffplay -autoexit {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
