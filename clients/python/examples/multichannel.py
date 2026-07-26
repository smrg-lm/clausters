#!/usr/bin/env python3
"""Multichannel without expansion: `dup`, the channel list, and `mix`.

Runs from the *installed* package, offline, like ``offline_render.py``::

    python -m venv .venv && . .venv/bin/activate
    pip install ./clients/python
    python clients/python/examples/multichannel.py out.wav

The point of interest is the two `dup` semantics and the fold:

- ``dup(lambda: sine(rand(...)), 12)`` **evaluates** the callable 12 times:
  twelve *distinct* sines, each with its own frozen `rand` detune — a thick
  unison bank. (``dup(node, n)`` would repeat one sine **by reference**:
  cheap, but twelve identical channels.)
- Operators **broadcast** over the list (``bank * env`` scales every channel
  by the same envelope, shared by reference) — no loop written.
- ``mix(bank)`` folds the twelve channels through the fused sums
  (`Sum4`/`Sum3`, not an `Add` chain), back to one signal.
- ``out(0, dup(sig))`` is the reference dup at its best: the mixed signal
  fanned to buses 0 and 1 — stereo out, one graph.
"""

import struct
import sys
import wave

from clausters import Session
from clausters.base import Routine
from clausters.defs import (
    DoneAction,
    Env,
    SynthDef,
    control,
    dup,
    env_gen,
    mix,
    out,
    rand,
    sine,
)

SR = 48000.0
VOICES = 12


def unison(name: str = "unison") -> SynthDef:
    """A detuned unison bank around ``freq``: each voice adds its own
    ``rand`` cents-scale offset, drawn once at synth init."""
    freq = control("freq", 110.0)
    gate = control("gate", 1.0)
    amp = control("amp", 0.1)
    env = env_gen(Env.asr(attack=0.4, release=1.2), gate=gate,
                  done_action=DoneAction.FREE_SELF)
    bank = dup(lambda: sine(freq + rand(-3.0, 3.0)), VOICES)
    sig = mix(bank) * (env * amp / VOICES)
    return SynthDef(name, out(0.0, dup(sig)))


def main():
    out_path = next((a for a in sys.argv[1:] if not a.startswith("-")), "multichannel.wav")

    session = Session.nrt(tempo=2.0)
    session.server.add_synthdef(unison())

    def sequence():
        for midi, dur in [(45, 3.0), (52, 3.0), (50, 4.0)]:
            freq = 440.0 * 2.0 ** ((midi - 69.0) / 12.0)
            voice = session.server.synth("unison", {"freq": freq, "amp": 0.4})
            yield dur - 0.5
            session.server.send_bundle(("/n_set", voice.id, "gate", 0.0))
            yield 0.5

    Routine(sequence).play(session.clock)
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
    print(f"wrote {out_path} - listen with: pw-play {out_path}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError) as e:
        sys.exit(str(e))
